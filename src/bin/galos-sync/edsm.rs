#![cfg(unix)]
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use async_std::task;
use chrono::offset::Utc;
use galos_db::Database;
use galos_db::systems::System;
use crate::Run;

#[derive(StructOpt, Debug)]
pub enum Cli {
    File(FileCli),
    Api(ApiCli),
}

#[derive(StructOpt, Debug)]
pub struct FileCli {
    // TODO: Type as a path.
    // TODO: Default, when not provided?
    #[structopt(name = "PATH")]
    pub path: String,
}

// TODO: -s single system, -S systems query?
#[derive(StructOpt, Debug)]
pub struct ApiCli {}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::File(fc) => {
                let systems = edsm::json(&fc.path);
                let bar = ProgressBar::new(systems.len() as u64);
                bar.set_style(ProgressStyle::default_bar()
                    .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
                    .unwrap()
                    .progress_chars("##-"));
                for system in bar.wrap_iter(systems.into_iter()) {
                    if system.id.is_none() || system.coords.is_none() {
                        continue;
                    }
                    let result = task::block_on(async {
                        System::create(db,
                            system.id.unwrap() as i64,
                            &system.name,
                            system.coords.unwrap(),
                            None, None, None, None, None, None,
                            Utc::now(),
                        ).await
                    });
                    match result {
                        Ok(_) => bar.set_message(format!("[EDSM] {}", system.name)),
                        Err(err) => bar.set_message(format!("[EDSM ERROR] {}", err)),
                    }
                }
            },
            Cli::Api(_ac) => {
                unimplemented!();
            },
        }
    }
}

