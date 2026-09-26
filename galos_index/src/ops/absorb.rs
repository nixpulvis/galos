//! One index directory folded into another, newest record winning.
//!
//! The engine behind `galos index merge -i INTO --from FROM`, and it exists
//! for one runbook. A live directory goes wrong; collection is pointed at a
//! fresh one so the feed keeps landing somewhere; the old one is repaired;
//! and the two then have to be made one without reading the galaxy again. A
//! re-import is six hundred gigabytes and most of a day. This is the two
//! directories' own records, folded.
//!
//! ## What is merged, and by what clock
//!
//! [`System::updated_at`] is the one clock a directory merge has. It is Unix
//! seconds, it is on every record, and it is what the tree already sorts
//! Recency by — so the rule is the rule the rest of the program already states
//! twice: **the newer record wins, a tie goes to the arriving one, and an
//! absence never contradicts.** See
//! [`SystemReport::over`](crate::accumulate::report::SystemReport::over) for a
//! system's own columns and [`crate::accumulate::merge`] for the things inside
//! it; both use `>=` against the stamp, as the database's `CASE WHEN $n >=
//! t.updated_at` upserts do, there being nothing to choose between two records
//! of the same moment and one of them having to win.
//!
//! Everything else follows the system record rather than deciding for
//! itself. A [`NameEntry`](crate::records::NameEntry) carries no stamp at all,
//! so a name crosses exactly when its system's record crossed; a sidecar row
//! is weighed in whichever direction the system record settled. The bodies
//! are the one thing with a clock of their own, and they are merged per body
//! — see [`merge::bodies_over`].
//!
//! **Except what is a function of the system's whole contents.** Record over
//! record is the wrong rule for a fact nobody reported — the kind of star a
//! ship arrives at, the light of the system, how far it reaches, what it
//! supercharges. Each of those was worked out from one side's bodies, and the
//! merged directory holds both sides', so each is worked out again over the
//! merged contents by the calls [`crate::accumulate::galaxy`] makes. See
//! [`Relit`], which names the directory that said two contradicting things
//! before this existed. It is why the bodies are folded *first*: the contents
//! have to be settled before the record over them can be written.
//!
//! **Nothing is ever withdrawn.** An absence on the incoming side says "I
//! have not heard", never "it is gone", which is `src/sink/tables.rs`'s rule
//! and holds here for the same reason: a feed cannot tell a system that has
//! emptied from one nobody has mentioned. So a merge only ever adds.
//!
//! ## Why the resume points and not the directories
//!
//! Both sides are read through [`Checkpoint`] and a directory without one is
//! refused rather than worked around. It has to be. What a directory *serves*
//! is a lossy projection — a payload downcasts the magnitude to `f32`, buckets
//! the temperature into six buckets and drops the age entirely
//! ([`crate::format::checkpoint`], `src/sink/index.rs`) — so a merge that read
//! the two directories instead of their resume points would silently coarsen
//! every system it carried, and the damage would be invisible until somebody
//! filtered by age. The full-precision inputs live in the resume point and
//! nowhere else.
//!
//! ## What is held
//!
//! Not the galaxy. `INTO`'s base is walked where it lies, in the mapping
//! [`Checkpoint::base`] hands out, and is never collected. What is held is
//! proportional to the *incoming* directory, which in the runbook above is
//! the small one: four bytes a record of sort order over `FROM`, one bit a
//! record of what has been consumed, and eight bytes for each address
//! `FROM` won. The sidecar tables are the exception and are the existing
//! cost of any publish — [`Sidecars::resume`] already holds the directory's
//! own three — with the incoming directory's three read beside them.
//!
//! ## What an interrupted merge leaves, and why it is safe
//!
//! This is the whole of an operator's confidence in the verb, so it is
//! stated rather than implied.
//!
//! The merged resume point is written beside `INTO`'s and renamed over it,
//! which is one step. The instant that rename lands, `INTO`'s resume point
//! holds the union at full precision and the tree it serves is *behind* it —
//! so the very next thing this does is unlink `index.bin`, and a directory
//! with no index file reads as nothing at all. Everything after that point
//! — the names, the sidecars, the rebuild — happens to a directory that is
//! already serving nothing, and a kill anywhere in it leaves a directory
//! serving nothing beside a resume point holding everything.
//!
//! **The bodies pass is in front of that rename and does write.** It has to be:
//! what it merges to is what the records are derived over. What it leaves is
//! nonetheless safe, and for a different reason — it never takes anything away.
//! [`merge::bodies_over`] keeps every body of both sides, so a kill inside it
//! leaves `INTO` serving every body record it was serving and some of `FROM`'s
//! besides. Nothing is lost. What is *not* true is that it is invisible: until
//! the union lands, those systems' records were derived before their own body
//! files, which is the disagreement `galos index verify` reports. Re-running
//! the merge settles it — the relight is worked out from the directory's own
//! merged contents, so the second run reaches the same answer the first was
//! going to.
//!
//! That state is recoverable two ways and neither of them reads a dump.
//! Running the merge again is idempotent: every `FROM` record is already in
//! `INTO`'s resume point and no older, so the union is the same union, the
//! names and sidecars are already there and change nothing, and the body
//! records merge to what they already hold. And `galos ingest --index INTO
//! --watch` takes it up as it stands — `agrees` (`src/sink/index.rs`)
//! explicitly permits a directory serving nothing beside a full resume
//! point, that being the one shape it cannot repair the other way round.
//!
//! What must **not** happen is the reverse: a directory serving the old tree
//! beside a resume point holding the union. A follower would resume the
//! union, publish deltas against a tree that is missing half of it, and
//! nothing anywhere would say so. That is why `index.bin` comes down the
//! moment the resume point is trusted and goes back up last of all, which is
//! also the order [`Build::finish`] writes in.
//!
//! ## The dry run
//!
//! `dry_run` does the refusals and all of the counting and writes nothing at
//! all: no compaction, no delta append, no body record, no rebuild. It is
//! what the operator runs first, and it reads both directories and neither
//! changes.

use crate::accumulate::bodies::{Bodies, OnDisk};
use crate::accumulate::merge;
use crate::build::cold::{
    Build, Built, OnStop, ResumeMark, Start, Summary, region_budget,
    resume_mark,
};
use crate::build::snapshot::BuildParams;
use crate::core::record::{Boost, StarKind, System};
use crate::format::checkpoint::{Checkpoint, Compaction, Provenance};
use crate::format::{layout, msgpack};
use crate::records::{
    Faction, PopulatedSystem, SystemBodies, SystemBoost, SystemReach, derive,
};
use crate::store::names::Names;
use crate::store::sidecars::{Moved, Sidecars};
use crate::store::{bodies, cells};
use chrono::NaiveDateTime;
use galos_photometry::{Magnitude, Temperature};
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

/// How many systems of the union go by between one word and the next.
///
/// A million, which over a galaxy is some two hundred words and over a test
/// is none. The union is a binary search and a 64-byte append per record, so
/// a word costs nothing beside it and the point is only that a pass of
/// minutes is not a terminal of silence.
const SAID_SYSTEMS: u64 = 1 << 20;

/// How many systems of the bodies pass go by between one word and the next.
///
/// Four thousand, three hundred times finer than the union's, because a
/// system here is two file reads and a decode of ~2.4 KB rather than eight
/// bytes of comparison: the pass is minutes where the union is seconds.
const SAID_BODIES: u64 = 1 << 12;

/// What a fold came to.
///
/// Counts rather than contents, as [`Summary`] is: what this describes is
/// a galaxy.
#[derive(Copy, Clone, Debug)]
pub struct Absorbed {
    /// Systems `INTO` held, its own log folded into its base.
    pub held: u64,
    /// Systems `FROM` contributed that `INTO` did not have at all.
    pub taken: u64,
    /// Systems of `INTO` that a newer record of `FROM` replaced.
    pub replaced: u64,
    /// Records of `FROM` refused as older than the one `INTO` held.
    ///
    /// Not a failure and not a loss: it is the merge working. A high count
    /// says the two directories overlap and `INTO` is the fresher of them.
    pub refused: u64,
    /// Names carried across, which is one per system `FROM` won that it had
    /// a name for and `INTO` did not already spell the same way.
    pub names: u64,
    /// Rows of `populated.bin` the fold changed.
    pub populated: u64,
    /// Rows of `reaches.bin` the fold changed.
    pub reaches: u64,
    /// Rows of `boosts.bin` the fold changed.
    pub boosts: u64,
    /// Factions `INTO`'s table did not name and `FROM`'s did.
    pub factions: u64,
    /// Systems whose body records were folded together.
    pub bodies: u64,
    /// The cursor the merged resume point carries — see [`older`].
    pub cursor: Option<NaiveDateTime>,
    /// What raising the tree again came to, absent for a dry run.
    pub rebuilt: Option<Summary>,
    /// Whether this only counted. A dry run writes nothing at all.
    pub dry_run: bool,
}

impl Absorbed {
    /// How many systems the merged directory holds.
    pub fn systems(&self) -> u64 {
        self.held + self.taken
    }
}

impl fmt::Display for Absorbed {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}{} held + {} taken -> {} systems ({} replaced, {} refused \
             as older), {} names, {} populated, {} reaches, {} boosts, \
             {} factions, {} systems of bodies; current to {}{}",
            match self.dry_run {
                true => "would fold: ",
                false => "",
            },
            self.held,
            self.taken,
            self.systems(),
            self.replaced,
            self.refused,
            self.names,
            self.populated,
            self.reaches,
            self.boosts,
            self.factions,
            self.bodies,
            match self.cursor {
                Some(at) => at.to_string(),
                None => "no clock".to_owned(),
            },
            match &self.rebuilt {
                Some(built) => format!(
                    "; the tree raised again over {} systems into {} cells",
                    built.systems, built.cells,
                ),
                None => String::new(),
            },
        )
    }
}

/// Why two directories were not merged.
///
/// Four of these are refusals in `one_hand`'s sense — the merge will not be
/// attempted, nothing has been written and both directories stand exactly as
/// they were found. [`Refused::Stopped`] and [`Refused::Failed`] are the
/// other kind: they can land after the union has been committed, and each
/// says what the directory is left holding.
#[derive(Debug)]
pub enum Refused {
    /// A directory this build does not read.
    Stale {
        /// Which of the two.
        dir: PathBuf,
        /// The format it says it is.
        version: u16,
    },
    /// A directory with nothing to merge from.
    Adrift {
        /// Which of the two.
        dir: PathBuf,
        /// The resume point that is missing or will not read.
        checkpoint: PathBuf,
        /// What reading it said.
        why: io::Error,
    },
    /// Two resume points written by different derivations.
    TwoHands {
        into: PathBuf,
        from: PathBuf,
        /// What `INTO`'s was derived by, `FROM`'s being the other.
        wrote: Provenance,
    },
    /// Two faction tables that number the same faction differently.
    Factions {
        /// The id both tables use.
        id: i32,
        /// What `INTO` calls it.
        into: String,
        /// What `FROM` calls it.
        from: String,
    },
    /// An incoming directory of more records than the sort order can index.
    TooMany { dir: PathBuf, records: usize },
    /// The run was asked to stop.
    Stopped {
        /// Which pass it was in.
        phase: Phase,
        /// Whether the merged resume point had already landed, which is
        /// what decides whether the directory serves anything until the
        /// merge is run again.
        ///
        /// **Not the same as "anything was written".** The bodies pass
        /// runs in front of the rename and does write, so a stop there
        /// is `false` and still leaves body records behind — it takes
        /// nothing away, which is why it is safe, and the `Display` for
        /// that one says so rather than claiming an untouched pair.
        committed: bool,
    },
    /// Something broke part way, named by which part.
    Failed { what: &'static str, why: io::Error },
}

impl fmt::Display for Refused {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Refused::Stale { dir, version } => write!(
                f,
                "{} is a directory of format {version} and this build \
                 serves {}: bring it forward with `galos index migrate -i \
                 {}` and merge again",
                dir.display(),
                crate::INDEX_VERSION,
                dir.display(),
            ),
            Refused::Adrift { dir, checkpoint, why } => write!(
                f,
                "{} has no resume point this build can read ({}: {why}), \
                 and a merge cannot be made from what it serves: the \
                 published payload downcasts each system's magnitude, \
                 buckets its temperature into six buckets and drops its age \
                 altogether, so folding one directory's cells into another \
                 would quietly coarsen every system it carried. Merge a \
                 directory that has its resume point beside it, or rebuild \
                 this one with `galos ingest --index {}`",
                dir.display(),
                checkpoint.display(),
                dir.display(),
            ),
            Refused::TwoHands { into, from, wrote } => {
                let (held, said, wanted) = match wrote {
                    Provenance::Database => ("a database", "a feed", from),
                    Provenance::Events => ("a feed", "a database", into),
                };
                write!(
                    f,
                    "{} was derived from {held} and {} from {said}, and a \
                     cursor means a different thing on each side, so the \
                     merged one would be neither: the older clock would \
                     either skip every row in the gap between the two or \
                     re-read the galaxy. Rebuild {} the other way, or keep \
                     the two directories apart",
                    into.display(),
                    from.display(),
                    wanted.display(),
                )
            }
            Refused::Factions { id, into, from } => write!(
                f,
                "faction {id} is {into:?} in one directory and {from:?} in \
                 the other, so the two were fed by different databases: a \
                 faction's id comes from a sequence `galos_db` mints on \
                 write, and a union of the two tables would colour the map \
                 by the wrong faction. Merge directories fed by one \
                 database, which is what a collection moved to a fresh \
                 store is",
            ),
            Refused::TooMany { dir, records } => write!(
                f,
                "{} holds {records} records and the merge orders the \
                 incoming directory with a 32-bit index, which reaches \
                 {}: merge it in halves, or raise this one afresh",
                dir.display(),
                u32::MAX,
            ),
            // The bodies pass is the one that writes in front of the
            // union, so it is the one this cannot say "nothing was
            // written" about. What it may leave is body records and
            // nothing else — see [`merge::bodies_over`], which takes nothing
            // away — and the tree those systems' records were derived
            // over is then older than their own body files, which is
            // what `galos index verify` reports and what re-running
            // settles.
            Refused::Stopped { phase: Phase::Bodies, committed: false } => {
                write!(
                    f,
                    "stopped during the bodies. Neither directory's tree \
                     or resume point was touched and nothing was taken \
                     away; what may stand is body records carried over \
                     early, which the same merge run again takes up",
                )
            }
            Refused::Stopped { phase, committed: false } => write!(
                f,
                "stopped during {phase}; nothing was written and both \
                 directories are as they were found",
            ),
            Refused::Stopped { phase, committed: true } => write!(
                f,
                "stopped during {phase}. The union is already in the \
                 resume point at full precision and the directory serves \
                 nothing until a tree is raised over it: run the same \
                 merge again, which is idempotent, or `galos ingest \
                 --index DIR` on it, which takes a directory in exactly \
                 this state up",
            ),
            Refused::Failed { what, why } => write!(f, "{what}: {why}"),
        }
    }
}

/// The I/O failure underneath, where there is one, so a caller reporting
/// the chain says what the filesystem said as well as which step it was.
impl std::error::Error for Refused {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Refused::Adrift { why, .. } | Refused::Failed { why, .. } => {
                Some(why)
            }
            _ => None,
        }
    }
}

/// Which pass a fold is in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Phase {
    /// The union of the two resume points, by [`System::updated_at`].
    Systems,
    /// The names of the systems the incoming directory won.
    Names,
    /// `populated.bin`, `reaches.bin`, `boosts.bin` and `factions.bin`.
    Sidecars,
    /// The body records, merged per body.
    Bodies,
    /// The tree raised again off the merged resume point.
    Rebuilding,
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            Phase::Systems => "systems",
            Phase::Names => "names",
            Phase::Sidecars => "sidecars",
            Phase::Bodies => "bodies",
            Phase::Rebuilding => "the rebuild",
        })
    }
}

/// How far a fold has got, for a caller with a terminal.
///
/// Handed over as each pass begins and then periodically within the two that
/// are minutes over a galaxy. A pass that cannot say how much there is to do
/// answers `total` of nought — the bodies pass learns how many systems the
/// incoming directory has scanned by walking them.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Folding {
    pub phase: Phase,
    /// Records of this pass that have gone by.
    pub done: u64,
    /// Records this pass has to get through, or nought where it cannot say.
    pub total: u64,
}

impl fmt::Display for Folding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match (self.done, self.total) {
            (0, 0) => write!(f, "{}", self.phase),
            (done, 0) => write!(f, "{} {done}", self.phase),
            (done, total) => write!(f, "{} {done}/{total}", self.phase),
        }
    }
}

/// Fold the directory `from` into the directory `into`, newest record
/// winning, and leave `into` as though one run had seen both feeds.
///
/// `into_checkpoint` and `from_checkpoint` are the two resume points, which
/// live beside their directories rather than in them and are what this
/// actually merges — see the module header for why a directory's own cells
/// will not do.
///
/// `stop` is asked between records of the union and between systems of the
/// bodies pass, which are the two places the time goes. It is **not** asked
/// during the rebuild: [`Build::finish`] says why at length, and the short
/// of it is that stopping between the payloads and the index file over them
/// is the one state the format is careful never to publish. A caller
/// unwilling to wait has the second Ctrl-C, and what that leaves is the
/// state the module header describes.
///
/// `said` is handed each pass as it begins and then periodically within the
/// two long ones.
///
/// `dry_run` counts and writes nothing.
pub fn absorb(
    into: &Path,
    into_checkpoint: &Path,
    from: &Path,
    from_checkpoint: &Path,
    dry_run: bool,
    stop: &dyn Fn() -> bool,
    said: &mut dyn FnMut(&Folding),
) -> Result<Absorbed, Refused> {
    for dir in [into, from] {
        if let Some(version) = cells::stale(dir) {
            return Err(Refused::Stale { dir: dir.to_owned(), version });
        }
    }
    let held = point(into, into_checkpoint)?;
    let incoming = point(from, from_checkpoint)?;
    if held.by != incoming.by {
        return Err(Refused::TwoHands {
            into: into.to_owned(),
            from: from.to_owned(),
            wrote: held.by,
        });
    }
    let by = held.by;
    let fresh = agreed(&factions(into)?, &factions(from)?)?;

    // The bodies first, because what they merge to is what the system
    // record has to be derived over — see [`Relit`] and [`carry_bodies`].
    let (bodies, mut relit) = carry_bodies(into, from, dry_run, stop, said)?;
    let union = unite(
        into,
        into_checkpoint,
        from,
        &held,
        &incoming,
        &mut relit,
        dry_run,
        stop,
        said,
    )?;
    let cursor = union.cursor;
    // The mappings of both bases, and `INTO`'s has just been renamed over.
    // Nothing reads either again; the rebuild opens the new one by path.
    drop(held);
    drop(incoming);

    let names = carry_names(into, from, &union.won, dry_run, said)?;
    let carried =
        carry_sidecars(into, from, &union.won, &relit, fresh, dry_run, said)?;
    let rebuilt = match dry_run {
        true => None,
        false => Some(rebuild(into, into_checkpoint, by, cursor, stop, said)?),
    };

    Ok(Absorbed {
        held: union.held,
        taken: union.taken,
        replaced: union.replaced,
        refused: union.refused,
        names,
        populated: carried.populated,
        reaches: carried.reaches,
        boosts: carried.boosts,
        factions: carried.factions,
        bodies,
        cursor,
        rebuilt,
        dry_run,
    })
}

/// Say which part of a merge an error came out of.
///
/// [`crate::build::cold`]'s own `step`, in this module's vocabulary: a merge
/// over a galaxy is minutes to hours, `std::fs` errors name no path, and a bare
/// "No such file or directory" at the end of one is not worth reading.
fn failed(what: &'static str) -> impl Fn(io::Error) -> Refused {
    move |why| Refused::Failed { what, why }
}

/// One directory's resume point, or the refusal that it has none.
fn point(dir: &Path, checkpoint: &Path) -> Result<Checkpoint, Refused> {
    Checkpoint::read(checkpoint).map_err(|why| Refused::Adrift {
        dir: dir.to_owned(),
        checkpoint: checkpoint.to_owned(),
        why,
    })
}

/// A published table, or no rows where the directory has no such file.
///
/// A sidecar an older build never wrote is an absence rather than a failure,
/// which is [`Sidecars::resume`]'s rule; a table that is there and will not
/// decode is an error, for that rule's other half.
fn table<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>, Refused> {
    match msgpack::read_meta::<Vec<T>>(path) {
        Ok(rows) => Ok(rows),
        Err(why) if why.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(why) => Err(Refused::Failed { what: "a published table", why }),
    }
}

/// What a directory's faction table names.
fn factions(dir: &Path) -> Result<Vec<Faction>, Refused> {
    table(&layout::factions_path(dir))
}

/// The factions `FROM` names that `INTO` does not, or the refusal that the
/// two tables number one faction differently.
///
/// A faction's id comes from a sequence `galos_db` mints when the row is first
/// written (`crate::accumulate::galaxy`, and [`Sidecars::add_factions`], which
/// only ever appends because of it). Two directories fed by the *same* database
/// agree on every id, which is the operator's actual case. Two fed by different
/// ones do not, and a union of those tables would put one database's name on
/// the other's id and colour the map by the wrong faction — silently, a faction
/// id being a number the client looks up and never checks.
fn agreed(
    ours: &[Faction],
    theirs: &[Faction],
) -> Result<Vec<Faction>, Refused> {
    let held: HashMap<i32, &str> =
        ours.iter().map(|it| (it.id, it.name.as_str())).collect();
    let mut fresh = Vec::new();
    for said in theirs {
        match held.get(&said.id) {
            Some(&name) if name != said.name => {
                return Err(Refused::Factions {
                    id: said.id,
                    into: name.to_owned(),
                    from: said.name.clone(),
                });
            }
            Some(_) => {}
            None => fresh.push(said.clone()),
        }
    }
    fresh.sort_unstable_by_key(|it| it.id);
    fresh.dedup_by_key(|it| it.id);
    Ok(fresh)
}

/// The cursor a merged resume point carries: the older of the two, and none
/// at all where either side has none.
///
/// The merged directory is only current as of the older of the two clocks,
/// and claiming the newer would skip every row that changed between them —
/// which is the one failure a resume point exists to prevent. Re-reading the
/// overlap costs nothing: every write path on both derivations is a guarded
/// upsert, so a row read twice is a row written once.
///
/// A side with no clock at all is not an older clock, it is no clock, and a
/// merged point that claimed the other side's would say it was current as of
/// a moment nothing had read to.
fn older(
    ours: Option<NaiveDateTime>,
    theirs: Option<NaiveDateTime>,
) -> Option<NaiveDateTime> {
    match (ours, theirs) {
        (Some(ours), Some(theirs)) => Some(ours.min(theirs)),
        _ => None,
    }
}

/// One resume point's records as one sequence: its base, and the log that
/// stands over it.
///
/// Neither half is copied. The base is the mapping [`Checkpoint::base`]
/// hands out and the log is the `Vec` the read decoded; this is only the
/// arithmetic that makes the two read as one.
struct Flat<'c> {
    base: &'c [System],
    log: &'c [System],
}

impl<'c> Flat<'c> {
    fn of(point: &'c Checkpoint) -> Flat<'c> {
        Flat { base: point.base(), log: point.deltas() }
    }

    fn len(&self) -> usize {
        self.base.len() + self.log.len()
    }

    fn at(&self, i: usize) -> &'c System {
        match self.base.get(i) {
            Some(system) => system,
            None => &self.log[i - self.base.len()],
        }
    }

    fn id(&self, i: usize) -> u64 {
        self.at(i).id64
    }
}

/// A bit a record, for "this one has been dealt with".
///
/// A `Vec<bool>` is a byte a record, which over the incoming galaxy is 200
/// MB against 25. Nothing clever beyond that.
struct Bits(Vec<u64>);

impl Bits {
    fn new(records: usize) -> Bits {
        Bits(vec![0; records.div_ceil(64)])
    }

    fn set(&mut self, at: usize) {
        self.0[at / 64] |= 1 << (at % 64);
    }

    fn has(&self, at: usize) -> bool {
        self.0[at / 64] >> (at % 64) & 1 == 1
    }
}

/// The incoming records in ascending `id64`, one `u32` each, the later
/// record winning where an id appears twice.
///
/// Four bytes a record of the *incoming* directory, which is the one the
/// runbook makes small — 800 MB at a whole galaxy, against 12.8 GB if the
/// records themselves were collected. They are not: they are already mapped,
/// and this is an order over them.
///
/// The sort is by id and then by *descending* position, so the last record
/// an id has sorts first within its run and [`Vec::dedup_by`], which keeps
/// the first of each run, keeps it. That is the same "later wins" the log
/// replay has — [`Tree::apply`](crate::Tree::apply) over a base — done in
/// place rather than into a second vector.
fn ordered(theirs: &Flat) -> Vec<u32> {
    let mut order: Vec<u32> = (0..theirs.len() as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        theirs.id(a as usize).cmp(&theirs.id(b as usize)).then(b.cmp(&a))
    });
    order.dedup_by(|a, b| theirs.id(*a as usize) == theirs.id(*b as usize));
    order
}

/// What the union came to, and which addresses the incoming side won.
#[derive(Default)]
struct Union {
    held: u64,
    taken: u64,
    replaced: u64,
    refused: u64,
    /// The addresses `FROM` took or replaced, ascending. What the names, the
    /// sidecars and nothing else are decided by.
    won: Vec<i64>,
    cursor: Option<NaiveDateTime>,
}

/// What a system's *merged contents* say about it, worked out again.
///
/// Record-over-record is the right rule for everything a report states — a
/// name, a place, a population, the politics, the stamp. It is the wrong
/// rule for the handful of facts that are a function of a system's whole
/// insides, because the winning record was derived over one side's bodies
/// and the merged directory holds both sides'.
///
/// Measured on two journal feeds of one system scanned with a different
/// body id on each side: the merged directory held both stars — body 0 a G
/// star, body 1 a neutron star — and carried the winner's derived columns
/// whole, so its payload and its `boosts.bin` row described a neutron
/// arrival star while its own body file said body 0 was a G star at 4.83.
/// One directory saying two contradicting things, which is exactly what
/// `galos index verify` exists to catch.
///
/// So these five are derived again, over the merged contents, by the same calls
/// [`crate::accumulate::galaxy`] makes and not by arithmetic of this module's
/// own — `Galaxy::system`, `Galaxy::reach_of` and `Galaxy::boost_of`. What
/// stays the winner's is `age_bucket` and `updated_at`: those are about when
/// the system was reported, not about what is in it.
#[derive(Copy, Clone, Debug)]
struct Relit {
    /// What kind of star a ship arrives at, over the merged stars.
    kind: StarKind,
    absolute_magnitude: f64,
    temperature: f64,
    /// How far the merged contents reach, [`SystemBodies::extent`].
    reach: Option<f32>,
    /// What the merged arrival star supercharges, where it supercharges.
    boost: Option<Boost>,
    /// Where the winning record puts the system, filled in by the union.
    ///
    /// A supercharge row carries a place — a router's question is where the
    /// cones are — and the place is the system's own, which only the
    /// records know. [`None`] until the union has written the record, and
    /// for an address neither resume point holds at all.
    position: Option<[f32; 3]>,
}

/// What one system's merged insides say about it, or [`None`] where nothing
/// in it has been scanned.
///
/// The three recipes are `galaxy.rs`'s, called rather than restated. The
/// one step worth pointing at is the photometry: a scanned magnitude is
/// bolometric, the star's whole output as one figure, and it is carried
/// through [`Magnitude::visual`] before anything sums it. That is where a
/// white dwarf keeps its faint brightness and a neutron star falls to
/// nothing, and leaving it out is invisible in the answer.
///
/// [`derive::lit`]'s fallback class is empty here. It is only reached where
/// the stars sum to no light at all — a system of nothing but black
/// holes — and a merge has no report to take a class from, so the default
/// M dwarf stands in, which is what the events derivation answers for a
/// system with no plotted route either.
fn relit_over(inside: &SystemBodies, address: i64) -> Option<Relit> {
    let class = derive::arrival_class(inside)?;
    let kind = StarKind::of(class);
    let boost = Boost::of(class);
    let stars = inside.stars.iter().map(|star| {
        let t = star.temperature as f64;
        let m = Magnitude(star.absolute_magnitude as f64);
        (m.visual(Temperature(t)).0, t)
    });
    let (absolute_magnitude, temperature) = derive::lit(stars, "");
    Some(Relit {
        kind,
        absolute_magnitude,
        temperature,
        reach: inside.extent(address),
        boost,
        position: None,
    })
}

/// One record as the merged directory holds it: the winner's own columns,
/// with whatever [`Relit`] has to say about its contents laid over them.
///
/// Also takes down where the winner puts the system, which is the place the
/// supercharge row is written at.
fn relight(record: &System, relit: &mut HashMap<i64, Relit>) -> System {
    let mut record = *record;
    let Some(afresh) = relit.get_mut(&(record.id64 as i64)) else {
        return record;
    };
    record.kind = afresh.kind;
    record.absolute_magnitude = afresh.absolute_magnitude;
    record.temperature = afresh.temperature;
    afresh.position = Some([
        record.position[0] as f32,
        record.position[1] as f32,
        record.position[2] as f32,
    ]);
    record
}

/// Fold the two resume points into one, in `INTO`'s place.
///
/// The compaction is written to a sibling temp file and renamed over
/// `INTO`'s base by [`Compaction::finish`], so a merge killed before that
/// rename leaves the resume point that stood whole, and one killed after it
/// leaves the union. There is no state in between.
///
/// `index.bin` comes down the instant the rename lands. See the module
/// header: from here until the rebuild republishes, the directory reads as
/// nothing at all, which is the only shape of it that cannot mislead a
/// follower.
#[allow(clippy::too_many_arguments)]
fn unite(
    into: &Path,
    into_checkpoint: &Path,
    from: &Path,
    held: &Checkpoint,
    incoming: &Checkpoint,
    relit: &mut HashMap<i64, Relit>,
    dry_run: bool,
    stop: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<Union, Refused> {
    let mut out = match dry_run {
        true => None,
        false => Some(
            Compaction::begin(into_checkpoint)
                .map_err(failed("the merged resume point"))?,
        ),
    };
    let walked = walk(&mut out, into, from, held, incoming, relit, stop, say);
    let mut union = match walked {
        Ok(union) => union,
        Err(refused) => {
            // The temp file is the galaxy's size and nothing will read it.
            if let Some(base) = out.take() {
                let _ = base.abandon();
            }
            return Err(refused);
        }
    };
    union.cursor = older(held.cursor, incoming.cursor);
    if let Some(base) = out.take() {
        base.finish(union.cursor, held.by)
            .map_err(failed("the merged resume point"))?;
        match std::fs::remove_file(into.join(layout::INDEX_FILE)) {
            Err(why) if why.kind() != io::ErrorKind::NotFound => {
                return Err(Refused::Failed { what: "the index file", why });
            }
            _ => {}
        }
    }
    Ok(union)
}

/// The union itself: `INTO`'s records in the order they lie, each weighed
/// against the incoming record of the same id, and then whatever of the
/// incoming side is left.
///
/// `INTO`'s own log is folded in as it goes. A base record the log has since
/// restated is written once, at the log's value; a log record naming a
/// system the base never held goes out after the base, at its last value.
/// That is the same thing [`Checkpoint::compact`] would do over
/// `base().chain(deltas())` if it deduplicated, and it has to be done here
/// rather than there because the merged base is written once and a system in
/// it twice is a system in two cells, which is not a tree.
#[allow(clippy::too_many_arguments)]
fn walk(
    out: &mut Option<Compaction>,
    into: &Path,
    from: &Path,
    held: &Checkpoint,
    incoming: &Checkpoint,
    relit: &mut HashMap<i64, Relit>,
    stop: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<Union, Refused> {
    let theirs = Flat::of(incoming);
    if theirs.len() > u32::MAX as usize {
        return Err(Refused::TooMany {
            dir: from.to_owned(),
            records: theirs.len(),
        });
    }
    let order = ordered(&theirs);
    let mut consumed = Bits::new(order.len());

    // Which record of `INTO`'s log answers for an id, so a system the log
    // restated is written at its later value and written once.
    let mut shadow: HashMap<u64, usize> =
        HashMap::with_capacity(held.deltas().len());
    for (j, record) in held.deltas().iter().enumerate() {
        shadow.insert(record.id64, j);
    }
    let mut restated = Bits::new(held.deltas().len());

    let total = (held.base().len() + held.deltas().len() + order.len()) as u64;
    let mut union = Union::default();
    let mut done = 0u64;
    say(&Folding { phase: Phase::Systems, done, total });

    {
        let mut fold = |stood: &System| -> Result<(), Refused> {
            if stop() {
                return Err(Refused::Stopped {
                    phase: Phase::Systems,
                    committed: false,
                });
            }
            union.held += 1;
            let mine = stood.id64;
            let mut arrived = false;
            let chosen = match order
                .binary_search_by(|&i| theirs.id(i as usize).cmp(&mine))
            {
                Ok(at) => {
                    consumed.set(at);
                    let said = theirs.at(order[at] as usize);
                    // A tie goes to the arriving record, as
                    // `SystemReport::over` and `merge::said_by` have it:
                    // there is nothing to choose between two records of the
                    // same second and one of them has to win.
                    match said.updated_at >= stood.updated_at {
                        true => {
                            union.replaced += 1;
                            union.won.push(mine as i64);
                            arrived = true;
                            said
                        }
                        false => {
                            union.refused += 1;
                            stood
                        }
                    }
                }
                Err(_) => stood,
            };
            // The other way a winning record can have been derived over
            // contents the merged directory does not hold: `INTO` scanned
            // this system and `FROM`, which won it, never did — so `FROM`'s
            // record came off a fallback class while `INTO`'s body file
            // holds real stars. The bodies pass never saw it, having walked
            // `FROM`'s pack, so it is read here. Bounded by the overlap,
            // which is bounded by the incoming directory, and a system
            // `INTO` has no record for costs one absent lookup.
            let address = mine as i64;
            if arrived && !relit.contains_key(&address) {
                let inside = bodies::read_bodies(into, address)
                    .map_err(failed("a standing body record"))?;
                if let Some(afresh) = relit_over(&inside, address) {
                    relit.insert(address, afresh);
                }
            }
            if let Some(base) = out.as_mut() {
                base.push(relight(chosen, relit))
                    .map_err(failed("the merged resume point"))?;
            } else {
                // A dry run writes nothing, but the supercharge rows it
                // counts are written at the winning record's place.
                relight(chosen, relit);
            }
            done += 1;
            if done % SAID_SYSTEMS == 0 {
                say(&Folding { phase: Phase::Systems, done, total });
            }
            Ok(())
        };

        for stood in held.base() {
            let stood = match shadow.get(&stood.id64) {
                Some(&j) => {
                    restated.set(j);
                    &held.deltas()[j]
                }
                None => stood,
            };
            fold(stood)?;
        }
        // And the log's own systems: the ones the base never held, each at
        // the last value the log gave it.
        for (j, record) in held.deltas().iter().enumerate() {
            if restated.has(j) || shadow[&record.id64] != j {
                continue;
            }
            fold(record)?;
        }
    }

    for (at, &i) in order.iter().enumerate() {
        if consumed.has(at) {
            continue;
        }
        if stop() {
            return Err(Refused::Stopped {
                phase: Phase::Systems,
                committed: false,
            });
        }
        // No standing record to have been derived over anything, so the
        // only relight is what the bodies pass worked out: `INTO` holding
        // body records for a system its resume point never named would be
        // an orphan, not a scan.
        let said = relight(theirs.at(i as usize), relit);
        if let Some(base) = out.as_mut() {
            base.push(said).map_err(failed("the merged resume point"))?;
        }
        union.taken += 1;
        union.won.push(said.id64 as i64);
        done += 1;
        if done % SAID_SYSTEMS == 0 {
            say(&Folding { phase: Phase::Systems, done, total });
        }
    }
    say(&Folding { phase: Phase::Systems, done, total });

    // Ascending, so the passes behind this one ask it with a binary search
    // rather than building a set of their own.
    union.won.sort_unstable();
    union.won.dedup();
    Ok(union)
}

/// Carry the names of the systems the incoming directory won.
///
/// A [`NameEntry`](crate::records::NameEntry) carries no stamp, so it cannot
/// decide for itself which of two spellings is the later one. It does not
/// have to: a name is a fact about a system, and the system's record has
/// just been decided. **A name crosses exactly when its system's record
/// crossed.**
///
/// Nothing is ever unnamed. A directory that has not heard a name is not a
/// directory saying the name is gone.
///
/// [`Names::name`] compares against the base row before it appends, so a
/// re-run writes nothing: by then the rebuild has folded the delta this
/// appends into a new generation, and the table already spells it that way.
fn carry_names(
    into: &Path,
    from: &Path,
    won: &[i64],
    dry_run: bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<u64, Refused> {
    let total = won.len() as u64;
    say(&Folding { phase: Phase::Names, done: 0, total });
    let mut ours = Names::open(into).map_err(failed("the names table"))?;
    let theirs = Names::open(from).map_err(failed("the names table"))?;
    let mut crossed = 0;
    for &address in won {
        if let Some(entry) = theirs.entry_of(address)
            && ours.name(entry)
        {
            crossed += 1;
        }
    }
    if !dry_run {
        // Onto `INTO`'s `names/delta.bin`, which the rebuild folds into a
        // fresh generation through `names::Writer::onto`.
        ours.publish(into).map_err(failed("the names table"))?;
    }
    say(&Folding { phase: Phase::Names, done: crossed, total });
    Ok(crossed)
}

/// What the sidecar pass moved.
#[derive(Copy, Clone, Debug, Default)]
struct Carried {
    populated: u64,
    reaches: u64,
    boosts: u64,
    factions: u64,
}

/// Fold the incoming directory's metadata tables into `INTO`'s.
///
/// Every row `FROM` publishes is weighed, and which way it is weighed is
/// decided by the system record: a row whose address `FROM` won is the newer
/// word, and one whose address it did not is the older. Not only the won
/// addresses, because the rule is *newest wins and a blank never
/// contradicts* — an older row still fills in a column `INTO` never had a
/// word for, and refusing to look at it would throw that away for nothing.
///
/// **Nothing is withdrawn.** An address `FROM` won but publishes no reach
/// for keeps `INTO`'s reach: a feed cannot tell a system whose scans were
/// forgotten from one nobody has scanned, which is `src/sink/tables.rs`'s
/// rule and is why the three tables are only ever added to here.
///
/// **A reach and a supercharge row are not carried where the system's
/// contents merged.** Both are a function of what is inside the system, so
/// for every address [`Relit`] speaks for they are taken from the
/// re-derivation over the merged contents rather than from either side's
/// published table — see [`Relit`] for the directory that said two
/// contradicting things before this did. Where the merged arrival star
/// supercharges nothing the row is *removed*, which is not a withdrawal by
/// silence: the merged contents state what the arrival star is, and a
/// statement is not an absence.
///
/// The factions are the union of the two, the two having already been held
/// against each other by [`agreed`].
#[allow(clippy::too_many_arguments)]
fn carry_sidecars(
    into: &Path,
    from: &Path,
    won: &[i64],
    relit: &HashMap<i64, Relit>,
    fresh: Vec<Faction>,
    dry_run: bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<Carried, Refused> {
    say(&Folding { phase: Phase::Sidecars, done: 0, total: 0 });
    let (mut ours, absent) =
        Sidecars::resume(into).map_err(failed("the sidecar tables"))?;
    let populated: Vec<PopulatedSystem> = table(&layout::populated_path(from))?;
    let reaches: Vec<SystemReach> = table(&layout::reaches_path(from))?;
    let boosts: Vec<SystemBoost> = table(&layout::boosts_path(from))?;

    let newer = |address: i64| won.binary_search(&address).is_ok();
    let mut moved = Moved::default();
    let mut carried = Carried::default();

    for row in populated {
        let arriving = newer(row.address);
        let merged = match ours.published(row.address) {
            Some(stood) => merge::populated_over(stood, row, arriving),
            None => row,
        };
        if ours.populate(merged) {
            moved.populated = true;
            carried.populated += 1;
        }
    }
    // A reach and a supercharge are one reading each, with nothing inside to
    // fill in, so the weighing is the whole of the rule: the arriving
    // reading where it is the newer, and where `INTO` has no reading at all.
    // An address the relight speaks for is skipped here and answered below,
    // neither side's published row being about the merged contents.
    for row in reaches {
        if relit.contains_key(&row.address) {
            continue;
        }
        let take = ours.reach_of(row.address).is_none() || newer(row.address);
        if take && ours.reach(row.address, row.reach) {
            moved.reaches = true;
            carried.reaches += 1;
        }
    }
    for row in boosts {
        if relit.contains_key(&row.address) {
            continue;
        }
        let take = ours.boost_of(row.address).is_none() || newer(row.address);
        if take && ours.boost(row) {
            moved.boosts = true;
            carried.boosts += 1;
        }
    }
    for (&address, afresh) in relit {
        if let Some(reach) = afresh.reach
            && ours.reach(address, reach)
        {
            moved.reaches = true;
            carried.reaches += 1;
        }
        let changed = match (afresh.boost, afresh.position) {
            (Some(boost), Some(position)) => {
                ours.boost(SystemBoost { address, boost, position })
            }
            // The merged arrival star supercharges nothing, so a row
            // saying it does is wrong rather than merely unheard.
            _ => ours.unboost(address),
        };
        if changed {
            moved.boosts = true;
            carried.boosts += 1;
        }
    }
    carried.factions = fresh.len() as u64;
    if ours.add_factions(fresh) {
        moved.factions = true;
    }

    if !dry_run && moved.any() {
        // A table this directory has no file for at all is written with the
        // ones that moved, for [`Sidecars::resume`]'s reason: a missing
        // supercharge table says "this index cannot say" to a client, and
        // the map refuses a supercharged route over one.
        moved.absorb(absent);
        ours.write(into, moved).map_err(failed("the sidecar tables"))?;
    }
    Ok(carried)
}

/// Fold the incoming directory's body records into `INTO`'s, per body.
///
/// `bodies/<address>` is written whole, so a system scanned on both sides
/// cannot simply take one side's record: that loses whatever the other side
/// scanned. Each system's record is read from both and merged body by body —
/// see [`merge::bodies_over`] for the rule.
///
/// What is walked is the incoming directory's *pack*
/// ([`bodies::each_address`]), one shard index at a time. A directory still
/// holding loose `bodies/<address>.bin` files from before the shards has
/// those moved in by `galos index pack`, which is one rename each and is
/// idempotent; until it has, those systems are not offered here. Reading is
/// through [`bodies::read_bodies`], which answers out of whichever of the
/// three layouts holds the system, so `INTO` half way through a packing is
/// read correctly whatever `FROM` is.
///
/// The writes go through the directory's own store, which batches them by
/// shard — a shard is two appends however many of the held systems fell in
/// it, and neither is a directory operation. It is
/// [`OnDisk::raising`], the store that does not read the disk first,
/// because this has already read the standing record and merged it: there
/// is nothing left underneath for the store to find. Every append leaves
/// the record behind it dead, and [`Build::finish`]'s own sweep gives
/// those bytes back once the index file stands.
///
/// **This runs before the union, and that is the whole reason it is
/// separate from it.** What a system's merged insides come to is what its
/// record has to be derived over — see [`Relit`] — and the union is what
/// writes the record, so the contents have to be settled first. The
/// [`Relit`] map it answers with is bounded by the incoming directory's
/// scanned systems.
///
/// Running before the union means a write before the resume point is
/// committed, and the module header says what that leaves. In short: this
/// pass never takes anything away — [`merge::bodies_over`] keeps every body of
/// both sides — so a kill inside it leaves `INTO` serving every body
/// record it served and some of `FROM`'s besides, beside system records
/// derived before them. Nothing is lost and re-running the merge settles
/// it, the relight being worked out from the directory's own merged
/// contents either way.
fn carry_bodies(
    into: &Path,
    from: &Path,
    dry_run: bool,
    stop: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<(u64, HashMap<i64, Relit>), Refused> {
    say(&Folding { phase: Phase::Bodies, done: 0, total: 0 });
    let mut store = OnDisk::raising(into);
    let mut folded = 0u64;
    let mut relit: HashMap<i64, Relit> = HashMap::new();
    let mut broke: Option<io::Error> = None;

    let finished = {
        let mut each = |address: i64| {
            if broke.is_some() {
                return;
            }
            folded += 1;
            let read = || -> io::Result<SystemBodies> {
                let said = bodies::read_bodies(from, address)?;
                let stood = bodies::read_bodies(into, address)?;
                Ok(merge::bodies_over(stood, said))
            };
            let merged = match read() {
                Ok(inside) => inside,
                Err(why) => {
                    broke = Some(why);
                    return;
                }
            };
            if let Some(afresh) = relit_over(&merged, address) {
                relit.insert(address, afresh);
            }
            if !dry_run {
                // Handed over rather than cloned, and taken once: a store
                // may in principle call an edit twice, and the second call
                // finds the merged record already in place.
                let mut once = Some(merged);
                store.edit(address, &mut |stood| {
                    if let Some(inside) = once.take() {
                        *stood = inside;
                    }
                });
            }
            if folded % SAID_BODIES == 0 {
                say(&Folding { phase: Phase::Bodies, done: folded, total: 0 });
            }
        };
        bodies::each_address(from, stop, &mut each)
            .map_err(failed("the incoming body records"))?
    };

    if let Some(why) = broke {
        return Err(Refused::Failed { what: "an incoming body record", why });
    }
    if !finished {
        return Err(Refused::Stopped {
            phase: Phase::Bodies,
            committed: false,
        });
    }
    if !dry_run {
        store.flush().map_err(failed("the merged body records"))?;
    }
    say(&Folding { phase: Phase::Bodies, done: folded, total: 0 });
    Ok((folded, relit))
}

/// Raise the tree again off the merged resume point.
///
/// The shipped cold path and nothing of its own:
/// [`Start::Resuming`] pushes every merged record back into the buckets and
/// `names::Writer::onto` folds the names base under the delta
/// [`carry_names`] appended, then [`Build::finish`] writes every payload,
/// the resume point, the names generation and the index file last of all,
/// and sweeps the payloads and body records nothing refers to any more.
///
/// **Nothing is pushed into it.** The union is already in the resume point
/// and pushing would double every system, a bucket spill having no reason to
/// deduplicate.
///
/// The mark is `INTO`'s own, kept: it says where the read *behind this
/// directory* got to in its own source, and `FROM`'s is about a different
/// read of a different thing. A directory with no mark at all — which is
/// every directory a database pass built, a database having no place in
/// itself to record — resumes as [`ResumeMark::nowhere`], and
/// [`Build::finish`] writes no mark unless one was given, so whatever stands
/// stands.
fn rebuild(
    into: &Path,
    into_checkpoint: &Path,
    by: Provenance,
    cursor: Option<NaiveDateTime>,
    stop: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&Folding),
) -> Result<Summary, Refused> {
    say(&Folding { phase: Phase::Rebuilding, done: 0, total: 0 });
    let start = Start::Resuming(
        resume_mark(into_checkpoint).unwrap_or_else(ResumeMark::nowhere),
    );
    let build = Build::begin(
        into,
        into_checkpoint,
        BuildParams::default(),
        region_budget(),
        start,
        stop,
    )
    .map_err(failed("the rebuild"))?;
    match build
        .finish(by, cursor, OnStop::Publish)
        .map_err(failed("the rebuild"))?
    {
        Built::Index(report) => Ok(report),
        // Only reachable where the union is empty and the run is already
        // stopping, `OnStop::Publish` publishing whatever was read
        // otherwise.
        Built::Stopped(_) => {
            Err(Refused::Stopped { phase: Phase::Rebuilding, committed: true })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::read::index::Index;
    use crate::records::{Body, NameEntry, Star};
    use crate::store::sidecars::{write_boosts, write_reaches};
    use chrono::{DateTime, Utc};
    use elite_journal::body::{Orbit, Spin};
    use std::collections::BTreeMap;
    use std::ops::ControlFlow;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Somewhere to merge in, removed with the value.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let at = std::env::temp_dir().join(format!(
                "galos-absorb-{}-{}-{}",
                name,
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            let _ = std::fs::remove_dir_all(&at);
            std::fs::create_dir_all(&at).expect("a scratch directory");
            Scratch(at)
        }

        fn join(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Nothing ever asks these to stop, and nothing listens to them.
    fn never() -> bool {
        false
    }

    fn quiet(_: &Folding) {}

    /// One system, spread over the cube by its address and photometered by
    /// its stamp — so which of two records of an address won is visible in
    /// the payload the merged directory publishes.
    fn system(id: u64, at: u32) -> System {
        let spread = |salt: u64| {
            let mut x = id.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ salt;
            x ^= x >> 29;
            x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            x ^= x >> 32;
            (x % 20_000) as f64 - 10_000.0
        };
        System {
            id64: id,
            position: [spread(1), spread(2), spread(3)],
            absolute_magnitude: 4.83 - (at % 97) as f64 / 10.0,
            temperature: 3_000.0 + (at % 13) as f64 * 500.0,
            age_bucket: (at % 8) as u32,
            updated_at: at,
            kind: StarKind::G,
        }
    }

    fn at(seconds: i64) -> Option<NaiveDateTime> {
        Some(DateTime::from_timestamp(seconds, 0).expect("a time").naive_utc())
    }

    /// A directory raised from `systems`, every one named `<tag> <id>`.
    fn raise(
        scratch: &Scratch,
        name: &str,
        systems: &[System],
        tag: &str,
        by: Provenance,
        cursor: Option<NaiveDateTime>,
    ) -> (PathBuf, PathBuf) {
        let dir = scratch.join(name);
        let checkpoint = scratch.join(&format!("{name}.checkpoint"));
        let stop = never;
        let mut build = Build::begin(
            &dir,
            &checkpoint,
            BuildParams::default(),
            region_budget(),
            Start::Fresh,
            &stop,
        )
        .expect("a build");
        for &system in systems {
            let entry = NameEntry {
                address: system.id64 as i64,
                name: format!("{tag} {}", system.id64).into(),
                position: [
                    system.position[0] as f32,
                    system.position[1] as f32,
                    system.position[2] as f32,
                ],
            };
            assert_eq!(
                build.push(system, entry).expect("a push"),
                ControlFlow::Continue(())
            );
        }
        match build.finish(by, cursor, OnStop::Publish).expect("a publish") {
            Built::Index(_) => {}
            Built::Stopped(it) => panic!("nothing asked it to stop: {it}"),
        }
        (dir, checkpoint)
    }

    /// The union the merge is supposed to come to: newest wins, a tie to
    /// the arriving record.
    fn union(xs: &[System], ys: &[System]) -> Vec<System> {
        let mut held: BTreeMap<u64, System> = BTreeMap::new();
        for &x in xs {
            held.insert(x.id64, x);
        }
        for &y in ys {
            match held.get(&y.id64) {
                Some(stood) if y.updated_at < stood.updated_at => {}
                _ => {
                    held.insert(y.id64, y);
                }
            }
        }
        held.into_values().collect()
    }

    /// Hold two directories to being the same derivation: the same cells
    /// owning the same systems.
    ///
    /// Not a comparison of `index.bin`'s bytes. A whole build and a pieced
    /// one sum a cell's aggregates in different orders and differ in the
    /// low mantissa bits of the summed light (`region.rs`), so the integer
    /// columns and the payloads are what can be held equal.
    fn same_tree(ours: &Path, theirs: &Path) {
        let left = Index::read(ours).expect("the merged index");
        let right = Index::read(theirs).expect("the whole index");
        assert_eq!(left.len(), right.len(), "a different number of cells");
        for cell in right.cells() {
            let held = left
                .get(cell.id)
                .unwrap_or_else(|| panic!("{:?} is missing", cell.id));
            assert_eq!(
                (held.rank_lo, held.rank_hi, held.child_mask),
                (cell.rank_lo, cell.rank_hi, cell.child_mask),
                "{:?} differs",
                cell.id,
            );
            assert_eq!(
                Index::read_payload(ours, cell.id).expect("a merged payload"),
                Index::read_payload(theirs, cell.id).expect("a whole payload"),
                "{:?} owns different systems",
                cell.id,
            );
        }
    }

    fn folded(
        into: &(PathBuf, PathBuf),
        from: &(PathBuf, PathBuf),
        dry_run: bool,
    ) -> Result<Absorbed, Refused> {
        absorb(&into.0, &into.1, &from.0, &from.1, dry_run, &never, &mut quiet)
    }

    /// The oracle: a directory with another folded into it holds exactly
    /// what one build over the union by recency would have held.
    ///
    /// The two galaxies overlap, and the overlap has records on both sides
    /// of the tie: some where the arriving record is newer, some where it
    /// is older, and some where the two stand at the same second, which is
    /// the case the `>=` decides. The union is over
    /// [`LEAF_CAP`](crate::build::snapshot::LEAF_CAP) systems, so the tree the
    /// comparison walks is one that actually divided.
    #[test]
    fn the_fold_is_the_whole_build_over_the_union() {
        let scratch = Scratch::new("oracle");
        let xs: Vec<System> = (1..=5_000)
            .map(|id| system(id, 1_700_000_000 + id as u32))
            .collect();
        let ys: Vec<System> = (4_000..=9_000)
            .map(|id| {
                // Below the overlap's midpoint the arriving record is older,
                // at it they are the same second, and above it newer.
                let shift = match id.cmp(&4_500) {
                    std::cmp::Ordering::Less => -500,
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 500,
                };
                system(id, (1_700_000_000 + id as i64 + shift) as u32)
            })
            .collect();

        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));
        let whole = union(&xs, &ys);
        let oracle = raise(
            &scratch,
            "oracle",
            &whole,
            "SYS",
            Provenance::Database,
            at(400),
        );

        let done = folded(&into, &from, false).expect("a fold");
        assert_eq!(done.systems(), whole.len() as u64);
        assert_eq!(done.held, xs.len() as u64);
        assert_eq!(done.taken, 4_000, "the systems only `from` had");
        assert_eq!(done.refused, 500, "the arriving record was older");
        assert_eq!(done.replaced, 501, "newer, and one at the same second");
        assert_eq!(done.cursor, at(400), "the older of the two clocks");

        same_tree(&into.0, &oracle.0);
        let names = Names::open(&into.0).expect("the merged names");
        assert_eq!(names.len(), whole.len());

        let built = done.rebuilt.expect("the tree was raised again");
        assert!(built.leaves > 1, "the comparison walked one leaf: {built}");
        // The union is already in the resume point, so the rebuild pushes
        // nothing into itself. Pushing would double every system, a bucket
        // spill having no reason to deduplicate.
        assert_eq!(built.systems, whole.len(), "{built}");
        assert!(built.is_consistent(), "{built}");
    }

    /// One system's photometry as the published build works it out: each
    /// star's bolometric magnitude carried to visual, then summed.
    ///
    /// `galaxy.rs`'s recipe written out rather than called, so the test is
    /// a statement of what the answer should be and not an echo of the
    /// code that produces it.
    fn lit_over(stars: &[&Star]) -> (f64, f64) {
        derive::lit(
            stars.iter().map(|star| {
                let t = star.temperature as f64;
                (
                    Magnitude(star.absolute_magnitude as f64)
                        .visual(Temperature(t))
                        .0,
                    t,
                )
            }),
            "",
        )
    }

    /// A fact that is a function of a system's whole contents is worked out
    /// again over the merged contents, not carried from the winning record.
    ///
    /// The case is a system scanned on both sides with a *different body
    /// id* on each: `into` scanned body 0, a G star at the drop point, and
    /// `from` scanned body 1, a neutron star further out, and reported the
    /// system later so its record wins. Carrying that record whole left the
    /// merged directory saying its arrival star was a neutron star, with a
    /// supercharge row to match, while its own body file said body 0 was a
    /// G star — one directory saying two contradicting things, which is
    /// what `galos index verify` exists to catch.
    #[test]
    fn what_is_a_function_of_the_contents_is_worked_out_again() {
        let scratch = Scratch::new("relit");
        let dwarf = a_star(0, 100);
        let neutron = Star {
            star_class: "N".to_owned(),
            absolute_magnitude: 2.0,
            temperature: 10_000_000.0,
            distance_from_arrival_ls: 100.0,
            ..a_star(1, 200)
        };
        let (dwarf_lit, dwarf_hot) = lit_over(&[&dwarf]);
        let (neutron_lit, neutron_hot) = lit_over(&[&neutron]);

        // Each side's record as its own build derived it: over its own
        // star, and nothing else.
        let xs = [System {
            absolute_magnitude: dwarf_lit,
            temperature: dwarf_hot,
            kind: StarKind::of("G"),
            ..system(1, 1_700_000_000)
        }];
        let ys = [System {
            absolute_magnitude: neutron_lit,
            temperature: neutron_hot,
            kind: StarKind::of("N"),
            ..system(1, 1_700_009_000)
        }];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));

        let stood = SystemBodies {
            stars: vec![dwarf.clone()],
            ..SystemBodies::default()
        };
        let arriving = SystemBodies {
            stars: vec![neutron.clone()],
            ..SystemBodies::default()
        };
        bodies::write_each(&into.0, [(1i64, &stood)]).expect("what stood");
        bodies::write_each(&from.0, [(1i64, &arriving)]).expect("what arrived");
        // And the sidecar rows each side derived over its own star: the
        // neutron one supercharges, so the arriving directory publishes a
        // row the merged directory must not keep.
        write_reaches(&into.0, &HashMap::from([(1i64, 10.0f32)]))
            .expect("the standing reaches");
        write_boosts(
            &from.0,
            &HashMap::from([(
                1i64,
                SystemBoost {
                    address: 1,
                    boost: Boost::Neutron,
                    position: [0.0; 3],
                },
            )]),
        )
        .expect("the arriving boosts");

        let done = folded(&into, &from, false).expect("a fold");
        assert_eq!(done.replaced, 1, "the arriving record won the system");

        let merged = bodies::read_bodies(&into.0, 1).expect("the bodies");
        assert_eq!(merged.stars.len(), 2, "both scans are on record");

        let point = Checkpoint::read(&into.1).expect("the resume point");
        let record = point
            .base()
            .iter()
            .find(|it| it.id64 == 1)
            .expect("the merged record");

        // The arrival star is the one at the drop point, which is the G
        // star the losing side scanned.
        assert_eq!(record.kind, StarKind::of("G"));
        assert_ne!(record.kind, StarKind::of("N"), "the winner's own star");
        let (light, tint) = lit_over(&[&dwarf, &neutron]);
        assert_eq!(record.absolute_magnitude, light);
        assert_eq!(record.temperature, tint);
        assert!(
            record.absolute_magnitude < dwarf_lit
                && record.absolute_magnitude < neutron_lit,
            "two stars' light adds, so the pair outshines either",
        );

        // A G star supercharges nothing, so the arriving row goes.
        let boosts: Vec<SystemBoost> =
            msgpack::read_meta(&layout::boosts_path(&into.0))
                .expect("the merged boosts");
        assert!(
            boosts.iter().all(|row| row.address != 1),
            "a supercharge row for a system that arrives at a G star",
        );

        // And the reach is over everything inside, not over one side's.
        let reaches: Vec<SystemReach> =
            msgpack::read_meta(&layout::reaches_path(&into.0))
                .expect("the merged reaches");
        let reach = reaches
            .iter()
            .find(|row| row.address == 1)
            .expect("a reach for a scanned system");
        assert_eq!(Some(reach.reach), merged.extent(1));
        assert_ne!(reach.reach, 10.0, "the standing reach was kept");
    }

    /// Folding the same directory in twice leaves what folding it in once
    /// did, which is what makes an interrupted merge safe to re-run.
    #[test]
    fn folding_twice_is_folding_once() {
        let scratch = Scratch::new("idempotent");
        let xs: Vec<System> =
            (1..=800).map(|id| system(id, 1_700_000_000 + id as u32)).collect();
        let ys: Vec<System> = (600..=1_400)
            .map(|id| system(id, 1_700_001_000 + id as u32))
            .collect();

        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));
        let oracle = raise(
            &scratch,
            "oracle",
            &union(&xs, &ys),
            "SYS",
            Provenance::Database,
            at(400),
        );

        let once = folded(&into, &from, false).expect("a first fold");
        same_tree(&into.0, &oracle.0);

        let twice = folded(&into, &from, false).expect("a second fold");
        same_tree(&into.0, &oracle.0);

        assert_eq!(twice.systems(), once.systems());
        assert_eq!(twice.taken, 0, "there was nothing left to take");
        assert_eq!(twice.names, 0, "every name was already spelled so");
        assert_eq!(
            Names::open(&into.0).expect("the names").len(),
            once.systems() as usize,
        );
    }

    /// A name crosses with its system and not otherwise: the arriving
    /// directory's spelling stands where its record won, and the standing
    /// one stands where it did not.
    #[test]
    fn a_name_follows_its_system() {
        let scratch = Scratch::new("names");
        // 3 is newer in `from`, 4 is newer in `into`, 5 and 6 are only in
        // `from`.
        let xs = [
            system(1, 1_700_000_000),
            system(3, 1_700_000_000),
            system(4, 1_700_009_000),
        ];
        let ys = [
            system(3, 1_700_009_000),
            system(4, 1_700_000_000),
            system(5, 1_700_000_000),
            system(6, 1_700_000_000),
        ];
        let into =
            raise(&scratch, "into", &xs, "AY", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "BEE", Provenance::Database, at(900));

        let done = folded(&into, &from, false).expect("a fold");
        assert_eq!(done.taken, 2);
        assert_eq!(done.replaced, 1);
        assert_eq!(done.refused, 1);

        let names = Names::open(&into.0).expect("the merged names");
        let named = |address: i64| {
            names.name_of(address).map(|it| it.to_string()).expect("a name")
        };
        assert_eq!(named(3), "BEE 3", "its record crossed, so its name did");
        assert_eq!(named(4), "AY 4", "its record lost, so its name did not");
        assert_eq!(named(5), "BEE 5");
        assert_eq!(named(1), "AY 1", "untouched by the fold");
    }

    /// A reach the arriving directory has no row for does not take the
    /// standing one away, even where its system record won.
    ///
    /// An absence says "I have not heard", never "it is gone".
    #[test]
    fn a_sidecar_row_is_not_withdrawn() {
        let scratch = Scratch::new("sidecars");
        let xs = [system(1, 1_700_000_000), system(2, 1_700_000_000)];
        let ys = [system(1, 1_700_009_000), system(2, 1_700_009_000)];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));

        write_reaches(&into.0, &HashMap::from([(1i64, 100.0f32)]))
            .expect("the standing reaches");
        write_reaches(&from.0, &HashMap::from([(2i64, 50.0f32)]))
            .expect("the arriving reaches");

        let done = folded(&into, &from, false).expect("a fold");
        assert_eq!(done.replaced, 2, "the arriving records won both");
        assert_eq!(done.reaches, 1, "one row crossed");

        let rows: Vec<SystemReach> =
            msgpack::read_meta(&layout::reaches_path(&into.0))
                .expect("the merged reaches");
        let held: HashMap<i64, f32> =
            rows.into_iter().map(|it| (it.address, it.reach)).collect();
        assert_eq!(
            held.get(&1),
            Some(&100.0),
            "won, but nothing was said about its reach",
        );
        assert_eq!(held.get(&2), Some(&50.0), "the arriving reach crossed");
    }

    /// A system scanned in both directories keeps both sides' bodies.
    ///
    /// `bodies/<address>` is written whole, so taking one side's record
    /// over the other is how a merge silently loses half a system — which
    /// is the failure this pass exists to prevent, and it cannot be seen
    /// from the tree.
    #[test]
    fn a_system_scanned_on_both_sides_keeps_both_scans() {
        let scratch = Scratch::new("bodies");
        let xs = [system(1, 1_700_000_000)];
        let ys = [system(1, 1_700_009_000)];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));

        let stood = SystemBodies {
            stars: vec![Star { temperature: 4_000.0, ..a_star(0, 100) }],
            bodies: vec![a_body(1, 100)],
            barycenters: Vec::new(),
        };
        let arriving = SystemBodies {
            stars: vec![Star { temperature: 5_100.0, ..a_star(0, 200) }],
            bodies: vec![a_body(2, 200)],
            barycenters: Vec::new(),
        };
        bodies::write_each(&into.0, [(1i64, &stood)]).expect("what stood");
        bodies::write_each(&from.0, [(1i64, &arriving)]).expect("what arrived");

        let done = folded(&into, &from, false).expect("a fold");
        assert_eq!(done.bodies, 1, "one system's record was folded");

        let merged =
            bodies::read_bodies(&into.0, 1).expect("the merged record");
        assert_eq!(merged.stars.len(), 1);
        assert_eq!(
            merged.stars[0].temperature, 5_100.0,
            "the later reading of the star both sides scanned",
        );
        let mut ids: Vec<i16> = merged.bodies.iter().map(|it| it.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 2], "neither side's bodies were dropped");
    }

    /// A dry run answers the counts and writes nothing: the standing
    /// directory still serves what it served and its resume point still
    /// holds what it held.
    #[test]
    fn a_dry_run_writes_nothing() {
        let scratch = Scratch::new("dry");
        let xs: Vec<System> =
            (1..=400).map(|id| system(id, 1_700_000_000 + id as u32)).collect();
        let ys: Vec<System> = (300..=700)
            .map(|id| system(id, 1_700_001_000 + id as u32))
            .collect();
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));

        let before = std::fs::read(into.1.as_path()).expect("the checkpoint");
        let cells = Index::read(&into.0).expect("the index").len();

        let done = folded(&into, &from, true).expect("a dry run");
        assert!(done.dry_run);
        assert_eq!(done.held, 400);
        assert_eq!(done.taken, 300);
        assert!(done.rebuilt.is_none());

        assert_eq!(
            std::fs::read(into.1.as_path()).expect("the checkpoint"),
            before,
            "the resume point was rewritten",
        );
        assert_eq!(
            Index::read(&into.0).expect("the index").len(),
            cells,
            "the served tree was rebuilt",
        );
    }

    /// A directory whose resume point is gone cannot be merged, because
    /// what it serves has had its photometry coarsened.
    #[test]
    fn a_directory_adrift_is_refused() {
        let scratch = Scratch::new("adrift");
        let xs = [system(1, 1_700_000_000)];
        let ys = [system(2, 1_700_000_000)];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));
        std::fs::remove_file(&from.1).expect("the resume point removed");

        match folded(&into, &from, false) {
            Err(Refused::Adrift { dir, .. }) => assert_eq!(dir, from.0),
            it => panic!("{:?}", it.map(|done| done.to_string())),
        }
    }

    /// Two resume points written by different derivations are refused: a
    /// cursor means a different thing on each side.
    #[test]
    fn two_hands_are_refused() {
        let scratch = Scratch::new("hands");
        let xs = [system(1, 1_700_000_000)];
        let ys = [system(2, 1_700_000_000)];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Events, None);

        match folded(&into, &from, false) {
            Err(Refused::TwoHands { wrote, .. }) => {
                assert_eq!(wrote, Provenance::Database)
            }
            it => panic!("{:?}", it.map(|done| done.to_string())),
        }
    }

    /// Two faction tables that number one faction differently were fed by
    /// different databases, and their union would colour the map by the
    /// wrong faction.
    #[test]
    fn a_faction_numbered_twice_is_refused() {
        let scratch = Scratch::new("factions");
        let xs = [system(1, 1_700_000_000)];
        let ys = [system(2, 1_700_000_000)];
        let into =
            raise(&scratch, "into", &xs, "SYS", Provenance::Database, at(400));
        let from =
            raise(&scratch, "from", &ys, "SYS", Provenance::Database, at(900));

        let ours = vec![Faction { id: 7, name: "Kumo Crew".to_owned() }];
        let theirs = vec![Faction { id: 7, name: "Aegis Core".to_owned() }];
        msgpack::write_meta(&layout::factions_path(&into.0), &ours)
            .expect("the standing factions");
        msgpack::write_meta(&layout::factions_path(&from.0), &theirs)
            .expect("the arriving factions");

        match folded(&into, &from, false) {
            Err(Refused::Factions { id, into, from }) => {
                assert_eq!(id, 7);
                assert_eq!(into, "Kumo Crew");
                assert_eq!(from, "Aegis Core");
            }
            it => panic!("{:?}", it.map(|done| done.to_string())),
        }
    }

    fn stamp(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a time")
    }

    fn a_star(id: i16, when: i64) -> Star {
        Star {
            system_address: 1,
            id,
            name: format!("Star {id}"),
            parents: Vec::new(),
            updated_at: stamp(when),
            updated_by: "a test".to_owned(),
            absolute_magnitude: 4.83,
            age_my: 4_600,
            distance_from_arrival_ls: 0.0,
            luminosity: "V".to_owned(),
            star_class: "G".to_owned(),
            stellar_mass: 1.0,
            subclass: 2,
            orbit: None,
            spin: Spin { period: 1.0, tilt: 0.0 },
            radius: 1.0,
            temperature: 5_778.0,
            mapped: false,
            discovered_at: None,
        }
    }

    fn a_body(id: i16, when: i64) -> Body {
        Body {
            system_address: 1,
            id,
            parents: Vec::new(),
            name: format!("Body {id}"),
            body_type: None,
            distance_from_arrival: Some(10.0),
            updated_at: stamp(when),
            updated_by: "a test".to_owned(),
            planet_class: "Icy body".to_owned(),
            tidal_lock: false,
            mass: 1.0,
            radius: 1.0,
            gravity: 1.0,
            temperature: Some(100.0),
            surface: None,
            orbit: Orbit {
                semi_major_axis: 1.0,
                eccentricity: 0.0,
                orbital_inclination: 0.0,
                periapsis: 0.0,
                orbital_period: 1.0,
                ascending_node: None,
                mean_anomaly: None,
            },
            spin: Spin { period: 1.0, tilt: 0.0 },
            mapped: false,
            discovered_at: None,
        }
    }
}
