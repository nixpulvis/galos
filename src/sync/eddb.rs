use async_std::task;
use structopt::StructOpt;
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
        eddb::System::each_csv(&self.path, &mut |system| {
            task::block_on(async {
                if let Some(address) = system.ed_system_address {
                    let position = PointZ {
                        x: system.coords.x,
                        y: system.coords.y,
                        z: system.coords.z
                    };
                    let result = System::create(db, address, &system.name, position,
                        system.population.unwrap_or(0), system.security, system.government,
                        system.allegiance, system.primary_economy, None, system.updated_at)
                        .await;
                    match result {
                        Ok(_) => println!("[EDDB] {}", system.name),
                        Err(err) => println!("[EDDB ERROR] {}", err),
                    }
                }
            });
            true
        });
    }
}
