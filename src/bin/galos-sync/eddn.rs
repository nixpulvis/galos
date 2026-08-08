#![cfg(unix)]
use crate::Run;
use async_std::task;
use chrono::{DateTime, Utc};
use eddn::{subscribe, Message, URL};
use elite_journal::body::Body as JournalBody;
use elite_journal::entry::incremental::exploration::ScanTarget;
use elite_journal::entry::market::Market as JournalMarket;
use elite_journal::entry::route::NavRoute;
use elite_journal::entry::{Entry, Event};
use elite_journal::station::Station as JournalStation;
use elite_journal::system::System as JournalSystem;
use elite_journal::system::Coordinate;
use galos_db::{
    barycenters::Barycenter, bodies::Body, markets::Market, stars::Star,
    stations::Station, systems::System, Database,
};
use std::time::Duration;
use structopt::StructOpt;
use tracing::{debug, info, warn};

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
                    envelop.schema_ref,
                    envelop.header.uploader_id,
                );
            } else if let Err(err) = result {
                warn!(error = %err, "unreadable message");
            }
        }
    }
}

/// Record arriving somewhere: the system, the body arrived at, the station
///
/// [`Event::Location`] and [`Event::CarrierJump`] describe a system in the
/// same terms and are worth the same to a galaxy being mapped, so they are
/// written the same way. `what` names which of the two it was and is what the
/// log lines are filed under.
async fn record_visit(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    system: &JournalSystem,
    body: Option<&JournalBody>,
    station: Option<&JournalStation>,
    what: &str,
) {
    match System::from_journal(db, timestamp, user, system).await {
        Ok(_) => info!(system = %system.name, "{}", what),
        Err(err) => {
            warn!(system = %system.name, error = %err, "{}", what)
        }
    }

    if let Some(body) = body {
        match Body::from_journal(db, timestamp, user, body, system.address)
            .await
        {
            Ok(_) => info!(body = %body.name, "{}", what),
            Err(err) => warn!(body = %body.name, error = %err, "{}", what),
        }
    }

    if let Some(station) = station {
        match Station::from_journal(db, timestamp, user, station, system.address)
            .await
        {
            Ok(_) => info!(station = %station.name, "{}", what),
            Err(err) => {
                warn!(station = %station.name, error = %err, "{}", what)
            }
        }
    }
}

/// Record how many bodies a system holds
///
/// The honk, the all-found tally and a nav beacon all report it, so they all
/// land here. `what` names which of them it was.
#[allow(clippy::too_many_arguments)]
async fn record_body_counts(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    address: i64,
    name: &str,
    position: Option<Coordinate>,
    body_count: i32,
    non_body_count: Option<i32>,
    what: &str,
) {
    match System::set_body_counts(
        db,
        address,
        name,
        position,
        body_count,
        non_body_count,
        timestamp,
        user,
    )
    .await
    {
        Ok(_) => info!(system = %name, bodies = body_count, "{}", what),
        Err(err) => warn!(system = %name, error = %err, "{}", what),
    }
}

fn process_message(
    db: &Database,
    message: Message,
    schema_ref: String,
    user: String,
) {
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
                // A barycenter is not a body and is not drawn. It is stored so
                // that a body naming it as an ancestor can be placed where it
                // belongs rather than at the middle of its system.
                Event::ScanBaryCentre(scan) => {
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
                        Ok(_) => {
                            info!(system = %system.name, "scan barycenter")
                        }
                        Err(err) => {
                            warn!(system = %system.name, error = %err, "scan barycenter")
                        }
                    }

                    match Barycenter::from_journal(
                        db,
                        entry.timestamp,
                        &user,
                        &scan,
                    )
                    .await
                    {
                        // A barycenter has no name of its own, so the id it is
                        // known by within its system is said along with the
                        // system, neither meaning much without the other.
                        Ok(_) => {
                            info!(system = %system.name, barycenter = scan.body_id, "scan barycenter")
                        }
                        Err(err) => {
                            warn!(system = %system.name, barycenter = scan.body_id, error = %err, "scan barycenter")
                        }
                    }
                }
                Event::Location(e) => {
                    record_visit(
                        db,
                        entry.timestamp,
                        &user,
                        &e.system,
                        e.body.as_ref(),
                        e.station.as_ref(),
                        "location",
                    )
                    .await
                }

                // A carrier jump is a system visit and says everything about
                // the system that arriving under your own power does, so it
                // is recorded the same way.
                Event::CarrierJump(e) => {
                    record_visit(
                        db,
                        entry.timestamp,
                        &user,
                        &e.system,
                        e.body.as_ref(),
                        e.station.as_ref(),
                        "carrier jump",
                    )
                    .await
                }

                // How much there is in a system, which is the other half of
                // knowing what has been found in it. Three events report the
                // same number under three different names.
                Event::FssDiscoveryScan(e) => {
                    record_body_counts(
                        db,
                        entry.timestamp,
                        &user,
                        e.system_address,
                        &e.system_name,
                        Some(e.star_pos),
                        e.body_count,
                        Some(e.non_body_count),
                        "fss discovery scan",
                    )
                    .await
                }

                Event::FssAllBodiesFound(e) => {
                    record_body_counts(
                        db,
                        entry.timestamp,
                        &user,
                        e.system_address,
                        &e.system_name,
                        Some(e.star_pos),
                        e.count,
                        None,
                        "fss all bodies found",
                    )
                    .await
                }

                Event::NavBeaconScan(e) => {
                    record_body_counts(
                        db,
                        entry.timestamp,
                        &user,
                        e.system_address,
                        &e.star_system,
                        Some(e.star_pos),
                        e.num_bodies,
                        None,
                        "nav beacon scan",
                    )
                    .await
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

            // Alpha and beta data, describing a galaxy that is not the one
            // being recorded here.
            Message::Test(_) => {}

            // A schema nothing here reads yet. Said at `debug` because it is
            // most of what EDDN carries, and saying it at all is the only way
            // to know what is going by.
            Message::Unmodeled(_) => {
                debug!(schema = %schema_ref, "unmodeled schema")
            }
        }
    })
}
