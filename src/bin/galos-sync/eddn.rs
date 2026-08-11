#![cfg(unix)]
use crate::Run;
use async_std::task;
use chrono::{DateTime, Utc};
use eddn::{subscribe, Message, URL};
use elite_journal::body::{Body as JournalBody, Signal};
use elite_journal::entry::incremental::exploration::ScanTarget;
use elite_journal::entry::market::Market as JournalMarket;
use elite_journal::entry::route::NavRoute;
use elite_journal::entry::{Entry, Event};
use elite_journal::station::Station as JournalStation;
use elite_journal::system::Coordinate;
use elite_journal::system::System as JournalSystem;
use galos_db::{
    barycenters::Barycenter, black_market::BlackMarket, bodies::Body,
    body_signals::BodySignal, clusters::Cluster, codex_entries::CodexEntry,
    markets::Market, outfitting::Outfitting, rings::Ring, shipyard::Shipyard,
    stars::Star, stations::Station, system_signals::SystemSignal,
    systems::System, Database,
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
                    &envelop.schema_ref,
                    &envelop.header.uploader_id,
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
        // Its address and where it says it is, both of which a write can be
        // refused over and neither of which the name gives.
        Err(err) => warn!(
            system = %system.name,
            address = system.address,
            position = ?system.pos,
            error = %err,
            "{}", what
        ),
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
        match Station::from_journal(
            db,
            timestamp,
            user,
            station,
            system.address,
        )
        .await
        {
            Ok(_) => info!(station = %station.name, "{}", what),
            Err(err) => {
                warn!(station = %station.name, error = %err, "{}", what)
            }
        }
    }
}

/// Make sure a system is on record before something points at it
///
/// Signals and codex entries hang off a system by foreign key and routinely
/// arrive for one nothing has heard of, since the honk that finds them is
/// often the first thing ever sent about the place. Each carries a name and a
/// position, which is all it takes to write the row they need.
async fn ensure_system(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    address: i64,
    name: &str,
    position: Option<Coordinate>,
) -> bool {
    let mut system = JournalSystem::new(address, name);
    system.pos = position;

    match System::from_journal(db, timestamp, user, &system).await {
        Ok(_) => true,
        // Its address and where it says it is, as `record_visit` says them. A
        // write is refused over both, and the name gives neither.
        Err(err) => {
            warn!(
                system = %name,
                address = address,
                position = ?position,
                error = %err,
                "system"
            );
            false
        }
    }
}

/// Note the station a market message names, where its system is known
///
/// Trade data names its system by name and never by address, so the system may
/// well be one nothing has heard of. The market itself records the name and
/// waits for the system either way; the station can only be written once there
/// is a system for it to belong to, which is what the market's key onto it
/// needs.
///
/// Best effort by design. A market whose station could not be written is still
/// worth recording, and `System::create` links both up when the system turns
/// up later.
async fn ensure_station_for_market(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    system_name: &str,
    station_name: &str,
) {
    let Ok(system) = System::fetch_by_name(db, system_name).await else {
        return;
    };

    if let Err(err) =
        Station::create(db, timestamp, user, system.address, station_name).await
    {
        warn!(station = %station_name, error = %err, "station");
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

/// Record what was found on a body's surface
///
/// The surface scan and the honk report the same kinds and counts, so both
/// land here. `what` names which of them it was.
#[allow(clippy::too_many_arguments)]
async fn record_body_signals(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    address: i64,
    name: &str,
    position: Option<Coordinate>,
    body_id: i16,
    signals: &[Signal],
    what: &str,
) {
    if !ensure_system(db, timestamp, user, address, name, position).await {
        return;
    }

    match BodySignal::from_journal(
        db, timestamp, user, address, body_id, signals,
    )
    .await
    {
        Ok(_) => {
            info!(system = %name, body = body_id, signals = signals.len(), "{}", what)
        }
        Err(err) => warn!(system = %name, error = %err, "{}", what),
    }
}

fn process_message(
    db: &Database,
    message: Message,
    schema_ref: &str,
    user: &str,
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
                        user,
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
                            user,
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
                            user,
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
                        ScanTarget::Cluster(cluster) => {
                            match Cluster::from_journal(
                                db,
                                entry.timestamp,
                                user,
                                &cluster,
                                scan.system_address,
                            )
                            .await
                            {
                                Ok(_) => {
                                    info!(cluster = %cluster.name, "scan")
                                }
                                Err(err) => {
                                    warn!(cluster = %cluster.name, error = %err, "scan")
                                }
                            }
                        }
                        ScanTarget::Ring(ring) => {
                            match Ring::from_journal(
                                db,
                                entry.timestamp,
                                user,
                                &ring,
                                scan.system_address,
                            )
                            .await
                            {
                                Ok(_) => info!(ring = %ring.name, "scan"),
                                Err(err) => {
                                    warn!(ring = %ring.name, error = %err, "scan")
                                }
                            }
                        }
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
                        user,
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
                        user,
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
                        user,
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
                        user,
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
                        user,
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
                        user,
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
                        user,
                        e.system_address,
                        &e.star_system,
                        Some(e.star_pos),
                        e.num_bodies,
                        None,
                        "nav beacon scan",
                    )
                    .await
                }

                // A settlement is a station on a planet's surface, and this is
                // the only thing that says where on the planet it is.
                Event::ApproachSettlement(e) => {
                    if !ensure_system(
                        db,
                        entry.timestamp,
                        user,
                        e.system_address,
                        &e.system_name,
                        Some(e.star_pos),
                    )
                    .await
                    {
                        return;
                    }

                    match Station::from_settlement(
                        db,
                        entry.timestamp,
                        user,
                        &e,
                    )
                    .await
                    {
                        Ok(_) => info!(
                            settlement = %e.name,
                            body = %e.body_name,
                            "approach settlement",
                        ),
                        Err(err) => warn!(
                            settlement = %e.name,
                            error = %err,
                            "approach settlement",
                        ),
                    }
                }

                // What is written on a body's surface. The surface scan and
                // the honk report it in the same terms, so both land in the
                // same place.
                Event::SAASignalsFound(e) => {
                    record_body_signals(
                        db,
                        entry.timestamp,
                        user,
                        e.system_address,
                        &e.star_system,
                        Some(e.star_pos),
                        e.body_id,
                        &e.signals,
                        "saa signals found",
                    )
                    .await
                }

                Event::FssBodySignals(e) => {
                    record_body_signals(
                        db,
                        entry.timestamp,
                        user,
                        e.system_address,
                        &e.star_system,
                        Some(e.star_pos),
                        e.body_id,
                        &e.signals,
                        "fss body signals",
                    )
                    .await
                }

                // What hangs in a system without being a body. Arrives as a
                // system's worth at a time.
                Event::FssSignalDiscovered(e) => {
                    // The schema only says the sender "should" add these, so
                    // a batch may name no system at all. Nothing can be
                    // created from one that does not, and the signals are
                    // still worth writing if the system is already known.
                    if let Some(ref name) = e.star_system {
                        if !ensure_system(
                            db,
                            entry.timestamp,
                            user,
                            e.system_address,
                            name,
                            e.star_pos,
                        )
                        .await
                        {
                            return;
                        }
                    }

                    match SystemSignal::from_journal(
                        db,
                        user,
                        e.system_address,
                        &e.signals,
                    )
                    .await
                    {
                        Ok(_) => info!(
                            system = %e.star_system.as_deref().unwrap_or("?"),
                            signals = e.signals.len(),
                            "fss signal discovered",
                        ),
                        Err(err) => warn!(
                            system = %e.star_system.as_deref().unwrap_or("?"),
                            error = %err,
                            "fss signal discovered",
                        ),
                    }
                }

                Event::CodexEntry(e) => {
                    if !ensure_system(
                        db,
                        entry.timestamp,
                        user,
                        e.system_address,
                        &e.system_name,
                        Some(e.star_pos),
                    )
                    .await
                    {
                        return;
                    }

                    match CodexEntry::from_journal(
                        db,
                        entry.timestamp,
                        user,
                        &e,
                    )
                    .await
                    {
                        Ok(_) => info!(
                            system = %e.system_name,
                            entry = e.entry_id,
                            "codex entry",
                        ),
                        Err(err) => warn!(
                            system = %e.system_name,
                            error = %err,
                            "codex entry",
                        ),
                    }
                }
                Event::Docked(e) => {
                    let system =
                        JournalSystem::new(e.system_address, &e.system_name);
                    match System::from_journal(
                        db,
                        entry.timestamp,
                        user,
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
                        user,
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
                        user,
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
                            user,
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
                        user,
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

            // The three schemas whose payload carries no `event` key, and so
            // could not be reached at all until messages were placed by their
            // `$schemaRef`.
            Message::Outfitting(ref e @ Entry { event: ref o, .. }) => {
                ensure_station_for_market(
                    db,
                    e.timestamp,
                    user,
                    &o.system_name,
                    &o.station_name,
                )
                .await;

                match Outfitting::from_journal(db, e.timestamp, o).await {
                    Ok(_) => info!(
                        station = %o.station_name,
                        modules = o.modules.len(),
                        "outfitting",
                    ),
                    Err(err) => {
                        warn!(station = %o.station_name, error = %err, "outfitting")
                    }
                }
            }

            Message::Shipyard(ref e @ Entry { event: ref s, .. }) => {
                ensure_station_for_market(
                    db,
                    e.timestamp,
                    user,
                    &s.system_name,
                    &s.station_name,
                )
                .await;

                match Shipyard::from_journal(db, e.timestamp, s).await {
                    Ok(_) => info!(
                        station = %s.station_name,
                        ships = s.ships.len(),
                        "shipyard",
                    ),
                    Err(err) => {
                        warn!(station = %s.station_name, error = %err, "shipyard")
                    }
                }
            }

            Message::BlackMarket(ref e @ Entry { event: ref b, .. }) => {
                // The schema does not require a market id, and a sale that
                // cannot name its market cannot be placed at a station.
                let Some(market_id) = b.market_id else {
                    debug!(station = %b.station_name, "black market without a market id");
                    return;
                };

                ensure_station_for_market(
                    db,
                    e.timestamp,
                    user,
                    &b.system_name,
                    &b.station_name,
                )
                .await;

                match BlackMarket::from_journal(db, e.timestamp, market_id, b)
                    .await
                {
                    Ok(_) => {
                        info!(station = %b.station_name, commodity = %b.name, "black market")
                    }
                    Err(err) => {
                        warn!(station = %b.station_name, error = %err, "black market")
                    }
                }
            }

            // A schema nothing here reads yet. Said at `debug` because it is
            // most of what EDDN carries, and saying it at all is the only way
            // to know what is going by.
            Message::Unmodeled(_) => {
                debug!(schema = %schema_ref, "unmodeled schema")
            }
        }
    })
}
