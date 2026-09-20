//! Reading what a publisher publishes into Postgres.
//!
//! The command line and the supervisor; the reading itself is
//! [`galos::read`], which is shared with the tool that writes the other
//! store. What is here is what the database half of a run needs and the
//! index half does not: the pool, whether it is opened for a bulk import,
//! and a sink per source onto it.
//!
//! Every `--from` is read at once, each on its own task with a [`Db`] of
//! its own. That is what makes a dump and the feed one run rather than two:
//! a dump being read does not wait on the feed, the feed does not wait on
//! it, and the pool behind them is shared.

use clap::Args;
use galos::read::{self, from, from::Source};
use galos::sink::{Db, Fan, Sink};
use galos::{shutdown, Shard, Shutdown};

/// The heading the flags of a run that never ends are listed under.
const FOLLOWING: &str = "Following what is published";

/// The heading the flags of a run that reads something finite are listed
/// under.
const IMPORTING: &str = "Importing what is already there";

/// The flags of an ingest.
///
/// The two headings the per-source flags are listed under are the two
/// things a run can be, and which one it is decides which of these mean
/// anything: see [`crate::Command::Ingest`], where a reader of `--help`
/// meets them, and [`refused`], where a run that mixed them up is stopped.
#[derive(Args)]
pub struct Ingest {
    /// Where to read from: `eddn`, `spool=DIR`, `journal=PATH`,
    /// `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH` or `spansh=PATH`.
    /// Repeatable; each is read once.
    ///
    /// A spool is a recorded feed — see `eddn record`. It is followed
    /// from this run's own cursor unless it is told otherwise:
    /// `spool=DIR,from=earliest` replays what is held, and
    /// `spool=DIR,since=2026-09-19T12:00:00Z` starts at an hour.
    #[arg(long = "from", value_name = "SOURCE", required = true)]
    from: Vec<Source>,

    /// Whose journal this is, overriding what the files say. Only with
    /// `--from journal=PATH`.
    #[arg(short = 'u', long, value_name = "NAME")]
    user: Option<String>,

    /// Keep following what was named rather than exiting, SECS apart. A
    /// second where SECS is left off.
    ///
    /// The longest a journal directory nothing is writing to is left
    /// alone: the filesystem says when a log was written to, so a journal
    /// is read as soon as the game writes rather than SECS later. EDDN is
    /// a subscription and follows either way.
    ///
    /// Only where there is something to follow: the feed, a spool, or a
    /// journal directory. A dump has an end.
    #[arg(
        long,
        value_name = "SECS",
        num_args = 0..=1,
        default_missing_value = "1",
        help_heading = FOLLOWING,
    )]
    watch: Option<u64>,

    /// EDDN's ZMQ address, the feed's own by default. Only with `--from
    /// eddn`.
    #[arg(short = 'r', long, value_name = "URL", help_heading = FOLLOWING)]
    remote: Option<String>,

    /// Seconds of EDDN silence before the connection is replaced, or 0 to
    /// leave it alone. Only with `--from eddn`.
    #[arg(long, value_name = "SECS", help_heading = FOLLOWING)]
    stall: Option<u64>,

    /// Open the database for a bulk import: commits left unflushed and a
    /// higher connection ceiling. For an import that can be re-run from its
    /// source, not for a live feed.
    #[arg(long, help_heading = IMPORTING)]
    bulk: bool,

    /// Read one share of a source: every Nth record of a dump, or every
    /// Nth file of a journal directory, offset I, so N processes cover it
    /// exactly once between them. A journal shards by file because a file
    /// is what names the commander who flew it. Every write is a guarded
    /// upsert keyed by an address, so shards that overlap cost only time.
    #[arg(
        long,
        value_name = "I/N",
        value_parser = from::shard,
        help_heading = IMPORTING,
    )]
    shard: Option<Shard>,

    /// EDSM's API: take everything in a cube this many light years across.
    /// Only with `--from edsm-api=NAME`.
    #[arg(
        long,
        short,
        value_name = "LY",
        conflicts_with = "sphere",
        help_heading = IMPORTING,
    )]
    cube: Option<u32>,

    /// EDSM's API: take everything within this many light years. Only with
    /// `--from edsm-api=NAME`.
    #[arg(long, short, value_name = "LY", help_heading = IMPORTING)]
    sphere: Option<u32>,
}

/// What these verbs call themselves in a spool's `cursors/` directory.
///
/// One name per consumer, because both verb groups read the same recorded
/// feed: this one following it into Postgres and `galos index ingest`
/// following it into a tree are two readers at two places in one
/// directory, and either writing the other's cursor would skip whatever
/// the other had not read yet. The name outlives the two binaries it was
/// coined for: changing it would abandon every cursor already written
/// under it.
const CONSUMER: &str = "galos-db";

impl Ingest {
    /// The per-source flags, as [`read::Qualifiers`] reads them.
    ///
    /// Both groups' `ingest` takes these same eight flags, and both the
    /// rules about them and the folding of them into a
    /// [`read::Options`] are written once, in `galos::read` — two copies
    /// drift, and what they would drift about is what a command line
    /// means.
    fn qualifiers(&self) -> read::Qualifiers<'_> {
        read::Qualifiers {
            sources: &self.from,
            user: self.user.as_deref(),
            remote: self.remote.as_deref(),
            stall: self.stall,
            watch: self.watch,
            cube: self.cube,
            sphere: self.sphere,
            shard: self.shard,
        }
    }
}

/// Open the pool, read every `--from` into it at once, and close them out.
///
/// The supervisor of an ingest: it installs the SIGINT handler, holds the
/// shutdown token every source reads, and joins them all before it answers
/// — so a Ctrl-C ends in each source's last write and the line it has to
/// say for itself, rather than in a killed process part way through a
/// transaction. The status is every source's status, and `&=` rather than
/// `&&` on purpose: one source failing is not a reason to stop reading the
/// others, it is a reason to exit 1 once they are done.
///
/// One pool, and a [`Db`] per source off it. The sink counts what it wrote
/// for the line at the end of a run, so per-source is how a run of three
/// says which of the three did what; the connections underneath are the
/// pool's and are shared.
pub async fn run(cli: Ingest) -> Result<bool, String> {
    // Every rule is a shared one: `--bulk` is the only flag here the
    // other tool does not have, and there is nothing to refuse it for —
    // every run of this verb writes a database.
    cli.qualifiers().refused()?;

    let shutdown = Shutdown::new();
    shutdown::on_interrupt(shutdown.clone());

    let db = super::open(cli.bulk).await?;

    let options = cli.qualifiers().options(CONSUMER);
    let mut reading = Vec::with_capacity(cli.from.len());
    for source in cli.from {
        // A fan of one. [`read::collect`] writes to whatever it is handed
        // and a fan is what it is handed; it is also where `finish` and the
        // closing line live, so a single sink goes through it rather than
        // around it.
        let sinks: Vec<Box<dyn Sink>> = vec![Box::new(Db::new(db.clone()))];

        let (options, shutdown) = (options.clone(), shutdown.clone());
        reading.push(async_std::task::spawn(async move {
            read::collect(source, options, Fan::of(sinks), shutdown).await
        }));
    }

    let mut ok = true;
    for task in reading {
        ok &= task.await;
    }
    Ok(ok)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    /// The flags of an ingest, as the command line would have parsed them
    ///
    /// Through the whole command line rather than through [`Ingest`]
    /// alone, which cannot parse itself: it is a `clap::Args`, and what
    /// turns argv into one is the verb it hangs off — two levels of verb
    /// now, `galos db ingest`, which is exactly what this should be
    /// asking.
    fn cli(said: &[&str]) -> Ingest {
        match parsed(said).expect("these flags should parse") {
            crate::db::Command::Ingest(it) => it,
            _ => unreachable!("the verb is `ingest`"),
        }
    }

    /// What `galos db ingest` makes of these flags, refusals and all.
    fn parsed(said: &[&str]) -> Result<crate::db::Command, clap::Error> {
        let mut argv = vec!["galos", "db", "ingest"];
        argv.extend_from_slice(said);
        crate::Cli::try_parse_from(argv).map(|cli| match cli.command {
            crate::Command::Db(db) => db.command,
            _ => unreachable!("the group is `db`"),
        })
    }

    /// The shared rules are reached from this verb's flags
    ///
    /// One test rather than one per rule: what each rule *means* is
    /// pinned in `galos::read`, where it is written, and what is this
    /// tool's business is that every qualifier on the command line
    /// arrives there. A flag left out of [`Ingest::qualifiers`] would be
    /// a flag silently ignored, which is the failure this catches.
    #[test]
    fn the_shared_rules_are_reached_from_here() {
        let named = cli(&["--from", "eddn", "--user", "HRC-2"]);
        let Err(said) = named.qualifiers().refused() else {
            panic!("a commander named over the feed was accepted")
        };
        assert!(said.contains("--user"), "should say why: {said}");

        let missing = cli(&["--from", "spansh=/tmp/no/such/galaxy.json"]);
        assert!(
            missing.qualifiers().refused().is_err(),
            "a dump that does not exist was accepted",
        );

        let sound = cli(&["--from", "journal=/tmp", "--user", "HRC-2"]);
        assert!(sound.qualifiers().refused().is_ok());
    }

    /// A share of nought, or one past the last, is refused as it is parsed
    ///
    /// Either is a process that reads no record of the file it was pointed
    /// at, and the whole point of the flag is that N of them cover the
    /// file exactly once. Said by the value parser rather than by the
    /// shared rules, which is why this one reads the parse and not a run.
    #[test]
    fn a_share_must_be_one_of_the_shares_there_are() {
        for said in ["0/0", "8/8", "9/8"] {
            let shared = parsed(&["--from", "eddn", "--shard", said]);
            assert!(shared.is_err(), "`--shard {said}` was accepted");
        }
        assert!(
            parsed(&["--from", "spansh=Cargo.toml", "--shard", "7/8"]).is_ok()
        );
    }

    /// A watch with no seconds is a second
    ///
    /// The flag takes an optional value, and what it means without one is
    /// the poll a journal directory is followed at. Given as a bare flag
    /// it must not parse as "no watch at all", which is what an
    /// `Option<u64>` with no `default_missing_value` would have done.
    #[test]
    fn a_watch_with_no_seconds_is_a_second() {
        assert_eq!(cli(&["--from", "eddn", "--watch"]).watch, Some(1));
        assert_eq!(cli(&["--from", "eddn", "--watch", "30"]).watch, Some(30));
        assert_eq!(cli(&["--from", "eddn"]).watch, None);
    }
}
