#![cfg(unix)]
use crate::Run;
use async_std::task;
use eddn::{subscribe, Message, URL};
use elite_journal::entry::incremental::exploration::ScanTarget;
use elite_journal::entry::market::Market as JournalMarket;
use elite_journal::entry::route::NavRoute;
use elite_journal::entry::{Entry, Event};
use elite_journal::system::System as JournalSystem;
use galos_db::{
    bodies::Body, markets::Market, stars::Star, stations::Station, systems::System, Database,
};
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
                process_message(db, envelop.message, envelop.header.uploader_id);
            } else if let Err(err) = result {
                println!("{}", err);
            }
        }
    }
}

fn process_message(db: &Database, message: Message, user: String) {
    task::block_on(async {
        match message {
            Message::Journal(entry) => match entry.event {
                Event::Scan(scan) => {
                    let mut system = JournalSystem::new(scan.system_address, &scan.star_system);
                    system.pos = Some(scan.star_pos);
                    match System::from_journal(db, entry.timestamp, &user, &system).await {
                        Ok(_) => println!("[EDDN] <SCN:sys> {}", system.name),
                        Err(err) => eprintln!("[EDDN] <SCN:sys> {}", err),
                    }

                    match scan.target {
                        ScanTarget::Star(star) => {
                            match Star::from_journal(
                                db,
                                entry.timestamp,
                                &user,
                                &star,
                                scan.system_address,
                            )
                            .await
                            {
                                Ok(_) => println!("[EDDN] <SCN:star> {}", star.name),
                                Err(err) => eprintln!("[EDDN] <SCN:star> {}", err),
                            }
                        }
                        ScanTarget::Body(body) => {
                            match Body::from_journal(
                                db,
                                entry.timestamp,
                                &user,
                                &body,
                                scan.system_address,
                            )
                            .await
                            {
                                Ok(_) => println!("[EDDN] <SCN:bod> {}", body.name),
                                Err(err) => eprintln!("[EDDN] <SCN:bod> {}", err),
                            }
                        }
                    }
                }
                Event::Location(e) => {
                    match System::from_journal(db, entry.timestamp, &user, &e.system).await {
                        Ok(_) => println!("[EDDN] <LOC:sys> {}", e.system.name),
                        Err(err) => eprintln!("[EDDN] <LOC:sys> {}", err),
                    }

                    if let Some(ref body) = e.body {
                        match Body::from_journal(
                            db,
                            entry.timestamp,
                            &user,
                            &body,
                            e.system.address,
                        )
                        .await
                        {
                            Ok(_) => println!("[EDDN] <LOC:bod> {}", body.name),
                            Err(err) => eprintln!("[EDDN] <LOC:bod> {}", err),
                        }
                    }

                    if let Some(ref station) = e.station {
                        match Station::from_journal(
                            db,
                            entry.timestamp,
                            &user,
                            &station,
                            e.system.address,
                        )
                        .await
                        {
                            Ok(_) => println!("[EDDN] <LOC:sta> {}", station.name),
                            Err(err) => eprintln!("[EDDN] <LOC:sta> {}", err),
                        }
                    }
                }
                Event::Docked(e) => {
                    let system = JournalSystem::new(e.system_address, &e.system_name);
                    match System::from_journal(db, entry.timestamp, &user, &system).await {
                        Ok(_) => println!("[EDDN] <DOC:sys> {}", system.name),
                        Err(err) => eprintln!("[EDDN] <DOC:sys> {}", err),
                    }

                    match Station::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &e.station,
                        e.system_address,
                    )
                    .await
                    {
                        Ok(_) => println!("[EDDN] <DOC:sta> {}", e.station.name),
                        Err(err) => eprintln!("[EDDN] <DOC:sta> {}", err),
                    }
                }
                Event::FsdJump(e) => {
                    match System::from_journal(db, entry.timestamp, &user, &e.system).await {
                        Ok(_) => println!("[EDDN] <FSD:sys> {}", e.system.name),
                        Err(err) => eprintln!("[EDDN] <FSD:sys> {}", err),
                    }
                }
                Event::NavRoute(NavRoute::Route(destinations)) => {
                    for destination in destinations {
                        let mut system = JournalSystem::new(
                            destination.system_address as i64,
                            &destination.star_system,
                        );
                        system.pos = Some(destination.star_pos);
                        match System::from_journal(db, entry.timestamp, &user, &system).await {
                            Ok(_) => println!("[EDDN] <ROU:sys> {}", system.name),
                            Err(err) => eprintln!("[EDDN] <ROU:sys> {}", err),
                        }
                    }
                }
                _ => {}
            },
            Message::Commodity(
                ref e @ Entry {
                    event: ref m @ JournalMarket { .. },
                    ..
                },
            ) => match Market::from_journal(db, e.timestamp, &m).await {
                Ok(_) => println!("[EDDN] <MKT:mkt> {}", m.station_name),
                Err(err) => eprintln!("[EDDN] <MKT:mkt> {}", err),
            },
            _ => {}
        }
    })
}
