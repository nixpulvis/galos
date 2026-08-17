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

use crate::Run;
use async_std::task;
use elite_journal::entry::{Entry, Event, NavRoute};
use galos_db::Database;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{stderr, BufRead, BufReader, IsTerminal};
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use tracing::{info, warn};

pub mod record;

/// Where a journal directory remembers whose it is
///
/// The `.log` files name the commander who flew them, and that is the answer
/// wherever it is given. Two things beside them do not. `NavRoute.json` says
/// where a ship is going and nothing about who is flying it, and a session
/// continued into a second file picks up without introducing itself again.
///
/// So the directory is asked to remember. What the logs said is written here
/// and read back when nothing in front of us says otherwise. Per directory
/// rather than per run: two directories are as likely to be two commanders as
/// one, and a name carried over from the last import would be a guess about
/// someone else's journal.
///
/// Written only where a whole directory was imported. A single file is read
/// out of a directory rather than being what is in it, and one archived log
/// does not get to say whose the rest are.
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
    fn run(&self, db: &Database) {
        let path = Path::new(&self.path);
        let Ok(meta) = fs::metadata(path) else {
            panic!("bad path: {}", self.path);
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

        let paths =
            if meta.is_dir() { logs(path) } else { vec![path.to_owned()] };

        // Read whole, as a directory always has been read here. What that
        // buys is order: a write is refused now where something newer already
        // stands in its place, so entries have to reach the database in the
        // order they happened or an import lands differently every time. A
        // file's own first entry is what says where the file belongs, which
        // is the order a commander's name carries forward in.
        let mut journals: Vec<(PathBuf, Vec<Entry<Event>>)> = paths
            .into_iter()
            .filter_map(|path| {
                let mut entries = read(&path)?;
                entries.sort_by_key(|entry| entry.timestamp);
                Some((path, entries))
            })
            .collect();
        journals.sort_by_key(|(_, entries)| {
            entries.first().map(|entry| entry.timestamp)
        });

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

        let bar = progress(journals.iter().map(|(_, e)| e.len() as u64).sum());
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
            // The bar and the log are drawn on the same terminal, and a line
            // printed under a bar lands on top of it. Standing the bar down
            // for a run of entries out of one file leaves the log the
            // terminal while they are written, and costs one redraw a run.
            // Per entry it would cost two writes for every line of every
            // journal ever flown, which is the whole import.
            bar.suspend(|| {
                task::block_on(async {
                    for (_, entry) in run {
                        record::entry(db, entry, user).await;
                    }
                })
            });
            bar.inc(run.len() as u64);
        }
        bar.finish();

        if meta.is_dir() {
            sidecars(db, &dir, known.as_deref().unwrap_or(UNKNOWN));

            // Only what the logs said. A name given on the command line is
            // for the run it was given on, and writing it here would file
            // every later import of this directory under it, the ones that
            // asked for nothing included.
            if self.user.is_none() {
                if let Some(name) = &known {
                    remember(&dir, name);
                }
            }
        }
    }
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

/// The journal files in a directory, in no particular order
///
/// Anything ending `.log`, which is every journal the game has written under
/// either of the two names it has given them.
fn logs(dir: &Path) -> Vec<PathBuf> {
    let read = match fs::read_dir(dir) {
        Ok(read) => read,
        Err(err) => {
            warn!(dir = %dir.display(), error = %err, "unreadable directory");
            return Vec::new();
        }
    };

    read.filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path.extension().and_then(OsStr::to_str) == Some("log")
        })
        .collect()
}

/// Read one journal file, saying what in it could not be read
///
/// `parse_journal_file` drops a line it cannot parse and says nothing, and
/// what it drops is not the odd corrupt line. Every event here is one this
/// claims to write, since an event nothing models is read as
/// [`Event::Other`] rather than failing -- so a line counted here is a scan,
/// or a honk, or a codex entry, going in the bin while the bar fills to the
/// end and the import reports itself finished.
///
/// Which is what is happening. Said once a file with a count and a reason
/// rather than once a line, since a journal that hits this hits it thousands
/// of times over and the reason is the same every time.
fn read(path: &Path) -> Option<Vec<Entry<Event>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) => {
            warn!(file = %path.display(), error = %err, "unreadable journal");
            return None;
        }
    };

    let mut entries = Vec::new();
    let mut unread = 0;
    let mut why = None;

    // A line the filesystem will not hand over is counted with the ones that
    // would not parse. `lines` is not fused, so the file goes on after one,
    // and one torn byte in a journal of thousands is worth that one line and
    // not the rest of them.
    for line in BufReader::new(file).lines() {
        let line = match line {
            Ok(line) => line,
            Err(err) => {
                unread += 1;
                why.get_or_insert_with(|| err.to_string());
                continue;
            }
        };

        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str(&line) {
            Ok(entry) => entries.push(entry),
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
            read = entries.len(),
            first = %why.unwrap_or_default(),
            "entries this cannot read",
        );
    }

    Some(entries)
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
///
/// Redirected, the bar is turned off rather than written out: what it draws is
/// a line rewritten in place, and a file of those is not a log of anything.
fn progress(entries: u64) -> ProgressBar {
    let bar = ProgressBar::new(entries);
    bar.set_style(ProgressStyle::default_bar()
        .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
        .unwrap()
        .progress_chars("##-"));

    if !stderr().is_terminal() {
        bar.set_draw_target(ProgressDrawTarget::hidden());
    }

    bar
}

/// Whose journal a set of entries is, read from what the game writes
///
/// Which entry answers this decides who every row of an import is filed
/// under, and the answer is spelled three different ways across the events
/// that give it. A rename that stopped one of them being read would not fail
/// anywhere: the event would simply fall through to `Other` and the file
/// would look like one flown by nobody.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entries(lines: &[&str]) -> Vec<Entry<Event>> {
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
        let entries = entries(&[
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
        let entries = entries(&[
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
        let entries = entries(&[
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
