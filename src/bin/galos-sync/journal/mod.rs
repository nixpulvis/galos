//! Importing what the game wrote while it was being played
//!
//! A journal directory read once, in the order it happened, and written to
//! the database through [`record`] -- the same thing the EDDN subscriber
//! writes through, the events being the same events.
//
// TODO: Publishing, which is the direction this does not go yet. Everything
// read here is something EDDN wants and is not getting from this commander,
// and the reading is already done.
//
// What it would take that importing does not is augmentation: a sender has
// to add `StarSystem` and `StarPos` to the events the game writes without
// them, tracked from the last arrival, cross-checked against whatever
// `SystemAddress` the event carries, and the message dropped where the two
// disagree, the game having a habit of pausing its journal and resuming it
// with events missing. Importing is excused that because the row is already
// there to point at; a sender has nobody to point at and has to say the
// whole thing.
//
// Reading a directory the way this does is a better place to do it from than
// a live sender has. The files are whole and in order before anything is
// looked at, so where a sender is guessing from what it has seen so far,
// this can look forward and back and know. What it does not have is the rest
// of what a sender owes EDDN -- the personal fields stripped, the `horizons`
// and `odyssey` flags off `LoadGame`, a schema and a header wrapped around
// each message, and the gateway's rules about how much and how often.

use crate::{bar, Run};
use async_std::task;
use elite_journal::entry::{Entry, Event, NavRoute};
use elite_journal::system::Coordinate;
use galos_db::Database;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, ErrorKind};
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use tracing::{info, warn};

pub mod record;

/// Where a journal directory remembers whose it is
///
/// A session continued into a second file introduces nobody and
/// `NavRoute.json` never says who is flying, so the name has to come from
/// somewhere. Per directory rather than per run: two directories are as
/// likely to be two commanders as one, and a name carried over from the last
/// import would be a guess about someone else's journal.
const COMMANDER: &str = ".galos-commander";

/// Who a journal is filed under when nothing anywhere says
///
/// Not a commander anyone has, which is the point. These rows came from a
/// journal and that is all this claims about them.
const UNKNOWN: &str = "unknown";

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "PATH")]
    pub path: String,

    #[structopt(
        short = "u",
        long = "user",
        help = "Whose journal this is, overriding what the files say"
    )]
    pub user: Option<String>,
    // TODO: `Market.json`, `Shipyard.json` and `Outfitting.json`, which the
    // game keeps beside its logs and rewrites at every station. Nothing reads
    // them yet: `elite_journal` models these three on the shape EDDN sends,
    // which is not the shape the game writes -- camelCase against Pascal,
    // `commodities` against `Items`, and item fields that differ under both.
    // Reading them needs types for the game's shape in `elite_journal`, which
    // convert into the ones `record::market` and its neighbors already take.
}

impl Run for Cli {
    /// Import what the path names, ending the process where any of it was not
    ///
    /// The status is the whole of what cron reads, so a run that lost a
    /// journal to the filesystem must not look like one with nothing left to
    /// do. What could be read is written either way.
    fn run(&self, db: &Database) {
        if !self.import(db) {
            std::process::exit(1);
        }
    }
}

impl Cli {
    /// Write what the path holds, answering whether all of it could be read
    fn import(&self, db: &Database) -> bool {
        let path = Path::new(&self.path);
        let Ok(meta) = fs::metadata(path) else {
            warn!(path = %path.display(), "nothing to import at this path");
            return false;
        };

        // A directory is a journal directory. A single file is one of the
        // files in one, so the directory holding it is asked who flew this --
        // and is not told anything back. One file is not the directory's, and
        // an archived log imported on its own would otherwise leave its
        // commander behind for everything imported there afterwards.
        let dir = if meta.is_dir() {
            path.to_owned()
        } else {
            path.parent().unwrap_or(Path::new(".")).to_owned()
        };

        let paths = if meta.is_dir() {
            match logs(path) {
                Some(paths) => paths,
                None => return false,
            }
        } else {
            vec![path.to_owned()]
        };

        // Read whole, as a directory always has been read here. What that
        // buys is order: a write is refused now where something newer already
        // stands in its place, so entries have to reach the database in the
        // order they happened or an import lands differently every time. A
        // file's own first entry is what says where the file belongs, which
        // is the order a commander's name carries forward in.
        let mut journals = Vec::new();
        let mut refused = 0;
        for path in &paths {
            match read(path) {
                Some(mut entries) => {
                    entries.sort_by_key(|entry| entry.timestamp);
                    journals.push((path.to_owned(), entries));
                }
                None => refused += 1,
            }
        }
        journals.sort_by_key(|(_, entries)| {
            entries.first().map(|entry| entry.timestamp)
        });

        // What could be read is still worth writing, and re-running costs
        // nothing, so the refused ones do not stop the rest. They do decide
        // the status: a journal not read is a journal not imported, and a run
        // that lost one quietly is a run nobody goes back to.
        if refused > 0 {
            warn!(
                path = %path.display(),
                refused = refused,
                read = journals.len(),
                "journals that could not be read",
            );
        }

        // What the command line said, which outranks every file, or what the
        // directory remembered, which is what a file introducing nobody falls
        // back to.
        let mut known = self.user.clone().or_else(|| remembered(&dir));

        // Who each journal is filed under. A session names its commander at
        // the top of the file it opens and a file continued from it names
        // nobody, so the answer carries forward from the file before.
        let users: Vec<Option<String>> = journals
            .iter()
            .map(|(_, entries)| {
                if self.user.is_none() {
                    if let Some(name) = commander(entries) {
                        known = Some(name);
                    }
                }
                known.clone()
            })
            .collect();

        // Every system the directory names, written before anything points
        // at one. Four of the events the game writes name only an address,
        // and the game writes them ahead of the arrival that would have made
        // the row, so without this the foreign key turns them all away.
        let names = gather_names(&journals);
        task::block_on(async {
            for (address, (journal, entry, name, pos)) in &names {
                let user = users[*journal].as_deref().unwrap_or(UNKNOWN);
                record::ensure_system(
                    db,
                    entry.timestamp,
                    user,
                    *address,
                    Some(name),
                    *pos,
                    "system named",
                )
                .await;
            }
        });

        let bar = progress(journals.iter().map(|(_, e)| e.len() as u64).sum());
        // Every line the log prints from here goes above the bar, so the bar
        // keeps the bottom line for the length of the import.
        let drawing = bar::under(&bar);

        for run in replay(&journals).chunk_by(|(a, _), (b, _)| a == b) {
            let journal = run[0].0;
            let user = users[journal].as_deref().unwrap_or(UNKNOWN);

            bar.set_message(
                journals[journal]
                    .0
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
            task::block_on(async {
                for (_, entry) in run {
                    record::entry(db, entry, user).await;
                }
            });
            bar.inc(run.len() as u64);
        }
        bar.finish();
        drop(drawing);

        // Whatever is beside the logs, whether one of them or all of them
        // were asked for. The route is where the ship is going now and there
        // is one copy of it, so the directory holding a single file carries
        // it just as much as the directory does.
        sidecars(db, &dir, known.as_deref().unwrap_or(UNKNOWN));

        // Only a whole directory gets to say whose it is, and only what the
        // logs said. A name given on the command line is for the run it was
        // given on, and writing it here would file every later import of
        // this directory under it, the ones that asked for nothing included.
        if meta.is_dir() && self.user.is_none() {
            if let Some(name) = &known {
                remember(&dir, name);
            }
        }

        refused == 0
    }
}

/// Where every system this import names is, by the address it is known by
///
/// The reason an importer reads a whole directory before it writes any of it.
/// The game writes an event per signal as it arrives somewhere and writes the
/// arrival that names the system afterwards, in the same second: 55 of Sol's
/// signals stand above its `FSDJump` in a journal here. So a system's signals
/// reach the database ahead of the thing that would have created the system,
/// and the foreign key turns every one of them away.
///
/// EDDN answers this by making a sender copy a name into every message it
/// forwards. An importer has the whole of it in front of it instead, so it
/// takes the name from wherever in the directory it is given. What is written
/// from this is a name and a place. Everything else about a system is written
/// by the events themselves, in the order they happened.
///
/// The first naming of an address wins, which is the earliest, so the row is
/// stamped at the first the import knows of the place rather than the last.
fn gather_names<'a>(
    journals: &'a [(PathBuf, Vec<Entry<Event>>)],
) -> BTreeMap<i64, (usize, &'a Entry<Event>, &'a str, Option<Coordinate>)> {
    let mut names = BTreeMap::new();

    for (journal, entry) in replay(journals) {
        let said: Option<(i64, &str, Option<Coordinate>)> = match &entry.event {
            Event::Location(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::CarrierJump(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::FsdJump(e) => {
                Some((e.system.address, &e.system.name, e.system.pos))
            }
            Event::Docked(e) => Some((e.system_address, &e.system_name, None)),
            Event::Scan(e) => {
                Some((e.system_address, &e.star_system, e.star_pos))
            }
            Event::ScanBaryCentre(e) => {
                Some((e.system_address, &e.star_system, e.star_pos))
            }
            Event::FssDiscoveryScan(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            Event::FssAllBodiesFound(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            Event::CodexEntry(e) => {
                Some((e.system_address, &e.system_name, e.star_pos))
            }
            // The events the game writes without a name say nothing here.
            // They are what this is for.
            _ => None,
        };

        if let Some((address, name, pos)) = said {
            names.entry(address).or_insert((journal, entry, name, pos));
        }
    }

    names
}

/// Every entry of every journal, in the order they happened
///
/// Each paired with the journal it came out of, which is what says who flew
/// it and what the bar is showing.
///
/// A whole file at a time is not that order. The Live and Legacy clients
/// write into one Saved Games directory and their files cover the same
/// afternoons, so replaying one file before starting the next puts an older
/// reading of a station over a newer one, which a guarded write cannot tell
/// from an update. Sorting is stable, so entries stamped the same second are
/// left in the order their files stand in.
fn replay(
    journals: &[(PathBuf, Vec<Entry<Event>>)],
) -> Vec<(usize, &Entry<Event>)> {
    let mut replayed: Vec<_> = journals
        .iter()
        .enumerate()
        .flat_map(|(journal, (_, entries))| {
            entries.iter().map(move |entry| (journal, entry))
        })
        .collect();

    replayed.sort_by_key(|(_, entry)| entry.timestamp);
    replayed
}

/// The journal files in a directory, in the order they were started
///
/// Anything ending `.log`, which is every journal the game has written under
/// either of the two names it has given them. A directory that will not open
/// answers nothing rather than none of them, which is a different thing and
/// is what the import's status turns on.
///
/// By name, which the game makes the order they were started in. `read_dir`
/// answers in whatever order the filesystem holds them, and on a directory
/// of any size that is a hash rather than an order. Two journals whose first
/// entries fall in the same second tie in every sort downstream, and a stable
/// sort breaks a tie by the order it was handed, so leaving this to the
/// filesystem is an import that lands differently between runs.
fn logs(dir: &Path) -> Option<Vec<PathBuf>> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) => {
            warn!(dir = %dir.display(), error = %err, "unreadable directory");
            return None;
        }
    };

    let mut logs: Vec<PathBuf> = read
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("log")
        })
        .collect();

    logs.sort();
    Some(logs)
}

/// Read one journal file, saying what in it could not be read
///
/// A line counted here is one this claims to write, since an event nothing
/// models is read as [`Event::Other`] rather than failing. Said once a file
/// with a count and a reason rather than once a line, a journal that hits
/// this hitting it thousands of times over for the same reason.
fn read(path: &Path) -> Option<Vec<Entry<Event>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            warn!(file = %path.display(), error = %err, "unreadable journal");
            return None;
        }
    };

    entries(BufReader::new(file), path)
}

/// The entries in an open journal, saying what in it could not be read
///
/// Two kinds of failure and only one of them is a line. `InvalidData` is a
/// torn one: `read_until` took its bytes and gave back something that is not
/// UTF-8, so the next line is there to read and this one is counted with the
/// lines that would not parse. Any other error took nothing, and `lines`
/// answers it again every time it is asked, so carrying on past one is a
/// loop with no end to it. That is the file having stopped rather than the
/// line, and it is reported as the file.
fn entries(journal: impl BufRead, path: &Path) -> Option<Vec<Entry<Event>>> {
    let mut found = Vec::new();
    let mut unread = 0;
    let mut why = None;

    for line in journal.lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) if err.kind() == ErrorKind::InvalidData => {
                unread += 1;
                why.get_or_insert_with(|| err.to_string());
                continue;
            }
            Err(err) => {
                warn!(
                    file = %path.display(),
                    error = %err,
                    read = found.len(),
                    "journal stopped being readable",
                );
                return None;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str(&line) {
            Ok(entry) => found.push(entry),
            Err(err) => {
                unread += 1;
                why.get_or_insert_with(|| err.to_string());
            }
        }
    }

    if unread > 0 {
        warn!(
            file = %path.display(),
            unread = unread,
            read = found.len(),
            first = %why.unwrap_or_default(),
            "entries this cannot read",
        );
    }

    Some(found)
}

/// The files the game keeps beside its logs
///
/// Each holds the whole of something as it stands rather than a record of it
/// changing, and is rewritten in place. Only the ones something here reads are
/// looked for; the rest -- `Status.json`, `Cargo.json`, `ShipLocker.json` and
/// their like -- describe a ship and a commander rather than a galaxy, and
/// there is nowhere to put them.
fn sidecars(db: &Database, dir: &Path, user: &str) {
    let route = dir.join("NavRoute.json");
    if !route.is_file() {
        return;
    }

    // Read here rather than through `parse_status_file`, which opens the file
    // behind an `unwrap`. This runs after every log has been written, and a
    // route the filesystem would not hand over is no reason to take the whole
    // import down at the end of it.
    let json = match fs::read_to_string(&route) {
        Ok(json) => json,
        Err(err) => {
            warn!(file = %route.display(), error = %err, "unreadable nav route");
            return;
        }
    };

    match serde_json::from_str::<Entry<NavRoute>>(&json) {
        Ok(entry) => task::block_on(record::nav_route(
            db,
            entry.timestamp,
            user,
            &entry.event.destinations,
        )),
        Err(err) => {
            warn!(file = %route.display(), error = %err, "unreadable nav route")
        }
    }
}

/// Who flew these entries, where they say
///
/// Named twice at the top of a session, by `Commander` and again by
/// `LoadGame`, and once more by `NewCommander` for the first file a journal
/// ever held. Any of them answers it. A file continued from an earlier session
/// names nobody and is left to whatever the directory remembers.
fn commander(entries: &[Entry<Event>]) -> Option<String> {
    entries.iter().find_map(|entry| match &entry.event {
        Event::Commander(commander) => Some(commander.name.clone()),
        Event::NewCommander(new) => Some(new.commander.name.clone()),
        Event::LoadGame(load) => {
            load.commander.as_ref().map(|c| c.name.clone())
        }
        _ => None,
    })
}

/// What a directory was told last time it was imported
fn remembered(dir: &Path) -> Option<String> {
    let name = fs::read_to_string(dir.join(COMMANDER)).ok()?;
    let name = name.trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// Tell a directory who flew what is in it
///
/// The directory belongs to the game, and writing to it is a courtesy to the
/// next import rather than something this one needs. Refused, say so and carry
/// on: everything is already written.
fn remember(dir: &Path, name: &str) {
    if remembered(dir).as_deref() == Some(name) {
        return;
    }

    let path = dir.join(COMMANDER);
    match fs::write(&path, format!("{}\n", name)) {
        Ok(()) => {
            info!(commander = %name, file = %path.display(), "remembered")
        }
        Err(err) => {
            warn!(file = %path.display(), error = %err, "could not be remembered")
        }
    }
}

/// How far along an import is, where there is someone to show
fn progress(entries: u64) -> ProgressBar {
    let bar = ProgressBar::new(entries);
    bar.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
        .unwrap()
        .progress_chars("##-"));

    if !bar::worth_drawing() {
        bar.set_draw_target(ProgressDrawTarget::hidden());
    }

    bar
}

/// Reading a journal directory, as far as the first write
///
/// What a file said, who flew it and the order it happened in are settled
/// before a row is touched, so all of it is answerable without a database.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn parse(lines: &[&str]) -> Vec<Entry<Event>> {
        lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("entry should parse"))
            .collect()
    }

    /// A directory of this test's own, emptied before it writes anything
    ///
    /// Named for the test, so two running at once do not share one, and left
    /// behind afterwards, which is what makes a failure readable.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join("galos-journal").join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a scratch directory should be made");
        dir
    }

    /// Write a journal file, byte for byte, and answer where it went
    fn journal(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        File::create(&path)
            .and_then(|mut file| file.write_all(bytes))
            .expect("a journal should be writable");
        path
    }

    /// One line of a journal that is not valid UTF-8
    ///
    /// A torn write or a bad sector. `BufRead::lines` answers this line with
    /// an error and goes on to the next one, so the file does not end here.
    const TORN: &[u8] = b"{\"timestamp\":\"2026-08-08T12:00:01Z\",\
        \"event\":\"Music\",\"MusicTrack\":\"\xff\xfe\"}";

    /// The header every journal file opens with, naming nobody
    const FILEHEADER: &str = r#"{
        "timestamp": "2026-08-08T12:00:00Z",
        "event": "Fileheader",
        "part": 1,
        "language": "English/UK",
        "gameversion": "4.0.0.1904",
        "build": "r308767/r0"
    }"#;

    /// `Commander`, which follows the header at the top of a session
    #[test]
    fn a_journal_names_who_flew_it() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:01Z",
                "event": "Commander",
                "FID": "F123456",
                "Name": "Nixpulvis"
            }"#,
        ]);

        assert_eq!(commander(&entries).as_deref(), Some("Nixpulvis"));
    }

    /// `LoadGame`, which says the same thing under another key
    #[test]
    fn a_loaded_game_names_one_as_well() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:02Z",
                "event": "LoadGame",
                "FID": "F123456",
                "Commander": "Nixpulvis",
                "Horizons": true,
                "GameMode": "Solo",
                "Credits": 1000,
                "Loan": 0
            }"#,
        ]);

        assert_eq!(commander(&entries).as_deref(), Some("Nixpulvis"));
    }

    /// A session continued into a second file introduces nobody
    ///
    /// The case the directory is asked to remember for. Nothing in the file
    /// says who flew it, and the entries in it are worth exactly as much as
    /// the ones in the file it continues.
    #[test]
    fn a_continued_journal_names_nobody() {
        let entries = parse(&[
            FILEHEADER,
            r#"{
                "timestamp": "2026-08-08T12:00:03Z",
                "event": "FSDJump",
                "StarSystem": "Sol",
                "StarPos": [0.0, 0.0, 0.0],
                "SystemAddress": 10477373803
            }"#,
        ]);

        assert_eq!(commander(&entries), None);
    }

    /// A line the filesystem will not hand over costs that line and no more
    ///
    /// One bad byte in a file of thousands. Everything either side of it was
    /// written by the game and is worth as much as it ever was.
    #[test]
    fn a_torn_line_does_not_take_the_file_with_it() {
        let dir = scratch("a_torn_line");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(
            br#"{"timestamp":"2026-08-08T12:00:00Z","event":"NavRoute"}"#,
        );
        bytes.push(b'\n');
        bytes.extend_from_slice(TORN);
        bytes.push(b'\n');
        bytes.extend_from_slice(
            br#"{"timestamp":"2026-08-08T12:00:02Z","event":"NavRoute"}"#,
        );
        bytes.push(b'\n');

        let entries = read(&journal(&dir, "Journal.torn.log", &bytes))
            .expect("the file should read");

        assert_eq!(entries.len(), 2);
    }

    /// A journal that stops being readable is not a journal read to the end
    ///
    /// `BufRead::lines` answers an error that consumed nothing by answering
    /// it again, and again, every time it is asked. So counting one and
    /// carrying on never reaches the end of the file: the import hangs, and
    /// says nothing while it does. Only a torn line is worth carrying on
    /// past, and a torn line is the one whose bytes were taken.
    #[test]
    fn a_journal_that_stops_being_readable_is_not_read_to_the_end() {
        /// A reader that fails the way a disk going away fails
        struct Failing(usize);

        impl std::io::Read for Failing {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                self.0 += 1;
                // Ends the file rather than the test, so a reader that
                // carries on past the error fails on the answer below
                // instead of hanging the suite.
                if self.0 > 100 {
                    return Ok(0);
                }
                Err(std::io::Error::other("the disk went away"))
            }
        }

        let dying = BufReader::new(Failing(0));

        assert!(entries(dying, Path::new("Journal.dying.log")).is_none());
    }

    /// A directory that will not open is not one holding no journals
    ///
    /// They look alike from here, and the import's status turns on telling
    /// them apart: nothing to import is a run that worked, and a directory
    /// the filesystem refused is not.
    #[test]
    fn a_directory_that_will_not_open_answers_nothing() {
        let dir = scratch("a_directory_that_will_not_open");

        assert_eq!(logs(&dir), Some(Vec::new()));
        assert_eq!(logs(&dir.join("no-such-directory")), None);
    }

    /// The journals of a directory come back in the order they were written
    ///
    /// `read_dir` answers in whatever order the filesystem holds them, which
    /// is not an order and need not be the same one twice. Two journals whose
    /// first entries fall in the same second tie in every sort downstream,
    /// and a tie is broken by the order they arrived in, so leaving that to
    /// the filesystem is an import that lands differently between runs. The
    /// game names them for when they were started, so the name is the order.
    #[test]
    fn the_journals_of_a_directory_are_in_the_order_they_were_started() {
        let dir = scratch("the_journals_of_a_directory");

        // Enough of them, written in the reverse of the order wanted, that a
        // directory held by hash has room to disagree with both. Three would
        // come back in order on a filesystem that happened to oblige and
        // pass this whatever the code did.
        let mut wanted: Vec<String> = (3..28)
            .map(|day| format!("Journal.2026-08-{:02}T000000.01.log", day))
            .collect();
        for name in wanted.iter().rev() {
            journal(&dir, name, b"");
        }
        journal(&dir, "NavRoute.json", b"{}");
        wanted.sort();

        let found: Vec<String> = logs(&dir)
            .expect("the directory should read")
            .iter()
            .map(|path| {
                path.file_name().unwrap().to_string_lossy().into_owned()
            })
            .collect();

        assert_eq!(found, wanted);
    }

    /// A journal that will not open is not one holding no entries
    #[test]
    fn a_journal_that_will_not_open_answers_nothing() {
        let dir = scratch("a_journal_that_will_not_open");

        let empty = read(&journal(&dir, "Journal.empty.log", b""));
        assert_eq!(empty.map(|entries| entries.len()), Some(0));
        assert!(read(&dir.join("no-such-journal.log")).is_none());
    }

    /// An entry that is nothing but the moment it happened
    fn at(minute: &str) -> Entry<Event> {
        serde_json::from_str(&format!(
            r#"{{ "timestamp": "2026-08-08T{}:00Z", "event": "NavRoute" }}"#,
            minute,
        ))
        .expect("entry should parse")
    }

    /// The minute each entry of a replay happened, in the order it is written
    fn minutes(journals: &[(PathBuf, Vec<Entry<Event>>)]) -> Vec<String> {
        replay(journals)
            .iter()
            .map(|(_, entry)| entry.timestamp.format("%H:%M").to_string())
            .collect()
    }

    /// A file at a time is not the order the game wrote them in
    ///
    /// The Live and Legacy clients write into one Saved Games directory, so
    /// two files covering the same afternoon is an ordinary directory rather
    /// than a damaged one.
    #[test]
    fn overlapping_journals_are_replayed_in_the_order_they_happened() {
        let journals = vec![
            (PathBuf::from("Journal.live.log"), vec![at("12:00"), at("18:00")]),
            (PathBuf::from("Journal.legacy.log"), vec![at("12:30")]),
        ];

        assert_eq!(minutes(&journals), ["12:00", "12:30", "18:00"]);
    }

    /// A system is named by an event standing below what points at it
    ///
    /// What the game writes on arriving somewhere: the signals it finds
    /// first, then the jump saying where it got to, all in one second. The
    /// signals name only an address, so read a line at a time there is
    /// nothing to make the row from and the foreign key turns every one of
    /// them away. Reading the directory whole is what answers it.
    #[test]
    fn a_system_named_below_what_points_at_it_is_still_named() {
        let journals = vec![(
            PathBuf::from("Journal.arriving.log"),
            parse(&[
                r#"{
                    "timestamp": "2026-08-12T04:02:36Z",
                    "event": "FSSSignalDiscovered",
                    "SystemAddress": 10477373803,
                    "SignalName": "Titan City"
                }"#,
                r#"{
                    "timestamp": "2026-08-12T04:02:36Z",
                    "event": "FSDJump",
                    "StarSystem": "Sol",
                    "StarPos": [1.0, 2.0, 3.0],
                    "SystemAddress": 10477373803
                }"#,
            ]),
        )];

        let names = gather_names(&journals);
        let (_, _, name, pos) =
            names.get(&10477373803).expect("Sol should be named");

        assert_eq!(*name, "Sol");
        assert_eq!(pos.map(|place| place.x), Some(1.0));
    }

    /// Entries stamped the same second keep the order their files are in
    ///
    /// The journal is stamped to the second and a busy one writes several
    /// inside one, so this decides more than a corner case. Sorting is
    /// stable, so what settles it is where the files stand.
    #[test]
    fn entries_stamped_alike_are_left_as_they_stand() {
        let journals = vec![
            (PathBuf::from("Journal.first.log"), vec![at("12:00")]),
            (PathBuf::from("Journal.second.log"), vec![at("12:00")]),
        ];

        let replayed = replay(&journals);
        assert_eq!(
            replayed.iter().map(|(j, _)| *j).collect::<Vec<_>>(),
            [0, 1]
        );
    }
}
