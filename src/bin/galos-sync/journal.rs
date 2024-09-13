use crate::Run;
use async_std::task;
use elite_journal::entry::{self, Event};
use galos_db::{systems::System, Database};
use indicatif::{ProgressBar, ProgressStyle};
use std::fs;
use structopt::StructOpt;

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
            task::block_on(async {
                let option = match entry.event {
                    Event::Location(e) => Some(e.system),
                    Event::FsdJump(e) => Some(e.system),
                    _ => None,
                };

                if let Some(system) = option {
                    // TODO: Take user as arg or something.
                    let result = System::from_journal(db, entry.timestamp, "JOURNAL", &system).await;
                    match result {
                        Ok(_) => bar.set_message(format!("[{}] {}", entry.timestamp, system.name)),
                        Err(err) => bar.set_message(format!("[ERROR {}] {}", entry.timestamp, err)),
                    }
                }
            });
        }
        bar.finish();
    }
}
