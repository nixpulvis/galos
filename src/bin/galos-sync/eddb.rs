//! A dump from EDDB, which is gone
//!
//! The site closed and stopped publishing, so nothing new arrives here and
//! nothing this reads is newer than the day it shut. What it takes is a dump
//! already on disk, and that is all it will ever take. The systems in one are
//! stamped with their own `updated_at`, so the guards in `System::create`
//! keep a newer reading from EDDN or a journal from being written over.

use crate::{bar, Run};
use async_std::task;
use elite_journal::system::Coordinate;
use galos_db::{
    systems::{Economies, System},
    Database,
};
use structopt::StructOpt;

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

        let bar = bar::progress(dump.len());
        let _drawing = bar::under(&bar);
        for result in bar.wrap_iter(dump.into_iter()) {
            if let Ok(system) = result {
                if let Some(address) = system.ed_system_address {
                    let position = Coordinate {
                        x: system.coords.x,
                        y: system.coords.y,
                        z: system.coords.z,
                    };
                    task::block_on(async {
                        let result = System::create(
                            db,
                            address as i64,
                            &system.name,
                            Some(position),
                            None,
                            system.population,
                            system.security,
                            system.government,
                            system.allegiance,
                            Economies::new(system.primary_economy, None),
                            system.updated_at,
                            "EDDB dump",
                        )
                        .await;
                        match result {
                            Ok(_) => bar
                                .set_message(format!("[EDDB] {}", system.name)),
                            Err(err) => {
                                bar.set_message(format!("[EDDB ERROR] {}", err))
                            }
                        }
                    });
                }
            }
        }
        bar.finish();
    }
}
