//! Spansh's `galaxy.json`, a system and everything in it
//!
//! <https://spansh.co.uk/dumps> publishes one complete system per line,
//! bodies nested inside it. The file is tens of gigabytes, so it is read a
//! line at a time through [`spansh::Lines`] and never held whole.
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
use galos::sink::tables::Tables;
use galos::sink::{Landed, Reporter, Sink, SystemReport};
use galos::{Shard, Shutdown};
use galos_index::bodies::Published;
use galos_index::{Build, Taking};
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
        let mut reading =
            match Reading::open(&self.path, self.shard, shutdown) {
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
    lines: spansh::Lines,
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
        let lines = spansh::Lines::open(path)?;
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
        Ok(Reading {
            path: path.to_owned(),
            lines,
            bar: bar::imported(&tag, extent),
            shard,
            shutdown: shutdown.clone(),
            read: 0,
            skipped: 0,
        })
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
            let at = self.lines.at() + 1;
            let text = match self.lines.next() {
                Ok(Some(text)) => text,
                Ok(None) => {
                    self.bar.done();
                    return Next::Ended;
                }
                // The file stopped being readable part way through, which a
                // half-written dump does.
                Err(error) => {
                    self.bar.abandoned("unreadable");
                    return Next::Failed { at, error };
                }
            };
            // The comma and the newline the reader trimmed off, so the bar
            // reaches the file's size at the last line.
            self.bar.through(text.len() as u64 + 2);
            self.read += 1;

            // Another process's line, and the cheapest place to find that
            // out: the bytes had to be read to reach the next line, and
            // nothing has been parsed yet. Counted by position in the file,
            // so the shards agree about whose it is without talking to each
            // other.
            if let Some(shard) = self.shard {
                if !shard.mine(self.read - 1) {
                    continue;
                }
            }

            // One line nothing can parse is one system missed rather than a
            // run ended: a file this size is not going to be read again for
            // it.
            let system: spansh::galaxy::System =
                match serde_json::from_str(text) {
                    Ok(system) => system,
                    Err(err) => {
                        self.skipped += 1;
                        self.bar.missed();
                        warn!(
                            line = at,
                            error = %err,
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
fn reported(system: spansh::galaxy::System) -> SystemReport {
    SystemReport {
        name: Some(system.name),
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
    /// A run asked to stop ends the read where it is. Nothing is published
    /// by that: the flag [`Reading`] asks between lines is the one
    /// [`Build`] asks per record, so the build behind this answers
    /// `Built::Stopped` and leaves the directory as it found it.
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
    /// The metadata tables ride in `tables`, patched out of the same
    /// galaxy the tree's system came from and written once the build has
    /// finished. Each is a row a system, so what this holds beyond the
    /// line it is on is those rows; item 1 of `TODO-scale-regions.md` is
    /// the chunking that would end that.
    pub fn read(
        &self,
        build: &mut Build<'_>,
        tables: &mut Tables,
    ) -> io::Result<()> {
        let mut reading = Reading::open(&self.path, None, &self.shutdown)?;
        let started = Instant::now();
        let (mut systems, mut bodies) = (0u64, 0);
        let by = crate::from::published("Spansh", &self.path);
        loop {
            let (report, scans) = match reading.next() {
                Next::System(report, scans) => (report, scans),
                Next::Ended | Next::Stopped => break,
                Next::Failed { error, .. } => return Err(error),
            };
            systems += 1;
            let address = report.address;
            let mut galaxy = galos_index::Galaxy::keeping(
                self.now,
                Box::new(Published::raising(self.dir.as_path())),
            );
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
            let took = match (
                galaxy.system_of(address),
                galaxy.name_of(address),
            ) {
                (Some(system), Some(name)) => match build.push(system, name)? {
                    // New by construction: a cold build writes a directory
                    // from nothing, so the updated count stays zero for the
                    // whole read.
                    Taking::More => {
                        reading.took(Some(Landed::New));
                        true
                    }
                    Taking::Stopped => break,
                },
                _ => false,
            };

            // Only for a system the tree took: a row for one it has not got
            // is a row the map can colour and never draw. And before the
            // bodies are settled, the reach and the boost being read off
            // what is still held rather than off the file just written.
            if took {
                tables.patch_tables(&galaxy, galaxy.touched());
            }
            bodies += galaxy.settle_bodies()?;
        }

        reading.unparsed();
        info!(
            systems,
            bodies,
            elapsed = ?started.elapsed(),
            dir = %self.dir.display(),
            "read the dump and wrote the body files",
        );
        Ok(())
    }
}
