#![cfg(unix)]
use std::collections::HashMap;
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
    #[structopt(name = "PATH")]
    pub path: String,
}

#[derive(StructOpt, Debug)]
pub struct ApiCli {
    #[structopt(name = "NAME")]
    pub name: String,

    #[structopt(name = "cube", long, short)]
    pub cube: Option<u32>,
    #[structopt(name = "sphere", long, short)]
    pub sphere: Option<u32>,
}

impl Cli {
    fn create_vec(db:&Database, systems: Vec<edsm::system::System>) {
        let mut imported = 0;
        let mut errors = HashMap::new();
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
                let r = System::create(db,
                    system.id.unwrap() as i64,
                    &system.name,
                    Some(system.coords.unwrap()),
                    system.information.population,
                    system.information.security,
                    system.information.government,
                    system.information.allegiance,
                    system.information.economy,
                    system.information.second_economy,
                    Utc::now(),
                ).await;
                r
            });
            match result {
                Ok(_) => {
                    bar.set_message(format!("[EDSM] {}", system.name));
                    imported += 1;
                },
                Err(err) => {
                    bar.set_message(format!("[EDSM ERROR] {}", err));
                    errors.entry(err.to_string())
                        .and_modify(|ns: &mut Vec<String>| ns.push(system.name.clone()))
                        .or_insert(vec![system.name]);
                },
            }
        }
        println!("Imported {} systems.", imported);
        for (err, system_names) in errors {
            println!("Failed to import {} systems:\n{}: {}", system_names.len(), err, system_names.join(", "));
        }
    }
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::File(fc) => {
                let systems = edsm::json(&fc.path);
                Cli::create_vec(db, systems);
            },
            Cli::Api(ac) => {
                let systems = if let Some(n) = ac.sphere {
                    edsm::api::systems_sphere(&ac.name, Some(n as f64), None).unwrap()
                } else if let Some(n) = ac.cube {
                    edsm::api::systems_cube(&ac.name, Some(n as f64)).unwrap()
                } else {
                    edsm::api::systems(&ac.name).unwrap()
                };
                Cli::create_vec(db, systems);
            },
        }
    }
}

