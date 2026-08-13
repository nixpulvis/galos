//! What the EDDN sync writes, written and read back
//!
//! These need a database of their own, named by `TEST_DATABASE_URL`. They
//! write systems and markets under addresses they own and delete only what a
//! test needs gone before it runs, so every run leaves those rows sitting in
//! whatever they were pointed at. `DATABASE_URL` is deliberately not read
//! here: the database being filled from EDDN is one `cargo test` must not be
//! able to reach.
//!
//! Each test stands down when `TEST_DATABASE_URL` says nothing, so CI passes
//! without a database -- it runs `SQLX_OFFLINE=true` against the cached query
//! metadata and never connects. Point them at a migrated database and they run
//! for real:
//!
//! ```sh
//! createdb galos_test
//! DATABASE_URL=postgresql://…/galos_test \
//!     cargo sqlx migrate run --source galos_db/migrations/
//! TEST_DATABASE_URL=postgresql://…/galos_test \
//!     cargo test -p galos_db --test write_path
//! ```
//!
//! What they are for is the half of a write that the `sqlx` macros cannot
//! check. The macros prove a statement is valid against the schema and that
//! its columns are the types the code reads them as. They say nothing about
//! whether the row lands, whether a second message replaces the first or sits
//! beside it, or whether a key onto something absent stops the write -- and
//! every one of those is a decision made per table here.

use chrono::{DateTime, TimeZone, Utc};
use elite_journal::body::{
    AtmosphereType, Body as JournalBody, Composition, Discovery, Material,
    Orbit, Signal, Spin, Star as JournalStar, Surface,
};
use elite_journal::entry::incremental::exploration::{
    Cluster as JournalCluster, CodexEntry as JournalCodex, Ring as JournalRing,
    SystemSignal as JournalSignal,
};
use elite_journal::entry::incremental::travel::ApproachSettlement;
use elite_journal::entry::market::{
    BlackMarket as JournalBlackMarket, Market as JournalMarket, Module,
    Outfitting as JournalOutfitting, PricedModule, Shipyard as JournalShipyard,
};
use elite_journal::station::{
    LandingPads, Service, Station as JournalStation, StationType,
};
use elite_journal::system::Coordinate;
use elite_journal::Allegiance;
use galos_db::{
    black_market::BlackMarket, bodies::Body, body_signals::BodySignal,
    clusters::Cluster, codex_entries::CodexEntry, markets::Market,
    outfitting::Outfitting, rings::Ring, shipyard::Shipyard, stars::Star,
    stations::Station, system_signals::SystemSignal, systems::System, Database,
};
use std::collections::BTreeMap;
use std::time::Duration;

/// Where these tests write, or nothing and they stand down
///
/// `TEST_DATABASE_URL` and never `DATABASE_URL`, so that a database being used
/// for anything else cannot be reached from here. Read out of the environment
/// or out of `.env`, either of which is a place to say it.
fn database_url() -> Option<String> {
    dotenv::dotenv().ok();
    std::env::var("TEST_DATABASE_URL").ok()
}

/// Forget a system, so a test may assert on what did not happen
///
/// These tests share one database and own an address each, and asserting that a
/// write was turned away means telling a refusal apart from a row an earlier run
/// left behind. Only this file writes these addresses, so nothing points at the
/// row being dropped.
async fn forget(address: i64) {
    let Some(url) = database_url() else { return };
    let Ok(pool) = sqlx::PgPool::connect(&url).await else { return };
    // Whatever hangs off the system, then the system. Only this file writes
    // these addresses, so nothing else loses anything.
    for table in [
        "clusters",
        "rings",
        "bodies",
        "stars",
        "barycenters",
        "body_signals",
        "system_signals",
        "codex_entries",
        "stations",
        "systems",
    ] {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE {} = $1",
            table,
            if table == "systems" { "address" } else { "system_address" },
        ))
        .bind(address)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("{} should be clearable: {}", table, e));
    }
}

/// Forget a market, for the reason [`forget`] exists
///
/// Keyed by its own id rather than by a system, so there is no reaching these
/// through the address a test owns.
async fn forget_market(id: i64) {
    let Some(url) = database_url() else { return };
    let Ok(pool) = sqlx::PgPool::connect(&url).await else { return };
    for table in
        ["commodities", "outfitting", "shipyard", "black_market", "markets"]
    {
        sqlx::query(&format!(
            "DELETE FROM {} WHERE {} = $1",
            table,
            if table == "markets" { "id" } else { "market_id" },
        ))
        .bind(id)
        .execute(&pool)
        .await
        .unwrap_or_else(|e| panic!("{} should be clearable: {}", table, e));
    }
}

/// A database to write to, or nothing and the test stands down
///
/// Standing down is for having nowhere to write. A url that is there and will
/// not connect is a database that was meant to be run against, so it fails.
macro_rules! db {
    () => {
        match database_url() {
            Some(url) => Database::from_url(&url)
                .await
                .expect("TEST_DATABASE_URL should connect"),
            None => {
                eprintln!("no TEST_DATABASE_URL: standing down");
                return;
            }
        }
    };
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_780_000_000 + secs, 0).unwrap()
}

fn somewhere(n: f64) -> Coordinate {
    Coordinate { x: n, y: n, z: n }
}

/// Each test owns its own address, so they can run at the same time
///
/// Two tests writing counts to one system would race: the whole point of one
/// of them is that a second message changes what the first wrote.
const COUNTS: i64 = 900_000_001;
const COUNTS_AGAIN: i64 = 900_000_007;
const BODY_SIGNALS: i64 = 900_000_002;
const SYSTEM_SIGNALS: i64 = 900_000_003;
const CODEX: i64 = 900_000_004;
const SETTLEMENT: i64 = 900_000_005;
const TRADE: i64 = 900_000_006;
const CLUSTER: i64 = 900_000_008;
const RESCAN: i64 = 900_000_009;
const REDOCK: i64 = 900_000_010;
const STALE: i64 = 900_000_011;
const SAME_SECOND: i64 = 900_000_012;
const RING: i64 = 900_000_013;
const UNMAPPED: i64 = 900_000_014;
const SHARED_A: i64 = 900_000_015;
const SHARED_B: i64 = 900_000_016;
const ORPHANED: i64 = 900_000_017;
const PLACED: i64 = 900_000_018;
const LATE: i64 = 900_000_019;
const LATE_COUNT: i64 = 900_000_020;
const LATE_SIGNAL: i64 = 900_000_021;
const CROWDED: i64 = 900_000_022;

/// A market is keyed by its own id, so these stand apart from the addresses
const CARRIER_MARKET: i64 = 128_900_001;
const LATE_STOCK: i64 = 128_900_002;

/// A honk is often the first thing heard about a system, so it writes the row
#[async_std::test]
async fn a_body_count_creates_the_system_it_counts() {
    let db = db!();

    System::set_body_counts(
        &db,
        COUNTS,
        "Test Counts",
        Some(somewhere(1.0)),
        40,
        Some(10),
        at(0),
        "test",
    )
    .await
    .expect("counts should write");

    let system = System::fetch(&db, COUNTS).await.expect("system should exist");

    assert_eq!(system.address, COUNTS);
    assert_eq!(system.body_count, Some(40));
    assert_eq!(system.non_body_count, Some(10));
}

/// The count is taken from a later message, and the belts are left alone
///
/// `FSSAllBodiesFound` and `NavBeaconScan` report the body count and say
/// nothing about belts and rings. Passing `None` for those must not erase what
/// the honk found.
#[async_std::test]
async fn a_later_count_does_not_erase_what_it_does_not_carry() {
    let db = db!();

    System::set_body_counts(
        &db,
        COUNTS_AGAIN,
        "Test Counts Again",
        Some(somewhere(7.0)),
        40,
        Some(10),
        at(0),
        "test",
    )
    .await
    .expect("the honk should write");

    System::set_body_counts(
        &db,
        COUNTS_AGAIN,
        "Test Counts Again",
        None,
        41,
        None,
        at(60),
        "test",
    )
    .await
    .expect("the tally should write");

    let system =
        System::fetch(&db, COUNTS_AGAIN).await.expect("system should exist");

    assert_eq!(system.body_count, Some(41));
    assert_eq!(system.non_body_count, Some(10));
}

/// Signals land on bodies nothing has scanned, which is most of them
///
/// The honk finds signals before anything identifies what it found them on. A
/// foreign key onto `bodies` would throw away exactly those, so there is none,
/// and this is what says so.
#[async_std::test]
async fn signals_are_kept_for_a_body_that_was_never_scanned() {
    let db = db!();

    System::set_body_counts(
        &db,
        BODY_SIGNALS,
        "Test Body Signals",
        Some(somewhere(2.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let signals = vec![
        Signal { ty: "$SAA_SignalType_Geological;".into(), count: 3 },
        Signal { ty: "$SAA_SignalType_Biological;".into(), count: 1 },
    ];

    BodySignal::from_journal(&db, at(0), "test", BODY_SIGNALS, 12, &signals)
        .await
        .expect("signals should write for an unknown body");

    let found = BodySignal::fetch(&db, BODY_SIGNALS, 12)
        .await
        .expect("signals should read");

    assert_eq!(found.len(), 2);
    assert_eq!(found[0].signal_type, "$SAA_SignalType_Biological;");
    assert_eq!(found[0].count, 1);
}

/// A second report of the same kind replaces the count rather than adding one
#[async_std::test]
async fn a_signal_seen_again_is_the_same_row() {
    let db = db!();

    System::set_body_counts(
        &db,
        BODY_SIGNALS,
        "Test Body Signals",
        Some(somewhere(2.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    for (count, when) in [(3, at(0)), (5, at(60))] {
        let signals =
            vec![Signal { ty: "$SAA_SignalType_Geological;".into(), count }];
        BodySignal::from_journal(&db, when, "test", BODY_SIGNALS, 13, &signals)
            .await
            .expect("signals should write");
    }

    let found = BodySignal::fetch(&db, BODY_SIGNALS, 13)
        .await
        .expect("signals should read");

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].count, 5);
}

/// Each signal in a batch is written under its own time, not the message's
#[async_std::test]
async fn a_batch_of_system_signals_keeps_each_signal_s_own_time() {
    let db = db!();

    System::set_body_counts(
        &db,
        SYSTEM_SIGNALS,
        "Test System Signals",
        Some(somewhere(3.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let signals = vec![
        JournalSignal {
            timestamp: at(0),
            signal_name: "Abraham Lincoln".into(),
            signal_type: None,
            is_station: Some(true),
            uss_type: None,
            spawning_state: None,
            spawning_faction: None,
            spawning_power: None,
            opposing_power: None,
            threat_level: None,
        },
        JournalSignal {
            timestamp: at(300),
            signal_name: "$USS_HighGradeEmissions;".into(),
            signal_type: Some("USS".into()),
            is_station: None,
            uss_type: Some("$USS_Type_VeryValuableSalvage;".into()),
            spawning_state: None,
            spawning_faction: None,
            spawning_power: None,
            opposing_power: None,
            threat_level: Some(0),
        },
    ];

    SystemSignal::from_journal(&db, "test", SYSTEM_SIGNALS, &signals)
        .await
        .expect("signals should write");

    let found = SystemSignal::fetch_all(&db, SYSTEM_SIGNALS)
        .await
        .expect("signals should read");

    assert_eq!(found.len(), 2);
    // Ordered most recently seen first, which is the later of the two.
    assert_eq!(found[0].name, "$USS_HighGradeEmissions;");
    assert_eq!(found[0].updated_at, at(300));
    assert_eq!(found[1].updated_at, at(0));
    assert_eq!(found[1].is_station, Some(true));
}

#[async_std::test]
async fn a_codex_sighting_is_one_row_per_kind_per_system() {
    let db = db!();

    System::set_body_counts(
        &db,
        CODEX,
        "Test Codex",
        Some(somewhere(4.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let entry = JournalCodex {
        system_name: "Test Codex".into(),
        star_pos: somewhere(4.0),
        system_address: CODEX,
        entry_id: 2100701,
        name: Some("$Codex_Ent_Sulphur_Name;".into()),
        category: Some("$Codex_Category_Biology;".into()),
        sub_category: None,
        region: None,
        body_id: None,
        body_name: None,
        nearest_destination: None,
        latitude: None,
        longitude: None,
    };

    CodexEntry::from_journal(&db, at(0), "test", &entry)
        .await
        .expect("sighting should write");

    // Found again, this time placed on a body.
    let placed = JournalCodex { body_id: Some(12), ..entry };
    CodexEntry::from_journal(&db, at(60), "test", &placed)
        .await
        .expect("second sighting should write");

    let found =
        CodexEntry::fetch_all(&db, CODEX).await.expect("sighting should read");

    assert_eq!(found.len(), 1);
    assert_eq!(found[0].body_id, Some(12));
    // The first sighting named it and the second did not have to again.
    assert_eq!(found[0].name.as_deref(), Some("$Codex_Ent_Sulphur_Name;"));
}

/// A settlement is a station, and docking at it later keeps where it is
#[async_std::test]
async fn a_settlement_keeps_its_place_on_the_body() {
    let db = db!();

    System::set_body_counts(
        &db,
        SETTLEMENT,
        "Test Settlement",
        Some(somewhere(5.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let settlement = ApproachSettlement {
        name: "Bloomfield Vision".into(),
        market_id: Some(3510085376),
        system_name: "Test Settlement".into(),
        star_pos: somewhere(5.0),
        system_address: SETTLEMENT,
        body_id: 12,
        body_name: "Test Settlement 4".into(),
        latitude: Some(12.5),
        longitude: Some(-47.25),
        faction: None,
        government: None,
        allegiance: None,
        services: None,
        economies: None,
    };

    Station::from_settlement(&db, at(0), "test", &settlement)
        .await
        .expect("settlement should write");

    let station = Station::fetch(&db, SETTLEMENT, "Bloomfield Vision")
        .await
        .expect("station should read");

    assert_eq!(station.body_id, Some(12));
    assert_eq!(station.latitude, Some(12.5));
    assert_eq!(station.longitude, Some(-47.25));
    assert_eq!(station.market_id, Some(3510085376));
}

/// Outfitting is the whole of what is stocked, so what is left out is gone
///
/// Nothing else can retire a module, and without this a station goes on
/// advertising one it stopped stocking months ago.
#[async_std::test]
async fn an_outfitting_message_replaces_what_came_before() {
    let db = db!();

    System::set_body_counts(
        &db,
        TRADE,
        "Test Trade",
        Some(somewhere(6.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    Station::create(&db, at(0), "test", TRADE, "Test Station")
        .await
        .expect("station should write");

    let priced = |name: &str, price: i64| {
        Module::Priced(PricedModule {
            id: 1,
            name: name.into(),
            buy_price: price,
            buy_merc_coins_price: 0,
        })
    };

    let first = JournalOutfitting {
        system_name: "Test Trade".into(),
        station_name: "Test Station".into(),
        market_id: 128016384,
        modules: vec![priced("Int_Engine_A", 100), priced("Int_Engine_B", 200)],
    };
    Outfitting::from_journal(&db, at(0), &first)
        .await
        .expect("outfitting should write");

    let second = JournalOutfitting {
        modules: vec![priced("Int_Engine_B", 250)],
        ..first
    };
    Outfitting::from_journal(&db, at(60), &second)
        .await
        .expect("outfitting should write again");

    let stocked = Outfitting::fetch_all(&db, 128016384)
        .await
        .expect("outfitting should read");

    assert_eq!(stocked.len(), 1);
    assert_eq!(stocked[0].module_name, "Int_Engine_B");
    assert_eq!(stocked[0].buy_price, Some(250));
}

/// A module from the older schema is stocked without a price
#[async_std::test]
async fn an_unpriced_module_is_still_stocked() {
    let db = db!();

    System::set_body_counts(
        &db,
        TRADE,
        "Test Trade",
        Some(somewhere(6.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    Station::create(&db, at(0), "test", TRADE, "Test Station 2")
        .await
        .expect("station should write");

    let outfitting = JournalOutfitting {
        system_name: "Test Trade".into(),
        station_name: "Test Station 2".into(),
        market_id: 128016385,
        modules: vec![Module::Named("Hpt_ChaffLauncher_Tiny".into())],
    };
    Outfitting::from_journal(&db, at(0), &outfitting)
        .await
        .expect("outfitting should write");

    let stocked = Outfitting::fetch_all(&db, 128016385)
        .await
        .expect("outfitting should read");

    assert_eq!(stocked.len(), 1);
    assert_eq!(stocked[0].module_name, "Hpt_ChaffLauncher_Tiny");
    assert_eq!(stocked[0].buy_price, None);
}

#[async_std::test]
async fn a_shipyard_message_replaces_what_came_before() {
    let db = db!();

    System::set_body_counts(
        &db,
        TRADE,
        "Test Trade",
        Some(somewhere(6.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    Station::create(&db, at(0), "test", TRADE, "Test Station 3")
        .await
        .expect("station should write");

    let yard = JournalShipyard {
        system_name: "Test Trade".into(),
        station_name: "Test Station 3".into(),
        market_id: 128016386,
        ships: vec!["SideWinder".into(), "Eagle".into()],
        allow_cobra_mk_iv: Some(false),
    };
    Shipyard::from_journal(&db, at(0), &yard)
        .await
        .expect("shipyard should write");

    let second = JournalShipyard { ships: vec!["SideWinder".into()], ..yard };
    Shipyard::from_journal(&db, at(60), &second)
        .await
        .expect("shipyard should write again");

    let stocked =
        Shipyard::fetch_all(&db, 128016386).await.expect("yard should read");

    assert_eq!(stocked.len(), 1);
    assert_eq!(stocked[0].ship_name, "SideWinder");
}

/// The black market clears nothing, because it never says what else it takes
///
/// The opposite decision to outfitting and the shipyard, for the opposite
/// reason: a message here is one commodity, so an absent one is not evidence.
#[async_std::test]
async fn a_black_market_sale_does_not_retire_the_others() {
    let db = db!();

    System::set_body_counts(
        &db,
        TRADE,
        "Test Trade",
        Some(somewhere(6.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    Station::create(&db, at(0), "test", TRADE, "Test Station 4")
        .await
        .expect("station should write");

    for (name, price, when) in [("Gold", 9432, at(0)), ("Silver", 4700, at(60))]
    {
        let sale = JournalBlackMarket {
            system_name: "Test Trade".into(),
            station_name: "Test Station 4".into(),
            market_id: Some(128016387),
            name: name.into(),
            sell_price: price,
            prohibited: true,
        };
        BlackMarket::from_journal(&db, when, 128016387, &sale)
            .await
            .expect("sale should write");
    }

    let taken = BlackMarket::fetch_all(&db, 128016387)
        .await
        .expect("sales should read");

    assert_eq!(taken.len(), 2);
    assert_eq!(taken[0].name, "gold");
    assert_eq!(taken[1].name, "silver");
}

/// A belt cluster lands, and a second sighting replaces it
///
/// Keyed on the system and the id the game numbers it by, as a body is. The
/// same cluster is scanned again every time a commander passes through, and a
/// second row for it would count one stretch of belt twice.
#[async_std::test]
async fn a_belt_cluster_is_one_row_however_often_it_is_scanned() {
    let db = db!();
    forget(CLUSTER).await;

    System::set_body_counts(
        &db,
        CLUSTER,
        "Test Cluster",
        Some(somewhere(8.0)),
        4,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let ring = |ty: &str, id: i16| {
        let mut parent = BTreeMap::new();
        parent.insert(ty.to_owned(), id);
        parent
    };
    let mut cluster = JournalCluster {
        name: "Test Cluster A Belt Cluster 1".into(),
        id: 5,
        parents: vec![ring("Ring", 1), ring("Star", 0)],
        distance_from_arrival: Some(12.5),
        discovery: Discovery { discovered: true, mapped: false },
    };

    Cluster::from_journal(&db, at(0), "test", &cluster, CLUSTER)
        .await
        .expect("cluster should write");

    let held = Cluster::fetch_all(&db, CLUSTER).await.expect("should read");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].name, "Test Cluster A Belt Cluster 1");
    assert_eq!(held[0].id, 5);
    assert_eq!(held[0].distance_from_arrival, Some(12.5));
    assert!(held[0].discovered);
    assert!(!held[0].mapped);
    // Nearest ancestor first: the ring it lies in, then what that goes round.
    assert_eq!(held[0].parent_ids, vec![1, 0]);
    assert_eq!(held[0].parent_types, vec!["Ring", "Star"]);

    cluster.discovery.mapped = true;
    Cluster::from_journal(&db, at(60), "test", &cluster, CLUSTER)
        .await
        .expect("second scan should write");

    let held = Cluster::fetch_all(&db, CLUSTER).await.expect("should read");
    assert_eq!(held.len(), 1, "a second scan wrote a second row");
    assert!(held[0].mapped);
    assert_eq!(held[0].updated_at, at(60));
}

/// A basic scan does not undo what a detailed one recorded
///
/// The same body is scanned again every time a commander passes through, and
/// not every look is as close as the last. A detailed scan states a body's
/// surface, its temperature and what it is made of; a basic one carries none
/// of the three, and carries them absent rather than carries them as nothing.
#[async_std::test]
async fn a_basic_rescan_keeps_what_a_detailed_one_found() {
    let db = db!();

    System::set_body_counts(
        &db,
        RESCAN,
        "Test Rescan",
        Some(somewhere(9.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let orbit = || Orbit {
        semi_major_axis: 1e11,
        eccentricity: 0.01,
        orbital_inclination: 0.,
        periapsis: 1.,
        orbital_period: 1e7,
        ascending_node: Some(0.),
        mean_anomaly: Some(0.),
    };
    let body = |temperature, surface| JournalBody {
        id: 1,
        name: "Test Rescan 1".into(),
        ty: None,
        distance_from_arrival: Some(12.5),
        parents: vec![],
        planet_class: "Rocky body".into(),
        tidal_lock: None,
        mass: 1.,
        radius: 6e6,
        gravity: 9.8,
        temperature,
        surface,
        orbit: orbit(),
        spin: Spin { period: 80000., tilt: 0.1 },
        discovery: Discovery { discovered: true, mapped: true },
    };

    let detailed = body(
        Some(500.),
        Some(Surface {
            atmosphere_type: AtmosphereType::SulphurDioxide,
            pressure: 101325.,
            composition: Composition { ice: 0., rock: 70., metal: 30. },
            landable: true,
            atmosphere: Some("thin sulphur dioxide atmosphere".into()),
            volcanism: Some("minor silicate vapour geysers".into()),
            terraform_state: None,
            materials: vec![Material { name: "iron".into(), percent: 22.0 }],
        }),
    );
    let mut detailed = detailed;
    detailed.tidal_lock = Some(true);

    Body::from_journal(&db, at(0), "test", &detailed, RESCAN)
        .await
        .expect("detailed scan should write");

    // The same body, seen from further off.
    let mut basic = body(None, None);
    // As a sender that passes the game's scans on without them sends it.
    basic.orbit.ascending_node = None;
    basic.orbit.mean_anomaly = None;
    let returned = Body::from_journal(&db, at(60), "test", &basic, RESCAN)
        .await
        .expect("basic scan should write");

    // What the write says it wrote is what is on record, materials included.
    // They are read back rather than handed on from the scan, which carried
    // none.
    assert_eq!(
        returned
            .surface
            .as_ref()
            .expect("the write reports a surface")
            .materials
            .len(),
        1,
        "the write reported materials it had not kept",
    );

    // Read back rather than taken from what the write returned. A write
    // returns the materials it was handed, and the question here is what the
    // table kept.
    let stored = Body::fetch(&db, RESCAN, 1).await.expect("should read back");

    assert_eq!(stored.temperature, Some(500.), "temperature was erased");
    assert!(stored.surface.is_some(), "the surface was erased");
    assert!(stored.tidal_lock, "tidal lock was erased");

    let surface = stored.surface.expect("a surface");
    assert!(surface.landable, "landable was erased");
    assert_eq!(
        surface.atmosphere.as_deref(),
        Some("thin sulphur dioxide atmosphere"),
    );
    assert_eq!(surface.materials.len(), 1, "the materials were erased");
    // The path was sent again and the place along it was not.
    assert_eq!(stored.orbit.semi_major_axis, 1e11);
    assert_eq!(
        stored.orbit.ascending_node,
        Some(0.),
        "the ascending node was erased",
    );
    assert_eq!(
        stored.orbit.mean_anomaly,
        Some(0.),
        "the mean anomaly was erased",
    );
}

/// A sparser station message does not undo a fuller one
///
/// Station messages differ in what they carry. Docking reports the services,
/// the pads and the economies; something that merely names the station in
/// passing reports none of them, and reports them absent rather than empty.
#[async_std::test]
async fn a_sparser_station_message_keeps_what_the_fuller_one_said() {
    let db = db!();

    System::set_body_counts(
        &db,
        REDOCK,
        "Test Redock",
        Some(somewhere(10.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let station = |ty, pads, services| JournalStation {
        name: "Test Redock Port".into(),
        ty,
        dist_from_star_ls: Some(120.5),
        market_id: Some(128_016_384),
        landing_pads: pads,
        faction: None,
        government: None,
        allegiance: None,
        services,
        economies: None,
        wanted: None,
    };

    let docked = station(
        Some(StationType::Coriolis),
        Some(LandingPads { large: 4, medium: 4, small: 8 }),
        Some(vec![Service::Dock, Service::Refuel, Service::Shipyard]),
    );
    Station::from_journal(&db, at(0), "test", &docked, REDOCK)
        .await
        .expect("docking should write");

    // The same station, named in passing.
    let mentioned = station(None, None, None);
    Station::from_journal(&db, at(60), "test", &mentioned, REDOCK)
        .await
        .expect("a mention should write");

    let stored = Station::fetch(&db, REDOCK, "Test Redock Port")
        .await
        .expect("should read back");

    assert_eq!(stored.ty, Some(StationType::Coriolis), "the kind was erased");
    assert_eq!(
        stored.landing_pads,
        Some(LandingPads { large: 4, medium: 4, small: 8 }),
        "the pads were erased",
    );
    assert_eq!(
        stored.services.as_ref().map(|s| s.len()),
        Some(3),
        "the services were erased",
    );
    assert_eq!(stored.dist_from_star_ls, Some(120.5));
}

/// A message older than what is stored does not undo it
///
/// A station's services, faction and allegiance all change, so a message that
/// carries an older reading of them is not merely redundant. Uploaders batch
/// and reconnect, and the gateway has been seen handing over a journal entry a
/// quarter of an hour after the game wrote it.
#[async_std::test]
async fn a_stale_station_message_does_not_undo_a_newer_one() {
    let db = db!();

    System::set_body_counts(
        &db,
        STALE,
        "Test Stale",
        Some(somewhere(11.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let station = |services| JournalStation {
        name: "Test Stale Port".into(),
        ty: Some(StationType::Coriolis),
        dist_from_star_ls: Some(120.5),
        market_id: Some(128_016_385),
        landing_pads: None,
        faction: None,
        government: None,
        allegiance: None,
        services,
        economies: None,
        wanted: None,
    };

    // What is known now.
    let now = station(Some(vec![Service::Dock, Service::Shipyard]));
    Station::from_journal(&db, at(600), "test", &now, STALE)
        .await
        .expect("the newer message should write");

    // An older reading of the same station, arriving late.
    let then = station(Some(vec![Service::Dock]));
    let answered = Station::from_journal(&db, at(0), "test", &then, STALE)
        .await
        .expect("a stale message should not fail");

    // Answered with what is on record rather than with what it carried.
    assert_eq!(
        answered.services.as_ref().map(|s| s.len()),
        Some(2),
        "the write answered with the stale message",
    );

    let stored = Station::fetch(&db, STALE, "Test Stale Port")
        .await
        .expect("should read back");
    assert_eq!(
        stored.services.as_ref().map(|s| s.len()),
        Some(2),
        "a stale message undid the newer one",
    );
    assert_eq!(stored.updated_at, at(600), "the older time was written");
}

/// A settlement approached again keeps where it stands
///
/// A latitude and a longitude are the one thing a surface station has that an
/// orbital does not, and they are optional on the event that reports them. An
/// approach that leaves them out says nothing about where the place is.
#[async_std::test]
async fn a_settlement_approached_again_keeps_its_place() {
    let db = db!();
    forget(PLACED).await;

    System::set_body_counts(
        &db,
        PLACED,
        "Test Placed",
        Some(somewhere(18.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let approach = |latitude, longitude| ApproachSettlement {
        name: "Test Placed Outpost".into(),
        market_id: Some(3_510_085_377),
        system_name: "Test Placed".into(),
        star_pos: somewhere(18.0),
        system_address: PLACED,
        body_id: 12,
        body_name: "Test Placed 4".into(),
        latitude,
        longitude,
        faction: None,
        government: None,
        allegiance: None,
        economies: None,
        services: None,
    };

    Station::from_settlement(
        &db,
        at(0),
        "test",
        &approach(Some(12.5), Some(-47.25)),
    )
    .await
    .expect("the first approach should write");

    Station::from_settlement(&db, at(60), "test", &approach(None, None))
        .await
        .expect("an approach without a place should write");

    let stored = Station::fetch(&db, PLACED, "Test Placed Outpost")
        .await
        .expect("should read back");
    assert_eq!(stored.latitude, Some(12.5), "the latitude was erased");
    assert_eq!(stored.longitude, Some(-47.25), "the longitude was erased");
}

/// Two messages in the same second both get to say their part
///
/// `Location` and `FSDJump` are sent together and do not carry the same
/// fields. Since a system keeps what a message leaves out, the second of them
/// can only fill in what the first did not have.
#[async_std::test]
async fn two_messages_in_one_second_both_land() {
    let db = db!();
    forget(SAME_SECOND).await;

    // The first says how many live there and nothing about who runs it.
    System::create(
        &db,
        SAME_SECOND,
        "Test Same Second",
        Some(somewhere(12.0)),
        None,
        Some(4_000_000),
        None,
        None,
        None,
        None,
        at(0),
        "test",
    )
    .await
    .expect("the first should write");

    // The second, in the same second, says the allegiance and not the rest.
    System::create(
        &db,
        SAME_SECOND,
        "Test Same Second",
        None,
        None,
        None,
        None,
        None,
        Some(Allegiance::Empire),
        None,
        at(0),
        "test",
    )
    .await
    .expect("the second should write");

    let stored = System::fetch(&db, SAME_SECOND).await.expect("should read");
    assert_eq!(stored.population, 4_000_000, "the population was lost");
    assert_eq!(
        stored.allegiance,
        Some(Allegiance::Empire),
        "the second message in the second did not land",
    );
}

/// A ring is kept where its belt clusters can find it
///
/// Every belt cluster on record names a ring as its nearest ancestor, by the id
/// this table is keyed on. Taken from the feed, along with a cluster lying in
/// it, so the walk from the cluster back to the star has something behind every
/// number.
#[async_std::test]
async fn a_ring_is_kept_where_its_clusters_can_find_it() {
    let db = db!();

    System::set_body_counts(
        &db,
        RING,
        "Test Ring",
        Some(somewhere(13.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let ring = |ty: &str, id: i16| {
        let mut parent = BTreeMap::new();
        parent.insert(ty.to_owned(), id);
        parent
    };
    let scanned = JournalRing {
        name: "Test Ring D 12 A Ring".into(),
        id: 65,
        parents: vec![ring("Planet", 64), ring("Star", 6)],
        distance_from_arrival: Some(377022.12),
        orbit: Orbit {
            semi_major_axis: 36267684.0,
            eccentricity: 0.,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 15402.967,
            ascending_node: Some(0.),
            mean_anomaly: Some(166.00566),
        },
        discovery: Discovery { discovered: false, mapped: false },
    };

    Ring::from_journal(&db, at(0), "test", &scanned, RING)
        .await
        .expect("ring should write");

    // The cluster that lies in it, naming the ring by the id above.
    let cluster = JournalCluster {
        name: "Test Ring D 12 A Belt Cluster 1".into(),
        id: 70,
        parents: vec![ring("Ring", 65), ring("Planet", 64)],
        distance_from_arrival: Some(377022.0),
        discovery: Discovery { discovered: true, mapped: false },
    };
    Cluster::from_journal(&db, at(0), "test", &cluster, RING)
        .await
        .expect("cluster should write");

    let held = Ring::fetch_all(&db, RING).await.expect("should read");
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].id, 65);
    assert_eq!(held[0].orbit.orbital_period, 15402.967);
    // The body it goes round, not a ring: that is what tells it from a cluster.
    assert_eq!(
        held[0].parent_types.first().map(String::as_str),
        Some("Planet")
    );

    // And the cluster's nearest ancestor is the ring just written.
    let clusters = Cluster::fetch_all(&db, RING).await.expect("should read");
    let lying_in = clusters
        .iter()
        .find(|c| c.id == 70)
        .expect("the cluster should be on record");
    assert_eq!(lying_in.parent_ids.first(), Some(&65));
    assert_eq!(lying_in.parent_ids.first(), Some(&held[0].id));
}

/// Once something has been mapped it stays mapped
///
/// The flags say whether anyone has found or mapped a thing, and the galaxy
/// has no way to undo either. A scan still reports `WasMapped` false after
/// another has reported it true: one report in every three hundred that come
/// back for the same body does, and the one this was found on was a ring.
#[async_std::test]
async fn a_thing_once_mapped_stays_mapped() {
    let db = db!();

    System::set_body_counts(
        &db,
        UNMAPPED,
        "Test Unmapped",
        Some(somewhere(14.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let hangs_off = |ty: &str, id: i16| {
        let mut parent = BTreeMap::new();
        parent.insert(ty.to_owned(), id);
        parent
    };
    let ring = |discovered, mapped| JournalRing {
        name: "Test Unmapped 2 A Ring".into(),
        id: 40,
        parents: vec![hangs_off("Planet", 39)],
        distance_from_arrival: Some(10.0),
        orbit: Orbit {
            semi_major_axis: 1e7,
            eccentricity: 0.,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 1e4,
            ascending_node: Some(0.),
            mean_anomaly: Some(0.),
        },
        discovery: Discovery { discovered, mapped },
    };

    Ring::from_journal(&db, at(0), "test", &ring(true, true), UNMAPPED)
        .await
        .expect("the first scan should write");

    // Another scan of the same ring, saying nobody has mapped it.
    Ring::from_journal(&db, at(60), "test", &ring(false, false), UNMAPPED)
        .await
        .expect("the second scan should write");

    let held = Ring::fetch_all(&db, UNMAPPED).await.expect("should read");
    let stored = held.iter().find(|r| r.id == 40).expect("on record");
    assert!(stored.mapped, "a later scan unmapped it");
    assert!(stored.discovered, "a later scan undiscovered it");
}

/// Two systems may stand at one point
///
/// A position is measured and arrives rounded to a thirty-second of a light
/// year, so two of four hundred billion systems landing on one point is a thing
/// to expect. Forbidding it cost the whole of the second message: its name, its
/// population and everything else it said, not merely its position.
#[async_std::test]
async fn two_systems_may_share_a_position() {
    let db = db!();
    forget(SHARED_A).await;
    forget(SHARED_B).await;

    let at_the_same_point = |address, name| {
        System::create(
            &db,
            address,
            name,
            Some(somewhere(15.0)),
            None,
            Some(1),
            None,
            None,
            None,
            None,
            at(0),
            "test",
        )
    };

    at_the_same_point(SHARED_A, "Test Shared A")
        .await
        .expect("the first should write");
    at_the_same_point(SHARED_B, "Test Shared B")
        .await
        .expect("the second should write, and not be refused the point");

    // Both on record, each by its own address.
    let first = System::fetch(&db, SHARED_A).await.expect("the first");
    let second = System::fetch(&db, SHARED_B).await.expect("the second");
    assert_eq!(first.position, second.position);
    assert_eq!(first.population, 1, "the first lost what it said");
    assert_eq!(second.population, 1, "the second lost what it said");
}

/// A scan that names no ancestor does not orphan what it describes
///
/// Parents default to empty where the field is absent, and an empty ancestry is
/// not an answer: the walk from a body back to its star runs through these, and
/// a blanked chain stops at a number with nothing behind it. The thing lands at
/// the middle of its system, which is somewhere it is not.
#[async_std::test]
async fn a_scan_naming_no_ancestor_keeps_the_ancestry() {
    let db = db!();
    forget(ORPHANED).await;

    System::set_body_counts(
        &db,
        ORPHANED,
        "Test Orphaned",
        Some(somewhere(17.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let hangs_off = |ty: &str, id: i16| {
        let mut parent = BTreeMap::new();
        parent.insert(ty.to_owned(), id);
        parent
    };
    let ring = |parents| JournalRing {
        name: "Test Orphaned 2 A Ring".into(),
        id: 41,
        parents,
        distance_from_arrival: Some(10.0),
        orbit: Orbit {
            semi_major_axis: 1e7,
            eccentricity: 0.,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 1e4,
            ascending_node: Some(0.),
            mean_anomaly: Some(0.),
        },
        discovery: Discovery { discovered: true, mapped: false },
    };

    Ring::from_journal(
        &db,
        at(0),
        "test",
        &ring(vec![hangs_off("Planet", 39), hangs_off("Star", 0)]),
        ORPHANED,
    )
    .await
    .expect("the first scan should write");

    // The same ring from a sender that left the field out.
    Ring::from_journal(&db, at(60), "test", &ring(vec![]), ORPHANED)
        .await
        .expect("a scan without parents should write");

    let held = Ring::fetch_all(&db, ORPHANED).await.expect("should read");
    let stored = held.iter().find(|r| r.id == 41).expect("on record");
    assert_eq!(stored.parent_ids, vec![39, 0], "the ancestry was blanked");
    assert_eq!(
        stored.parent_types.first().map(String::as_str),
        Some("Planet"),
        "the ancestry was blanked",
    );

    // A secondary star, which goes round the barycenter the pair share. The
    // primary is the one thing an empty ancestry is true of, and nothing tells
    // that apart from a scan that left the field out.
    let star = |parents| JournalStar {
        name: "Test Orphaned B".into(),
        id: 2,
        parents,
        absolute_magnitude: 8.4,
        age_my: 1000,
        distance_from_arrival_ls: 900.,
        luminosity: "V".into(),
        star_class: "M".into(),
        stellar_mass: 0.35,
        subclass: 4,
        orbit: Some(Orbit {
            semi_major_axis: 3e11,
            eccentricity: 0.1,
            orbital_inclination: 0.,
            periapsis: 0.,
            orbital_period: 1e9,
            ascending_node: Some(0.),
            mean_anomaly: Some(0.),
        }),
        spin: Spin { period: 1e5, tilt: 0. },
        radius: 3e8,
        temperature: 3200.,
        discovery: Discovery { discovered: true, mapped: false },
    };

    Star::from_journal(
        &db,
        at(0),
        "test",
        &star(vec![hangs_off("Null", 1)]),
        ORPHANED,
    )
    .await
    .expect("the first scan should write");

    let answered =
        Star::from_journal(&db, at(60), "test", &star(vec![]), ORPHANED)
            .await
            .expect("a scan without parents should write");

    assert_eq!(
        answered.parent_id(),
        Some(1),
        "the answer forgot what the row kept",
    );
    let stored = Star::fetch(&db, ORPHANED, 2).await.expect("on record");
    assert_eq!(stored.parent_id(), Some(1), "the ancestry was blanked");
}

/// A message delivered late is taken without putting the row back in time
///
/// Uploaders batch and reconnect, so a fuller scan can arrive behind a thinner
/// one that was taken later. Refusing it outright would throw away everything
/// it found, since a body's figures do not change and the two disagree only in
/// how much they carry. What it must not do is leave the row saying it was last
/// heard of at a moment two messages ago.
#[async_std::test]
async fn a_message_delivered_late_does_not_put_the_stamp_back() {
    let db = db!();
    forget(LATE).await;

    System::set_body_counts(
        &db,
        LATE,
        "Test Late",
        Some(somewhere(19.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let body = |temperature| JournalBody {
        id: 1,
        name: "Test Late 1".into(),
        ty: None,
        distance_from_arrival: Some(12.5),
        parents: vec![],
        planet_class: "Rocky body".into(),
        tidal_lock: None,
        mass: 1.,
        radius: 6e6,
        gravity: 9.8,
        temperature,
        surface: None,
        orbit: Orbit {
            semi_major_axis: 1e11,
            eccentricity: 0.01,
            orbital_inclination: 0.,
            periapsis: 1.,
            orbital_period: 1e7,
            ascending_node: Some(0.),
            mean_anomaly: Some(0.),
        },
        spin: Spin { period: 80000., tilt: 0.1 },
        discovery: Discovery { discovered: true, mapped: true },
    };

    Body::from_journal(&db, at(600), "newer", &body(None), LATE)
        .await
        .expect("the scan taken later should write");

    // The fuller scan, taken ten minutes before and arriving after.
    Body::from_journal(&db, at(0), "older", &body(Some(500.)), LATE)
        .await
        .expect("the scan delivered late should write");

    let stored = Body::fetch(&db, LATE, 1).await.expect("should read back");
    assert_eq!(
        stored.temperature,
        Some(500.),
        "the late message was thrown away",
    );
    assert_eq!(stored.updated_at, at(600), "the stamp went back in time");
    assert_eq!(stored.updated_by, "newer", "the sender went back with it");
}

/// A count delivered late is taken without putting the system back in time
///
/// The counts are the one thing written under no timestamp guard at all, for
/// the reason [`System::set_body_counts`] gives: a system does not gain or lose
/// bodies, so the oldest honk is as good as the newest. That is a reason to
/// take the count, not a reason to believe the system was last heard of then.
#[async_std::test]
async fn a_count_delivered_late_does_not_put_the_stamp_back() {
    let db = db!();
    forget(LATE_COUNT).await;

    let honk = |bodies, non_bodies, secs, user| {
        System::set_body_counts(
            &db,
            LATE_COUNT,
            "Test Late Count",
            Some(somewhere(20.0)),
            bodies,
            non_bodies,
            at(secs),
            user,
        )
    };

    honk(40, None, 600, "newer").await.expect("the newer honk should write");
    // The honk that counted the belts, taken ten minutes before.
    honk(40, Some(7), 0, "older").await.expect("the late honk should write");

    let stored = System::fetch(&db, LATE_COUNT).await.expect("should read");
    assert_eq!(stored.body_count, Some(40));
    assert_eq!(stored.non_body_count, Some(7), "the late count was refused");
    assert_eq!(stored.updated_at, at(600), "the stamp went back in time");
    assert_eq!(stored.updated_by, "newer", "the sender went back with it");
}

/// The later reading of a signal wins, whichever of the two arrives first
///
/// A count is the whole of what a signal's row holds, so there is nothing for an
/// older message to fill in and the newer reading is simply the answer. Taking
/// whichever arrived last instead would leave a body's geology reading as
/// whatever the slowest uploader saw.
#[async_std::test]
async fn the_later_reading_of_a_signal_wins() {
    let db = db!();
    forget(LATE_SIGNAL).await;

    System::set_body_counts(
        &db,
        LATE_SIGNAL,
        "Test Late Signal",
        Some(somewhere(21.0)),
        1,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    let found = |count| {
        vec![Signal { ty: "$SAA_SignalType_Geological;".into(), count }]
    };

    BodySignal::from_journal(&db, at(600), "newer", LATE_SIGNAL, 3, &found(9))
        .await
        .expect("the reading taken later should write");

    // The same kind, read ten minutes before and arriving after.
    BodySignal::from_journal(&db, at(0), "older", LATE_SIGNAL, 3, &found(2))
        .await
        .expect("the reading delivered late should write");

    let stored = BodySignal::fetch(&db, LATE_SIGNAL, 3)
        .await
        .expect("signals should read");
    let signal = stored.first().expect("on record");
    assert_eq!(signal.count, 9, "the older reading won");
    assert_eq!(signal.updated_by, "newer", "the older sender won");
}

/// A message delivered late does not put a carrier back where it was
///
/// A fleet carrier jumps, and one of them turns up in this database under three
/// systems in an hour. Where it is now is what the newest message about it says,
/// and a message naming the system it has already left is not that.
#[async_std::test]
async fn a_late_message_does_not_put_a_carrier_back() {
    let db = db!();
    forget_market(CARRIER_MARKET).await;

    let docked_at = |system: &str| JournalMarket {
        system_name: system.into(),
        station_name: "X9K-45Z".into(),
        market_id: CARRIER_MARKET,
        commodities: vec![],
    };

    Market::from_journal(&db, at(600), &docked_at("Test Carrier Here"))
        .await
        .expect("the newer message should write");

    // The system it jumped out of, named by a sender catching up.
    let placed =
        Market::from_journal(&db, at(0), &docked_at("Test Carrier Gone"))
            .await
            .expect("the late message should write");

    assert_eq!(
        placed.system_name, "TEST CARRIER HERE",
        "the carrier was put back where it had been",
    );
}

/// A list of stock delivered late does not replace a newer one
///
/// Outfitting, the shipyard and the commodities are each read as the whole of
/// what is traded, so an older message does not add to the list -- it clears it
/// and writes prices the station has since moved on from.
#[async_std::test]
async fn a_late_list_of_stock_does_not_replace_a_newer_one() {
    let db = db!();
    forget_market(LATE_STOCK).await;

    let priced = |name: &str, price: i64| {
        Module::Priced(PricedModule {
            id: 1,
            name: name.into(),
            buy_price: price,
            buy_merc_coins_price: 0,
        })
    };
    let stocking = |module| JournalOutfitting {
        system_name: "Test Late Stock".into(),
        station_name: "Test Late Bay".into(),
        market_id: LATE_STOCK,
        modules: vec![module],
    };

    Outfitting::from_journal(
        &db,
        at(600),
        &stocking(priced("Int_Engine_B", 250)),
    )
    .await
    .expect("the newer message should write");

    Outfitting::from_journal(
        &db,
        at(0),
        &stocking(priced("Int_Engine_A", 100)),
    )
    .await
    .expect("the late message should write");

    let stocked = Outfitting::fetch_all(&db, LATE_STOCK)
        .await
        .expect("outfitting should read");

    assert_eq!(stocked.len(), 1, "the late message rewrote the bay");
    assert_eq!(
        stocked[0].module_name, "Int_Engine_B",
        "the late message rewrote the bay",
    );
}

/// More trade messages at once than the pool has connections to answer with
///
/// Each of these opens a transaction and then has to find the system its market
/// stands in. Asked of the pool rather than of the transaction it is already
/// holding, that second question waits on a connection its five neighbours are
/// holding transactions open on, and none of them can answer until one of the
/// others lets go. Nothing does, so they all wait out the acquire timeout.
///
/// Eight against a pool of five, so more than one has to be waiting on a
/// connection at once for this to say anything.
#[async_std::test]
async fn trade_messages_do_not_wait_on_each_other() {
    let Some(url) = database_url() else { return };
    let db = Database::from_url(&url).await.expect("a database");
    // The market points at the station, so it goes first.
    forget_market(128_016_388).await;
    forget(CROWDED).await;

    System::create(
        &db,
        CROWDED,
        "Test Crowded",
        Some(Coordinate { x: 0., y: 0., z: 0. }),
        None,
        None,
        None,
        None,
        None,
        None,
        at(0),
        "test",
    )
    .await
    .expect("system should write");

    Station::create(&db, at(0), "test", CROWDED, "Test Crowded Station")
        .await
        .expect("station should write");

    let mut running = Vec::new();
    for _ in 0..8 {
        let db = db.clone();
        running.push(async_std::task::spawn(async move {
            let outfitting = JournalOutfitting {
                system_name: "Test Crowded".into(),
                station_name: "Test Crowded Station".into(),
                market_id: 128_016_388,
                modules: vec![Module::Named("Hpt_ChaffLauncher_Tiny".into())],
            };
            Outfitting::from_journal(&db, at(0), &outfitting).await
        }));
    }

    // Waited on under a bound, since what goes wrong here is waiting rather
    // than failing: connections held against each other are released by the
    // acquire timeout, which is longer than a test should sit for.
    for running in running {
        async_std::future::timeout(Duration::from_secs(20), running)
            .await
            .expect("the messages should not be waiting on each other")
            .expect("every message should be written");
    }
}
