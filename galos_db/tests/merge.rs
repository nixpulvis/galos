//! Two databases folded into one, against a third that saw both streams
//!
//! The property [`galos_db::merge`] claims is that merging B into A leaves A
//! holding what a single database fed both streams would hold. That is not a
//! property anything can be reasoned into: it is the composition of a dozen
//! guarded upserts, a faction remap, five list-valued replacements and a
//! trigger, and the only honest way to know it is to build the third
//! database and compare.
//!
//! So each test here makes three: A gets one list of events, B gets another,
//! C gets both in timestamp order, then B is merged into A and every table
//! of A is compared against C row for row.
//!
//! Two columns are excluded from the comparison and both for the same
//! reason — they are not facts about the galaxy, they are facts about which
//! database you are looking at.
//!
//! - `received_at` is `clock_timestamp()` at the write, so A's and C's
//!   differ by however long the test took. The merge deliberately re-stamps
//!   it rather than carrying the source's across; see the module doc there.
//! - `factions.id` is a `serial`, which is the whole reason the merge remaps
//!   factions by name. A and C mint their ids in different orders by
//!   construction — the events below are arranged so that they do — and
//!   every `faction_id` is therefore resolved back to the name it stands for
//!   before anything is compared. Comparing the numbers would be comparing
//!   the two databases' minting history, which is exactly what nobody wants
//!   preserved.
//!
//! What the events exercise, each named where it is written below: a system
//! both sides saw with different stamps, in both directions; a faction only
//! B knows; a faction both know that A numbered first; a market both saw
//! with different `listed_at`, in both directions; a body scanned by both,
//! in both directions; and the influence journal, whose rows the
//! `system_factions` merge causes rather than carries.
//!
//! `TEST_DATABASE_URL` names a server and the tests stand down without one,
//! exactly as `write_path.rs` does:
//!
//! ```sh
//! TEST_DATABASE_URL=postgresql://localhost/postgres \
//!     cargo test -p galos_db --test merge
//! ```

use chrono::{DateTime, TimeZone, Utc};
use elite_journal::body::{
    AtmosphereType, Body as JournalBody, Composition, Discovery, Material,
    Orbit, Spin, Surface,
};
use elite_journal::entry::market::{Commodity, Market as JournalMarket};
use elite_journal::prelude::{
    Allegiance, FactionInfo, Government, Happiness, Security, State,
    StateTrend,
};
use elite_journal::station::{
    LandingPads, Service, Station as JournalStation, StationType,
};
use elite_journal::system::Coordinate;
use galos_db::bodies::Body;
use galos_db::factions::{Faction, SystemFaction};
use galos_db::markets::Market;
use galos_db::merge::{merge, Merged, Table};
use galos_db::stations::Station;
use galos_db::systems::System;
use galos_db::testing::Scratch;
use galos_db::Database;
use galos_index::SystemName;

/// A database of this test's own, or nothing and the test stands down
macro_rules! db {
    () => {
        match Scratch::new().await {
            Some(db) => db,
            None => return,
        }
    };
}

fn at(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_790_000_000 + secs, 0).unwrap()
}

/// The systems the streams below name.
const SHARED: i64 = 910_000_001;
const LATE: i64 = 910_000_002;
const OURS: i64 = 910_000_003;
const OTHERS: i64 = 910_000_004;

/// Markets are keyed by their own ids, not by a system's.
const CARGO: i64 = 129_000_001;
const STORE: i64 = 129_000_002;

/// One thing somebody saw, written by the path the sync writes it by.
///
/// Deliberately the typed writers and not a hand-rolled `INSERT`: what is
/// being tested is that a merge agrees with the write path, so a test that
/// wrote its own rows would be testing the merge against itself.
enum Saw {
    /// A system, as a visit describes it.
    System {
        address: i64,
        name: &'static str,
        population: u64,
        allegiance: Allegiance,
    },
    /// Who holds a system, how strongly, and what they are going through.
    ///
    /// The faction is created by name, which is what mints an id, so the
    /// order these appear in a stream is the order that stream's database
    /// numbers its factions.
    Politics {
        address: i64,
        faction: &'static str,
        influence: f32,
        state: State,
    },
    /// A surface scan: the body, and the whole of what it is made of.
    Scan {
        address: i64,
        body: i16,
        name: &'static str,
        material: &'static str,
        percent: f64,
        mapped: bool,
    },
    /// Docking somewhere, which is what describes a station.
    ///
    /// `told` is whether the sender said anything beyond the station
    /// being there: a `Docked` event carries the whole description and a
    /// system's station list names it and no more, and the two arrive in
    /// either order. That is the `COALESCE` half of the rule -- an older
    /// full description is not erased by a newer bare mention -- which a
    /// merge has to reach from both sides.
    Dock {
        address: i64,
        station: &'static str,
        market: i64,
        told: bool,
    },
    /// A market message: the whole of what a station trades.
    Trade {
        market: i64,
        system: &'static str,
        station: &'static str,
        commodity: &'static str,
        price: i32,
    },
}

/// Write one thing somebody saw, at the moment they saw it.
async fn tell(db: &Database, when: DateTime<Utc>, saw: &Saw) {
    let mut conn = db.acquire().await.expect("a connection");

    match saw {
        Saw::System { address, name, population, allegiance } => {
            System::create(
                &mut conn,
                *address,
                &SystemName::new(*name),
                Some(Coordinate { x: 1.0, y: 2.0, z: 3.0 }),
                Some("G".to_owned()),
                Some(*population),
                Some(Security::Medium),
                Some(Government::Democracy),
                Some(*allegiance),
                None,
                when,
                "test",
            )
            .await
            .expect("the system should write");
        }

        Saw::Politics { address, faction, influence, state } => {
            let row = Faction::create(&mut conn, faction)
                .await
                .expect("the faction should write");
            let info = FactionInfo {
                name: (*faction).to_owned(),
                state: Some(*state),
                government: Government::Democracy,
                influence: *influence,
                allegiance: Allegiance::Independent,
                happiness: Some(Happiness::Happy),
                pending_states: vec![],
                active_states: vec![StateTrend {
                    state: *state,
                    trend: None,
                }],
                recovering_states: vec![],
                reputation: None,
                squadron_faction: false,
                home_system: false,
                happiest_system: false,
            };
            SystemFaction::from_journal(
                &mut conn,
                *address,
                row.id as u32,
                &info,
                when,
            )
            .await
            .expect("the faction's standing should write");
        }

        Saw::Scan { address, body, name, material, percent, mapped } => {
            let scanned = JournalBody {
                id: *body,
                name: (*name).to_owned(),
                ty: None,
                distance_from_arrival: Some(12.5),
                parents: vec![],
                planet_class: "Rocky body".into(),
                tidal_lock: Some(false),
                mass: 1.,
                radius: 6e6,
                gravity: 9.8,
                temperature: Some(500.),
                surface: Some(Surface {
                    atmosphere_type: AtmosphereType::SulphurDioxide,
                    pressure: 101325.,
                    composition: Composition {
                        ice: 0.,
                        rock: 70.,
                        metal: 30.,
                    },
                    landable: true,
                    atmosphere: Some("thin sulphur dioxide".into()),
                    volcanism: None,
                    terraform_state: None,
                    materials: vec![Material {
                        name: (*material).to_owned(),
                        percent: *percent,
                    }],
                }),
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
                discovery: Discovery { discovered: true, mapped: *mapped },
            };
            Body::from_journal(
                &mut conn,
                when,
                "test",
                &scanned,
                *address,
                Some(when),
            )
            .await
            .expect("the scan should write");
        }

        Saw::Dock { address, station, market, told } => {
            let docked = JournalStation {
                name: (*station).to_owned(),
                ty: told.then_some(StationType::Coriolis),
                dist_from_star_ls: told.then_some(120.5),
                market_id: Some(*market),
                landing_pads: told.then_some(LandingPads {
                    large: 4,
                    medium: 4,
                    small: 8,
                }),
                faction: None,
                government: told.then_some(Government::Democracy),
                allegiance: told.then_some(Allegiance::Federation),
                services: told
                    .then(|| vec![Service::Dock, Service::Refuel]),
                economies: None,
                wanted: None,
            };
            Station::from_journal(&mut conn, when, "test", &docked, *address)
                .await
                .expect("docking should write");
        }

        Saw::Trade { market, system, station, commodity, price } => {
            let message = JournalMarket {
                market_id: *market,
                system_name: (*system).to_owned(),
                station_name: (*station).to_owned(),
                commodities: vec![Commodity {
                    name: (*commodity).to_owned(),
                    mean_price: *price,
                    buy_price: *price,
                    sell_price: price - 1,
                    demand: 0,
                    demand_bracket: 0,
                    stock: 10,
                    stock_bracket: 1,
                }],
            };
            Market::from_journal(&mut conn, when, "test", &message)
                .await
                .expect("the market should write");
        }
    }
}

/// What A heard.
///
/// A mints `Alpha` and then `Beta`; B mints `Gamma`, `Beta` and then
/// `Alpha`, so no faction's id is the same number on both sides and the
/// remap has something to do. `Beta` is the one both know that A numbered
/// first.
fn mine() -> Vec<(i64, Saw)> {
    vec![
        // The system both sides saw, A's reading the older of the two.
        (2, Saw::System {
            address: SHARED,
            name: "Test Merge Shared",
            population: 100,
            allegiance: Allegiance::Federation,
        }),
        (4, Saw::Politics {
            address: SHARED,
            faction: "Test Merge Alpha",
            influence: 0.50,
            state: State::Boom,
        }),
        (6, Saw::Politics {
            address: SHARED,
            faction: "Test Merge Beta",
            influence: 0.30,
            state: State::Expansion,
        }),
        // A second reading of Alpha, which is a transition A witnessed and
        // the influence journal holds.
        (8, Saw::Politics {
            address: SHARED,
            faction: "Test Merge Alpha",
            influence: 0.55,
            state: State::Election,
        }),
        // The body both scanned, A's scan the older of the two.
        (12, Saw::Scan {
            address: SHARED,
            body: 1,
            name: "Test Merge Shared 1",
            material: "iron",
            percent: 22.0,
            mapped: false,
        }),
        // The station both sides docked at, A's the fuller description
        // and the older one: the merge has to keep what B's bare mention
        // does not repeat.
        (10, Saw::Dock {
            address: SHARED,
            station: "Test Merge Dock",
            market: CARGO,
            told: true,
        }),
        // The other station, the other way round: A names it in passing,
        // later than B described it.
        (24, Saw::Dock {
            address: SHARED,
            station: "Test Merge Store",
            market: STORE,
            told: false,
        }),
        // The system only A knows.
        (28, Saw::System {
            address: OURS,
            name: "Test Merge Ours",
            population: 7,
            allegiance: Allegiance::Independent,
        }),
        // The system both saw, A's reading the newer this time.
        (30, Saw::System {
            address: LATE,
            name: "Test Merge Late",
            population: 900,
            allegiance: Allegiance::Independent,
        }),
        // The market both saw, A's list the older of the two.
        (40, Saw::Trade {
            market: CARGO,
            system: "Test Merge Shared",
            station: "Test Merge Dock",
            commodity: "gold",
            price: 100,
        }),
        // The body both scanned, A's scan the newer this time.
        (45, Saw::Scan {
            address: SHARED,
            body: 2,
            name: "Test Merge Shared 2",
            material: "carbon",
            percent: 11.0,
            mapped: true,
        }),
        // The market both saw, A's list the newer this time.
        (60, Saw::Trade {
            market: STORE,
            system: "Test Merge Shared",
            station: "Test Merge Store",
            commodity: "silver",
            price: 20,
        }),
    ]
}

/// What B heard.
fn theirs() -> Vec<(i64, Saw)> {
    vec![
        // The system only B knows, and the faction only B knows, minted
        // before A has minted anything -- so the interleaved database
        // numbers its factions in an order the merged one cannot.
        (1, Saw::System {
            address: OTHERS,
            name: "Test Merge Theirs",
            population: 3,
            allegiance: Allegiance::Empire,
        }),
        (3, Saw::Politics {
            address: OTHERS,
            faction: "Test Merge Gamma",
            influence: 0.90,
            state: State::Investment,
        }),
        (5, Saw::System {
            address: SHARED,
            name: "Test Merge Shared",
            population: 150,
            allegiance: Allegiance::Federation,
        }),
        (7, Saw::Politics {
            address: SHARED,
            faction: "Test Merge Beta",
            influence: 0.32,
            state: State::Expansion,
        }),
        (11, Saw::Politics {
            address: SHARED,
            faction: "Test Merge Alpha",
            influence: 0.60,
            state: State::War,
        }),
        // B described this one fully, and earlier than A named it.
        (14, Saw::Dock {
            address: SHARED,
            station: "Test Merge Store",
            market: STORE,
            told: true,
        }),
        (15, Saw::Scan {
            address: SHARED,
            body: 2,
            name: "Test Merge Shared 2",
            material: "sulphur",
            percent: 4.0,
            mapped: false,
        }),
        (20, Saw::System {
            address: LATE,
            name: "Test Merge Late",
            population: 400,
            allegiance: Allegiance::Empire,
        }),
        // And named this one in passing, later than A described it.
        (22, Saw::Dock {
            address: SHARED,
            station: "Test Merge Dock",
            market: CARGO,
            told: false,
        }),
        (25, Saw::Scan {
            address: SHARED,
            body: 1,
            name: "Test Merge Shared 1",
            material: "nickel",
            percent: 18.0,
            mapped: true,
        }),
        (35, Saw::Trade {
            market: STORE,
            system: "Test Merge Shared",
            station: "Test Merge Store",
            commodity: "tritium",
            price: 50,
        }),
        (50, Saw::Trade {
            market: CARGO,
            system: "Test Merge Shared",
            station: "Test Merge Dock",
            commodity: "platinum",
            price: 300,
        }),
    ]
}

/// Feed a database a stream, in the order its clock says.
async fn feed(db: &Database, mut stream: Vec<(i64, Saw)>) {
    stream.sort_by_key(|(when, _)| *when);
    for (when, saw) in &stream {
        tell(db, at(*when), saw).await;
    }
}

/// Both streams as one database would have heard them.
fn both() -> Vec<(i64, Saw)> {
    let mut all = mine();
    all.extend(theirs());
    all.sort_by_key(|(when, _)| *when);
    all
}

/// Every table this program is responsible for, in catalog order.
async fn tables(db: &Database) -> Vec<String> {
    let mut conn = db.acquire().await.expect("a connection");
    sqlx::query_scalar(
        "SELECT c.relname::text \
           FROM pg_class c \
           JOIN pg_namespace n ON n.oid = c.relnamespace \
          WHERE n.nspname = 'public' \
            AND c.relkind = 'r' \
            AND c.relname <> '_sqlx_migrations' \
            AND NOT EXISTS (SELECT 1 FROM pg_depend d \
                             WHERE d.objid = c.oid AND d.deptype = 'e') \
          ORDER BY c.relname",
    )
    .fetch_all(&mut *conn)
    .await
    .expect("the catalog should answer")
}

/// One table's rows, as text, with the two per-database columns resolved
/// away.
///
/// `ROW(...)::text` rather than a typed read per table: what is being
/// compared is whether two databases hold the same rows, and Postgres'
/// own rendering says that for every column type in the schema — including
/// the enums and the geometry — without this test knowing any of them.
/// Ordered by that text, which is a total order and the same one on both
/// sides.
async fn rows(db: &Database, table: &str) -> Vec<String> {
    let mut conn = db.acquire().await.expect("a connection");

    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns \
          WHERE table_schema = 'public' AND table_name = $1 \
          ORDER BY ordinal_position",
    )
    .bind(table)
    .fetch_all(&mut *conn)
    .await
    .expect("the catalog should answer");

    let read: Vec<String> = columns
        .iter()
        .filter(|c| {
            // When a report reached this database, which is not a fact
            // about the galaxy, and the serial, which is not either.
            c.as_str() != "received_at"
                && !(table == "factions" && c.as_str() == "id")
        })
        .map(|c| match c.as_str() {
            "faction_id" | "faction_1_id" | "faction_2_id" => format!(
                "(SELECT f.name FROM factions f WHERE f.id = t.\"{c}\")"
            ),
            _ => format!("t.\"{c}\""),
        })
        .collect();

    sqlx::query_scalar(&format!(
        "SELECT ROW({})::text FROM \"{table}\" AS t ORDER BY 1",
        read.join(", "),
    ))
    .fetch_all(&mut *conn)
    .await
    .expect("the rows should read")
}

/// Every table of two databases, compared row for row.
async fn agree(merged: &Database, saw_both: &Database, what: &str) {
    for table in tables(merged).await {
        let ours = rows(merged, &table).await;
        let theirs = rows(saw_both, &table).await;
        assert_eq!(
            ours, theirs,
            "{what}: {table} differs between the merged database and \
             the one that saw both streams",
        );
    }
}

/// Merging B into A leaves A holding what a database that saw both holds
///
/// The whole claim, and the reason this file exists. Every table, every
/// row, every column bar the two that are facts about which database you
/// are looking at.
#[async_std::test]
async fn a_merge_agrees_with_a_database_that_saw_both_streams() {
    let a = db!();
    let b = db!();
    let c = db!();

    feed(&a, mine()).await;
    feed(&b, theirs()).await;
    feed(&c, both()).await;

    let did = merge(&a, &b, None, false, &mut |_| {})
        .await
        .expect("the merge should run");

    // The faction only B knows had to be minted here, and every faction B
    // holds had to be resolved to an id of this database's.
    assert_eq!(did.factions, 3, "all three factions should be resolved");
    assert_eq!(
        did.skipped,
        vec!["articles".to_owned()],
        "articles is the only table a merge will not carry",
    );

    agree(&a, &c, "after one merge").await;

    a.done().await;
    b.done().await;
    c.done().await;
}

/// Running the merge twice leaves what running it once did
///
/// A merge is something an operator runs, watches fail half way through a
/// network hiccup, and runs again. Every statement in it is an upsert or a
/// replacement keyed by something natural, so the second run has nothing
/// left to do — and "nothing left to do" has to mean the rows, not just
/// the counts.
#[async_std::test]
async fn a_merge_run_twice_does_what_it_did_once() {
    let a = db!();
    let b = db!();
    let c = db!();

    feed(&a, mine()).await;
    feed(&b, theirs()).await;
    feed(&c, both()).await;

    merge(&a, &b, None, false, &mut |_| {})
        .await
        .expect("the first merge should run");
    let again = merge(&a, &b, None, false, &mut |_| {})
        .await
        .expect("the second merge should run");

    agree(&a, &c, "after two merges").await;

    // And nothing landed the second time round: every row B holds is now
    // either older than what A has or identical to it.
    let made: u64 = again.tables.iter().map(|t| t.inserted).sum();
    assert_eq!(
        made, 0,
        "a second merge inserted rows: {}",
        again
            .tables
            .iter()
            .filter(|t| t.inserted > 0)
            .map(|t| t.to_string())
            .collect::<Vec<_>>()
            .join("; "),
    );

    a.done().await;
    b.done().await;
    c.done().await;
}

/// A dry run counts what a real run does and writes none of it
///
/// The point of `--dry-run` is to be told what a merge will do before it
/// does it, which is worth nothing if the two arithmetics can differ. They
/// cannot, because a dry run *is* the real run with a `ROLLBACK` at the
/// end.
#[async_std::test]
async fn a_dry_run_counts_what_a_real_run_does_and_writes_nothing() {
    let a = db!();
    let b = db!();
    let c = db!();

    feed(&a, mine()).await;
    feed(&b, theirs()).await;
    // C is A's stream alone here: it is the "nothing was written" oracle.
    feed(&c, mine()).await;

    let mut said: Vec<String> = Vec::new();
    let dry = merge(&a, &b, None, true, &mut |t: &Table| {
        said.push(t.to_string())
    })
    .await
    .expect("the dry run should run");

    assert!(
        said.iter().any(|line| line.starts_with("systems:")),
        "the caller should have been told about each table: {:?}",
        said,
    );

    agree(&a, &c, "after a dry run").await;

    let wet = merge(&a, &b, None, false, &mut |_| {})
        .await
        .expect("the real merge should run");

    assert_eq!(
        counted(&dry),
        counted(&wet),
        "a dry run and the real run disagreed about what would happen",
    );

    a.done().await;
    b.done().await;
    c.done().await;
}

/// What a run did, as something two runs can be compared by.
fn counted(merged: &Merged) -> Vec<(String, u64, u64, u64, u64)> {
    merged
        .tables
        .iter()
        .map(|t| {
            (t.name.clone(), t.read, t.inserted, t.updated, t.refused)
        })
        .collect()
}

/// `--since` carries what was collected after a moment and nothing else
///
/// The case this is for is the ordinary one: the restored database is
/// current up to the backup, and the only thing worth carrying is what
/// landed in the other one afterwards. A bound that leaked older rows
/// would be a slow merge rather than a wrong one; a bound that dropped
/// newer ones would lose exactly what the run was for.
///
/// The interesting half is the table with no clock of its own. A body's
/// materials are bounded by the body's stamp, not by their own -- they
/// have none -- so a body left behind by the bound must leave its
/// materials behind with it, or they arrive with nothing to be weighed
/// against and are refused for the wrong reason.
#[async_std::test]
async fn since_carries_what_came_after_it_and_no_more() {
    let a = db!();
    let b = db!();

    feed(&a, mine()).await;
    feed(&b, theirs()).await;

    merge(&a, &b, Some(at(30).naive_utc()), false, &mut |_| {})
        .await
        .expect("the bounded merge should run");

    let mut conn = a.acquire().await.expect("a connection");

    // B read this system at t5 and A at t2, so an unbounded merge would
    // have taken B's figure.
    let population: Option<i64> = sqlx::query_scalar(
        "SELECT population FROM systems WHERE address = $1",
    )
    .bind(SHARED)
    .fetch_one(&mut *conn)
    .await
    .expect("the system should be there");
    assert_eq!(
        population,
        Some(100),
        "a reading from before the bound was carried across anyway",
    );

    // B scanned this body at t25, which is before the bound, so neither
    // the body nor the materials hanging off it should have moved.
    let materials: Vec<String> = sqlx::query_scalar(
        "SELECT name::text FROM body_materials \
          WHERE system_address = $1 AND body_id = 1 ORDER BY name",
    )
    .bind(SHARED)
    .fetch_all(&mut *conn)
    .await
    .expect("the materials should read");
    assert_eq!(
        materials,
        vec!["iron".to_owned()],
        "a list whose body was left behind by the bound came across",
    );

    // And what did happen after the bound is here: B listed this market
    // at t50.
    let traded: Vec<String> = sqlx::query_scalar(
        "SELECT name::text FROM commodities WHERE market_id = $1 \
          ORDER BY name",
    )
    .bind(CARGO)
    .fetch_all(&mut *conn)
    .await
    .expect("the market should read");
    assert_eq!(traded, vec!["platinum".to_owned()]);

    a.done().await;
    b.done().await;
}

/// The lists that a merge replaces whole, replaced whole and in both
/// directions
///
/// Stated on its own because it is the failure that row-by-row merging
/// looks exactly like a success for: unioned, the shared station stocks
/// what it sold on two different days, and nothing about the row counts
/// says so.
#[async_std::test]
async fn a_market_takes_the_newer_list_entire() {
    let a = db!();
    let b = db!();

    feed(&a, mine()).await;
    feed(&b, theirs()).await;

    merge(&a, &b, None, false, &mut |_| {})
        .await
        .expect("the merge should run");

    let mut conn = a.acquire().await.expect("a connection");
    let traded = |market: i64| {
        let statement = "SELECT name::text FROM commodities \
                          WHERE market_id = $1 ORDER BY name";
        sqlx::query_scalar::<_, String>(statement).bind(market)
    };

    // B's list is the newer one, so it replaces A's rather than joining it.
    assert_eq!(
        traded(CARGO).fetch_all(&mut *conn).await.expect("a market"),
        vec!["platinum".to_owned()],
        "the newer list should have replaced the older one entire",
    );
    // A's is the newer one here, so B's is refused entire.
    assert_eq!(
        traded(STORE).fetch_all(&mut *conn).await.expect("a market"),
        vec!["silver".to_owned()],
        "an older list should not have been added to a newer one",
    );

    // The same rule one level down, where the list carries no clock of its
    // own and takes its body's: B scanned body 1 later and A scanned body
    // 2 later, so one body's materials come from each side.
    let materials = |body: i16| {
        let statement = "SELECT name::text FROM body_materials \
                          WHERE system_address = $1 AND body_id = $2 \
                          ORDER BY name";
        sqlx::query_scalar::<_, String>(statement).bind(SHARED).bind(body)
    };
    assert_eq!(
        materials(1).fetch_all(&mut *conn).await.expect("a body"),
        vec!["nickel".to_owned()],
    );
    assert_eq!(
        materials(2).fetch_all(&mut *conn).await.expect("a body"),
        vec!["carbon".to_owned()],
    );

    a.done().await;
    b.done().await;
}

/// Two databases at different migration versions are not merged
///
/// The mistake this refuses is a quiet one: two databases a migration
/// apart agree about most of their columns, so the merge would run, write
/// most of what it should, and leave the column the migration added out of
/// every row it touched.
#[async_std::test]
async fn a_merge_across_a_schema_difference_is_refused() {
    let a = db!();
    let b = db!();

    // One migration taken off B's record, which is what a database that
    // has not run the newest one looks like.
    let mut conn = b.acquire().await.expect("a connection");
    sqlx::query(
        "DELETE FROM _sqlx_migrations \
          WHERE version = (SELECT MAX(version) FROM _sqlx_migrations)",
    )
    .execute(&mut *conn)
    .await
    .expect("the row should go");
    drop(conn);

    let refused = merge(&a, &b, None, false, &mut |_| {}).await;

    match refused {
        Err(galos_db::Error::Divergent(..)) => {}
        Err(e) => panic!("refused for the wrong reason: {}", e),
        Ok(_) => panic!("a merge across a schema difference was allowed"),
    }

    a.done().await;
    b.done().await;
}
