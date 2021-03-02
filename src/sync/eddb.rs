use async_std::task;
use structopt::StructOpt;
use indicatif::{ProgressBar, ProgressStyle};
use galos_db::{Database, systems::{System, PointZ}};
use crate::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // TODO: Type as a path.
    // TODO: Default, when not provided?
    #[structopt(name = "PATH")]
    pub path: String,
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let mut dump = match eddb::Dump::csv(&self.path) {
            Ok(d) => d,
            Err(err) => panic!("{}", err),
        };

        let bar = ProgressBar::new(dump.len());
        bar.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}/{eta_precise}] {bar:40} {pos:>7}/{len:7} ({percent}%) {msg}")
            .progress_chars("##-"));
        for result in bar.wrap_iter(dump.into_iter()) {
            if let Ok(system) = result {
                if let Some(address) = system.ed_system_address {
                    let position = PointZ {
                        x: system.coords.x,
                        y: system.coords.y,
                        z: system.coords.z
                    };
                    task::block_on(async {
                        let result = System::create(db, address, &system.name, position,
                            system.population.unwrap_or(0), system.security, system.government,
                            system.allegiance, system.primary_economy, None, system.updated_at)
                            .await;
                        match result {
                            Ok(_) => bar.set_message(&format!("[EDDB] {}", system.name)),
                            Err(err) => bar.set_message(&format!("[EDDB ERROR] {}", err)),
                        }
                    });
                }
            }
        }
    }
}
