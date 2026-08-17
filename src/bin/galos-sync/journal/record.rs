//! What a journal says, written to the database
//!
//! Every event read here arrives two ways. The game writes them to the `.log`
//! files it keeps while it is played, and EDDN carries the same events
//! forwarded by everyone else's copy of the game with the personal parts
//! stripped out. A scan is a scan either way, and describes the same galaxy,
//! so it is written the same way and this is the one place that says how.
//!
//! What the two do not share is how a message arrives and what has to be true
//! of it before it is worth reading: a `$schemaRef` to place, a socket that
//! may have stopped carrying anything, a directory of files to put in order.
//! None of that is here. This starts once something holds an entry and knows
//! whose it is.

use chrono::{DateTime, Utc};
use elite_journal::body::{Body as JournalBody, Signal};
use elite_journal::entry::incremental::exploration::ScanTarget;
use elite_journal::entry::market::{
    BlackMarket as JournalBlackMarket, Market as JournalMarket,
    Outfitting as JournalOutfitting, Shipyard as JournalShipyard,
};
use elite_journal::entry::route::Destination;
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
use tracing::{debug, info, warn};

/// Write everything one journal entry has to say
///
/// An entry that says nothing worth keeping is not an error and is passed
/// over. Neither is an entry whose write is refused: what a single message
/// could not do is said at `warn` and the next one is read, since a feed that
/// stopped at the first system it could not place would stop for good.
pub async fn entry(db: &Database, entry: &Entry<Event>, user: &str) {
    match &entry.event {
        Event::Scan(scan) => {
            ensure_system(
                db,
                entry.timestamp,
                user,
                scan.system_address,
                Some(&scan.star_system),
                scan.star_pos,
                "scan",
            )
            .await;

            match &scan.target {
                ScanTarget::Star(star) => match Star::from_journal(
                    db,
                    entry.timestamp,
                    user,
                    star,
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
                    body,
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
                        cluster,
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
                ScanTarget::Ring(ring) => match Ring::from_journal(
                    db,
                    entry.timestamp,
                    user,
                    ring,
                    scan.system_address,
                )
                .await
                {
                    Ok(_) => info!(ring = %ring.name, "scan"),
                    Err(err) => {
                        warn!(ring = %ring.name, error = %err, "scan")
                    }
                },
            }
        }
        // A barycenter is not a body and is not drawn. It is stored so that a
        // body naming it as an ancestor can be placed where it belongs rather
        // than at the middle of its system.
        Event::ScanBaryCentre(scan) => {
            ensure_system(
                db,
                entry.timestamp,
                user,
                scan.system_address,
                Some(&scan.star_system),
                scan.star_pos,
                "scan barycenter",
            )
            .await;

            match Barycenter::from_journal(db, entry.timestamp, user, scan)
                .await
            {
                // A barycenter has no name of its own, so the id it is known
                // by within its system is said along with the system, neither
                // meaning much without the other.
                Ok(_) => {
                    info!(system = %scan.star_system, barycenter = scan.body_id, "scan barycenter")
                }
                Err(err) => {
                    warn!(system = %scan.star_system, barycenter = scan.body_id, error = %err, "scan barycenter")
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

        // A carrier jump is a system visit and says everything about the
        // system that arriving under your own power does, so it is recorded
        // the same way.
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

        // How much there is in a system, which is the other half of knowing
        // what has been found in it. Three events report the same number under
        // three different names.
        Event::FssDiscoveryScan(e) => {
            record_body_counts(
                db,
                entry.timestamp,
                user,
                e.system_address,
                Some(&e.system_name),
                e.star_pos,
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
                Some(&e.system_name),
                e.star_pos,
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
                e.star_system.as_deref(),
                e.star_pos,
                e.num_bodies,
                None,
                "nav beacon scan",
            )
            .await
        }

        // A settlement is a station on a planet's surface, and this is the
        // only thing that says where on the planet it is.
        Event::ApproachSettlement(e) => {
            if !ensure_system(
                db,
                entry.timestamp,
                user,
                e.system_address,
                e.system_name.as_deref(),
                e.star_pos,
                "approach settlement",
            )
            .await
            {
                return;
            }

            match Station::from_settlement(db, entry.timestamp, user, e).await {
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

        // What is written on a body's surface. The surface scan and the honk
        // report it in the same terms, so both land in the same place.
        Event::SAASignalsFound(e) => {
            record_body_signals(
                db,
                entry.timestamp,
                user,
                e.system_address,
                e.star_system.as_deref(),
                e.star_pos,
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
                e.star_system.as_deref(),
                e.star_pos,
                e.body_id,
                &e.signals,
                "fss body signals",
            )
            .await
        }

        // What hangs in a system without being a body. A batch of them from
        // EDDN, one at a time from the game.
        Event::FssSignalDiscovered(e) => {
            if !ensure_system(
                db,
                entry.timestamp,
                user,
                e.system_address,
                e.star_system.as_deref(),
                e.star_pos,
                "fss signal discovered",
            )
            .await
            {
                return;
            }

            match SystemSignal::from_journal(
                db,
                entry.timestamp,
                user,
                e.system_address,
                &e.signals,
            )
            .await
            {
                Ok(_) => info!(
                    system = %named(e.star_system.as_deref()),
                    address = e.system_address,
                    signals = e.signals.len(),
                    "fss signal discovered",
                ),
                Err(err) => warn!(
                    system = %named(e.star_system.as_deref()),
                    address = e.system_address,
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
                Some(&e.system_name),
                e.star_pos,
                "codex entry",
            )
            .await
            {
                return;
            }

            match CodexEntry::from_journal(db, entry.timestamp, user, e).await {
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
            // A docking says nothing about where the system is, only which one
            // it is.
            ensure_system(
                db,
                entry.timestamp,
                user,
                e.system_address,
                Some(&e.system_name),
                None,
                "docked",
            )
            .await;

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
            match System::from_journal(db, entry.timestamp, user, &e.system)
                .await
            {
                Ok(_) => info!(system = %e.system.name, "fsd jump"),
                Err(err) => {
                    warn!(system = %e.system.name, error = %err, "fsd jump")
                }
            }
        }
        Event::NavRoute(plotted) => {
            nav_route(db, entry.timestamp, user, &plotted.destinations).await
        }
        _ => {}
    }
}

/// Write where a ship said it was going
///
/// Arrives as an event in the log and as the whole of `NavRoute.json` beside
/// it, saying the same thing both ways. Each stop names a system, where it is
/// and what burns at the middle of it, which is worth keeping whether or not
/// the route is ever flown.
pub async fn nav_route(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    destinations: &[Destination],
) {
    for destination in destinations {
        match System::create(
            db,
            destination.system_address as i64,
            &destination.star_system,
            Some(destination.star_pos),
            Some(destination.star_class.clone()),
            None,
            None,
            None,
            None,
            None,
            timestamp,
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

/// Write what a station buys and sells
pub async fn market(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    market: &JournalMarket,
) {
    match Market::from_journal(db, timestamp, user, market).await {
        // A market can arrive before anything that would create the system it
        // names, and is recorded with no system to belong to until that turns
        // up. The name it gave is all there is to go on in the meantime.
        Ok(written) => info!(
            market = %market.station_name,
            system = %market.system_name,
            orphan = written.system_address.is_none(),
            "commodity",
        ),
        Err(err) => {
            warn!(market = %market.station_name, error = %err, "commodity")
        }
    }
}

/// Write what a station sells in its outfitting bay
pub async fn outfitting(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    outfitting: &JournalOutfitting,
) {
    match Outfitting::from_journal(db, timestamp, user, outfitting).await {
        Ok(_) => info!(
            station = %outfitting.station_name,
            modules = outfitting.modules.len(),
            "outfitting",
        ),
        Err(err) => {
            warn!(station = %outfitting.station_name, error = %err, "outfitting")
        }
    }
}

/// Write what a station sells in its shipyard
pub async fn shipyard(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    shipyard: &JournalShipyard,
) {
    match Shipyard::from_journal(db, timestamp, user, shipyard).await {
        Ok(_) => info!(
            station = %shipyard.station_name,
            ships = shipyard.ships.len(),
            "shipyard",
        ),
        Err(err) => {
            warn!(station = %shipyard.station_name, error = %err, "shipyard")
        }
    }
}

/// Write one commodity as a station's black market takes it
pub async fn black_market(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    black_market: &JournalBlackMarket,
) {
    // The schema does not require a market id, and a sale that cannot name its
    // market cannot be placed at a station.
    let Some(market_id) = black_market.market_id else {
        debug!(station = %black_market.station_name, "black market without a market id");
        return;
    };

    match BlackMarket::from_journal(
        db,
        timestamp,
        user,
        market_id,
        black_market,
    )
    .await
    {
        Ok(_) => {
            info!(station = %black_market.station_name, commodity = %black_market.name, "black market")
        }
        Err(err) => {
            warn!(station = %black_market.station_name, error = %err, "black market")
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
///
/// Whether what follows is worth attempting. A row cannot be created for a
/// system nothing has named, so an event naming only an address writes nothing
/// here and goes on anyway: the arrival that had to come first is what wrote
/// that row, and where it did not the foreign key is what says so. A scan
/// writes its star regardless: the system may already be on record from
/// something else, and a write refused here is not the same as no row to hang
/// off.
pub(super) async fn ensure_system(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    address: i64,
    name: Option<&str>,
    position: Option<Coordinate>,
    what: &str,
) -> bool {
    let Some(name) = name else {
        return true;
    };

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
                "{}", what
            );
            false
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
    name: Option<&str>,
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
        Ok(_) => info!(
            system = %named(name),
            address = address,
            bodies = body_count,
            "{}", what
        ),
        Err(err) => warn!(
            system = %named(name),
            address = address,
            error = %err,
            "{}", what
        ),
    }
}

/// What to call a system in a log line where the event did not name one
///
/// Several of the events the game writes carry an address and no name. The
/// address is on the line beside this wherever it would help.
fn named(name: Option<&str>) -> &str {
    name.unwrap_or("unnamed")
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
    name: Option<&str>,
    position: Option<Coordinate>,
    body_id: i16,
    signals: &[Signal],
    what: &str,
) {
    if !ensure_system(db, timestamp, user, address, name, position, what).await
    {
        return;
    }

    match BodySignal::from_journal(
        db, timestamp, user, address, body_id, signals,
    )
    .await
    {
        Ok(_) => info!(
            system = %named(name),
            address = address,
            body = body_id,
            signals = signals.len(),
            "{}", what
        ),
        Err(err) => warn!(
            system = %named(name),
            address = address,
            error = %err,
            "{}", what
        ),
    }
}
