use crate::Run;
use async_std::task;
use elite_journal::entry::{parse_journal_file, Entry, Event};
use galos_db::Database;
use indicatif::{ProgressBar, ProgressStyle};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use structopt::StructOpt;
use tracing::warn;

pub mod record;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let path = Path::new(&self.path);
        let Ok(meta) = fs::metadata(path) else {
            panic!("bad path: {}", self.path);
        };

        let paths =
            if meta.is_dir() { logs(path) } else { vec![path.to_owned()] };

        // Read whole, as a directory always has been read here. What that
        // buys is order: a write is refused now where something newer already
        // stands in its place, and an older one fills in only what has never
        // been held, so the same directory imported in two orders lands two
        // different ways. A file's own first entry is what says where the
        // file belongs, the names having carried two different timestamps
        // over the years and sorting against each other in neither.
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

        let bar = ProgressBar::new(
            journals.iter().map(|(_, e)| e.len() as u64).sum(),
        );
        bar.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
            .unwrap()
            .progress_chars("##-"));
        for (path, entries) in &journals {
            bar.set_message(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
            );
            task::block_on(async {
                for entry in entries {
                    // TODO: Take user as arg or something.
                    record::entry(db, entry, "JOURNAL").await;
                }
            });
            bar.inc(entries.len() as u64);
        }
        bar.finish();
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
