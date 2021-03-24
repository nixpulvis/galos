use async_std::task;
use structopt::StructOpt;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use chrono::offset::Utc;
use elite_journal::system::Coordinate;
use galos_db::{Database, systems::System};
use crate::Run;

#[derive(StructOpt, Debug)]
pub enum Cli {
    File(FileCli),
    Api(ApiCli),
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        match self {
            Cli::File(cli) => cli.run(db),
            Cli::Api(cli) => cli.run(db),
        }
    }
}

#[derive(StructOpt, Debug)]
pub struct FileCli {
    // TODO: Type as a path.
    // TODO: Default, when not provided?
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for FileCli {
    fn run(&self, db: &Database) {
        let systems = edsm::json(&self.path);
        let bar = ProgressBar::new(systems.len() as u64);
        bar.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
            .progress_chars("##-"));
        for system in bar.wrap_iter(systems.into_iter()) {
            let coords = if let Some(c) = &system.coords {
                c
            } else {
                println!("[EDSM ERROR] {} no coords", &system.name);
                continue;
            };

            let address = if let Some(i) = system.id64 {
               i
            } else {
                println!("[EDSM ERROR] {} address", &system.name);
                continue;
            };

            let position = Coordinate {
                x: coords.x,
                y: coords.y,
                z: coords.z
            };

            task::block_on(async {
                let result = System::create(db, address, &system.name, position,
                    system.information.population,
                    None, None, None, None, None,
                    // TODO: Need up use elite_journal types in edsm.
                    // system.information.security,
                    // system.information.government,
                    // system.information.allegiance,
                    // system.information.economy,
                    // system.information.second_economy,
                    Utc::now())
                    .await;
                match result {
                    Ok(_) => bar.set_message(&format!("[EDDB] {}", system.name)),
                    Err(err) => bar.set_message(&format!("[EDDB ERROR] {}", err)),
                }
            });
        }
    }
}

// TODO: -s single system, -S systems query?
#[derive(StructOpt, Debug)]
pub struct ApiCli {}

impl Run for ApiCli {
    fn run(&self, _db: &Database) {
        unimplemented!();
    }
}
