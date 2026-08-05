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
    bodies::Body, markets::Market, stars::Star, stations::Station,
    systems::System, Database,
};
use std::time::Duration;
use structopt::StructOpt;
use tracing::{info, warn};

/// How long EDDN may carry nothing before its connection is replaced
///
/// A busy hour of it runs at 31 messages a second and does not go a second
/// without one, so two minutes of quiet is not EDDN being quiet.
const STALL: Duration = Duration::from_secs(120);

#[derive(StructOpt, Debug)]
pub struct Cli {
    // Type as a URL? ZMQ doesn't bother :(
    #[structopt(short = "r", long = "remote", default_value = URL, help = "ZMQ remote address")]
    pub url: String,

    #[structopt(
        long = "stall",
        help = "Seconds of silence before the connection is replaced, or 0 to leave it alone"
    )]
    pub stall: Option<u64>,
    // TODO: Filters?
}

impl Run for Cli {
    fn run(&self, db: &Database) {
        let stall = match self.stall {
            None => Some(STALL),
            Some(0) => None,
            Some(secs) => Some(Duration::from_secs(secs)),
        };

        for result in subscribe(&self.url, stall) {
            if let Ok(envelop) = result {
                process_message(
                    db,
                    envelop.message,
                    envelop.header.uploader_id,
                );
            } else if let Err(err) = result {
                warn!(error = %err, "unreadable message");
            }
        }
    }
}

fn process_message(db: &Database, message: Message, user: String) {
    task::block_on(async {
        match message {
            Message::Journal(entry) => match entry.event {
                Event::Scan(scan) => {
                    let mut system = JournalSystem::new(
                        scan.system_address,
                        &scan.star_system,
                    );
                    system.pos = Some(scan.star_pos);
                    match System::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &system,
                    )
                    .await
                    {
                        Ok(_) => info!(system = %system.name, "scan"),
                        Err(err) => {
                            warn!(system = %system.name, error = %err, "scan")
                        }
                    }

                    match scan.target {
                        ScanTarget::Star(star) => match Star::from_journal(
                            db,
                            entry.timestamp,
                            &user,
                            &star,
                            scan.system_address,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!(star = %star.name, "scan")
                            }
                            Err(err) => {
                                warn!(star = %star.name, error = %err, "scan")
                            }
                        },
                        ScanTarget::Body(body) => match Body::from_journal(
                            db,
                            entry.timestamp,
                            &user,
                            &body,
                            scan.system_address,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!(body = %body.name, "scan")
                            }
                            Err(err) => {
                                warn!(body = %body.name, error = %err, "scan")
                            }
                        },
                    }
                }
                Event::Location(e) => {
                    match System::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &e.system,
                    )
                    .await
                    {
                        Ok(_) => info!(system = %e.system.name, "location"),
                        Err(err) => {
                            warn!(system = %e.system.name, error = %err, "location")
                        }
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
                            Ok(_) => info!(body = %body.name, "location"),
                            Err(err) => {
                                warn!(body = %body.name, error = %err, "location")
                            }
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
                            Ok(_) => {
                                info!(station = %station.name, "location")
                            }
                            Err(err) => {
                                warn!(station = %station.name, error = %err, "location")
                            }
                        }
                    }
                }
                Event::Docked(e) => {
                    let system =
                        JournalSystem::new(e.system_address, &e.system_name);
                    match System::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &system,
                    )
                    .await
                    {
                        Ok(_) => info!(system = %system.name, "docked"),
                        Err(err) => {
                            warn!(system = %system.name, error = %err, "docked")
                        }
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
                        Ok(_) => {
                            info!(station = %e.station.name, "docked")
                        }
                        Err(err) => {
                            warn!(station = %e.station.name, error = %err, "docked")
                        }
                    }
                }
                Event::FsdJump(e) => {
                    match System::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &e.system,
                    )
                    .await
                    {
                        Ok(_) => info!(system = %e.system.name, "fsd jump"),
                        Err(err) => {
                            warn!(system = %e.system.name, error = %err, "fsd jump")
                        }
                    }
                }
                Event::NavRoute(NavRoute::Route(destinations)) => {
                    for destination in destinations {
                        match System::create(
                            db,
                            destination.system_address as i64,
                            &destination.star_system,
                            Some(destination.star_pos),
                            Some(destination.star_class),
                            None,
                            None,
                            None,
                            None,
                            None,
                            entry.timestamp,
                            &user,
                        )
                        .await
                        {
                            Ok(_) => {
                                info!(system = %destination.star_system, "nav route")
                            }
                            Err(err) => {
                                warn!(system = %destination.star_system, error = %err, "nav route")
                            }
                        }
                    }
                }
                _ => {}
            },
            Message::Commodity(
                ref e @ Entry { event: ref m @ JournalMarket { .. }, .. },
            ) => {
                // A market message cannot name its system by address, only
                // by name, so the system may well be one we have never seen.
                // Record the prices regardless. The station and the link to
                // the system follow whenever the system itself turns up.
                if let Ok(system) =
                    System::fetch_by_name(db, &m.system_name).await
                {
                    match Station::create(
                        db,
                        e.timestamp,
                        &user,
                        system.address,
                        &m.station_name,
                    )
                    .await
                    {
                        Ok(_) => {
                            info!(station = %m.station_name, "commodity")
                        }
                        Err(err) => {
                            warn!(station = %m.station_name, error = %err, "commodity")
                        }
                    }
                }

                match Market::from_journal(db, e.timestamp, &m).await {
                    // A market can arrive before anything that would create
                    // the system it names, and is recorded with no system to
                    // belong to until that turns up. The name it gave is all
                    // there is to go on in the meantime.
                    Ok(market) => info!(
                        market = %m.station_name,
                        system = %m.system_name,
                        orphan = market.system_address.is_none(),
                        "commodity",
                    ),
                    Err(err) => {
                        warn!(market = %m.station_name, error = %err, "commodity")
                    }
                }
            }
            _ => {}
        }
    })
}
