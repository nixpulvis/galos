//! Reading a journal directory while the game is still writing to it.
//!
//! The game keeps its logs in one directory, one file per session, named so
//! they sort into the order they were written. The newest is open and being
//! appended to line by line for as long as somebody is playing, and the rest
//! are finished and will never change again. Importing reads the lot;
//! following reads what has arrived since it last looked, which is one seek
//! per file and usually nothing at all.
//!
//! Three things make that harder than `tail -f`.
//!
//! **A line may be half written.** The game appends one JSON object per line
//! and a poll can land in the middle of one. So a read stops at the last
//! newline it saw and the offset advances only that far; the tail of the file
//! is left for the next poll, which will find the rest of it. Nothing is
//! parsed that is not a whole line, so a torn write costs a poll's latency and
//! never an event.
//!
//! **A file may be replaced.** The commander clears their journal directory,
//! restores a backup, or points the map at a different one. The offsets are
//! kept per path and a file shorter than the offset held for it has been
//! rewritten rather than appended to, so it is read again from the start.
//! Applying an event twice is free — everything downstream is keyed by
//! address and body id — so re-reading is always the safe answer.
//!
//! **The directory grows.** A new session opens a new file, which is a path
//! with no offset held for it and is therefore read whole. That is also what
//! the first poll does to every file in the directory, which is how a
//! commander's whole history arrives: the first poll is the import, and every
//! poll after it is the tail.
//!
//! Beside the logs sits `NavRoute.json`, which is not a log at all: the game
//! rewrites it whole each time a route is plotted, and it is the only place a
//! journal names systems the ship has not been to. It is read when it changes
//! and handed back as the [`Event::NavRoute`] the log's own event is written
//! without, so everything downstream still sees one stream of entries.
//!
//! What is *not* here is any judgement about what an event means. This hands
//! back entries in the order the files hold them and [`crate::galaxy`] decides
//! what they say.

use elite_journal::entry::route::NavRoute;
use elite_journal::entry::{Entry, Event};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use tracing::{debug, warn};

/// A journal directory and how far into each of its files has been read.
///
/// Held across polls, which is the whole of the state: the files themselves
/// are opened, read from an offset and closed again each time, so a follower
/// holds no descriptor and a game that rotates its log under one costs
/// nothing.
#[derive(Debug)]
pub struct Follower {
    dir: PathBuf,
    /// How many bytes of each file have been turned into entries, by path.
    ///
    /// Bytes and not lines: a line count cannot be seeked to, and the point of
    /// holding anything is not to read a gigabyte of finished logs on every
    /// poll.
    read: HashMap<PathBuf, u64>,
    /// The length and modification time `NavRoute.json` was last read at.
    ///
    /// Not an offset: the file is rewritten whole rather than appended to, so
    /// there is no tail of it to follow and the question is only whether it is
    /// the same file it was. Both halves, since a route replotted inside one
    /// second onto a list of the same length moves neither on its own.
    route: Option<(u64, std::time::SystemTime)>,
}

/// What one poll found.
#[derive(Debug, Default)]
pub struct Read {
    /// The entries, in the order the files hold them.
    pub entries: Vec<Entry<Event>>,
    /// Lines that were whole and did not parse, counted rather than kept.
    ///
    /// An event the model does not know is [`Event::Other`] and is not one of
    /// these: reaching here means the line was not an entry at all. Counted
    /// because a journal that hits it hits it thousands of times for the one
    /// reason, and a warning per line is a log nobody reads.
    pub unread: usize,
    /// Files that were read from the start because they had been replaced.
    pub restarted: usize,
    /// Whether the route file was read this pass.
    pub routed: bool,
}

impl Follower {
    /// A follower over the journal directory at `dir`, having read none of it.
    pub fn new(dir: impl Into<PathBuf>) -> Follower {
        Follower { dir: dir.into(), read: HashMap::new(), route: None }
    }

    /// The directory being followed.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Take the directory as already read, without reading any of it.
    ///
    /// For a caller that has another way of reading the whole directory and
    /// wants to follow it afterwards — `galos-sync journal --watch`, which
    /// imports the lot in timestamp order across every file and then tails
    /// what arrives. Without this the first poll would hand the importer's
    /// work back to it a second time, which is a gigabyte of finished logs
    /// re-parsed and re-written for nothing.
    ///
    /// Call it **before** the other read, not after. The offsets are fixed at
    /// the moment this is called, so anything the game writes while the other
    /// read is running is read by both — which costs a duplicate write, and
    /// every write downstream is idempotent. Called afterwards it would be
    /// the other way about: whatever arrived during the read would fall in
    /// the gap between the two and be seen by neither.
    ///
    /// Answers how many bytes were passed over.
    pub fn caught_up(&mut self) -> io::Result<u64> {
        let mut skipped = 0;
        for path in logs(&self.dir)? {
            let Ok(meta) = path.metadata() else { continue };
            skipped += meta
                .len()
                .saturating_sub(self.read.get(&path).copied().unwrap_or(0));
            self.read.insert(path, meta.len());
        }
        // The route file is the whole of what it says rather than a tail, so
        // "already read" is the reading it stands at now.
        let route = self.dir.join(ROUTE_FILE);
        if let Ok(meta) = route.metadata()
            && let Ok(at) = meta.modified()
        {
            self.route = Some((meta.len(), at));
        }
        Ok(skipped)
    }

    /// Everything written to the directory since the last poll.
    ///
    /// The first poll reads every file whole, which is the import. Errors
    /// reading one file are warned and the rest of the directory is read: a
    /// journal with one unreadable log in it is a journal, and stopping at it
    /// would lose everything written since.
    pub fn poll(&mut self) -> io::Result<Read> {
        let mut found = Read::default();
        for path in logs(&self.dir)? {
            let len = match path.metadata() {
                Ok(meta) => meta.len(),
                Err(err) => {
                    warn!(file = %path.display(), error = %err, "unstattable");
                    continue;
                }
            };
            let held = self.read.get(&path).copied().unwrap_or(0);

            // Shorter than what has been read out of it: this is not the file
            // whose offset that was. Read it again from the top.
            let from = if len < held {
                found.restarted += 1;
                debug!(file = %path.display(), "journal rewritten, rereading");
                0
            } else if len == held {
                continue;
            } else {
                held
            };

            match self.since(&path, from, &mut found) {
                Ok(at) => {
                    self.read.insert(path, at);
                }
                Err(err) => {
                    warn!(file = %path.display(), error = %err, "unreadable");
                }
            }
        }
        self.route(&mut found);
        Ok(found)
    }

    /// The route file, if it has been rewritten since it was last read.
    ///
    /// Warned and passed over where it cannot be read or does not parse: a
    /// route is a bonus — systems named ahead of the ship — and a journal
    /// whose route file is missing or half written is still a journal. The
    /// game does write it empty, and an empty route is a route: it says the
    /// ship is no longer going anywhere, and nothing here removes systems, so
    /// reading it costs a pass and changes nothing.
    fn route(&mut self, found: &mut Read) {
        let path = self.dir.join(ROUTE_FILE);
        let Ok(meta) = path.metadata() else { return };
        let Ok(at) = meta.modified() else { return };
        let now = (meta.len(), at);
        if self.route == Some(now) {
            return;
        }

        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(err) => {
                warn!(file = %path.display(), error = %err, "unreadable route");
                return;
            }
        };
        match serde_json::from_str::<Entry<NavRoute>>(&text) {
            Ok(entry) => {
                self.route = Some(now);
                found.routed = true;
                found.entries.push(Entry {
                    timestamp: entry.timestamp,
                    event: Event::NavRoute(entry.event),
                    horizons: entry.horizons,
                    odyssey: entry.odyssey,
                });
            }
            // Not recorded as read, so the next poll tries again: this is
            // most often the file caught mid-rewrite.
            Err(err) => {
                debug!(file = %path.display(), error = %err, "unread route");
            }
        }
    }

    /// Read whole lines from `from`, answering the offset reached.
    ///
    /// The offset lands on the last newline read and never past it, so a
    /// partial line at the end of the file is left where it is and read again
    /// whole once the game has finished writing it.
    fn since(
        &self,
        path: &Path,
        from: u64,
        found: &mut Read,
    ) -> io::Result<u64> {
        let mut file = BufReader::new(File::open(path)?);
        file.seek(SeekFrom::Start(from))?;

        let mut at = from;
        let mut line = Vec::new();
        loop {
            line.clear();
            let bytes = file.read_until(b'\n', &mut line)?;
            if bytes == 0 {
                break;
            }
            if line.last() != Some(&b'\n') {
                // The end of the file, mid-line. Leave it unread.
                break;
            }
            at += bytes as u64;

            let text = match std::str::from_utf8(&line) {
                Ok(text) => text.trim(),
                Err(_) => {
                    found.unread += 1;
                    continue;
                }
            };
            if text.is_empty() {
                continue;
            }
            match serde_json::from_str::<Entry<Event>>(text) {
                Ok(entry) => found.entries.push(entry),
                Err(_) => found.unread += 1,
            }
        }
        Ok(at)
    }
}

/// Where the game writes the route the ship is flying, beside its logs.
const ROUTE_FILE: &str = "NavRoute.json";

/// The `.log` files in a journal directory, in the order they were written.
///
/// Sorted by name, which is the order the game's own naming gives: the
/// timestamp is the leading part of it and the part number follows, so a
/// session continued into a second file sorts after the first. The same rule
/// `galos-sync journal` reads a directory by.
fn logs(dir: &Path) -> io::Result<Vec<PathBuf>> {
    let mut logs: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("log")
        })
        .collect();
    logs.sort();
    Ok(logs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// A scratch journal directory of this test's own.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_journal_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    fn jump(system: &str, address: i64, at: [f64; 3]) -> String {
        format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump","StarSystem":"{}","SystemAddress":{},"StarPos":[{},{},{}]}}"#,
            system, address, at[0], at[1], at[2],
        )
    }

    /// A poll reads what has been appended and nothing it has read before
    ///
    /// The whole of what following is. Read whole every time, a commander's
    /// journal is a gigabyte of finished logs re-parsed every second.
    #[test]
    fn a_poll_reads_only_what_arrived() {
        let dir = scratch("appended");
        let path = dir.join("Journal.2026-08-08T120000.01.log");
        std::fs::write(&path, format!("{}\n", jump("Sol", 1, [0.0; 3])))
            .expect("the first line");

        let mut follower = Follower::new(&dir);
        assert_eq!(follower.poll().expect("a poll").entries.len(), 1);
        assert_eq!(
            follower.poll().expect("a second poll").entries.len(),
            0,
            "a quiet journal was read again"
        );

        let mut file =
            File::options().append(true).open(&path).expect("the log opens");
        writeln!(file, "{}", jump("Alpha Centauri", 2, [3.0, 0.0, 3.0]))
            .expect("the second line");

        let found = follower.poll().expect("a third poll");
        assert_eq!(found.entries.len(), 1, "the appended line was not read");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A line still being written is left for the next poll
    ///
    /// The game appends one object per line and a poll lands where it lands.
    /// Parsing half an object throws the event away, and the offset moving
    /// past it means nothing ever reads the other half.
    #[test]
    fn a_half_written_line_waits() {
        let dir = scratch("torn");
        let path = dir.join("Journal.2026-08-08T120000.01.log");
        let whole = jump("Sol", 1, [0.0; 3]);
        let (head, tail) = whole.split_at(whole.len() / 2);
        std::fs::write(&path, head).expect("half a line");

        let mut follower = Follower::new(&dir);
        let found = follower.poll().expect("a poll");
        assert!(found.entries.is_empty(), "half a line was read as an event");
        assert_eq!(found.unread, 0, "half a line was counted as a bad one");

        let mut file =
            File::options().append(true).open(&path).expect("the log opens");
        writeln!(file, "{tail}").expect("the rest of the line");
        assert_eq!(
            follower.poll().expect("a second poll").entries.len(),
            1,
            "the finished line never arrived"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file that has shrunk is not the file whose offset was held
    #[test]
    fn a_replaced_journal_is_read_again() {
        let dir = scratch("replaced");
        let path = dir.join("Journal.2026-08-08T120000.01.log");
        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                jump("Sol", 1, [0.0; 3]),
                jump("Alpha Centauri", 2, [3.0, 0.0, 3.0])
            ),
        )
        .expect("two lines");

        let mut follower = Follower::new(&dir);
        assert_eq!(follower.poll().expect("a poll").entries.len(), 2);

        std::fs::write(&path, format!("{}\n", jump("Sol", 1, [0.0; 3])))
            .expect("one line in its place");
        let found = follower.poll().expect("a second poll");
        assert_eq!(found.restarted, 1, "the shorter file was taken as a tail");
        assert_eq!(found.entries.len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
