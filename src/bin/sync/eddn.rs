use async_std::task;
use structopt::StructOpt;
use elite_journal::Event;
use eddn::{URL, subscribe, Message};
use galos_db::{Database, systems::System};
use crate::SyncDb;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // Type as a URL?
    #[structopt(short = "r", long = "remote", default_value = URL)]
    pub url: String,

    // TODO: Filters?
}

impl SyncDb for Cli {
    fn sync_db(&self, db: &Database) {
        for result in subscribe(&self.url) {
            if let Ok(envelop) = result {
                process_message(db, envelop.message);
            } else if let Err(err) = result {
                println!("{}", err);
            }
        };
    }
}

fn process_message(db: &Database, message: Message) {
    task::block_on(async {
    match message {
        Message::Journal(entry) => {
            if let Some(system) = match entry.event {
                Event::Location(e) => {
                    Some(e.system)
                },
                Event::FsdJump(e) => {
                    Some(e.system)
                },
                _ => None,
            } {
                let result = System::create(
                    db,
                    system.system_address as i64,
                    &system.star_system.to_uppercase(),
                    system.population as i64,
                    Some(system.system_government),
                    Some(system.system_allegiance),
                    Some(system.system_economy),
                    Some(system.system_second_economy),
                ).await;
                match result {
                    Ok(system) => println!("[EDDN] {}", system.name),
                    Err(err) => println!("[EDDN] {}", err),
                }
            }
        },
        _ => {}
    }
    })
}
