//! What the EDDN sync writes, written and read back
//!
//! These need a database. CI has none -- it runs `SQLX_OFFLINE=true` against
//! the cached query metadata and never connects -- so each of them stands down
//! when `DATABASE_URL` says nothing rather than failing. Run them against a
//! migrated database and they run for real:
//!
//! ```sh
//! DATABASE_URL=postgresql://…/galos cargo test -p galos_db --test write_path
//! ```
//!
//! What they are for is the half of a write that the `sqlx` macros cannot
//! check. The macros prove a statement is valid against the schema and that
//! its columns are the types the code reads them as. They say nothing about
//! whether the row lands, whether a second message replaces the first or sits
//! beside it, or whether a key onto something absent stops the write -- and
//! every one of those is a decision made per table here.

use chrono::{DateTime, TimeZone, Utc};
use elite_journal::body::Signal;
use elite_journal::entry::incremental::exploration::{
    CodexEntry as JournalCodex, SystemSignal as JournalSignal,
};
use elite_journal::entry::incremental::travel::ApproachSettlement;
use elite_journal::entry::market::{
    BlackMarket as JournalBlackMarket, Module, Outfitting as JournalOutfitting,
    PricedModule, Shipyard as JournalShipyard,
};
use elite_journal::system::Coordinate;
use galos_db::{
    black_market::BlackMarket, body_signals::BodySignal,
    codex_entries::CodexEntry, outfitting::Outfitting, shipyard::Shipyard,
    stations::Station, system_signals::SystemSignal, systems::System, Database,
};

/// A database to write to, or nothing and the test stands down
macro_rules! db {
    () => {
        match Database::new().await {
            Ok(db) => db,
            Err(_) => {
                eprintln!("no DATABASE_URL: standing down");
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
