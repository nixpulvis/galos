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

use crate::{
    barycenters::Barycenter,
    black_market::BlackMarket,
    bodies::Body,
    body_signals::BodySignal,
    clusters::Cluster,
    codex_entries::CodexEntry,
    markets::Market,
    outfitting::Outfitting,
    rings::Ring,
    shipyard::Shipyard,
    sqlx::{PgConnection, Postgres, Transaction},
    stars::Star,
    stations::Station,
    system_signals::SystemSignal,
    systems::{Landed, System},
    Database, Error,
};
use chrono::{DateTime, Utc};
use elite_journal::body::{Body as JournalBody, Signal};
use elite_journal::entry::incremental::exploration::{ScanTarget, ScanType};
use elite_journal::entry::market::{
    BlackMarket as JournalBlackMarket, Market as JournalMarket,
    Outfitting as JournalOutfitting, Shipyard as JournalShipyard,
};
use elite_journal::entry::route::Destination;
use elite_journal::entry::{Entry, Event};
use elite_journal::station::Station as JournalStation;
use elite_journal::system::System as JournalSystem;
use galos_index::accumulate::merge;
use galos_index::SystemReport;
use tracing::{debug, info, warn};

/// A write inside a message that was turned away
///
/// Carries nothing. The line naming what was refused and why is written
/// where it happened, which is the only place that knows what it was. What
/// reaches the caller is that the transaction is no longer usable: a
/// statement that fails inside one aborts it, so every write after it would
/// fail too, and what the message wrote before it has to go back.
struct Refused;

/// What every refusal says, after the kind of message it was refused in
const DISCARDED: &str = "discarded, and the whole entry with it";

/// Turn a write that could not be made into the refusal of its message
///
/// This line is the only record there will be of what was dropped. The
/// entry goes back whole, so nothing is left in the database to find it by,
/// and a row lost this way is otherwise visible only by deriving the index
/// twice and diffing the two directories. So it names the system the write
/// was for, the thing within it that went, and the constraint the row broke
/// where the database named one -- a unique key says which two rows the
/// database holds to be one thing, which is the difference between a bug in
/// what is being written and a bug in what is writing it.
fn refused(
    what: &str,
    system: &str,
    address: i64,
    dropped: &str,
    err: &Error,
) -> Refused {
    warn!(
        system = %system,
        address = address,
        dropped = %dropped,
        constraint = broken(err).unwrap_or("none"),
        error = %err,
        "{} {}", what, DISCARDED
    );
    Refused
}

/// The constraint a write broke, where the database named one
///
/// [`None`] for every other reason a write fails, a lost connection among
/// them.
fn broken(err: &Error) -> Option<&str> {
    match err {
        Error::Sqlx(sqlx::Error::Database(err)) => err.constraint(),
        _ => None,
    }
}

/// A transaction for one message's writes
///
/// `None` where the database would not give one, which is that message
/// missed and said at `warn` under `what`, as any other refusal here is.
async fn begin(
    db: &Database,
    what: &str,
) -> Option<Transaction<'static, Postgres>> {
    match db.begin().await {
        Ok(tx) => Some(tx),
        Err(err) => {
            warn!(error = %err, "no transaction for this {}", what);
            None
        }
    }
}

/// Commit what a message wrote, or take it back where one write was refused
///
/// Every row a message implies lands together or none of them does, so a run
/// interrupted part way through has written whole messages and re-running
/// costs nothing.
///
/// Returns what the message wrote if it is on record, and [`None`] if it
/// is not: a refused write takes the whole message back, and a transaction
/// that will not close leaves nothing behind either.
async fn end<T>(
    tx: Transaction<'static, Postgres>,
    wrote: Result<T, Refused>,
    what: &str,
) -> Option<T> {
    let closed = match &wrote {
        Ok(_) => tx.commit().await,
        Err(Refused) => tx.rollback().await,
    };
    if let Err(err) = closed {
        warn!(error = %err, "could not close out this {}", what);
        return None;
    }
    wrote.ok()
}

/// Write one system report, which is a message of its own
///
/// What the sources with nothing below system level say — a dump's rows, a
/// journal's pre-pass over the systems its entries name — and the row
/// everything [`entry`] writes hangs off. Returns what the write did, or
/// [`None`] where no row was written; see
/// [`System::report`](crate::systems::System::report).
pub async fn system(
    db: &Database,
    report: &SystemReport,
    user: &str,
) -> Option<Landed> {
    let Some(mut tx) = begin(db, "system").await else { return None };
    let wrote = system_row(&mut tx, report, user).await;
    end(tx, wrote, "system").await.flatten()
}

/// Write everything one journal entry has to say
///
/// An entry that says nothing worth keeping is not an error and is passed
/// over. Neither is an entry whose write is refused: what a single message
/// could not do is said at `warn` and the next one is read, since a feed that
/// stopped at the first system it could not place would stop for good.
///
/// One transaction for the whole entry. Every row it implies lands together
/// or none of them does — a body with its materials, a station with its
/// economies — and nothing is held back past the entry it belongs to.
///
/// Returns what the entry's own system row write did, or [`None`] where
/// the entry named no system or the whole entry was taken back.
pub async fn entry(
    db: &Database,
    entry: &Entry<Event>,
    user: &str,
) -> Option<Landed> {
    let Some(mut tx) = begin(db, "entry").await else { return None };
    let wrote = write(&mut tx, entry, user).await;
    end(tx, wrote, "entry").await.flatten()
}

/// What one entry says, onto the connection its transaction is on
///
/// Stops at the first write refused, which is what [`Refused`] is for.
///
/// The system it happened in comes first and comes from
/// [`SystemReport::of`], which is the one fan-out over the events that name
/// one — the same call `galos_index::Galaxy` makes, so the two halves of
/// this program cannot disagree about which systems exist. Everything below
/// is what only Postgres keeps: the stations, the markets, the signals, the
/// codex sightings and the factions, each hanging off that row by a foreign
/// key, which is why the row is written before any of them is attempted.
///
/// Returns what the write of that system row did — an entry names at most
/// one system.
async fn write(
    conn: &mut PgConnection,
    entry: &Entry<Event>,
    user: &str,
) -> Result<Option<Landed>, Refused> {
    // Nothing named a system is nothing to hang anything off and nothing to
    // wait for.
    let landed = match SystemReport::of(entry) {
        Some(report) => system_row(conn, &report, user).await?,
        None => None,
    };

    match &entry.event {
        Event::Scan(scan) => {
            // A kind nobody has modelled is a kind whose discovery flag
            // `merge::discovered_at` trusts without anybody having decided
            // it should be, so it is said out loud rather than passed over.
            // What arrives here is what to add to `ScanType`.
            if let Some(ScanType::Other(kind)) = &scan.scan_type {
                warn!(kind = %kind, "scan of an unmodeled kind");
            }
            let found = merge::discovered_at(scan, entry.timestamp);
            match &scan.target {
                ScanTarget::Star(star) => {
                    match Star::from_journal(
                        conn,
                        entry.timestamp,
                        user,
                        star,
                        scan.system_address,
                        found,
                    )
                    .await
                    {
                        Ok(_) => {
                            info!(star = %star.name, "scan")
                        }
                        Err(err) => {
                            return Err(refused(
                                "scan",
                                &scan.star_system,
                                scan.system_address,
                                &star.name,
                                &err,
                            ))
                        }
                    }

                    // A star nothing stands between the ship and is the one
                    // it drops at, and its class is what the system
                    // supercharges with and falls back to for its light.
                    if star.distance_from_arrival_ls == 0.0 {
                        match System::set_primary_star_class(
                            conn,
                            scan.system_address,
                            &star.star_class,
                        )
                        .await
                        {
                            Ok(true) => info!(
                                system = %scan.star_system,
                                class = %star.star_class,
                                "arrival star",
                            ),
                            Ok(false) => {}
                            Err(err) => {
                                return Err(refused(
                                    "arrival star",
                                    &scan.star_system,
                                    scan.system_address,
                                    &star.name,
                                    &err,
                                ))
                            }
                        }
                    }
                }
                ScanTarget::Body(body) => match Body::from_journal(
                    conn,
                    entry.timestamp,
                    user,
                    body,
                    scan.system_address,
                    found,
                )
                .await
                {
                    Ok(_) => {
                        info!(body = %body.name, "scan")
                    }
                    Err(err) => {
                        return Err(refused(
                            "scan",
                            &scan.star_system,
                            scan.system_address,
                            &body.name,
                            &err,
                        ))
                    }
                },
                ScanTarget::Cluster(cluster) => {
                    match Cluster::from_journal(
                        conn,
                        entry.timestamp,
                        user,
                        cluster,
                        scan.system_address,
                        found,
                    )
                    .await
                    {
                        Ok(_) => {
                            info!(cluster = %cluster.name, "scan")
                        }
                        Err(err) => {
                            return Err(refused(
                                "scan",
                                &scan.star_system,
                                scan.system_address,
                                &cluster.name,
                                &err,
                            ))
                        }
                    }
                }
                ScanTarget::Ring(ring) => match Ring::from_journal(
                    conn,
                    entry.timestamp,
                    user,
                    ring,
                    scan.system_address,
                    found,
                )
                .await
                {
                    Ok(_) => info!(ring = %ring.name, "scan"),
                    Err(err) => {
                        return Err(refused(
                            "scan",
                            &scan.star_system,
                            scan.system_address,
                            &ring.name,
                            &err,
                        ))
                    }
                },
            }
        }
        // A barycenter is not a body and is not drawn. It is stored so that a
        // body naming it as an ancestor can be placed where it belongs rather
        // than at the middle of its system.
        Event::ScanBaryCentre(scan) => {
            match Barycenter::from_journal(conn, entry.timestamp, user, scan)
                .await
            {
                // A barycenter has no name of its own, so the id it is known
                // by within its system is said along with the system, neither
                // meaning much without the other.
                Ok(_) => {
                    info!(system = %scan.star_system, barycenter = scan.body_id, "scan barycenter")
                }
                Err(err) => {
                    return Err(refused(
                        "scan barycenter",
                        &scan.star_system,
                        scan.system_address,
                        &scan.body_id.to_string(),
                        &err,
                    ))
                }
            }
        }
        Event::Location(e) => {
            record_visit(
                conn,
                entry.timestamp,
                user,
                &e.system,
                e.body.as_ref(),
                e.station.as_ref(),
                "location",
            )
            .await?
        }

        // A carrier jump is a system visit and says everything about the
        // system that arriving under your own power does, so it is recorded
        // the same way.
        Event::CarrierJump(e) => {
            record_visit(
                conn,
                entry.timestamp,
                user,
                &e.system,
                e.body.as_ref(),
                e.station.as_ref(),
                "carrier jump",
            )
            .await?
        }

        // An arrival under the ship's own power, which says the same six
        // political columns a `Location` does and carries no body and no
        // station.
        Event::FsdJump(e) => {
            match System::factions(conn, &e.system, entry.timestamp).await {
                Ok(()) => info!(system = %e.system.name, "fsd jump"),
                Err(err) => {
                    return Err(refused(
                        "fsd jump",
                        &e.system.name,
                        e.system.address,
                        "factions",
                        &err,
                    ))
                }
            }
        }

        // How much there is in a system is the other half of knowing what
        // has been found in it, and all three events that report it say it
        // in the report above. Nothing left to write.
        Event::FssDiscoveryScan(_)
        | Event::FssAllBodiesFound(_)
        | Event::NavBeaconScan(_) => {}

        // A settlement is a station on a planet's surface, and this is the
        // only thing that says where on the planet it is.
        Event::ApproachSettlement(e) => {
            match Station::from_settlement(conn, entry.timestamp, user, e).await
            {
                Ok(_) => info!(
                    settlement = %e.name,
                    body = %e.body_name,
                    "approach settlement",
                ),
                Err(err) => {
                    return Err(refused(
                        "approach settlement",
                        named(e.system_name.as_deref()),
                        e.system_address,
                        &e.name,
                        &err,
                    ))
                }
            }
        }

        // What is written on a body's surface. The surface scan and the honk
        // report it in the same terms, so both land in the same place.
        Event::SAASignalsFound(e) => {
            record_body_signals(
                conn,
                entry.timestamp,
                user,
                e.system_address,
                e.star_system.as_deref(),
                e.body_id,
                &e.signals,
                "saa signals found",
            )
            .await?
        }

        Event::FssBodySignals(e) => {
            record_body_signals(
                conn,
                entry.timestamp,
                user,
                e.system_address,
                e.star_system.as_deref(),
                e.body_id,
                &e.signals,
                "fss body signals",
            )
            .await?
        }

        // What hangs in a system without being a body. A batch of them from
        // EDDN, one at a time from the game.
        Event::FssSignalDiscovered(e) => {
            match SystemSignal::from_journal(
                conn,
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
                Err(err) => {
                    return Err(refused(
                        "fss signal discovered",
                        named(e.star_system.as_deref()),
                        e.system_address,
                        "system signals",
                        &err,
                    ))
                }
            }
        }

        Event::CodexEntry(e) => {
            match CodexEntry::from_journal(conn, entry.timestamp, user, e).await
            {
                Ok(_) => info!(
                    system = %e.system_name,
                    entry = e.entry_id,
                    "codex entry",
                ),
                Err(err) => {
                    return Err(refused(
                        "codex entry",
                        &e.system_name,
                        e.system_address,
                        &e.entry_id.to_string(),
                        &err,
                    ))
                }
            }
        }
        Event::Docked(e) => {
            match Station::from_journal(
                conn,
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
                    return Err(refused(
                        "docked",
                        &e.system_name,
                        e.system_address,
                        &e.station.name,
                        &err,
                    ))
                }
            }
        }
        Event::NavRoute(plotted) => {
            nav_route(conn, entry.timestamp, user, &plotted.destinations)
                .await?
        }
        _ => {}
    }

    Ok(landed)
}

/// Write where a ship said it was going
///
/// Arrives as an event in the log and as the whole of `NavRoute.json` beside
/// it, saying the same thing both ways. Each stop names a system, where it is
/// and what burns at the middle of it, which is worth keeping whether or not
/// the route is ever flown.
///
/// The one event that states a system per stop rather than one, which is why
/// it is [`SystemReport::plotted`] over the destinations here and in
/// `galos_index::Galaxy` rather than an arm of [`SystemReport::of`].
async fn nav_route(
    conn: &mut PgConnection,
    timestamp: DateTime<Utc>,
    user: &str,
    destinations: &[Destination],
) -> Result<(), Refused> {
    for stop in destinations {
        let report = SystemReport::plotted(timestamp, stop);
        system_row(conn, &report, user).await?;
        info!(system = %stop.star_system, "nav route");
    }
    Ok(())
}

/// Write what a station buys and sells
///
/// One transaction: a market is a row per commodity and they land together.
pub async fn market(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    market: &JournalMarket,
) {
    let Some(mut tx) = begin(db, "commodity").await else { return };
    let wrote =
        match Market::from_journal(&mut tx, timestamp, user, market).await {
            // A market can arrive before anything that would create the
            // system it names, and is recorded with no system to belong to
            // until that turns up. The name it gave is all there is to go on
            // in the meantime.
            Ok(written) => {
                info!(
                    market = %market.station_name,
                    system = %market.system_name,
                    orphan = written.system_address.is_none(),
                    "commodity",
                );
                Ok(())
            }
            Err(err) => {
                warn!(market = %market.station_name, error = %err, "commodity");
                Err(Refused)
            }
        };
    end(tx, wrote, "commodity").await;
}

/// Write what a station sells in its outfitting bay
///
/// One transaction: a bay is a row per module and they land together.
pub async fn outfitting(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    outfitting: &JournalOutfitting,
) {
    let Some(mut tx) = begin(db, "outfitting").await else { return };
    let wrote =
        match Outfitting::from_journal(&mut tx, timestamp, user, outfitting)
            .await
        {
            Ok(_) => {
                info!(
                    station = %outfitting.station_name,
                    modules = outfitting.modules.len(),
                    "outfitting",
                );
                Ok(())
            }
            Err(err) => {
                warn!(
                    station = %outfitting.station_name,
                    error = %err,
                    "outfitting",
                );
                Err(Refused)
            }
        };
    end(tx, wrote, "outfitting").await;
}

/// Write what a station sells in its shipyard
///
/// One transaction: a shipyard is a row per ship and they land together.
pub async fn shipyard(
    db: &Database,
    timestamp: DateTime<Utc>,
    user: &str,
    shipyard: &JournalShipyard,
) {
    let Some(mut tx) = begin(db, "shipyard").await else { return };
    let wrote = match Shipyard::from_journal(&mut tx, timestamp, user, shipyard)
        .await
    {
        Ok(_) => {
            info!(
                station = %shipyard.station_name,
                ships = shipyard.ships.len(),
                "shipyard",
            );
            Ok(())
        }
        Err(err) => {
            warn!(
                station = %shipyard.station_name,
                error = %err,
                "shipyard",
            );
            Err(Refused)
        }
    };
    end(tx, wrote, "shipyard").await;
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

    let Some(mut tx) = begin(db, "black market").await else { return };
    let wrote = match BlackMarket::from_journal(
        &mut tx,
        timestamp,
        user,
        market_id,
        black_market,
    )
    .await
    {
        Ok(_) => {
            info!(station = %black_market.station_name, commodity = %black_market.name, "black market");
            Ok(())
        }
        Err(err) => {
            warn!(station = %black_market.station_name, error = %err, "black market");
            Err(Refused)
        }
    };
    end(tx, wrote, "black market").await;
}

/// Record arriving somewhere: the factions, the body arrived at, the station
///
/// [`Event::Location`] and [`Event::CarrierJump`] describe a system in the
/// same terms and are worth the same to a galaxy being mapped, so they are
/// written the same way. `what` names which of the two it was and is what the
/// log lines are filed under.
///
/// The system's own columns are not here: they are the report [`write`] wrote
/// before this was called. What is left is the three things an arrival
/// carries that the index has no column for.
#[allow(clippy::too_many_arguments)]
async fn record_visit(
    conn: &mut PgConnection,
    timestamp: DateTime<Utc>,
    user: &str,
    system: &JournalSystem,
    body: Option<&JournalBody>,
    station: Option<&JournalStation>,
    what: &str,
) -> Result<(), Refused> {
    match System::factions(conn, system, timestamp).await {
        Ok(()) => info!(system = %system.name, "{}", what),
        Err(err) => {
            return Err(refused(
                what,
                &system.name,
                system.address,
                "factions",
                &err,
            ))
        }
    }

    if let Some(body) = body {
        // No discovery time: none of the events that land here is a scan, so
        // whatever the body carries for it was never reported. See
        // `galos_index::accumulate::merge::discovered_at`.
        match Body::from_journal(
            conn,
            timestamp,
            user,
            body,
            system.address,
            None,
        )
        .await
        {
            Ok(_) => info!(body = %body.name, "{}", what),
            Err(err) => {
                return Err(refused(
                    what,
                    &system.name,
                    system.address,
                    &body.name,
                    &err,
                ))
            }
        }
    }

    if let Some(station) = station {
        match Station::from_journal(
            conn,
            timestamp,
            user,
            station,
            system.address,
        )
        .await
        {
            Ok(_) => info!(station = %station.name, "{}", what),
            Err(err) => {
                return Err(refused(
                    what,
                    &system.name,
                    system.address,
                    &station.name,
                    &err,
                ))
            }
        }
    }

    Ok(())
}

/// Write the system a report describes, onto the connection it is given
///
/// `galos_db::systems::System::report` does the deciding — which statements
/// a report's parts call for, and that a report naming only an address
/// writes nothing, a `systems` row being uncreatable without a name. What is
/// here is the log line.
///
/// Refused where the row could not be written, which takes the whole message
/// back: everything keyed onto a system hangs off that row by a foreign key,
/// so a message that could not place its system has nothing left to write.
/// It is one message, and the next one is read.
async fn system_row(
    conn: &mut PgConnection,
    report: &SystemReport,
    user: &str,
) -> Result<Option<Landed>, Refused> {
    match System::report(conn, report, user).await {
        Ok(landed) => Ok(landed),
        // Its address and where it says it is. A write is refused over both,
        // and the name gives neither.
        Err(err) => {
            warn!(
                system = %named(report.name.as_deref()),
                address = report.address,
                position = ?report.position,
                constraint = broken(&err).unwrap_or("none"),
                error = %err,
                "system {}", DISCARDED
            );
            Err(Refused)
        }
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
    conn: &mut PgConnection,
    timestamp: DateTime<Utc>,
    user: &str,
    address: i64,
    name: Option<&str>,
    body_id: i16,
    signals: &[Signal],
    what: &str,
) -> Result<(), Refused> {
    match BodySignal::from_journal(
        conn, timestamp, user, address, body_id, signals,
    )
    .await
    {
        Ok(_) => {
            info!(
                system = %named(name),
                address = address,
                body = body_id,
                signals = signals.len(),
                "{}", what
            );
            Ok(())
        }
        Err(err) => {
            Err(refused(what, named(name), address, &body_id.to_string(), &err))
        }
    }
}
