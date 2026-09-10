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
//! restores a backup, or points the map at a different one. What is kept per
//! path is the offset read out of the file and how the file looked when that
//! offset was taken — its length and when it was last written — because the
//! offset alone cannot say. It stops at the last newline, so it is short of
//! the length whenever the game was mid-line, and a file rewritten to a
//! length between the two would read as one appended to. Shorter than it
//! was, or dated before the reading was taken, and it is read again from the
//! start. Applying an event twice is free — everything downstream is keyed by
//! address and body id — so re-reading is always the safe answer.
//!
//! What that misses is a file replaced by one at least as long whose
//! modification time moved forward, which is exactly what an append looks
//! like and cannot be told from one without reading the whole file back. A
//! copied-in log longer than the one it replaced is the case, and it costs
//! whatever the two files do not have in common.
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
use std::io::{self, BufRead, BufReader, Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
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
    /// How far into each file has been read, and what the file looked like
    /// then, by path.
    ///
    /// Bytes and not lines: a line count cannot be seeked to, and the point of
    /// holding anything is not to read a gigabyte of finished logs on every
    /// poll.
    read: HashMap<PathBuf, At>,
    /// The length and modification time `NavRoute.json` was last read at.
    ///
    /// Not an offset: the file is rewritten whole rather than appended to, so
    /// there is no tail of it to follow and the question is only whether it is
    /// the same file it was. Both halves, because a route replotted onto a
    /// list of the same length leaves the length where it was.
    ///
    /// Neither half is proof. A filesystem that dates writes to the second
    /// gives two rewrites inside one tick the same time, so a replot onto the
    /// same number of stops in that tick is missed — until the route changes
    /// length or is plotted again. That is systems named ahead of a ship that
    /// has not flown to them yet, which is the least of what a journal says.
    route: Option<(u64, SystemTime)>,
}

/// How far into one log has been read, and what the file was when it was.
///
/// The offset alone cannot say whether the file that is there now is the file
/// it came out of. A read stops at the last newline, so the offset is short
/// of the length whenever the game was mid-line, and a file rewritten to a
/// length between the two reads as one that has been appended to. Holding
/// the length as well as the offset is what tells those apart.
#[derive(Clone, Copy, Debug, PartialEq)]
struct At {
    /// The offset entries have been made out of, which lands on a newline.
    read: u64,
    /// How long the file was then, which is at or past `read`.
    len: u64,
    /// When it was last written, where the filesystem says. [`None`] on one
    /// that does not, which leaves the length as the whole of the answer.
    modified: Option<SystemTime>,
}

impl At {
    /// Whether a file now `len` long and last written at `modified` is a
    /// different file from the one this offset came out of.
    ///
    /// Shorter than it was, or written before this reading was taken. An
    /// append can do neither.
    fn replaced(&self, len: u64, modified: Option<SystemTime>) -> bool {
        len < self.len
            || matches!(
                (modified, self.modified),
                (Some(now), Some(then)) if now < then
            )
    }

    /// Whether the file is the same length and the same age it was, which is
    /// a file nothing has been written to since it was read.
    fn quiet(&self, len: u64, modified: Option<SystemTime>) -> bool {
        self.len == len && self.modified == modified
    }
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
            // The end of the last whole line rather than the length. The
            // game may be a few bytes into writing the next one, and an
            // offset inside a line has the next poll read that line from its
            // middle: the entry is parsed as garbage and lost, which is the
            // one thing following is supposed never to do.
            let read = match last_line_end(&path, meta.len()) {
                Ok(read) => read,
                Err(err) => {
                    warn!(file = %path.display(), error = %err, "unreadable");
                    continue;
                }
            };
            let held = self.read.get(&path).map_or(0, |at| at.read);
            skipped += read.saturating_sub(held);
            self.read.insert(
                path,
                At { read, len: meta.len(), modified: meta.modified().ok() },
            );
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
            let (len, modified) = match path.metadata() {
                Ok(meta) => (meta.len(), meta.modified().ok()),
                Err(err) => {
                    warn!(file = %path.display(), error = %err, "unstattable");
                    continue;
                }
            };

            let from = match self.read.get(&path) {
                // Not the file that offset came out of. Read it again from
                // the top.
                Some(held) if held.replaced(len, modified) => {
                    found.restarted += 1;
                    debug!(
                        file = %path.display(),
                        "journal rewritten, rereading",
                    );
                    0
                }
                // Nothing has been written to it since it was read, so
                // whatever the last poll left unread is still all there is.
                Some(held) if held.quiet(len, modified) => continue,
                Some(held) => held.read,
                None => 0,
            };

            match self.since(&path, from, &mut found) {
                Ok(read) => {
                    self.read.insert(path, At { read, len, modified });
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

/// The offset just past the last newline in the first `len` bytes of `path`,
/// or zero where there is no whole line in them at all.
///
/// What taking a directory as read has to record, and the length is not it: a
/// journal caught mid-write ends inside a line, and an offset there is that
/// line read from its middle on the next poll — one entry parsed as garbage
/// and dropped.
///
/// Read backwards a block at a time from the end, so a finished log of a
/// gigabyte costs one read of a few kilobytes: a journal line is hundreds of
/// bytes, so the newline is in the first block looked at unless the game is
/// writing something far longer than it has ever written.
fn last_line_end(path: &Path, len: u64) -> io::Result<u64> {
    const BLOCK: u64 = 8 * 1024;
    let mut file = File::open(path)?;
    let mut block = vec![0u8; BLOCK as usize];
    let mut end = len;
    while end > 0 {
        let from = end.saturating_sub(BLOCK);
        let want = (end - from) as usize;
        file.seek(SeekFrom::Start(from))?;
        file.read_exact(&mut block[..want])?;
        if let Some(found) = block[..want].iter().rposition(|&b| b == b'\n') {
            return Ok(from + found as u64 + 1);
        }
        end = from;
    }
    Ok(0)
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

    /// A file replaced by one longer than the offset held is read again
    ///
    /// The offset is not the length. A poll that lands mid-line records the
    /// newline before it and leaves the rest, so a file rewritten to a length
    /// past that offset and short of the length seen is a file the offset
    /// says nothing about — and read from there it is a line parsed from its
    /// middle with everything before it gone.
    #[test]
    fn a_file_replaced_past_the_offset_is_read_again() {
        let dir = scratch("rewritten");
        let path = dir.join("Journal.2026-08-08T120000.01.log");
        // One whole line and the beginning of a second, which is the game
        // caught in the middle of writing.
        let torn = jump("Barnard's Star", 3, [-3.03, -0.09, -3.16]);
        std::fs::write(
            &path,
            format!(
                "{}\n{}",
                jump("Sol", 1, [0.0; 3]),
                &torn[..torn.len() / 2]
            ),
        )
        .expect("a log caught mid-line");

        let mut follower = Follower::new(&dir);
        assert_eq!(follower.poll().expect("a poll").entries.len(), 1);

        // A file put in its place, longer than the offset held and shorter
        // than the length that was seen.
        std::fs::write(
            &path,
            format!("{}\n", jump("Alpha Centauri", 2, [3.03, -0.09, 3.16])),
        )
        .expect("one line in its place");

        let found = follower.poll().expect("a second poll");
        assert_eq!(found.restarted, 1, "the new file was taken as a tail");
        assert_eq!(found.entries.len(), 1, "the new file's one line was lost");
        assert_eq!(found.unread, 0, "a line was read from its middle");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Taking a directory as read stops at the last whole line
    ///
    /// The importer this is for reads the whole directory its own way, and
    /// the game goes on writing while it does. Taken as read to the length,
    /// the next poll starts inside whatever line was half written and parses
    /// its tail: one entry lost, and the only one the commander was there
    /// for.
    #[test]
    fn catching_up_stops_at_the_last_whole_line() {
        let dir = scratch("caught_up");
        let path = dir.join("Journal.2026-08-08T120000.01.log");
        let whole = jump("Alpha Centauri", 2, [3.03, -0.09, 3.16]);
        let (head, tail) = whole.split_at(whole.len() / 2);
        std::fs::write(&path, format!("{}\n{head}", jump("Sol", 1, [0.0; 3])))
            .expect("a log caught mid-line");

        let mut follower = Follower::new(&dir);
        follower.caught_up().expect("the directory is taken as read");

        let mut file =
            File::options().append(true).open(&path).expect("the log opens");
        writeln!(file, "{tail}").expect("the rest of the line");

        let found = follower.poll().expect("a poll");
        assert_eq!(found.unread, 0, "a line was read from its middle");
        assert_eq!(found.entries.len(), 1, "the torn line was skipped over");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
