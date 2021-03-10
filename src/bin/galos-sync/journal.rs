use async_std::task;
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use elite_journal::entry::{parse_journal_dir, Entry, Event};
use galos_db::{Database, systems::System};
use crate::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        // TODO: Support files and dirs.
        let entries = parse_journal_dir(&self.path).unwrap();
        let bar = ProgressBar::new(entries.len() as u64);
        bar.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
            .progress_chars("##-"));
        for entry in bar.wrap_iter(entries.into_iter()) {
            task::block_on(async {
                if let Some(system) = match entry.event {
                    Event::Location(e) => {
                        Some(e.system)
                    },
                    Event::FsdJump(e) => {
                        Some(e.system)
                    },
                    _ => None,
                } {
                    let result = System::from_journal(db, &system, entry.timestamp).await;
                    match result {
                        Ok(_) => bar.set_message(&format!("[{}] {}", entry.timestamp, system.name)),
                        Err(err) => bar.set_message(&format!("[ERROR {}] {}", entry.timestamp, err)),
                    }
                }
            });
        }
    }
}
