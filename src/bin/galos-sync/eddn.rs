#![cfg(unix)]
use crate::Run;
use async_std::task;
use eddn::{subscribe, Message, URL};
use elite_journal::entry::Event;
use elite_journal::system::System as JournalSystem;
use galos_db::bodies::Body;
use galos_db::{systems::System, Database};
use structopt::StructOpt;

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
        }
    }
}

fn process_message(db: &Database, message: Message) {
    task::block_on(async {
        match message {
            Message::Journal(entry) => match entry.event {
                Event::Scan(e) => {
                    let system = JournalSystem::new(e.system_address, e.star_pos, &e.star_system);
                    match System::from_journal(db, entry.timestamp, &system).await {
                        Ok(_) => println!("[EDDN] <SCN:sys> {}", system.name),
                        Err(err) => eprintln!("[EDDN] <SCN:sys> {}", err),
                    }

                    match Body::from_journal(db, entry.timestamp, &e.body, e.system_address).await {
                        Ok(_) => println!("[EDDN] <SCN:bod> {}", e.body.name),
                        Err(err) => eprintln!("[EDDN] <SCN:bod> {}", err),
                    }
                }
                Event::Location(e) => {
                    match System::from_journal(db, entry.timestamp, &e.system).await {
                        Ok(_) => println!("[EDDN] <LOC:sys> {}", e.system.name),
                        Err(err) => eprintln!("[EDDN] <LOC:sys> {}", err),
                    }

                    if let Some(body) = e.body {
                        match Body::from_journal(db, entry.timestamp, &body, e.system.address).await
                        {
                            Ok(_) => println!("[EDDN] <LOC:bod> {}", body.name),
                            Err(err) => eprintln!("[EDDN] <LOC:bod> {}", err),
                        }
                    }
                }
                Event::FsdJump(e) => {
                    match System::from_journal(db, entry.timestamp, &e.system).await {
                        Ok(_) => println!("[EDDN] <FSD:sys> {}", e.system.name),
                        Err(err) => eprintln!("[EDDN] <FSD:sys> {}", err),
                    }
                }
                _ => {}
            },
            _ => {}
        }
    })
}
