use crate::Run;
use async_std::task;
use elite_journal::entry::{self, Event};
use galos_db::{systems::System, Database};
use indicatif::{ProgressBar, ProgressStyle};
use notify::{RecursiveMode, Watcher};
use std::{fs, path::Path};
use structopt::StructOpt;

#[derive(StructOpt, Debug)]
pub struct Cli {
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        // TODO: Setup CLI flag
        if true {
            self.run_watch(db)
        } else {
            self.run_dump(db)
        }
    }
}

impl Cli {
    fn run_watch(&self, db: &Database) {
        // setup debouncer
        let (tx, rx) = std::sync::mpsc::channel();

        // Automatically select the best implementation for your platform.
        let mut watcher = notify::recommended_watcher(move |res| {
            tx.send(res);
        })
        .expect("ERROR: unable to get filesystem watcher");

        // Add a path to be watched. All files and directories at that path and
        // below will be monitored for changes.
        watcher
            .watch(Path::new("."), RecursiveMode::Recursive)
            .expect("ERROR: unable to setup watcher");

        for result in rx {
            match result {
                Ok(event) => {
                    println!("Event {event:?}")
                }
                Err(error) => println!("Error {error:?}"),
            }
        }
    }

    fn run_dump(&self, db: &Database) {
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
                    let result = System::from_journal(
                        db,
                        entry.timestamp,
                        "JOURNAL",
                        &system,
                    )
                    .await;
                    match result {
                        Ok(_) => bar.set_message(format!(
                            "[{}] {}",
                            entry.timestamp, system.name
                        )),
                        Err(err) => bar.set_message(format!(
                            "[ERROR {}] {}",
                            entry.timestamp, err
                        )),
                    }
                }
            });
        }
        bar.finish();
    }
}
