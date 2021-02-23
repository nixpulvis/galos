use structopt::StructOpt;
use elite_journal::Event;
use eddn::{URL, subscribe, Message};
use galos_db::Database;
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
            match result {
                Ok(envelop) => process_message(envelop.message),
                // TODO: error! log
                Err(err) => println!("{}", err),
            }
        }
    }
}

fn process_message(message: Message) {
    match message {
        Message::Journal(entry) => {
            match entry.event {
                Event::Location(e) => {
                    dbg!(e);
                },
                Event::FsdJump(e) => {
                    dbg!(e);
                },
                Event::Docked(e) => {
                    dbg!(e);
                },
                _ => {},
            }
        },
        _ => {}
    }
}
