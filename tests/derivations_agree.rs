//! The same events through both derivations publish the same galaxy.
//!
//! The last thing the source and sink rework asked for, and the reason
//! `sink/` is in the library rather than in the binary: no integration test
//! could reach it there.
//!
//! `galos-sync` derives the index two ways. One writes the events to Postgres
//! and builds the directory from the rows (`sink::Db`, then
//! `galos_db::index::catch_up`); the other accumulates the events into
//! `galos_index::Galaxy` and publishes the directory from that
//! (`sink::Index`). Both take the same [`Sink`] trait, so this hands one list
//! of readings — each entry and whoever said it, a commander's journal and a
//! published dump both — to each, and compares what they published.
//!
//! ## What is compared, and what is not
//!
//! Everything either side derives about the systems this test owns: the names
//! and places, the populated columns, the reaches, the supercharges, and the
//! whole of every system's bodies — stars, bodies and barycentres, field for
//! field, `updated_by` and `discovered_at` included.
//!
//! Not compared, and each for a stated reason:
//!
//! - **Anything the readings below do not write.** A build reads every row
//!   there is, and the database here is this test's own and holds nothing
//!   else, so what either directory is compared over is named rather than
//!   taken wholesale only because a body file is read per system.
//! - **The names table's row order.** Address-sorted now, both derivations
//!   publishing it through `galos_index::names::Writer` — so the order *is*
//!   an invariant of the format, and what keeps it out of the comparison is
//!   the comparison's own shape: each side is cut down to the systems this
//!   test owns, a handful out of a mapped table, so what is checked is the
//!   content keyed by address. The order *inside* a system is compared:
//!   both derivations write a body file in `id` order, so a system's stars,
//!   bodies and barycentres are compared as lists.
//! - **The cell payloads.** A payload's magnitude and temperature come from
//!   `galos_index::derive::lit` over exactly the stars compared here, and
//!   that function is one copy with tests of its own. What the comparison
//!   would add is the tree's arithmetic, not the derivations' agreement.
//! - **Faction ids.** A journal names factions and numbers none of them; the
//!   ids are `galos_db`'s, minted on write. Argued in
//!   `galos_index::galaxy`'s header and not going away.
//!
//! Needs a server to reach, named by `TEST_DATABASE_URL` as `galos_db`'s
//! write-path tests have it -- a server and not a database, the database
//! being made and dropped by the test -- and stands down without one.

use elite_journal::entry::{Entry, Event};
use galos::sink::{Db, Index, Reporter, Sink};
use galos_db::index::{never, Parts};
use galos_db::testing::Scratch;
use galos_db::Database;
use galos_index::meta::{Boost, PopulatedSystem, SystemBodies};
use galos_index::{FsSource, Source as _};
use spansh::System;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// One of the five addresses this test writes under.
///
/// Fixed, the database being this test's own and holding nothing else. What
/// either directory is compared over is still named rather than taken
/// wholesale, because a body file is read per system: see [`Published`].
fn mine(n: i64) -> i64 {
    910_000_000 + n
}

/// The systems this test owns, which is what either directory is compared
/// over.
fn ours() -> [i64; 5] {
    [mine(1), mine(2), mine(3), mine(4), mine(5)]
}

/// Whoever both sides file a commander's own readings under.
///
/// The same name to both, so `updated_by` can be compared rather than
/// excused: both sinks keep whatever the source named, whichever kind of
/// name it is. Handing one side "cmdr" and the other an anonymised uploader
/// id would be comparing two honest but different answers.
const CMDR: &str = "cmdr";

/// Whoever both sides file [`dumped`]'s readings under.
///
/// Nobody flew a dump: the file it was read out of is the whole of its
/// provenance, and this is the name `galos-sync spansh` hands a sink for
/// one — `sync::from::published`, publisher and file name. A sink that took
/// an uploader for nobody and filed these under
/// `galos_index::galaxy::UNKNOWN` would publish a different `updated_by`
/// from the rows, which is the whole of what makes that column worth
/// comparing.
const DUMP: &str = "Spansh galaxy_agreed.json";

/// A scratch directory of this test's own, emptied first.
fn scratch(name: &str) -> (PathBuf, PathBuf) {
    let root = std::env::temp_dir()
        .join(format!("galos_agree_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).expect("a scratch root");
    (root.join("index"), root.join("index.checkpoint"))
}

/// One entry, read the way a source hands it over: tag and all.
fn entry(json: &str) -> Arc<Entry<Event>> {
    Arc::new(serde_json::from_str(json).expect("the entry should parse"))
}

/// One of each kind of thing a journal says about the sky.
///
/// An arrival with politics, a scan of a star and of a body, a honk's counts,
/// two barycentres, a route, and the three kinds of event that name a system
/// without describing one — a codex sighting, a signal batch and a docking.
/// What a dump says is [`dumped`], and the two together are [`readings`].
fn events() -> Vec<Arc<Entry<Event>>> {
    let [sol, deciat, shinrarta, colonia, routed] = ours();
    vec![
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:00:00Z","event":"FSDJump",
            "StarSystem":"Agreed Sol","SystemAddress":{sol},
            "StarPos":[1.0,2.0,3.0],"Population":22780919531,
            "SystemAllegiance":"Federation","SystemEconomy":"$economy_Refinery;",
            "SystemSecurity":"$SYSTEM_SECURITY_high;"}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:01:00Z","event":"Scan",
            "ScanType":"Detailed","StarSystem":"Agreed Sol",
            "SystemAddress":{sol},"StarPos":[1.0,2.0,3.0],
            "BodyName":"Agreed Sol A","BodyID":0,"StarType":"N","Subclass":2,
            "StellarMass":1.0,"Radius":696000000.0,"AbsoluteMagnitude":4.83,
            "Age_MY":4600,"SurfaceTemperature":5778.0,"Luminosity":"V",
            "RotationPeriod":2000000.0,"AxialTilt":0.0,
            "DistanceFromArrivalLS":0.0,"WasDiscovered":false,
            "WasMapped":false}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:02:00Z","event":"Scan",
            "ScanType":"Detailed","StarSystem":"Agreed Sol",
            "SystemAddress":{sol},"StarPos":[1.0,2.0,3.0],
            "BodyName":"Agreed Sol A 3","BodyID":3,"BodyType":"Planet",
            "Parents":[{{"Star":0}}],"PlanetClass":"Earthlike body",
            "MassEM":1.0,"Radius":6371000.0,"SurfaceGravity":9.8,
            "SemiMajorAxis":1.0,"Eccentricity":0.0167,
            "OrbitalInclination":0.0,"Periapsis":114.2,
            "OrbitalPeriod":31558000.0,"RotationPeriod":86164.0,
            "AxialTilt":0.41,"DistanceFromArrivalLS":499.0,"TidalLock":true,
            "SurfaceTemperature":288.0,"AtmosphereType":"Nitrogen",
            "SurfacePressure":101325.0,"Landable":true,
            "Atmosphere":"thick nitrogen atmosphere",
            "Volcanism":"minor water geysers","TerraformState":"Terraformable",
            "Composition":{{"Ice":0.1,"Rock":0.7,"Metal":0.2}},
            "Materials":[{{"Name":"iron","Percent":20.0}}],
            "WasDiscovered":true,"WasMapped":true}}"#
        )),
        // The same body seen from further off, which must not take away what
        // the closer look found.
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:03:00Z","event":"Scan",
            "ScanType":"AutoScan","StarSystem":"Agreed Sol",
            "SystemAddress":{sol},"StarPos":[1.0,2.0,3.0],
            "BodyName":"Agreed Sol A 3","BodyID":3,"BodyType":"Planet",
            "Parents":[{{"Star":0}}],"PlanetClass":"Earthlike body",
            "MassEM":1.0,"Radius":6371000.0,"SurfaceGravity":9.8,
            "SemiMajorAxis":1.0,"Eccentricity":0.0167,
            "OrbitalInclination":0.0,"Periapsis":114.2,
            "OrbitalPeriod":31558000.0,"RotationPeriod":86164.0,
            "AxialTilt":0.41,"DistanceFromArrivalLS":499.0,
            "WasDiscovered":true,"WasMapped":false}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:04:00Z","event":"ScanBaryCentre",
            "StarSystem":"Agreed Sol","SystemAddress":{sol},
            "StarPos":[1.0,2.0,3.0],"BodyID":1,"SemiMajorAxis":2.0e11,
            "Eccentricity":0.05,"OrbitalInclination":1.0,"Periapsis":90.0,
            "OrbitalPeriod":1.0e9,"AscendingNode":10.0,"MeanAnomaly":20.0}}"#
        )),
        // A second barycentre, so the system holds more than one and the
        // order the two are published in is something to agree about.
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:04:30Z","event":"ScanBaryCentre",
            "StarSystem":"Agreed Sol","SystemAddress":{sol},
            "StarPos":[1.0,2.0,3.0],"BodyID":2,"SemiMajorAxis":3.0e11,
            "Eccentricity":0.1,"OrbitalInclination":2.0,"Periapsis":45.0,
            "OrbitalPeriod":2.0e9,"AscendingNode":20.0,"MeanAnomaly":40.0}}"#
        )),
        // And the first of the two seen again, which rewrites the row it is
        // held in: a system's barycentres are published in `id` order and
        // not in the order the rows were last written.
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:04:45Z","event":"ScanBaryCentre",
            "StarSystem":"Agreed Sol","SystemAddress":{sol},
            "StarPos":[1.0,2.0,3.0],"BodyID":1,"SemiMajorAxis":2.0e11,
            "Eccentricity":0.05,"OrbitalInclination":1.0,"Periapsis":90.0,
            "OrbitalPeriod":1.0e9,"AscendingNode":10.0,"MeanAnomaly":20.0}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:05:00Z","event":"FSSDiscoveryScan",
            "SystemName":"Agreed Sol","SystemAddress":{sol},
            "StarPos":[1.0,2.0,3.0],"BodyCount":40,"NonBodyCount":10,
            "Progress":1.0}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:06:00Z","event":"CodexEntry",
            "System":"Agreed Deciat","SystemAddress":{deciat},
            "StarPos":[10.0,-1.0,-4.0],"EntryID":2100201,
            "Name":"$Codex_Ent_L_Dwarf_Name;"}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:07:00Z",
            "event":"FSSSignalDiscovered","StarSystem":"Agreed Shinrarta",
            "SystemAddress":{shinrarta},"StarPos":[5.0,1.0,2.0],
            "signals":[{{"SignalName":"Jameson Memorial"}}]}}"#
        )),
        // Names a system and never places it, so neither side publishes it.
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:08:00Z","event":"Docked",
            "StarSystem":"Agreed Colonia","SystemAddress":{colonia},
            "StationName":"Jaques Station","StationType":"Orbis",
            "MarketID":3510250752}}"#
        )),
        entry(&format!(
            r#"{{"timestamp":"2026-08-08T12:09:00Z","event":"NavRoute",
            "Route":[{{"StarSystem":"Agreed Route","SystemAddress":{routed},
            "StarPos":[3.0,0.0,3.0],"StarClass":"G"}}]}}"#
        )),
    ]
}

/// One system as a dump lists it, and the scans it stands for.
///
/// What `galos-sync spansh` hands a sink: a line of `galaxy.json`, turned
/// into scans by [`System::scans`]. Nobody flew here, so the file it was
/// read out of is the provenance and both sinks are handed [`DUMP`].
///
/// The star is listed twice, under one name and two `bodyId`s, as the real
/// file lists `39 Omicron Ophiuchi`: one record at the arrival point and one
/// 11342 light seconds off, agreeing to six digits on magnitude and
/// temperature. Two records of one star are one star, and the one kept is
/// the nearer of the two — the far record is the lower `bodyId`, so keeping
/// the nearer is not keeping the first.
fn dumped() -> Vec<Arc<Entry<Event>>> {
    let system: System = serde_json::from_str(&format!(
        r#"{{"id64":{deciat},"name":"Agreed Deciat",
        "coords":{{"x":10.0,"y":-1.0,"z":-4.0}},
        "date":"2026-08-08T12:10:00Z","bodies":[
        {{"bodyId":1,"name":"Agreed Deciat","type":"Star",
        "subType":"F (White) Star","distanceToArrival":11342.859803,
        "age":756,"spectralClass":"F6","luminosity":"IV",
        "absoluteMagnitude":1.858093,"solarMasses":1.363281,
        "solarRadius":1.29641721926671,"surfaceTemperature":7441.0,
        "rotationalPeriod":2.6440361880787,"axialTilt":0.165753,
        "updateTime":"2026-08-08T12:10:00Z"}},
        {{"bodyId":2,"name":"Agreed Deciat","type":"Star",
        "subType":"F (White) Star","distanceToArrival":0.0,
        "age":756,"spectralClass":"F6","luminosity":"IV",
        "absoluteMagnitude":1.858093,"solarMasses":1.363281,
        "solarRadius":1.29641721926671,"surfaceTemperature":7441.0,
        "rotationalPeriod":2.13180385483796,"axialTilt":0.122308,
        "updateTime":"2026-08-08T12:10:00Z"}}]}}"#,
        deciat = mine(2),
    ))
    .expect("the dumped system should parse");
    system.scans().into_iter().map(Arc::new).collect()
}

/// Every reading either sink is handed, and who said it.
///
/// A journal's entries are a commander's own and are filed under their name;
/// a dump's are nobody's and are filed under the file. Each sink is handed
/// the same claim for the same entry, so a sink that anonymises one kind of
/// name and not the other is a disagreement rather than a difference of
/// fixture.
fn readings() -> Vec<(Arc<Entry<Event>>, Reporter<'static>)> {
    let journalled =
        events().into_iter().map(|it| (it, Reporter::Commander(CMDR)));
    let published =
        dumped().into_iter().map(|it| (it, Reporter::Uploader(DUMP)));
    journalled.chain(published).collect()
}

/// Everything a directory publishes about the systems this test owns.
#[derive(Debug, PartialEq)]
struct Published {
    /// Address to name and place. Keyed rather than listed because each
    /// side is cut down to the systems this test owns, which is no part of
    /// either published table's own order.
    names: HashMap<i64, (String, [f32; 3])>,
    populated: HashMap<i64, PopulatedSystem>,
    reaches: HashMap<i64, f32>,
    /// Address to supercharge and place. The place is in the table so a
    /// router needs no other, which makes it something the two derivations
    /// have to agree about.
    boosts: HashMap<i64, (Boost, [f32; 3])>,
    bodies: HashMap<i64, SystemBodies>,
}

impl Published {
    async fn read(dir: &Path) -> Published {
        let source = FsSource::new(dir);
        let owned = ours();
        let ours = |address: &i64| owned.contains(address);

        let table = source.names().await.expect("the names table");
        let names = table
            .addresses()
            .filter(|address| ours(address))
            .filter_map(|address| {
                let entry = table.entry_of(address)?;
                Some((address, (entry.name.to_string(), entry.position)))
            })
            .collect();
        let populated = source
            .populated()
            .await
            .expect("the populated table")
            .into_iter()
            .filter(|it| ours(&it.address))
            .map(|it| {
                (it.address, PopulatedSystem { factions: Vec::new(), ..it })
            })
            .collect();
        let reaches = source
            .reaches()
            .await
            .expect("the reaches table")
            .into_iter()
            .filter(|it| ours(&it.address))
            .map(|it| (it.address, it.reach))
            .collect();
        let boosts = source
            .boosts()
            .await
            .expect("the boosts table")
            .expect("a published boosts table")
            .into_iter()
            .filter(|it| ours(&it.address))
            .map(|it| (it.address, (it.boost, it.position)))
            .collect();

        let mut bodies = HashMap::new();
        for address in owned {
            let inside =
                source.bodies(address).await.expect("a body file reads");
            if inside != SystemBodies::default() {
                bodies.insert(address, inside);
            }
        }

        Published { names, populated, reaches, boosts, bodies }
    }
}

/// One list of readings, written to Postgres and built from the rows.
async fn from_the_database(db: &Database, dir: &Path, checkpoint: &Path) {
    let mut sink = Db::new(db.clone());
    for (entry, by) in readings() {
        sink.entry(entry, by).await;
    }
    sink.flush().await.expect("the database sink flushes");

    let stop = || false;
    galos_db::index::catch_up(db, dir, checkpoint, Parts::ALL, &stop)
        .await
        .expect("the index should build");
}

/// The same readings, accumulated and published with no database.
async fn from_the_events(dir: &Path, checkpoint: &Path) {
    let mut sink = Index::open(dir, checkpoint, None, never())
        .expect("the index sink opens");
    for (entry, by) in readings() {
        sink.entry(entry, by).await;
    }
    sink.finish().await.expect("the index sink finishes");
}

/// The two derivations publish the same galaxy
///
/// The one test that would have caught every drift found so far: four events
/// writing a positioned row on one side and nothing at all on the other, a
/// published row withdrawn for want of hearing about a system, an event's
/// thin row written whole over a build's rich one, one star listed twice and
/// so counted twice, a system's rows published in the order they were last
/// written rather than in `id` order, and an uploader filed under nobody.
#[async_std::test]
async fn both_derivations_publish_the_same_galaxy() {
    let Some(db) = Scratch::new().await else { return };

    let (built, built_at) = scratch("built");
    let (heard, heard_at) = scratch("heard");
    from_the_database(&db, &built, &built_at).await;
    from_the_events(&heard, &heard_at).await;

    let from_rows = Published::read(&built).await;
    let from_events = Published::read(&heard).await;

    // Named first, because a difference here explains every other one: a
    // system neither side placed is published by neither, and a system one
    // side never heard of is missing from all four of its tables.
    assert_eq!(
        from_rows.names, from_events.names,
        "the two derivations disagree about which systems exist",
    );
    assert!(
        !from_events.names.contains_key(&mine(4)),
        "a system a docking never placed was published",
    );
    assert_eq!(from_events.names.len(), 4, "a system went missing");

    assert_eq!(from_rows.populated, from_events.populated);
    assert_eq!(from_rows.reaches, from_events.reaches);
    assert_eq!(from_rows.boosts, from_events.boosts);
    assert_eq!(from_rows.bodies, from_events.bodies);

    // And the fixture reaches what it claims to: a neutron star's
    // supercharge, a body that kept what only the detailed scan said, and
    // two barycentres nothing draws but everything inside the system is
    // placed about.
    assert_eq!(from_events.boosts.len(), 1, "the neutron star was not read");
    // Against the place the *reading* carried, not against the names
    // table's: that table stopped holding positions when a name became a
    // function of its address, and what it answers with now is the middle
    // of the boxel the address names. The supercharge table is one of the
    // tables that still carries an exact place, because the router reads
    // it to find where four million jet cones sit.
    assert_eq!(
        from_events.boosts[&mine(1)],
        (Boost::Neutron, [1.0, 2.0, 3.0]),
        "the supercharge is published somewhere the system is not",
    );
    let inside = &from_events.bodies[&mine(1)];
    assert_eq!(inside.stars.len(), 1);
    // Both sides by name, rather than one and the equality above: the rows
    // are ordered by `id` in SQL and the events are filed in the order they
    // arrived, and a body file in any other order is two body files.
    for side in [&from_rows, &from_events] {
        let inside = &side.bodies[&mine(1)];
        assert_eq!(
            inside.barycenters.iter().map(|it| it.id).collect::<Vec<_>>(),
            vec![1, 2],
            "the barycentres are not published in id order",
        );
    }
    assert!(
        inside.bodies[0].surface.is_some(),
        "the basic rescan took the surface away",
    );
    assert!(inside.bodies[0].mapped, "the basic rescan unmapped the body");

    // And the dumped system's star, listed twice, is one star: the record
    // kept is the one at the arrival point rather than the lower `bodyId`
    // eleven thousand light seconds off, and it is filed under the file it
    // was published in rather than under nobody.
    let deciat = &from_events.bodies[&mine(2)];
    assert_eq!(deciat.stars.len(), 1, "one star was published twice");
    assert_eq!(deciat.stars[0].id, 2, "the record kept is not the nearer");
    assert_eq!(
        deciat.stars[0].distance_from_arrival_ls, 0.0,
        "the record kept is not the one at the arrival point",
    );
    assert_eq!(
        deciat.stars[0].updated_by, DUMP,
        "an uploader was taken for nobody",
    );

    for (dir, _) in [(built, built_at), (heard, heard_at)] {
        let _ = std::fs::remove_dir_all(dir.parent().expect("a scratch root"));
    }

    db.done().await;
}
