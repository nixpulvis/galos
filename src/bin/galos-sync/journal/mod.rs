use crate::Run;
use async_std::task;
use elite_journal::entry;
use galos_db::Database;
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use structopt::StructOpt;

pub mod record;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let entries = if let Ok(m) = fs::metadata(&self.path) {
            if m.is_dir() {
                entry::parse_journal_dir(&self.path).unwrap()
            } else {
                entry::parse_journal_file(&self.path).unwrap()
            }
        } else {
            panic!("bad path: {}", self.path);
        };

        let bar = ProgressBar::new(entries.len() as u64);
        bar.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
            .unwrap()
            .progress_chars("##-"));
        for entry in bar.wrap_iter(entries.into_iter()) {
            bar.set_message(entry.timestamp.to_string());
            // TODO: Take user as arg or something.
            task::block_on(record::entry(db, &entry, "JOURNAL"));
        }
        bar.finish();
    }
}
