use crate::Run;
use async_std::task;
use elite_journal::entry::{parse_journal_file, Entry, Event};
use galos_db::Database;
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use std::ffi::OsStr;
use std::fs;
use std::io::{stderr, IsTerminal};
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use tracing::{info, warn};

pub mod record;

/// Where a journal directory remembers whose it is
///
/// The `.log` files name the commander who flew them, and that is the answer
/// wherever it is given. It is not always given: a session continued into a
/// second file picks up without introducing itself again.
///
/// So the directory is asked to remember. What the logs said is written here
/// and read back when nothing in front of us says otherwise. Per directory
/// rather than per run: two directories are as likely to be two commanders as
/// one, and a name carried over from the last import would be a guess about
/// someone else's journal.
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
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let path = Path::new(&self.path);
        let Ok(meta) = fs::metadata(path) else {
            panic!("bad path: {}", self.path);
        };

        // A directory is a journal directory. A single file is one of the
        // files in one, so the directory holding it is asked the same
        // questions -- who flew this -- and told the answer afterwards.
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
        // file's own first entry is what says where the file belongs.
        let mut journals: Vec<(PathBuf, Vec<Entry<Event>>)> = paths
            .into_iter()
            .filter_map(|path| match parse_journal_file(&path) {
                Ok(mut entries) => {
                    entries.sort_by_key(|entry| entry.timestamp);
                    Some((path, entries))
                }
                Err(err) => {
                    warn!(file = %path.display(), error = %err, "unreadable journal");
                    None
                }
            })
            .collect();
        journals.sort_by_key(|(_, entries)| {
            entries.first().map(|entry| entry.timestamp)
        });

        // What the command line said, which outranks every file, and what is
        // left to fall back on where a file names nobody.
        let forced = self.user.as_deref();
        let mut known = forced.map(str::to_owned).or_else(|| remembered(&dir));

        let bar = progress(journals.iter().map(|(_, e)| e.len() as u64).sum());
        for (path, entries) in &journals {
            if forced.is_none() {
                if let Some(name) = commander(entries) {
                    known = Some(name);
                }
            }
            let user = forced.or(known.as_deref()).unwrap_or(UNKNOWN);

            bar.set_message(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
            // The bar and the log are drawn on the same terminal, and a line
            // printed under a bar lands on top of it. Standing the bar down
            // for the length of a file leaves the log the terminal while that
            // file is written, and costs one redraw a file. Per entry it
            // would cost two writes for every line of every journal ever
            // flown, which is the whole import.
            bar.suspend(|| {
                task::block_on(async {
                    for entry in entries {
                        record::entry(db, entry, user).await;
                    }
                })
            });
            bar.inc(entries.len() as u64);
        }
        bar.finish();

        if let Some(name) = forced.or(known.as_deref()) {
            remember(&dir, name);
        }
    }
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

    fn entries(lines: &[&str]) -> Vec<Entry<Event>> {
        lines
            .iter()
            .map(|line| serde_json::from_str(line).expect("entry should parse"))
            .collect()
    }

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
}
