#![cfg(unix)]
use async_std::task;
use structopt::StructOpt;
use elite_journal::entry::Event;
use elite_journal::system::System as JournalSystem;
use galos_db::bodies::Body;
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
            match entry.event {
                Event::Scan(e) => {
                    let system = JournalSystem::new(e.system_address, e.star_pos, &e.star_system);
                    match System::from_journal(db, entry.timestamp, &system).await {
                        Ok(_) => eprintln!("<SCN:sys> {}", system.name),
                        Err(err) => eprintln!("<SCN:sys> {}", err),
                    }

                    match Body::from_journal(db, entry.timestamp, &e.body, e.system_address).await {
                        Ok(_) => eprintln!("<SCN:bod> {}", e.body.name),
                        Err(err) => eprintln!("<SCN:bod> {}", err),
                    }
                },
                Event::Location(e) => {
                    match System::from_journal(db, entry.timestamp, &e.system).await {
                        Ok(_) => eprintln!("<LOC:sys> {}", e.system.name),
                        Err(err) => eprintln!("<LOC:sys> {}", err),
                    }

                    if let Some(body) = e.body {
                        match Body::from_journal(db, entry.timestamp, &body, e.system.address).await {
                            Ok(_) => eprintln!("<LOC:bod> {}", body.name),
                            Err(err) => eprintln!("<LOC:bod> {}", err),
                        }
                    }
                },
                Event::FsdJump(e) => {
                    match System::from_journal(db, entry.timestamp, &e.system).await {
                        Ok(_) => eprintln!("<FSD:sys> {}", e.system.name),
                        Err(err) => eprintln!("<FSD:sys> {}", err),
                    }
                },
                _ => {}
            }
        },
        _ => {}
    }
    })
}
