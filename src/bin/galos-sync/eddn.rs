#![cfg(unix)]
use async_std::task;
use structopt::StructOpt;
use elite_journal::entry::Event;
use eddn::{URL, subscribe, Message};
use galos_db::{Database, systems::System};
use crate::Run;

#[derive(StructOpt, Debug)]
pub struct Cli {
    // Type as a URL? ZMQ doesn't bother :(
    #[structopt(short = "r", long = "remote", default_value = URL, help = "ZMQ remote address")]
    pub url: String,

    // TODO: Filters?
}

impl Run for Cli {
    fn run(&self, db: &Database) {
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
                let result = System::from_journal(db, &system, entry.timestamp).await;
                match result {
                    Ok(_) => println!("[EDDN] {}", system.name),
                    Err(err) => println!("[EDDN ERROR] {}", err),
                }
            }
        },
        _ => {}
    }
    })
}
