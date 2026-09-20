//! Everything that is done to a galaxy index directory.
//!
//! ```sh
//! galos-index status .index/full
//! galos-index ingest --from eddn --dir .index/full        # keep it current
//! galos-index build --from spansh=galaxy.json --dir .index/full
//! galos-index build --from database --watch 5             # from the rows
//! galos-index verify .index/full --bodies
//! galos-index sweep .index/full --bodies --apply
//! galos-index diff .index/from_dump .index/from_db
//! ```
//!
//! **Database-free, and that is the point.** Build it with
//! `--no-default-features` and there is no `sqlx`, no `dotenv` and no
//! `DATABASE_URL` anywhere in it: an index is a file format a client draws
//! from with no server at all, so the tool that fills and repairs one runs
//! on a machine with no Postgres installed. `build --from database` is the
//! one verb that needs the `db` feature, and it is the one verb that is
//! about the other store.
//!
//! `ingest` and `build` are both "fill this directory", and which one to
//! reach for is a question about *memory*, not about taste. `ingest` holds
//! a live tree and the whole names table — a kilobyte a system — because
//! something may be reading the directory while it is written, and that is
//! what makes a feed, a journal or a small dump work. `build` holds one
//! region at a time and publishes nothing until it is done, which is the
//! only way a two hundred million system dump fits in memory at all. See
//! [`galos::read::cold`].
//!
//! `status` says what one directory holds and `verify` says what is wrong
//! with it — both read-only; `diff` says whether two of them are the same
//! derivation, which is the question a dump-built index and a
//! database-built index of the same galaxy are there to answer.
//!
//! The rest write, and each takes `<dir>.lock` for as long as it holds the
//! directory: `sweep` gives back what nothing refers to, `pack` moves loose
//! body files into the shards, `migrate` brings a directory's format
//! forward. Every one of them weighs before it acts and reports before it
//! is asked to act, because the passes are minutes over a galaxy and a
//! silent terminal is not a run anybody can judge. Ctrl-C is answered
//! between shards; a second one kills.

use clap::{Parser, Subcommand};
use galos_index::geometry::MAX_LEVEL;
use galos_index::{
    source, store, Bodies, Cell, Index, NameEntry, PopulatedSystem, Published,
    SystemBoost, SystemReach,
};
use serde::de::DeserializeOwned;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

mod fill;

/// Fill, inspect and repair a galaxy index directory.
#[derive(Parser)]
#[command(name = "galos-index", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
    /// Clear a lock left behind by a builder that was killed, and take it.
    ///
    /// The refusal names the pid holding the directory. Check it first: a
    /// lock cleared while its builder is merely slow to answer is two
    /// writers over one directory, which is what the lock is for.
    #[arg(long, global = true)]
    force_lock: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Summarise a built index directory: its shape and the galaxy's summed light.
    Status {
        /// The index directory to read.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Read a publisher into the directory, following it where it does not
    /// end.
    ///
    /// Holds a live tree and publishes on a beat, so a map reading the
    /// directory sees what arrives. A galaxy-sized dump wants `build`
    /// instead — see the `--from spansh=PATH` note there.
    Ingest(fill::Ingest),
    /// Derive the whole directory from one finite source, publishing
    /// nothing until it is built.
    Build(fill::Build),
    /// Compare two built index directories: are they the same derivation?
    Diff {
        /// The two index directories to compare.
        a: PathBuf,
        b: PathBuf,
        /// Compare the body files as well, which is a file a scanned
        /// system and hours of them over a galaxy.
        #[arg(long)]
        bodies: bool,
        /// Name the rows that differ, which holds both names tables in
        /// memory: a galaxy's worth is tens of gigabytes.
        #[arg(long)]
        detail: bool,
        /// How many differing rows to name before counting the rest.
        #[arg(long, default_value_t = 5)]
        limit: usize,
    },
    /// Walk a directory's loose body files into the packed shard files.
    Pack {
        /// The index directory to pack.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Give back what a directory holds and nothing refers to: the
    /// payloads of cells the index no longer names, and with `--bodies`
    /// the dead records in the body shards.
    Sweep {
        /// The index directory to sweep.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Sweep the body shards too, which a re-import leaves a dead
        /// record in for every system it rewrote.
        #[arg(long)]
        bodies: bool,
        /// Do it. Without this everything is only weighed and reported.
        #[arg(long)]
        apply: bool,
    },
    /// Say what a directory holds and what is wrong with it, writing
    /// nothing.
    Verify {
        /// The index directory to read.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Weigh the body shards against the tree, which reads every
        /// system's address: a galaxy is 1.6 GB held and a minute.
        #[arg(long)]
        bodies: bool,
    },
    /// Bring a directory's format forward, in place: its names chunks
    /// folded into the mapped table, its payloads rewritten to the columns
    /// this build reads, and the table itself brought to the version this
    /// build writes.
    ///
    /// One verb rather than three, because there is no order to choose
    /// between: the chunks are older than the table, the table is read by
    /// the payload rewrite, and a directory half forward is one the next
    /// refusal names again. Idempotent, so running it on something already
    /// current costs a version read apiece.
    Migrate {
        /// The index directory to bring forward.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
    },
    /// Write the sector dictionary `galos_index::procedural` derives names
    /// through, learned from a built directory.
    Sectors {
        /// The index directory to learn from.
        #[arg(default_value = ".galos_index")]
        dir: PathBuf,
        /// Where to write it. `galos_index/data/sectors.csv` is the one
        /// the crate compiles in.
        #[arg(long, short)]
        out: Option<PathBuf>,
        /// Write it even where a sector the dictionary already names comes
        /// out differently, which re-spells every name dropped under it.
        #[arg(long)]
        force: bool,
    },
}

/// Leave with `code`, having dropped whatever was holding the directory
///
/// **`std::process::exit` runs no destructors**, so a command that exits
/// out of its error arm while holding [`galos_index::Lock`] leaves the lock
/// file behind and the next run refuses the directory as "already being
/// written" by a process that is gone. Reported twice in one sitting, once
/// off `upgrade` and once off `pack`.
///
/// So the lock is handed over and dropped here, on the way out. A command
/// that holds nothing passes nothing.
fn leave(lock: Option<galos_index::Lock>, code: i32) -> ! {
    drop(lock);
    std::process::exit(code)
}

/// What `RUST_LOG` falls back to.
///
/// Plain `info`, there being no chatty dependency to silence: the one
/// crate that needed naming was `sqlx`, and a build of this tool need not
/// contain it. `galos_db::HEARD` is what the database tool uses, and it
/// names its own.
const HEARD: &str = "info";

#[async_std::main]
async fn main() -> ExitCode {
    // The reporting verbs print; `ingest` and `build` trace, and so does
    // everything they call. Nothing a crate traces goes anywhere until
    // something is listening for it.
    tracing_subscriber::fmt()
        // Above whatever bars are drawing, so they keep the bottom lines
        // and the log does not land on top of them.
        .with_writer(galos::bar::Log)
        // Color is for a terminal. Redirected, it would be escape codes
        // around every line of the log.
        .with_ansi(std::io::stderr().is_terminal())
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| HEARD.into()),
        )
        .init();

    let cli = Cli::parse();
    let forced = cli.force_lock;
    match cli.command {
        // The two that fill a directory install their own handler: they
        // have a publish and a resume point to close out, and what asks
        // them to stop is a `Shutdown` several tasks read.
        Command::Ingest(it) => fill::ingest(it, forced).await,
        Command::Build(it) => fill::build(it, forced).await,
        // Everything else is one pass over the directory, stopped between
        // shards by the flag [`stopping`] reads.
        pass => {
            asking_to_stop();
            match pass {
                Command::Status { dir } => status(&dir),
                Command::Diff { a, b, bodies, detail, limit } => {
                    diff(&a, &b, Compare { bodies, detail, limit })
                }
                Command::Pack { dir } => pack(&dir, forced),
                Command::Sweep { dir, bodies, apply } => {
                    sweep(&dir, bodies, apply, forced)
                }
                Command::Verify { dir, bodies } => verify(&dir, bodies),
                Command::Migrate { dir } => migrate(&dir, forced),
                Command::Sectors { dir, out, force } => {
                    sectors(&dir, out.as_deref(), force)
                }
                // Answered above, under the handler that suits them.
                Command::Ingest(_) | Command::Build(_) => unreachable!(),
            }
            ExitCode::SUCCESS
        }
    }
}

/// Whether the run has been asked to stop, for the passes that answer one.
///
/// A process-wide flag rather than a token threaded through: these are
/// one-pass commands, the handler is installed once at the top of `main`,
/// and what reads it is a `&dyn Fn() -> bool` several layers down inside
/// `galos_index`.
static ASKED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Take Ctrl-C, so a pass over a galaxy can be stopped and say so.
///
/// **The default action is to die where it stands**, which for a sweep is
/// a directory part way through one — recoverable, since every pass here
/// is idempotent, but it is not the answer an operator asked for and it
/// says nothing about how far it got. The flag [`stopping`] reads is
/// checked between shards, so a stop lands in the time one shard takes
/// and the command reports what it reclaimed before it was stopped.
///
/// A *second* Ctrl-C leaves at once, which is the answer for a pass that
/// is somehow not reaching its next flag check.
///
/// Through `ctrlc`, which is what the filling verbs install as well — one
/// mechanism, rather than a hand-rolled `sigaction` here and a crate
/// there. What differs is only what is asked to stop: a pass has a flag,
/// and a run that follows a feed has a `Shutdown` several tasks read.
fn asking_to_stop() {
    let installed = ctrlc::set_handler(|| {
        if ASKED.swap(true, std::sync::atomic::Ordering::Relaxed) {
            eprintln!("stopping now");
            std::process::exit(130);
        }
        eprintln!(
            "stopping at the end of this shard, which takes a moment. \
             Ctrl-C again to stop now."
        );
    });
    if let Err(err) = installed {
        eprintln!(
            "no signal handler; Ctrl-C will stop this pass where it \
                   stands: {err}"
        );
    }
}

/// Whether whoever asked for this run has stopped wanting it.
fn stopping() -> impl Fn() -> bool + Sync {
    || ASKED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Count, and on request give back, what the directory holds and nothing
/// refers to.
///
/// Two kinds, and the second only when it is asked for:
///
/// - **The payloads of cells the published tree does not name.** What a
///   whole-directory rebuild leaves behind: it writes its own cells and
///   knows nothing of the tree that stood before it. Measured at 200,248
///   files and 4.9 GB on a directory rebuilt from the database over one
///   built from a dump.
/// - **The dead records in the body shards**, on `--bodies`: a re-import
///   appends a fresh record for every system and the one behind it is
///   dead. Measured on a re-imported galaxy at 161.1 GB.
///
/// A build sweeps both for itself now — `store::sweep_payloads` and
/// `pack::sweep_bodies`, run once the new index file stands — so this is
/// for the directories written before it did, and for looking before
/// acting. **Reporting is the default**, because acting on a served
/// directory on a typo is not, and because the weighing is what says what
/// the run is about to cost.
///
/// Safe to run against a directory a map is *reading*: a payload the tree
/// does not name has no reader, and a shard is reclaimed whole. Not safe
/// against one something is *writing*, which is what [`held`] is for.
fn sweep(dir: &Path, bodies: bool, apply: bool, forced: bool) {
    let lock = held(dir, forced);
    let index = match galos_index::Index::read(dir) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    };
    let at = std::time::Instant::now();
    match galos_index::sweep_payloads(dir, &index, apply) {
        Ok(swept) if swept.orphans == 0 => {
            println!(
                "{}: every payload belongs to a cell of the {} the index \
                 names",
                dir.display(),
                index.len(),
            );
        }
        Ok(swept) => {
            let one = swept.orphans == 1;
            println!(
                "{}: {} payload{} {}, {}, in {:.1?}",
                dir.display(),
                swept.orphans,
                if one { "" } else { "s" },
                if one { "names no cell" } else { "name no cell" },
                match apply {
                    true => format!("removed ({})", size(swept.bytes)),
                    false => format!("holding {}", size(swept.bytes)),
                },
                at.elapsed(),
            );
            if !apply {
                println!("pass --apply to remove them");
            }
        }
        Err(err) => {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
    if bodies {
        if let Err(err) = sweep_bodies(dir, apply) {
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
}

/// Weigh, and on request reclaim, the dead records in the body shards.
///
/// **Weighed first, always.** The weighing is a read of 4,096 index files
/// — a second over a galaxy — and what it answers is how long the rest of
/// the run will take and what it will give back. A pass that reclaims 161
/// GB is minutes of work, and minutes of a silent terminal is a run
/// nobody can tell from a hung one.
fn sweep_bodies(dir: &Path, apply: bool) -> io::Result<()> {
    let stop = stopping();
    let at = std::time::Instant::now();
    let weighed = galos_index::pack::weigh(dir, &stop)?;
    println!(
        "{}: {} shards, {} systems, {} live, {} dead, in {:.1?}",
        dir.display(),
        weighed.shards,
        weighed.records,
        size(weighed.live),
        size(weighed.dead),
        at.elapsed(),
    );
    if weighed.loose > 0 {
        println!(
            "{} body files are still loose; `galos-index pack` moves them",
            weighed.loose,
        );
    }
    if weighed.reclaimable == 0 {
        println!("nothing in the shards is worth reclaiming");
        return Ok(());
    }
    if !apply {
        println!(
            "{} of that would be given back; pass --apply to do it",
            size(weighed.reclaimable),
        );
        return Ok(());
    }

    // Said before the wait and not after it: this is the minutes.
    println!("reclaiming {} ...", size(weighed.reclaimable));
    let from = std::time::Instant::now();
    let said = |run: &galos_index::Reclaimed| {
        eprint!(
            "\r{} shards, {} reclaimed, {:.0?}",
            run.shards,
            size(run.bytes),
            from.elapsed(),
        );
    };
    let swept = galos_index::pack::sweep_bodies(dir, &stop, &said);
    eprintln!();
    let swept = swept?;
    println!(
        "{}: {} shards, {} reclaimed ({} punched, {} copied), in {:.1?}{}",
        dir.display(),
        swept.shards,
        size(swept.bytes),
        size(swept.punched),
        size(swept.bytes - swept.punched),
        from.elapsed(),
        match swept.finished {
            true => "",
            false => ", and the rest were not reached",
        },
    );
    Ok(())
}

/// Say what a directory holds and what is wrong with it, writing nothing.
///
/// Four questions, in the order a directory fails them:
///
/// 1. **Does it have a tree at all?** `index.bin` is the one mandatory
///    file: everything under it reads as empty where it is missing, so a
///    directory without it serves nothing and nothing else here matters.
/// 2. **Is every cell the tree names backed by a payload?** An orphan —
///    a payload no cell names — is dead weight and `sweep` removes it. A
///    **hole** is the other way round and is the serious one: a cell the
///    tree names with no payload under it reads as a cell with no systems
///    in it, which is a galaxy quietly missing a piece.
/// 3. **What do the body shards hold, and what is dead in them?**
/// 4. On `--bodies`, **do the shards answer for systems the tree does not
///    name?** That one reads every address in the directory — 1.6 GB held
///    over a galaxy — so it is asked for rather than run by default, and
///    it says what it is about to cost before it does it.
///
/// No lock: nothing here writes. A directory being written underneath
/// gives numbers from either side of a publish, which is worth knowing
/// before reading too much into a single hole.
fn verify(dir: &Path, bodies: bool) {
    let stop = stopping();
    let index = match Index::read(dir) {
        Ok(index) => index,
        Err(err) => {
            eprintln!("{}: no index to verify: {err}", dir.display());
            std::process::exit(1);
        }
    };
    // **Every cell that owns systems, and not only the leaves.** An
    // internal cell keeps the systems its children are too coarse to
    // draw, so it has a payload of its own: checking the leaves alone
    // read 186.8 M of a 200.07 M-system directory and called the
    // difference nothing.
    let owners: Vec<Cell> =
        index.cells().filter(|it| it.slice_len() > 0).copied().collect();
    let leaves = index.cells().filter(|it| it.is_leaf()).count();
    let systems = index.root().map(|it| it.aggregate.count()).unwrap_or(0);
    println!("{}", dir.display());
    println!(
        "  index       {} cells ({leaves} leaves, {} with systems of their \
         own), {systems} systems",
        index.len(),
        owners.len(),
    );

    // A hole is a cell the tree names with nothing under it, and a short
    // payload is the same wound half open: both read as a cell with fewer
    // systems in it than the index says, which is a galaxy quietly
    // missing a piece. Mapped rather than read — the count is in the
    // payload's own header, and 4.9 GB of columns need not be decoded to
    // compare it.
    let at = std::time::Instant::now();
    let mut holes = 0usize;
    let mut short = 0usize;
    let mut held = 0u64;
    for (n, cell) in owners.iter().enumerate() {
        if stop() {
            println!("  payloads    stopped after {n} of {}", owners.len());
            return;
        }
        if n % 4096 == 0 {
            eprint!("\r  payloads    {n} of {} read", owners.len());
        }
        match store::Payload::open(dir, cell.id) {
            Ok(Some(payload)) => {
                held += payload.len() as u64;
                short += usize::from(payload.len() as u64 != cell.slice_len());
            }
            Ok(None) => holes += 1,
            Err(err) => {
                eprintln!("\n{}: {err}", dir.display());
                std::process::exit(1);
            }
        }
    }
    let orphans = match galos_index::sweep_payloads(dir, &index, false) {
        Ok(swept) => swept,
        Err(err) => {
            eprintln!("\n{}: {err}", dir.display());
            std::process::exit(1);
        }
    };
    eprint!("\r");
    println!(
        "  payloads    {held} systems held, {holes} missing, {short} short, \
         {} named by no cell ({}), in {:.0?}",
        orphans.orphans,
        size(orphans.bytes),
        at.elapsed(),
    );

    match galos_index::pack::weigh(dir, &stop) {
        Ok(weighed) => {
            println!(
                "  bodies      {} shards, {} systems, {} live, {} dead \
                 ({} reclaimable)",
                weighed.shards,
                weighed.records,
                size(weighed.live),
                size(weighed.dead),
                size(weighed.reclaimable),
            );
            if weighed.loose > 0 {
                println!("              {} still loose", weighed.loose);
            }
            if bodies {
                strays(dir, &owners, systems, weighed.records, &stop);
            }
        }
        Err(err) => println!("  bodies      unreadable: {err}"),
    }

    // The names table last, it being the one part a directory can serve
    // without: a build stopped before its fold has chunks and no base.
    match galos_index::Names::open(dir) {
        Ok(names) => println!("  names       {} systems named", names.len()),
        Err(err) => println!("  names       unreadable: {err}"),
    }

    if holes > 0 {
        println!(
            "\n{holes} cells the tree names have no payload: that is systems \
             the index says are there and cannot serve. `galos-index build` \
             is the repair.",
        );
        std::process::exit(1);
    }
}

/// Count the systems the shards answer for that the tree does not name.
///
/// **The galaxy-sized half of `verify`, and why it is behind a flag.**
/// There is no address list in the directory: the tree's addresses are in
/// the payloads, one leaf at a time, so answering this means holding every
/// one of them — 8 bytes a system, 1.6 GB over a galaxy — and walking
/// every shard's index against it.
///
/// A stray is not a wrong answer, it is dead weight: bodies are reached by
/// clicking a system the tree holds, so a record for a system the tree
/// lost is a file nobody can ask for. They are what a rebuild from a
/// smaller source leaves — a database-derived directory over a
/// dump-derived one keeps the dump's bodies — and counting them is how an
/// operator finds out. Nothing here removes them: withdrawing a scan is a
/// decision about data, not about space.
fn strays(
    dir: &Path,
    owners: &[Cell],
    systems: u64,
    records: u64,
    stop: &dyn Fn() -> bool,
) {
    println!(
        "              reading the {systems} addresses the tree names, \
         which is about {} held ...",
        size(systems * 8),
    );
    let at = std::time::Instant::now();
    let mut named: Vec<u64> = Vec::with_capacity(systems as usize);
    for cell in owners {
        if stop() {
            println!("              stopped before the addresses were read");
            return;
        }
        let Ok(Some(payload)) = store::Payload::open(dir, cell.id) else {
            continue;
        };
        named.extend((0..payload.len()).map(|at| payload.id64_at(at)));
    }
    named.sort_unstable();

    let mut strays = 0u64;
    let walked = galos_index::pack::each_address(dir, stop, &mut |address| {
        if named.binary_search(&(address as u64)).is_err() {
            strays += 1;
        }
    });
    match walked {
        Ok(true) => println!(
            "              {strays} of {records} body records are for \
             systems the tree does not name, in {:.1?}",
            at.elapsed(),
        ),
        Ok(false) => println!("              stopped after {strays} strays"),
        Err(err) => println!("              unreadable: {err}"),
    }
}

/// Bytes in the unit a person would have said them in.
fn size(bytes: u64) -> String {
    const KB: f64 = 1e3;
    let bytes = bytes as f64;
    match bytes {
        b if b >= 1e9 => format!("{:.1} GB", b / 1e9),
        b if b >= 1e6 => format!("{:.1} MB", b / 1e6),
        b if b >= KB => format!("{:.1} kB", b / KB),
        b => format!("{b:.0} bytes"),
    }
}

/// Bring a directory up to the format this build reads.
///
/// What [`galos_index::store`]'s version refusal names, so an operator met
/// by "rebuild the directory" has one thing to run. Three rewrites, in the
/// only order they can happen in:
///
/// 1. **The names chunks are folded into the mapped table.** A build
///    streams chunks and the table is what a read draws from, so a galaxy
///    left unfolded is a galaxy of names nothing can spell. It is an
///    external sort of gigabytes, which is why it is a verb and not
///    something discovered at the front of somebody's import.
/// 2. **The payloads are rewritten to the columns this build reads.**
///
///    The ones written before them hold every field the new ones do but
///    the star kind, and that is derivable from `bodies/` — the scan
///    record the class comes from. So this joins the two and rewrites each
///    cell, where the alternative is running the importer over the dump
///    again.
/// 3. **The table comes to the version this build writes**, where it is
///    behind. That rewrite is what drops every name the address spells —
///    97.4 % of a galaxy, and 3.94 GB of `text.bin` down to 133 MB.
///
/// Idempotent and interruptible: a directory with no chunks is not
/// folded, a cell already columnar is left alone, `index.bin` is rewritten
/// last, and a table already at this version is not touched. The bodies,
/// the sidecars and the tree itself are unchanged.
fn migrate(dir: &Path, forced: bool) {
    let lock = held(dir, forced);
    if !fold_names(dir, &lock) {
        leave(Some(lock), 2);
    }
    let at = std::time::Instant::now();
    // No stop flag of its own: a run cut short by a Ctrl-C leaves the
    // directory in a state the next run takes up, `index.bin` being
    // rewritten last.
    let stop = || false;
    let mut said = |wrote: &galos_index::upgrade::Rewrote| {
        // The sweep first and the rewrite after it, which is the order they
        // happen in: a line about cells while the scan record is still
        // being read would be a line of zeroes.
        match wrote.cells == 0 && wrote.kept == 0 {
            true => {
                eprint!("\r{} systems swept, {:.0?}", wrote.swept, at.elapsed())
            }
            false => eprint!(
                "\r{} cells, {} systems, {} classed, {} already columnar, \
                 {:.0?}",
                wrote.cells,
                wrote.systems,
                wrote.classed,
                wrote.kept,
                at.elapsed(),
            ),
        }
    };

    match galos_index::upgrade::rewrite(dir, &stop, &mut said) {
        Ok(wrote) => {
            eprintln!();
            println!(
                "{} cells rewritten, {} systems, {} of them classed, \
                 {} already columnar, in {:.1?}",
                wrote.cells,
                wrote.systems,
                wrote.classed,
                wrote.kept,
                at.elapsed(),
            );
            // Only where there was a table to bring forward, which is a
            // directory built before the place rode in the published row.
            if wrote.placed > 0 {
                println!("{} supercharge rows given their place", wrote.placed,);
            }
            names_forward(dir, &lock);
        }
        Err(err) => {
            eprintln!();
            eprintln!("{}: {err}", dir.display());
            leave(Some(lock), 1);
        }
    }
}

/// Bring a directory's names table to the version this build writes.
///
/// The rewrite is the whole base — an external sort over every row — so it
/// runs only where the version says it is owed, which makes `upgrade`
/// idempotent over a table already forward. What it buys is the text of
/// every name the address spells: measured over a 200,071,629-name table,
/// `text.bin` 3.94 GB to 133 MB.
///
/// A table that cannot be read is not a failure of the payload rewrite
/// that has already landed, so this reports and leaves the exit code
/// alone.
fn names_forward(dir: &Path, lock: &galos_index::Lock) {
    let _ = lock;
    match galos_index::names::version(dir) {
        Ok(None) => {}
        Ok(Some(version)) if version >= galos_index::names::writes() => {
            println!("the names table is already version {version}");
        }
        Ok(Some(version)) => {
            let at = std::time::Instant::now();
            println!(
                "rewriting the names table, version {version} to {}",
                galos_index::names::writes(),
            );
            match galos_index::names::compact(dir) {
                Ok(count) => {
                    println!("{count} names rewritten in {:.1?}", at.elapsed())
                }
                Err(err) => eprintln!("the names table: {err}"),
            }
        }
        Err(err) => eprintln!("the names table: {err}"),
    }
}

/// Pack a directory's loose body files, saying what it moved.
///
/// The same migration a sync runs at every open, for a directory nothing is
/// about to sync: a galaxy of loose files is hours of packing, and an
/// operator would rather spend them on purpose. Interruptible, idempotent,
/// and safe to run against a directory a map is *reading* — a loose file is
/// dropped only once the pack holds its record, and a read falls back to
/// whatever is still loose.
///
/// Not safe to run against a directory something is *writing*, which is
/// what [`held`] is for.
fn pack(dir: &Path, forced: bool) {
    let lock = held(dir, forced);
    let start = std::time::Instant::now();
    match galos_index::pack::pack(dir, &|| false) {
        Ok(done) => println!(
            "{}: {} files packed in {:.1?}{}",
            dir.display(),
            done.moved,
            start.elapsed(),
            match done.finished {
                true => "",
                false => ", and some are still loose",
            },
        ),
        Err(e) => {
            eprintln!("cannot pack {}: {e}", dir.display());
            leave(Some(lock), 2);
        }
    }
}

/// Fold a directory's names chunks into the mapped table, saying what it
/// came to.
///
/// The same migration a sync runs at its open, for a directory nothing is
/// about to sync — a galaxy's worth of chunks is an external sort of
/// gigabytes, and an operator would rather spend it on purpose than
/// discover it at the front of a build.
///
/// Idempotent: a directory with no chunks has nothing to do. Safe to run
/// against a directory a map is *reading*, the table being swapped in by
/// one rename and the chunks removed only after.
/// Answers whether it folded what was there, so the caller holding the
/// lock is the one that exits — see [`leave`].
fn fold_names(dir: &Path, lock: &galos_index::Lock) -> bool {
    let _ = lock;
    let start = std::time::Instant::now();
    match galos_index::names::fold_chunks(dir) {
        Ok(Some(named)) => println!(
            "{}: {named} systems folded into the mapped table in {:.1?}",
            dir.display(),
            start.elapsed(),
        ),
        Ok(None) => println!("{}: no names chunks to fold", dir.display()),
        Err(e) => {
            eprintln!("cannot read {}: {e}", dir.display());
            return false;
        }
    }
    true
}

/// Learn the sector dictionary from a directory's names table.
///
/// What `galos_index::procedural` compiles in, and the only way to refresh
/// it: a sector enters the dictionary when the first system in it is
/// reported, so the file is as complete as the galaxy anybody has imported.
///
/// **A name that claims more than one sector coordinate is left out.**
/// Those are Frontier's hand-authored regions — `COL 285 SECTOR`, `IC 2944
/// SECTOR` — laid over the procedural grid as spheres, and their boxels are
/// numbered from the region's own origin. Deriving one would spell the
/// wrong name for every procedural system in the same cell, so the module
/// answers nothing there and every system under a region is stored.
///
/// **An entry that already exists may be added to but never changed.**
/// That is the one safety property the whole scheme rests on: a name is
/// dropped from the table because the dictionary spelled it, so a
/// regeneration that renamed a sector would silently re-spell every name
/// already dropped under it. A key whose name disagrees with the one
/// compiled in is therefore refused rather than written, and `--force`
/// is the only way past — which is what somebody rebuilding the dictionary
/// on purpose, against a table they are about to rewrite anyway, passes.
///
/// Read-only on the directory, and takes no lock: it reads the published
/// table and writes somewhere else entirely.
fn sectors(dir: &Path, out: Option<&Path>, force: bool) {
    let table = match galos_index::names::Table::open(dir) {
        Ok(table) => table,
        Err(e) => {
            eprintln!("cannot read the names table at {}: {e}", dir.display());
            std::process::exit(2);
        }
    };

    // Which names each sector coordinate is claimed by, and which
    // coordinates each name claims.
    let mut votes: BTreeMap<u32, BTreeMap<String, u64>> = BTreeMap::new();
    let mut claims: BTreeMap<String, std::collections::BTreeSet<u32>> =
        BTreeMap::new();
    for row in 0..table.len() {
        let name = table.name_at(row);
        let Some(sector) = sector_words(&name) else { continue };
        let key = galos_index::procedural::sector_key(
            elite_journal::Boxel::of(table.address_at(row)).sector,
        );
        let sector = sector.to_owned();
        *votes.entry(key).or_default().entry(sector.clone()).or_insert(0) += 1;
        claims.entry(sector).or_default().insert(key);
    }

    let mut written = 0usize;
    let mut regions = 0usize;
    let mut added = 0usize;
    let mut changed: Vec<(u32, &'static str, String)> = Vec::new();
    let mut text = String::new();
    for (key, names) in &votes {
        let settled = names
            .iter()
            .filter(|(name, _)| claims[name.as_str()].len() == 1)
            .max_by_key(|(_, rows)| **rows);
        match settled {
            Some((name, _)) => {
                written += 1;
                match galos_index::procedural::sector_at(*key) {
                    Some(held) if held != name => {
                        changed.push((*key, held, name.clone()));
                    }
                    Some(_) => {}
                    None => added += 1,
                }
                text.push_str(&format!("{key},{name}\n"));
            }
            None => regions += 1,
        }
    }

    // What the dictionary holds and this table does not: a directory
    // smaller than the galaxy the file was learned from, which is the
    // ordinary case for anything but a full import. Dropping those entries
    // would put every name under them back into the text at the next fold,
    // so it is refused alongside the renames.
    let lost = galos_index::procedural::sectors()
        .filter(|(key, _)| !votes.contains_key(key))
        .count();

    if (!changed.is_empty() || lost > 0) && !force {
        if !changed.is_empty() {
            eprintln!(
                "{} sector(s) would be renamed, and a rename re-spells \
                 every name already dropped under them:",
                changed.len(),
            );
            for (key, held, found) in changed.iter().take(10) {
                eprintln!("  {key}: {held} would become {found}");
            }
        }
        if lost > 0 {
            eprintln!(
                "{lost} sector(s) the dictionary names are not in this \
                 table, and dropping them puts every name under them back \
                 into the text"
            );
        }
        eprintln!("pass --force to write it anyway");
        std::process::exit(1);
    }

    let out = out.unwrap_or(Path::new("galos_index/data/sectors.csv"));
    if let Err(e) = std::fs::write(out, &text) {
        eprintln!("cannot write {}: {e}", out.display());
        std::process::exit(2);
    }
    eprintln!(
        "{written} sectors, {} bytes to {}; {added} new, {} renamed, \
         {lost} dropped, {regions} coordinates only a hand-authored region \
         claims",
        text.len(),
        out.display(),
        changed.len(),
    );
}

/// The words a procedural name begins with, or [`None`] where it is not
/// one.
///
/// The shape and nothing else: whether the address agrees is
/// `galos_index::procedural`'s business, and this runs before there is a
/// dictionary for it to agree through.
fn sector_words(name: &str) -> Option<&str> {
    let (head, last) = name.rsplit_once(' ')?;
    let digit = last.find(|c: char| c.is_ascii_digit())?;
    let (class, numbers) = last.split_at(digit);
    if class.len() != 1 || !class.as_bytes()[0].is_ascii_uppercase() {
        return None;
    }
    if !numbers.bytes().all(|b| b.is_ascii_digit() || b == b'-') {
        return None;
    }
    let (sector, code) = head.rsplit_once(' ')?;
    let (pair, one) = code.split_once('-')?;
    (pair.len() == 2 && one.len() == 1).then_some(sector)
}

/// Take the directory for as long as this command holds it, or refuse.
///
/// Every command here that *writes* needs it, and for the reason the lock
/// exists: a builder and one of these share scratch paths — the names
/// writer's is `names/.building`, and whoever opens it second removes what
/// the first is streaming into. Measured the hard way: a fold run beside a
/// live import unlinked the import's row file and the build ended in a bare
/// "No such file or directory" three minutes later.
///
/// `ingest` and `build` take the same lock, so either order refuses
/// rather than interleaves.
///
/// `forced` is `--force-lock`, and is for the one thing a refusal cannot
/// tell apart from a live builder: a lock whose process was killed. The
/// refusal names the pid, and clearing one that is still running is two
/// writers over a directory published whole — see [`galos_index::Lock`].
fn held(dir: &Path, forced: bool) -> galos_index::Lock {
    let taken = match forced {
        true => galos_index::Lock::force(dir),
        false => galos_index::Lock::take(dir),
    };
    match taken {
        Ok(lock) => lock,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}

/// Print a summary of a built index directory.
fn status(dir: &Path) {
    let index = match Index::read(dir) {
        Ok(index) => index,
        Err(e) => {
            eprintln!("cannot read index at {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    if index.is_empty() {
        println!("{}: empty index", dir.display());
        return;
    }

    // Walk the cells once for the shape.
    let mut leaves = 0usize;
    let mut deepest = 0u8;
    let mut per_level = [0usize; MAX_LEVEL as usize + 1];
    let mut largest_leaf = 0u64;
    let mut owned = 0u64;
    for cell in index.cells() {
        if cell.is_leaf() {
            leaves += 1;
            largest_leaf = largest_leaf.max(cell.slice_len());
        }
        deepest = deepest.max(cell.id.level);
        per_level[cell.id.level as usize] += 1;
        owned += cell.slice_len();
    }
    let cells = index.len();
    let root = index.root().expect("a non-empty index has a root");
    let systems = root.aggregate.count();

    println!("{}", dir.display());
    println!(
        "  cells         {cells}  ({leaves} leaves, {} internal)",
        cells - leaves
    );
    println!("  levels        0..{deepest}");
    println!("  systems       {systems}");
    if owned == systems {
        println!("  owned         {owned}  (sum of cell slices, matches)");
    } else {
        println!("  owned         {owned}  (MISMATCH: expected {systems})");
    }
    println!("  largest leaf  {largest_leaf} systems");
    match root.aggregate.m_min() {
        Some(m) => println!("  brightest     M_abs {m:.2}"),
        None => println!("  brightest     none"),
    }
    println!("  total flux    {:.3e}  (relative)", root.aggregate.total_flux());

    // On-disk footprint, straight off the filesystem.
    if let Ok(meta) = std::fs::metadata(dir.join(store::INDEX_FILE)) {
        print!("  on disk       index.bin ({:.2} MB)", mib(meta.len()));
        let (count, bytes) = payload_footprint(&dir.join(store::PAYLOAD_DIR));
        print!(", {count} payload files ({:.2} MB)", mib(bytes));
        println!();
    }

    println!("  cells per level:");
    for (level, count) in per_level.iter().enumerate() {
        if *count > 0 {
            println!("    L{level:<2}  {count}");
        }
    }
}

/// How many payload files a directory holds and how many bytes they take.
///
/// Payloads are sharded one directory deep, and a directory published
/// before the sharding still has them loose, so both are walked.
fn payload_footprint(dir: &Path) -> (u64, u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return (0, 0) };
    let mut count = 0;
    let mut bytes = 0;
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            let (n, b) = payload_footprint(&entry.path());
            count += n;
            bytes += b;
        } else {
            count += 1;
            bytes += meta.len();
        }
    }
    (count, bytes)
}

/// Bytes as mebibytes.
fn mib(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0)
}

/// What a comparison was asked to look at.
struct Compare {
    /// Compare the body files too.
    bodies: bool,
    /// Name differing rows rather than counting them.
    detail: bool,
    /// How many of those to name.
    limit: usize,
}

/// What a part of the comparison found.
///
/// Three answers rather than two, because a float that moved in its last
/// bit is not a directory that disagrees: see [`aggregates`].
#[derive(Clone, Copy, PartialEq)]
enum Verdict {
    Same,
    Differ,
}

impl Verdict {
    /// Whichever of the two is the worse news.
    fn and(self, other: Verdict) -> Verdict {
        match self == Verdict::Same && other == Verdict::Same {
            true => Verdict::Same,
            false => Verdict::Differ,
        }
    }
}

/// How far apart two summed `f64` aggregates may be and still be the same
/// derivation.
///
/// A cell's flux is summed in the order its systems were merged, which is
/// a `HashMap` iteration order, so two honest builds of one galaxy differ
/// in the last bits — the crate's own cross-build test compares `index.bin`
/// by its integers alone for exactly this reason. Anything past this is a
/// difference in what was summed rather than in what order.
const DRIFT: f64 = 1e-9;

/// Compare two built index directories.
///
/// Exit code is the answer: 0 they are the same derivation, 1 they differ,
/// 2 one of them could not be read. That is what makes it scriptable, and
/// a 610 GB import checked against a database-built index of the same file
/// is what it is for.
///
/// What is compared, and how:
///
/// - the cell tree, by its integer columns — the cells present, `rank_lo`,
///   `rank_hi`, `child_mask` and each aggregate's count;
/// - the aggregates' summed light, to [`DRIFT`];
/// - every cell's payload, byte for byte, which is the systems it owns and
///   the order it owns them in;
/// - the names table, by count and an order-independent digest, read a
///   chunk at a time so a galaxy's worth is never held;
/// - `populated.bin`, `reaches.bin` and `boosts.bin`, row by row, absent
///   and empty told apart;
/// - the body files, on `--bodies`, which is a file a scanned system.
fn diff(a: &Path, b: &Path, how: Compare) {
    let left = open(a);
    let right = open(b);
    println!("{} vs {}", a.display(), b.display());

    let (verdict, shared) = cells(&left, &right, how.limit);
    let verdict = verdict
        .and(aggregates(&shared))
        .and(payloads(a, b, &shared, how.limit))
        .and(names(a, b, &how))
        .and(tables(a, b, &how));
    let verdict = match how.bodies {
        true => verdict.and(bodies(a, b, how.limit)),
        false => {
            println!("  bodies        not compared (pass --bodies)");
            verdict
        }
    };

    match verdict {
        Verdict::Same => println!("the same derivation"),
        Verdict::Differ => {
            println!("the two directories differ");
            std::process::exit(1);
        }
    }
}

/// Read a directory's index file, or say which one could not be read.
fn open(dir: &Path) -> Index {
    match Index::read(dir) {
        Ok(index) => index,
        Err(e) => {
            eprintln!("cannot read index at {}: {e}", dir.display());
            std::process::exit(2);
        }
    }
}

/// The cells, by the columns that do not drift.
///
/// Answers the cells both sides hold, for everything downstream to compare
/// over: a cell only one side has is already a difference and has no pair
/// to be read against.
fn cells(a: &Index, b: &Index, limit: usize) -> (Verdict, Vec<(Cell, Cell)>) {
    let keyed = |index: &Index| -> BTreeMap<(u8, u64), Cell> {
        index.cells().map(|it| ((it.id.level, it.id.morton()), *it)).collect()
    };
    let left = keyed(a);
    let right = keyed(b);

    let only_left: Vec<_> =
        left.keys().filter(|it| !right.contains_key(*it)).collect();
    let only_right: Vec<_> =
        right.keys().filter(|it| !left.contains_key(*it)).collect();
    let shared: Vec<(Cell, Cell)> = left
        .iter()
        .filter_map(|(key, cell)| right.get(key).map(|it| (*cell, *it)))
        .collect();

    match only_left.is_empty() && only_right.is_empty() {
        true => println!("  cells         {} in both", shared.len()),
        false => {
            println!(
                "  cells         {} in both, {} only in A, {} only in B",
                shared.len(),
                only_left.len(),
                only_right.len(),
            );
            for (level, morton) in
                only_left.iter().chain(&only_right).take(limit)
            {
                println!("                  L{level} {morton:016x}");
            }
        }
    }

    // The integer columns: what a cell holds, how much of it, and where in
    // the ranking its slice sits.
    let mut differing = Vec::new();
    for (left, right) in &shared {
        let same = left.rank_lo == right.rank_lo
            && left.rank_hi == right.rank_hi
            && left.child_mask == right.child_mask
            && left.aggregate.count() == right.aggregate.count();
        if !same {
            differing.push(*left);
        }
    }
    match differing.is_empty() {
        true => println!("  columns       identical"),
        false => {
            println!("  columns       {} cells differ", differing.len());
            for cell in differing.iter().take(limit) {
                println!(
                    "                  L{} {:016x}",
                    cell.id.level,
                    cell.id.morton(),
                );
            }
        }
    }

    let verdict = match only_left.is_empty()
        && only_right.is_empty()
        && differing.is_empty()
    {
        true => Verdict::Same,
        false => Verdict::Differ,
    };
    (verdict, shared)
}

/// The summed light, which is allowed to drift and not to move.
fn aggregates(shared: &[(Cell, Cell)]) -> Verdict {
    let mut worst = 0.0f64;
    let mut faintest = 0.0f32;
    for (left, right) in shared {
        let (x, y) =
            (left.aggregate.total_flux(), right.aggregate.total_flux());
        let scale = x.abs().max(y.abs());
        if scale > 0.0 {
            worst = worst.max((x - y).abs() / scale);
        }
        match (left.aggregate.m_min(), right.aggregate.m_min()) {
            (Some(x), Some(y)) => faintest = faintest.max((x - y).abs()),
            (None, None) => {}
            // One cell owns something bright and the other owns nothing:
            // the columns above have already called that a difference.
            _ => faintest = f32::INFINITY,
        }
    }
    match worst <= DRIFT && faintest == 0.0 {
        true => {
            println!(
                "  aggregates    agree (flux within {worst:.1e}, same M_abs)"
            );
            Verdict::Same
        }
        false => {
            println!(
                "  aggregates    flux differs by {worst:.1e}, M_abs by \
                 {faintest:.3}",
            );
            Verdict::Differ
        }
    }
}

/// Every shared cell's payload, byte for byte.
///
/// Read through [`Index::read_payload`], so a directory part way through
/// the shard migration answers off whichever path it has.
fn payloads(
    a: &Path,
    b: &Path,
    shared: &[(Cell, Cell)],
    limit: usize,
) -> Verdict {
    let mut differing = Vec::new();
    for (cell, _) in shared {
        let left = Index::read_payload(a, cell.id);
        let right = Index::read_payload(b, cell.id);
        match (left, right) {
            (Ok(left), Ok(right)) if left == right => {}
            (Ok(_), Ok(_)) => differing.push(cell.id),
            (left, right) => {
                if let Err(e) = left {
                    eprintln!("cannot read a payload of {}: {e}", a.display());
                    std::process::exit(2);
                }
                if let Err(e) = right {
                    eprintln!("cannot read a payload of {}: {e}", b.display());
                    std::process::exit(2);
                }
            }
        }
    }
    match differing.is_empty() {
        true => {
            println!("  payloads      {} identical", shared.len());
            Verdict::Same
        }
        false => {
            println!(
                "  payloads      {} of {} differ",
                differing.len(),
                shared.len(),
            );
            for id in differing.iter().take(limit) {
                println!(
                    "                  L{} {:016x}",
                    id.level,
                    id.morton()
                );
            }
            Verdict::Differ
        }
    }
}

/// The names table, a chunk at a time.
///
/// By count and digest rather than by holding the table: 200 M entries is
/// tens of gigabytes on each side, and which chunk a system landed in is
/// append order rather than an invariant, so the digest is over the entries
/// and not over the files. `--detail` is the road that names the rows, and
/// it is the one that holds both tables.
fn names(a: &Path, b: &Path, how: &Compare) -> Verdict {
    let left = digest(a).unwrap_or_else(|e| fatal(a, e));
    let right = digest(b).unwrap_or_else(|e| fatal(b, e));
    if left == right {
        println!("  names         {} entries, identical", left.0);
        return Verdict::Same;
    }

    println!("  names         {} entries in A, {} in B", left.0, right.0);
    match how.detail {
        true => {
            rows(
                "names rows",
                Some(entries(a).unwrap_or_else(|e| fatal(a, e))),
                Some(entries(b).unwrap_or_else(|e| fatal(b, e))),
                |it: &NameEntry| it.address,
                how,
            );
        }
        false => println!("                  pass --detail to name the rows"),
    }
    Verdict::Differ
}

/// A names table's count and an order-independent digest of its entries.
///
/// Read off the mapping a row at a time, base and log together, so the
/// digest of a galaxy costs a row and not a table.
fn digest(dir: &Path) -> io::Result<(usize, u64, u64)> {
    let held = galos_index::Names::open(dir)?;
    let (mut count, mut sum, mut xor) = (0usize, 0u64, 0u64);
    for address in held.addresses() {
        let Some(entry) = held.entry_of(address) else {
            continue;
        };
        let mut hasher = DefaultHasher::new();
        entry.address.hash(&mut hasher);
        entry.name.hash(&mut hasher);
        for axis in entry.position {
            axis.to_bits().hash(&mut hasher);
        }
        let hash = hasher.finish();
        count += 1;
        sum = sum.wrapping_add(hash);
        xor ^= hash;
    }
    Ok((count, sum, xor))
}

/// Every row of a names table, for the road that names them.
///
/// The one place a whole table is held: `--detail` is asked for a pair of
/// directories a human is going to read the difference between, not for a
/// galaxy.
fn entries(dir: &Path) -> io::Result<Vec<NameEntry>> {
    let held = galos_index::Names::open(dir)?;
    Ok(held.addresses().filter_map(|at| held.entry_of(at)).collect())
}

/// The three tables a record can fill.
///
/// Each is written sorted by address, so equal content is equal bytes and
/// a row comparison says the same thing as a byte one — but a row
/// comparison can say which system.
fn tables(a: &Path, b: &Path, how: &Compare) -> Verdict {
    let populated = rows(
        "populated",
        table::<PopulatedSystem>(a, &source::populated_path(a)),
        table::<PopulatedSystem>(b, &source::populated_path(b)),
        |it: &PopulatedSystem| it.address,
        how,
    );
    let reaches = rows(
        "reaches",
        table::<SystemReach>(a, &source::reaches_path(a)),
        table::<SystemReach>(b, &source::reaches_path(b)),
        |it: &SystemReach| it.address,
        how,
    );
    let boosts = rows(
        "boosts",
        table::<SystemBoost>(a, &source::boosts_path(a)),
        table::<SystemBoost>(b, &source::boosts_path(b)),
        |it: &SystemBoost| it.address,
        how,
    );
    populated.and(reaches).and(boosts)
}

/// One table, or [`None`] where the directory does not hold it.
///
/// An absent table is not an empty one: it says this index cannot tell,
/// where an empty one says the galaxy has none.
fn table<T: DeserializeOwned>(dir: &Path, path: &Path) -> Option<Vec<T>> {
    match source::read_meta(path) {
        Ok(rows) => Some(rows),
        Err(e) if e.kind() == io::ErrorKind::NotFound => None,
        Err(e) => fatal(dir, e),
    }
}

/// Compare two tables of rows keyed by address.
fn rows<T: PartialEq>(
    label: &str,
    a: Option<Vec<T>>,
    b: Option<Vec<T>>,
    key: impl Fn(&T) -> i64,
    how: &Compare,
) -> Verdict {
    let (mut left, mut right) = match (a, b) {
        (None, None) => {
            println!("  {label:<13} absent from both");
            return Verdict::Same;
        }
        (Some(_), None) => {
            println!("  {label:<13} only A holds one");
            return Verdict::Differ;
        }
        (None, Some(_)) => {
            println!("  {label:<13} only B holds one");
            return Verdict::Differ;
        }
        (Some(left), Some(right)) => (left, right),
    };
    left.sort_by_key(&key);
    right.sort_by_key(&key);

    // Two sorted runs walked together: a row on one side and not the other
    // is a missing system, and one on both that is not the same row is a
    // system the two derivations say different things about.
    let (mut i, mut j) = (0usize, 0usize);
    let (mut only_left, mut only_right) = (0usize, 0usize);
    let mut differing = Vec::new();
    while i < left.len() && j < right.len() {
        let (x, y) = (key(&left[i]), key(&right[j]));
        match x.cmp(&y) {
            std::cmp::Ordering::Less => {
                only_left += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_right += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                if left[i] != right[j] {
                    differing.push(x);
                }
                i += 1;
                j += 1;
            }
        }
    }
    only_left += left.len() - i;
    only_right += right.len() - j;

    if only_left == 0 && only_right == 0 && differing.is_empty() {
        println!("  {label:<13} {} rows, identical", left.len());
        return Verdict::Same;
    }
    println!(
        "  {label:<13} {} rows in A, {} in B: {} only in A, {} only in B, \
         {} differ",
        left.len(),
        right.len(),
        only_left,
        only_right,
        differing.len(),
    );
    if how.detail {
        for address in differing.iter().take(how.limit) {
            println!("                  {address}");
        }
    }
    Verdict::Differ
}

/// The body files, which is a file a scanned system.
fn bodies(a: &Path, b: &Path, limit: usize) -> Verdict {
    let left = Published::new(a).scanned();
    let right = Published::new(b).scanned();

    let mut differing = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    let (mut only_left, mut only_right) = (0usize, 0usize);
    while i < left.len() && j < right.len() {
        match left[i].cmp(&right[j]) {
            std::cmp::Ordering::Less => {
                only_left += 1;
                i += 1;
            }
            std::cmp::Ordering::Greater => {
                only_right += 1;
                j += 1;
            }
            std::cmp::Ordering::Equal => {
                let address = left[i];
                let x = source::read_bodies(a, address)
                    .unwrap_or_else(|e| fatal(a, e));
                let y = source::read_bodies(b, address)
                    .unwrap_or_else(|e| fatal(b, e));
                if x != y {
                    differing.push(address);
                }
                i += 1;
                j += 1;
            }
        }
    }
    only_left += left.len() - i;
    only_right += right.len() - j;

    if only_left == 0 && only_right == 0 && differing.is_empty() {
        println!("  bodies        {} files, identical", left.len());
        return Verdict::Same;
    }
    println!(
        "  bodies        {} files in A, {} in B: {} only in A, {} only in \
         B, {} differ",
        left.len(),
        right.len(),
        only_left,
        only_right,
        differing.len(),
    );
    for address in differing.iter().take(limit) {
        println!("                  {address}");
    }
    Verdict::Differ
}

/// A directory that stopped being readable part way through a comparison.
fn fatal(dir: &Path, e: io::Error) -> ! {
    eprintln!("cannot read {}: {e}", dir.display());
    std::process::exit(2);
}
