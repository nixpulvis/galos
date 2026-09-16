//! Spansh's `galaxy.json`, a system and everything in it
//!
//! <https://spansh.co.uk/dumps> publishes one complete system per line,
//! bodies nested inside it. The file is tens of gigabytes, so it is read a
//! line at a time through [`spansh::Dump`] and never held whole.
//!
//! Two things reach a sink per line: the system itself, through
//! [`Sink::system`], and one entry per body it gives a home to, through
//! [`Sink::entry`] — which is where a dump's bodies join the scans a journal
//! and EDDN carry.
//!
//! Stamped with the dump's own times — the system with its `date`, each body
//! with its own `updateTime` — and not with `Utc::now()`. The writes
//! downstream are guarded, newer winning and older filling blanks, so a
//! reading claiming to be from now would stand over whatever a commander has
//! sent since the file was published.
//!
//! ## The two ways a dump reaches an index
//!
//! [`Dump`] is the fan-out: one reading into a database and an index both,
//! through the sinks. What the index sink holds while it does that is a live
//! [`Tree`](galos_index::Tree) and the whole names table, which is a
//! kilobyte a system — fine for a day's slice, and two hundred gigabytes for
//! the whole galaxy. A publish does not shorten that: it writes the cells a
//! pass dirtied and clears the dirty set, and the tree goes on holding every
//! system it has been told about, because the next one to arrive may be
//! owned by any cell in it.
//!
//! [`Galaxy`] is the other way: the dump pushed into
//! [`Build`], for a run that writes an index and
//! nothing else. The file is read once and nothing the galaxy's size scales
//! is held — one line at a time, each system spilled to its bucket's file
//! and each name to a chunk — and what is held while the tree is raised is
//! one region's systems, read back off that region's spill and dropped when
//! it has been built. That is what the cut buys: a region is disjoint, so
//! once it is built nothing outside it has anything left to ask it.
//! [`galos_index::region_budget`] is how many systems that is. See
//! [`main`](crate)'s routing for which run gets which.
//!
//! Both read the file through [`Reading`], which holds the reading itself —
//! the lines, the bar, the share of the file this process takes and the
//! lines nothing could parse — and hands back one system at a time. Each
//! way is the short loop over it that its own side needs.

use chrono::{DateTime, Utc};
use elite_journal::entry::{Entry, Event};
use galos::bar;
use galos::sink::{Landed, Reporter, Sink, SystemName, SystemReport};
use galos::{Shard, Shutdown};
use galos_index::bodies::Shared;
use galos_index::{Build, LeftOff, Rows, Taking};
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use tracing::{info, warn};

/// A galaxy dump already on disk: `--from spansh=PATH`.
pub struct Dump {
    pub path: PathBuf,

    /// One line in `n`, where the run was told to take a share of the file.
    pub shard: Option<Shard>,
}

impl Dump {
    /// Read the file, answering whether it could be opened at all.
    pub async fn read(&self, sink: &mut dyn Sink, shutdown: &Shutdown) -> bool {
        let mut reading = match Reading::open(&self.path, self.shard, shutdown)
        {
            Ok(reading) => reading,
            Err(err) => {
                warn!(
                    file = %self.path.display(),
                    error = %err,
                    "unreadable dump",
                );
                return false;
            }
        };

        let by = crate::from::published("Spansh", &self.path);
        loop {
            let (report, scans) = match reading.next() {
                Next::System(report, scans) => (report, scans),
                Next::Stopped | Next::Ended => break,
                // What was read stands.
                Next::Failed { at, error } => {
                    warn!(line = at, error = %error, "dump ended badly");
                    break;
                }
            };
            let landed = sink.system(&report, &by).await;
            reading.took(landed);
            // Nobody flew here and a file was published, so both sinks take
            // the file's own name as provenance. Each of these writes the
            // same system row again, already counted above.
            for entry in scans {
                sink.entry(Arc::new(entry), Reporter::Uploader(&by)).await;
            }
        }

        reading.unparsed();
        true
    }
}

/// What a pull off a [`Reading`] found
///
/// The four things the next line can turn out to be. The last three are
/// the caller's to answer, which is where the two ways differ.
enum Next {
    /// A system: what it says about itself, and the scans it stands for.
    ///
    /// The two things both ways ask of a dumped system, derived once. The
    /// address is [`SystemReport::address`], so nothing keeps the parsed
    /// system.
    System(SystemReport, Vec<Entry<Event>>),
    /// The run was asked to stop between lines.
    Stopped,
    /// The file was read to the end.
    Ended,
    /// The file stopped being readable, part way through line `at`.
    Failed { at: u64, error: io::Error },
}

/// What a read that cannot go on is reported as.
///
/// Only [`spansh::Error::Unreadable`] ever reaches here — a line nothing
/// could parse is counted and passed over — and it carries the reader's
/// own `io::Error`, handed back whole so what ended the run is what the
/// file said rather than a sentence about it.
fn failed(error: spansh::Error) -> io::Error {
    match error {
        spansh::Error::Unreadable(error) => error,
        unparsed => io::Error::other(unparsed.to_string()),
    }
}

/// Where a stopped read had got to, as the build carries it.
///
/// [`Build::mark`] takes a caller's place as bytes and does not read them;
/// this is what this caller puts in them. The file and its size are the
/// check: a scratch full of spills means nothing beside a dump that has
/// been replaced since, and a dump that has grown is a new galaxy rather
/// than a longer one.
///
/// `now` rides along because a build ages every system against one moment.
/// A resumed run that took its own clock would bin half the galaxy's
/// Recency against one hour and half against another, and the two halves
/// would be a directory nobody could tell had been built twice.
#[derive(Debug, Serialize, Deserialize)]
pub struct Place {
    /// The dump these bytes are an offset into, as it was named.
    file: String,
    /// Its length when the mark was taken.
    size: u64,
    /// Bytes read, always the end of a line.
    at: u64,
    /// Lines read, so a warning names the line the file holds.
    line: u64,
    /// Systems and body files the stopped run had taken, for the one line
    /// at the end of the read.
    systems: u64,
    bodies: usize,
    /// The clock the stopped run dated its Recency by.
    pub now: DateTime<Utc>,
}

impl Place {
    /// What a stopped build left, where it was reading this same file.
    ///
    /// [`None`] for a mark this cannot read, or one taken against another
    /// file or another length of it — a read that cannot be taken up is a
    /// read that starts over, never a run that fails.
    pub fn of(kept: &LeftOff, path: &Path) -> Option<Place> {
        let place: Place = rmp_serde::from_slice(kept.cursor()).ok()?;
        let size = std::fs::metadata(path).ok()?.len();
        let named = path.to_string_lossy();
        match place.file == named && place.size == size {
            true => Some(place),
            false => {
                warn!(
                    file = %named,
                    was = %place.file,
                    "a stopped build's spills are of another dump; \
                     reading this one from the start",
                );
                None
            }
        }
    }

    /// How far into the file this is, in bytes.
    pub fn at(&self) -> u64 {
        self.at
    }

    /// This place as the bytes [`Build::mark`] carries.
    ///
    /// Infallible: a struct of scalars and one string, and MessagePack has
    /// no encoding failure for it, so a mark is never lost to one.
    fn bytes(&self) -> Vec<u8> {
        rmp_serde::to_vec(self).unwrap_or_default()
    }
}

/// One reading of a dump, pulled a system at a time
///
/// What a read of the file needs whichever way it is going: the lines, the
/// bar drawn against the file's bytes, the share of the file this process
/// takes, and what nothing could parse. The loop belongs to the caller —
/// the fan-out awaits its sinks and the cold build does not — and so does
/// what to do about a stop, an end, or a file that stopped being readable.
struct Reading {
    /// The file being read, for saying which one a warning is about.
    path: PathBuf,
    dump: spansh::Dump,
    bar: bar::Import,
    /// One line in `n`, where the run was told to take a share of the file.
    shard: Option<Shard>,
    /// Asked between lines.
    shutdown: Shutdown,
    /// Lines read, whosever they are, which is what a share is counted by.
    read: u64,
    /// Lines nothing could parse, so systems missed.
    skipped: u64,
}

impl Reading {
    /// Open a dump and draw its bar.
    ///
    /// The bar is drawn only once the file is open, so a path that cannot
    /// be read leaves no line behind.
    fn open(
        path: &Path,
        shard: Option<Shard>,
        shutdown: &Shutdown,
    ) -> io::Result<Reading> {
        Reading::opened(path, shard, shutdown, None)
    }

    /// The same, carrying on from where a stopped read left off.
    ///
    /// The bar is put straight to the byte the previous run reached and
    /// its tally to the systems that run took, so what it draws and what
    /// it counts are both the file rather than this run's share of it.
    fn opened(
        path: &Path,
        shard: Option<Shard>,
        shutdown: &Shutdown,
        from: Option<&Place>,
    ) -> io::Result<Reading> {
        let (at, line) = from.map_or((0, 0), |it| (it.at, it.line));
        let dump = spansh::Dump::open_at(path, at, line)?;
        let tag = match shard {
            Some(shard) => format!("Spansh {shard}"),
            None => "Spansh".to_string(),
        };
        // Bytes, not systems: a file's system count is not knowable
        // without reading it, and an estimate from the mean line so far
        // walks backwards whenever a dense stretch arrives. The size is
        // known at open and bytes only go up. A shard reads every line to
        // find its own, so it covers the whole file either way.
        let size = std::fs::metadata(path).map(|it| it.len()).ok();
        let extent = bar::Extent::Bytes(size.unwrap_or(0));
        let mut bar = bar::imported(&tag, extent);
        if let Some(place) = from {
            bar.taken_up(place.systems, at);
        }
        Ok(Reading {
            path: path.to_owned(),
            dump,
            bar,
            shard,
            shutdown: shutdown.clone(),
            read: line,
            skipped: 0,
        })
    }

    /// The byte and the line the read has reached, which is always the end
    /// of a line and so a place another read can start at.
    fn here(&self) -> (u64, u64) {
        (self.dump.bytes(), self.dump.at())
    }

    /// Where the read stands, for the build to mark.
    ///
    /// `at` is a point [`here`](Self::here) answered — this one or the one
    /// before the line being read, which is what a stop part way through a
    /// line wants.
    fn place(
        &self,
        at: (u64, u64),
        now: DateTime<Utc>,
        systems: u64,
        bodies: usize,
    ) -> Place {
        Place {
            file: self.path.to_string_lossy().into_owned(),
            size: std::fs::metadata(&self.path).map(|it| it.len()).unwrap_or(0),
            at: at.0,
            line: at.1,
            systems,
            bodies,
            now,
        }
    }

    /// The next system, or why there is not one.
    ///
    /// Every line costs the bar its bytes before anything else happens to
    /// it: a line another shard owns and a line nothing can parse were
    /// both read to get past them.
    fn next(&mut self) -> Next {
        loop {
            // A dump is a day's read and the run may have been asked to
            // stop an hour into one. What has been written stands: every
            // write is its own guarded upsert and the next run re-reads the
            // file.
            if self.shutdown.asked() {
                self.bar.abandoned("stopped");
                return Next::Stopped;
            }
            let at = self.dump.at() + 1;
            let was = self.dump.bytes();

            // Another process's line, and the cheapest place to find that
            // out: whose a line is depends on its position in the file, so
            // it is passed over unparsed. Counted by position, so the
            // shards agree about whose it is without talking to each other.
            let mine = self.shard.is_none_or(|shard| shard.mine(self.read));
            let got = match mine {
                true => self.dump.next(),
                false => match self.dump.pass() {
                    Ok(true) => None,
                    Ok(false) => {
                        self.bar.done();
                        return Next::Ended;
                    }
                    Err(fault) => {
                        self.bar.abandoned("unreadable");
                        return Next::Failed { at, error: failed(fault) };
                    }
                },
            };

            // The bytes the line cost, comma and newline included, so the
            // bar reaches the file's size at the last line.
            if self.dump.bytes() > was {
                self.bar.through(self.dump.bytes() - was);
                self.read += 1;
            }

            let system = match got {
                Some(Ok(system)) => system,
                // The end of the file, or a line this shard passed over.
                None if mine => {
                    self.bar.done();
                    return Next::Ended;
                }
                None => continue,
                // The file stopped being readable part way through, which
                // a half-written dump does.
                Some(Err(fault @ spansh::Error::Unreadable(_))) => {
                    self.bar.abandoned("unreadable");
                    return Next::Failed { at, error: failed(fault) };
                }
                // One line nothing can parse is one system missed rather
                // than a run ended: a file this size is not going to be
                // read again for it.
                Some(Err(fault)) => {
                    self.skipped += 1;
                    self.bar.missed();
                    warn!(
                        line = at,
                        error = %fault,
                        skipped = self.skipped,
                        "unparsed system",
                    );
                    continue;
                }
            };

            // The scans first, so the name and the place are moved into the
            // report rather than copied out of a system still borrowed.
            let scans = system.scans();
            return Next::System(reported(system), scans);
        }
    }

    /// One system the caller took in, and what its store did with it.
    fn took(&mut self, landed: Option<Landed>) {
        self.bar.took(landed);
    }

    /// Say how many lines nothing could parse, where any did not.
    ///
    /// Each was warned about as it was read; this is the one line a reader
    /// of the log finds without counting them.
    fn unparsed(&self) {
        if self.skipped > 0 {
            warn!(
                skipped = self.skipped,
                file = %self.path.display(),
                "unparsed systems",
            );
        }
    }
}

/// What one dumped system says about itself, as a report.
///
/// The one statement of it, read by both ways into an index: the fan-out
/// hands this to [`Sink::system`] and the cold build hands it to the same
/// accumulator directly. Two of these that had drifted apart would be two
/// directories that disagree about the political columns of every system in
/// the galaxy.
fn reported(system: spansh::System) -> SystemReport {
    SystemReport {
        name: Some(SystemName::new(system.name)),
        position: Some(system.coords),
        population: system.population,
        security: system.security,
        government: system.government,
        allegiance: system.allegiance,
        primary_economy: system.primary_economy,
        secondary_economy: system.secondary_economy,
        body_count: system.body_count,
        ..SystemReport::new(system.id64, system.update_time)
    }
}

/// A galaxy dump as a cold build reads it: `--from spansh=PATH --index DIR`
/// with nothing else to write to.
///
/// One read of the file, pushed into [`Build`] a line at a time. Nothing
/// holds more than the line it is on, and the body files are written as the
/// read goes.
///
/// The peer of `galos_db::index`'s own read, which pushes the rows of two
/// merged SQL cursors into the same builder.
pub struct Galaxy {
    /// The file to read.
    pub path: PathBuf,
    /// The directory being built, which is where the body files go.
    pub dir: PathBuf,
    /// What dates the Recency reading, so every system in the build is aged
    /// against the same moment.
    pub now: DateTime<Utc>,
    /// Asked between lines; a stop ends the build rather than publishing a
    /// prefix of the galaxy as the whole of it.
    pub shutdown: Shutdown,
}

impl Galaxy {
    /// Read the file into `build`, one system to a line.
    ///
    /// A line nothing can parse is warned and skipped, as the fan-out skips
    /// it. A file that stops being readable is not: a truncated read would
    /// publish a prefix of the galaxy and a resume point saying it was the
    /// whole of it.
    ///
    /// A run asked to stop ends the read where it is and marks the place,
    /// so the next run over the same file takes up the spills rather than
    /// reading the galaxy again; see [`Build::mark`] and [`Place`]. The
    /// place is marked every [`MARKED`] systems as well, which is what a
    /// kill rather than a Ctrl-C falls back to. Nothing of the directory is
    /// published either way: the flag [`Reading`] asks between lines is the
    /// one [`Build`] asks per record, so the build behind this answers
    /// `Built::Stopped`.
    ///
    /// One system's worth of galaxy per line: the accumulator the fan-out
    /// feeds, fed the same report and the same scans and then asked what it
    /// derived. That is what keeps the two derivations the same one — the
    /// photometry fallback, the Recency bucketing and the merge that turns a
    /// dump's bodies into a body file are read here rather than restated.
    ///
    /// The body files are written per system rather than held: a dump names
    /// each system once, so a system's file is whole the moment its line has
    /// been read. The store is [`Published::raising`] for the same reason —
    /// a build from nothing can only be told back what it has just said, so
    /// nothing is read from the directory it is writing.
    ///
    /// The metadata tables ride in `rows`, which is the same
    /// [`galos_index::Rows`] the mark is cut against: a row a system,
    /// written as it is derived and made into the three tables when the
    /// read is over, so that neither the read nor a stop holds a galaxy's
    /// worth of them.
    pub fn read(
        &self,
        build: &mut Build<'_>,
        rows: &mut Rows,
        from: Option<Place>,
    ) -> io::Result<()> {
        let mut reading =
            Reading::opened(&self.path, None, &self.shutdown, from.as_ref())?;
        let started = Instant::now();
        let (mut systems, mut bodies) =
            from.as_ref().map_or((0u64, 0), |it| (it.systems, it.bodies));
        let taken_up = systems;
        let by = crate::from::published("Spansh", &self.path);
        // One store for the whole read, though the accumulator is a line's.
        // What it holds is what makes the body records go out a shard at a
        // time rather than one append a system; see `galos_index::pack` and
        // [`Shared`].
        let store = Shared::raising(self.dir.as_path());
        loop {
            // Where the line about to be read begins. A stop part way
            // through one is marked here rather than after it: the build
            // refuses the record it was asked to stop on, so a mark past
            // that line would stand for a system nothing ever spilled.
            let began = reading.here();
            let (report, scans) = match reading.next() {
                Next::System(report, scans) => (report, scans),
                Next::Ended => break,
                Next::Stopped => {
                    build.mark(
                        &reading
                            .place(began, self.now, systems, bodies)
                            .bytes(),
                    );
                    break;
                }
                Next::Failed { error, .. } => return Err(error),
            };
            systems += 1;
            let address = report.address;
            let mut galaxy =
                galos_index::Galaxy::keeping(self.now, Box::new(store.clone()));
            galaxy.hear(report);
            // Nobody flew here and a file was published, so the file's own
            // name is what these bodies are filed under — the same
            // provenance the fan-out hands its sinks.
            galaxy.reported_by(&by);
            for entry in &scans {
                galaxy.read(entry);
            }

            // Placed or named and not both is neither: the tree and the
            // names table have to agree about it, and a tree standing over a
            // system the names table has no row for will not reopen.
            let took =
                match (galaxy.system_of(address), galaxy.name_of(address)) {
                    (Some(system), Some(name)) => match build
                        .push(system, name)?
                    {
                        // New by construction: a cold build writes a directory
                        // from nothing, so the updated count stays zero for the
                        // whole read.
                        Taking::More => {
                            reading.took(Some(Landed::New));
                            true
                        }
                        Taking::Stopped => {
                            build.mark(
                                &reading
                                    .place(began, self.now, systems - 1, bodies)
                                    .bytes(),
                            );
                            break;
                        }
                    },
                    _ => false,
                };

            // Only for a system the tree took: a row for one it has not got
            // is a row the map can colour and never draw. And before the
            // bodies are settled, the reach and the boost being read off
            // what is still held rather than off the file just written.
            if took {
                rows.take(&galaxy, galaxy.touched())?;
            }
            // What the store has written since it was last asked. It holds
            // what it is told until [`Published::CARRIED`] systems have
            // piled up, so most lines add nothing here and the line that
            // does adds a shard's worth at a time — see `galos_index::pack`.
            bodies += store.written();

            // What the publish at the end of the read will record, kept
            // current so a read that runs to the end of the file marks the
            // end of it. Nothing is written by this.
            build.mark(
                &reading
                    .place(reading.here(), self.now, systems, bodies)
                    .bytes(),
            );
        }

        reading.unparsed();
        // Whatever is still held, whether the read ended or was stopped:
        // the body records are the only copy of what a line said about a
        // system's insides.
        bodies += store.settle()?;
        let elapsed = started.elapsed();
        info!(
            systems,
            taken_up,
            bodies,
            per_min = bar::per_minute(systems - taken_up, elapsed),
            elapsed = ?elapsed,
            dir = %self.dir.display(),
            "read the dump and wrote the body files",
        );
        Ok(())
    }
}
