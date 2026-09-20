//! Events, accumulated into what the index wants.
//!
//! The index has two inputs and neither names a source.
//! [`Build`](crate::Build) takes records - a database's rows, a dump's
//! lines - and builds the whole tree at once. This takes events, one at a
//! time, as they arrive: EDDN's feed, a commander's own `.log` files, a
//! relay. What reaches it is an [`Entry<Event>`] and nothing about where it
//! came from.
//!
//! The peer of `galos_db::index`, which derives the same tree and metadata
//! from Postgres rows already merged from every commander. This derives them
//! from the events themselves, so a body scanned in the game is on the map a
//! second later without a database having been asked.
//!
//! The fan-out is not merely the same one `galos_db::record`
//! does - it *is* that one. Both sides call [`SystemReport::of`], because a
//! scan is a scan whether it arrives off a socket or off a disk, and the two
//! derivations had already disagreed once about which events name a system.
//! What differs is where it lands: `galos_db::record` writes fourteen tables and a
//! build reads them back, while this keeps the two shapes the index wants -
//! [`crate::System`] and the metadata records - and skips the round trip.
//!
//! Narrower below system level, and not above it. `galos_db::record` also records
//! dockings, settlements, body signals and codex entries, and the index has
//! a column for none of them: a signal is what is written on a surface, a
//! codex entry is a sighting and a settlement is a station, and the map
//! draws none of the three. But every one of those events names the system
//! it happened in, which is what a [`SystemReport`] is, so this holds the
//! system and drops the thing.
//!
//! ## What a second look does
//!
//! Nothing here. Both rules live beside this one, where the database's side
//! of the program can be held against them: [`SystemReport::over`] for a
//! system's own columns and [`crate::merge`] for the things inside it.
//! Between them they are the write path's `ON CONFLICT DO UPDATE` clauses
//! stated in Rust — a reading wins where a scan is one, what a scan does
//! not state leaves what stands, and the two facts about history only ever
//! go one way.
//!
//! ## What an event cannot say
//!
//! Three gaps, all of them stated here rather than papered over, because a
//! layer that quietly answered them wrongly would be worse than one that says
//! nothing:
//!
//! - **Factions have no ids.** A faction's numeric id is `galos_db`'s, minted
//!   when the row is first written; an event names factions and numbers
//!   nothing. So [`Galaxy`] publishes no faction table and leaves
//!   [`PopulatedSystem::factions`] empty, and a system's factions keep coming
//!   from whatever is underneath. Filling that column with ids of its own
//!   would collide with real ones and colour the map by the wrong faction.
//! - **A system's own row is a sighting.** A database row is everything
//!   anybody has reported about a system; a row here is what the events fed
//!   in happened to say — one commander's travels off their own `.log`
//!   files, everyone's off a feed — plus the stops on a route they plotted.
//! - **Most systems have no scanned star.** The published build falls back to
//!   a `primary_star_class` column when nothing is scanned. No event
//!   carries one: there are the classes that have been scanned and the ones
//!   a plotted route names, and past those the default stands in — exactly
//!   as it does for a system EDDN knows nothing about either.
//!
//! ## Time
//!
//! The Recency axis is measured from a clock this derivation holds
//! ([`Galaxy::now`]) rather than from a database's. That is the same rule
//! `galos_db::index` follows for the same reason — one clock compared against
//! itself — and here it is simpler, the events carrying their own timestamps
//! and no upsert standing between them and the reading.

use crate::bodies::{Bodies, Kept};
// The Recency edges and the bucketing over them are kept once, in
// [`crate::derive`], rather than named again here. How many buckets
// there are is part of the published format — a cell aggregate is a count
// per bucket and the client's Recency control indexes straight into them —
// so this derivation and the database's must read the same edges or one
// galaxy bins itself two ways.
use crate::derive;
use crate::merge;
use crate::meta::{
    Boost, NameEntry, PopulatedSystem, StarKind, SystemBodies, SystemBoost,
    SystemReach,
};
use crate::report::SystemReport;
use crate::tree::System;
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::{Scan, ScanTarget};
use elite_journal::entry::{Entry, Event};
use galos_photometry::{Magnitude, Temperature};
use std::collections::{HashMap, HashSet};

/// Who a reading is filed under when nothing names anybody.
///
/// An `ingest --from journal` files them under the same word, and for the same
/// reason: nothing said who and that is the whole of the claim. Not what a
/// reading nobody *flew* is filed under — a published file says which file
/// it was, and `updated_by` is provenance rather than a claim about a
/// person.
pub const UNKNOWN: &str = "unknown";

/// Everything the events have said, in the shapes the index wants.
///
/// The systems table is held whole in memory. A commander's journal is
/// thousands of systems where the galaxy is a hundred and twenty-nine
/// million, so it is megabytes rather than gigabytes and every derivation
/// over it is a pass that costs nothing worth measuring — which is why a
/// source over this rebuilds rather than editing, and why there is no
/// checkpoint, no cursor and no incremental publish anywhere in this
/// derivation. The bodies, which is the part a feed makes unbounded, are
/// behind [`Bodies`] rather than here.
#[derive(Debug)]
pub struct Galaxy {
    /// What dates the Recency reading. Set once by the caller per pass, so
    /// every system in one publish is aged against the same moment.
    now: DateTime<Utc>,
    /// Who what arrives is filed under: a commander, as the last
    /// `Commander` or `LoadGame` said, or whoever a caller named.
    by: String,
    /// Every system anything has reported, merged down to one report each.
    ///
    /// The same type a source hands in, which is the point: what is held and
    /// what arrives are one shape, so the merge is
    /// [`SystemReport::over`] and there is no accumulator of this module's
    /// own for the database's half to drift away from.
    systems: HashMap<i64, SystemReport>,
    /// Where the things scanned inside a system are kept.
    ///
    /// Behind a trait because the answer differs by who is asking, and the
    /// difference is a gigabyte: see [`crate::bodies`]. Nothing in here knows
    /// which store it has.
    inside: Box<dyn Bodies>,
    /// Systems touched since the last [`Galaxy::settle`].
    touched: HashSet<i64>,
}

impl Default for Galaxy {
    fn default() -> Galaxy {
        Galaxy::new(Utc::now())
    }
}

impl Galaxy {
    /// An empty galaxy, aged against `now`, keeping what it scans in memory.
    pub fn new(now: DateTime<Utc>) -> Galaxy {
        Galaxy::keeping(now, Box::new(Kept::new()))
    }

    /// An empty galaxy that keeps what it scans in `inside`.
    ///
    /// The one thing worth choosing about a galaxy. A feed carries everyone's
    /// scans and holding them all is a process that grows for as long as it
    /// runs; a directory-backed store makes the published body files the only
    /// copy. See [`crate::bodies`].
    pub fn keeping(now: DateTime<Utc>, inside: Box<dyn Bodies>) -> Galaxy {
        Galaxy {
            now,
            by: UNKNOWN.to_string(),
            systems: HashMap::new(),
            inside,
            touched: HashSet::new(),
        }
    }

    /// Make durable whatever the store is holding, answering how many body
    /// files were written since this was last called.
    ///
    /// Nothing where the store is memory. Called on the beat whoever owns
    /// the galaxy publishes on. A store that forced a flush of its own in
    /// between is counted here too, so the answer is what the publish wrote
    /// and not what was left to write at the end of it.
    pub fn settle_bodies(&mut self) -> std::io::Result<usize> {
        self.inside.flush()?;
        Ok(self.inside.written())
    }

    /// Date the Recency reading from `now` from here on.
    ///
    /// A watch running for hours would otherwise go on measuring every
    /// system's age from the moment it started, and the youngest bucket is a
    /// day wide.
    pub fn dated(&mut self, now: DateTime<Utc>) {
        self.now = now;
    }

    /// Who a reading arriving now is filed under.
    pub fn by(&self) -> &str {
        &self.by
    }

    /// File what arrives from here on under `who`.
    ///
    /// For a caller that knows better than the events do. Reading a journal
    /// directly, [`Self::read`] answers this out of the `Commander` and
    /// `LoadGame` events the files carry; a caller taking several sources
    /// into one galaxy has to say, because EDDN carries neither of those
    /// events and an accumulator left to its own devices would go on filing
    /// everybody's scans under whoever was flying locally.
    ///
    /// A publisher is a name as much as a commander is: nobody flew a dump
    /// and the file it was read out of is what its bodies are filed under.
    /// What nothing named at all is [`UNKNOWN`].
    pub fn reported_by(&mut self, who: &str) {
        if self.by != who {
            self.by = who.to_string();
        }
    }

    /// How many systems have been named.
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Whether nothing has been named at all.
    pub fn is_empty(&self) -> bool {
        self.systems.is_empty()
    }

    /// Which systems have been touched since the last [`Self::settle`].
    pub fn touched(&self) -> &HashSet<i64> {
        &self.touched
    }

    /// Forget what has been touched, a publish having taken it.
    pub fn settle(&mut self) {
        self.touched.clear();
    }

    /// Read everything one entry has to say, answering whether it said
    /// anything.
    ///
    /// An entry with nothing in it for the index is passed over rather than
    /// refused: the events are mostly combat, cargo and docking, and none of
    /// that is a fact about the sky.
    ///
    /// Which events name a system, and what each of them says about it, is
    /// [`SystemReport::of`]'s answer rather than this function's — the same
    /// answer `galos db ingest`'s write path gets, which is what stops the two
    /// derivations from disagreeing about which systems exist. What is left
    /// here is the three things a report does not carry: a route's several
    /// systems, the things scanned inside one, and who is flying.
    pub fn read(&mut self, entry: &Entry<Event>) -> bool {
        let at = entry.timestamp;
        let reported = match SystemReport::of(entry) {
            Some(report) => {
                self.hear(report);
                true
            }
            None => false,
        };

        match &entry.event {
            // The one event that states a system per stop rather than one,
            // which is the whole reason it is not an arm of
            // [`SystemReport::of`].
            Event::NavRoute(route) => {
                for stop in &route.destinations {
                    self.hear(SystemReport::plotted(at, stop));
                }
                return reported || !route.destinations.is_empty();
            }
            // What the scan looked at. The system it looked at it from is
            // the report above.
            Event::Scan(scan) => self.scan(at, scan),
            // A barycentre is not a body and is not drawn. It is kept so
            // that a body naming it as an ancestor can be placed where it
            // belongs rather than at the middle of its system.
            Event::ScanBaryCentre(center) => {
                let by = self.by.clone();
                self.inside.edit(center.system_address, &mut |inside| {
                    merge::put(
                        &mut inside.barycenters,
                        center.body_id,
                        |it| it.id,
                        |held| merge::barycenter(center, at, &by, held),
                    )
                });
            }
            // Who is flying, which is who a scan is filed under. Not a fact
            // about the sky, so neither of these says anything on its own.
            Event::Commander(who) => self.by = who.name.clone(),
            Event::LoadGame(game) => {
                if let Some(who) = &game.commander {
                    self.by = who.name.clone();
                }
            }
            _ => {}
        }
        reported
    }

    /// Take what a report says over whatever has been said about the system
    /// before.
    ///
    /// Every way into this galaxy above body level. An event arrives as a
    /// report through [`Self::read`]; EDSM's and EDDB's dumps arrive as one
    /// straight from a sink, which is not an event and never was — nobody
    /// flew anywhere, a file was published — and states the same columns
    /// somebody else read off months ago.
    ///
    /// The merge is [`SystemReport::over`], which is the write path's `ON
    /// CONFLICT DO UPDATE` stated in Rust: an arrival delivered after a scan
    /// does not lose the politics only it carries, a dump does not overwrite
    /// what a commander saw yesterday, and a docking moves nothing but the
    /// clock.
    ///
    /// A report's own `at` is when the reading was taken rather than when it
    /// arrived, which is what the Recency axis wants: an EDDB dump lands in
    /// the oldest bucket where it belongs rather than looking like news.
    pub fn hear(&mut self, said: SystemReport) {
        let address = said.address;
        self.touched.insert(address);
        match self.systems.get_mut(&address) {
            Some(held) => held.over(said),
            None => {
                self.systems.insert(address, said);
            }
        }
    }

    /// Whatever a scan looked at, filed inside the system it was seen from.
    ///
    /// The rule is [`crate::merge`]'s, which is the write path's:
    /// a reading wins where the scan is one, and what a basic `AutoScan`
    /// does not mention leaves what a closer look found.
    fn scan(&mut self, at: DateTime<Utc>, scan: &Scan) {
        let address = scan.system_address;
        let by = self.by.clone();
        let found = merge::discovered_at(scan, at);
        match &scan.target {
            ScanTarget::Star(star) => {
                self.inside.edit(address, &mut |inside| {
                    merge::put(
                        &mut inside.stars,
                        star.id,
                        |it| it.id,
                        |held| merge::star(address, star, at, &by, found, held),
                    )
                })
            }
            ScanTarget::Body(body) => {
                self.inside.edit(address, &mut |inside| {
                    merge::put(
                        &mut inside.bodies,
                        body.id,
                        |it| it.id,
                        |held| merge::body(address, body, at, &by, found, held),
                    )
                })
            }
            // A belt cluster and a ring are numbered bodies of the system and
            // neither has a record in [`SystemBodies`], which carries what the
            // map draws inside a system. The scan still counts as having been
            // in the system, which its report has already recorded.
            ScanTarget::Cluster(_) | ScanTarget::Ring(_) => {}
        }
    }

    /// Every placed system, as the tree takes them.
    ///
    /// A system nothing ever carried a `StarPos` for is left out: the
    /// tree is built on position and there is nowhere to put one. That is
    /// rarer than it sounds — the game writes `StarPos` on the arrival event
    /// and on every scan — and a system named only by an event that omitted
    /// it is one the map has nothing to draw for anyway.
    pub fn systems(&self) -> Vec<System> {
        self.systems
            .keys()
            .filter_map(|&address| self.system_of(address))
            .collect()
    }

    /// One system as the tree takes it, where it has been placed.
    ///
    /// What a sink following a feed asks, against the handful of addresses a
    /// pass touched, rather than deriving the whole galaxy to publish fifty
    /// systems. [`Self::systems`] is this over everything.
    pub fn system_of(&self, address: i64) -> Option<System> {
        let report = self.systems.get(&address)?;
        Some(self.system(report, report.placed()?))
    }

    /// One system's name and place, where it has been named and placed.
    ///
    /// Nothing for a system nothing named. A `NavBeaconScan` carries a
    /// position and an optional name, so a system known only from one may be
    /// placed and nameless — and a blank name in this table is a blank row
    /// in whatever searches it, which the map does. The tree still draws it
    /// out of the position it did carry.
    pub fn name_of(&self, address: i64) -> Option<NameEntry> {
        let report = self.systems.get(&address)?;
        let at = report.placed()?;
        Some(NameEntry {
            address,
            name: report.named()?.clone(),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
        })
    }

    /// How far one system reaches, where anything in it has been scanned.
    pub fn reach_of(&self, address: i64) -> Option<f32> {
        self.inside.read(address).extent(address)
    }

    /// What one system's arrival star can supercharge, and where it sits.
    ///
    /// Nothing for a system nothing has placed: the published table is what
    /// a router reads, and a cone with no place is no waypoint. Which is
    /// the same rule the database derivation's `placed` carries.
    pub fn boost_of(&self, address: i64) -> Option<SystemBoost> {
        let boost = Boost::of(&self.arrival_class(address)?)?;
        let at = self.systems.get(&address)?.placed()?;
        Some(SystemBoost {
            address,
            boost,
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
        })
    }

    /// One system's political columns, where anybody lives in it.
    ///
    /// [`SystemReport::populated`], which is the projection both
    /// derivations publish through.
    pub fn populated_of(&self, address: i64) -> Option<PopulatedSystem> {
        self.systems.get(&address)?.populated()
    }

    /// One system's photometry and place, by the same fallback chain the
    /// published build uses.
    ///
    /// [`derive::lit`]'s chain, which is the one both derivations of the
    /// index answer with: its scanned stars if it has any, failing that the
    /// arrival star's class, which only a plotted route states here, failing
    /// that the default M dwarf the galaxy is mostly made of.
    fn system(&self, report: &SystemReport, position: [f64; 3]) -> System {
        let inside = self.inside.read(report.address);
        // The scanned magnitude is bolometric — the star's whole output as
        // one figure — so it is turned into the visual magnitude the sky
        // sees before anything sums it. That is where a white dwarf keeps
        // its faint scanned brightness and a neutron star falls to nothing,
        // and it is the one step that would be easy to leave out and
        // impossible to see the absence of.
        let stars = inside.stars.iter().map(|star| {
            let t = star.temperature as f64;
            let m = Magnitude(star.absolute_magnitude as f64);
            (m.visual(Temperature(t)).0, t)
        });
        let (absolute_magnitude, temperature) =
            derive::lit(stars, report.star_class.as_deref().unwrap_or(""));
        let (age_bucket, updated_at) =
            derive::updated(report.at.naive_utc(), self.now.naive_utc());
        System {
            id64: report.address as u64,
            position,
            absolute_magnitude,
            temperature,
            age_bucket,
            updated_at,
            // The arrival star, by the same rule the boost table is derived
            // by: nearest the drop point, ties by body id, and the class a
            // plotted route named where nothing has been scanned. Nothing
            // said reads as nothing said — see [`StarKind::Unknown`].
            kind: self
                .arrival_class(report.address)
                .map_or(StarKind::Unknown, |class| StarKind::of(&class)),
        }
    }

    /// Every placed system's name and where it sits.
    pub fn names(&self) -> Vec<NameEntry> {
        let mut table: Vec<NameEntry> =
            self.systems.keys().filter_map(|&a| self.name_of(a)).collect();
        table.sort_by_key(|entry| entry.address);
        table
    }

    /// How far each scanned system reaches, in metres.
    ///
    /// [`crate::inside`]'s answer, which is the same call the published
    /// build makes: the map sizes a system by this and draws the inside of it
    /// from the same records, and a shell smaller than the orbits it contains
    /// is the one thing a reach cannot be.
    pub fn reaches(&self) -> Vec<SystemReach> {
        let mut table: Vec<SystemReach> = self
            .inside
            .scanned()
            .into_iter()
            .filter_map(|address| {
                Some(SystemReach { address, reach: self.reach_of(address)? })
            })
            .collect();
        table.sort_by_key(|it| it.address);
        table
    }

    /// Which systems can supercharge a drive, and on what.
    ///
    /// Off the arrival star's class, which is the one that matters: a ship
    /// drops in at the main star and can reach its jet cone without crossing
    /// the system. The arrival star is the scanned star standing nearest the
    /// drop point; where nothing has been scanned the route file's class
    /// stands in, that being a statement about the same star.
    pub fn boosts(&self) -> Vec<SystemBoost> {
        let mut table: Vec<SystemBoost> =
            self.systems.keys().filter_map(|&a| self.boost_of(a)).collect();
        table.sort_by_key(|it| it.address);
        table
    }

    /// The class of the star a ship drops in at, as far as anything says.
    ///
    /// [`derive::arrival_class`] over what has been scanned, and where
    /// nothing has been the class a plotted route named, that being the only
    /// other statement about the same star.
    fn arrival_class(&self, address: i64) -> Option<String> {
        let inside = self.inside.read(address);
        derive::arrival_class(&inside)
            .map(str::to_owned)
            .or_else(|| self.systems.get(&address)?.star_class.clone())
    }

    /// The systems anybody lives in, with the political columns a colour and a
    /// filter are read from.
    ///
    /// `factions` is empty here and always will be; see the module header.
    pub fn populated(&self) -> Vec<PopulatedSystem> {
        let mut table: Vec<PopulatedSystem> =
            self.systems.keys().filter_map(|&a| self.populated_of(a)).collect();
        table.sort_by_key(|it| it.address);
        table
    }

    /// What has been scanned inside a system, empty where nothing has.
    pub fn bodies(&self, address: i64) -> SystemBodies {
        self.inside.read(address).into_owned()
    }

    /// Every system with anything scanned in it.
    pub fn scanned(&self) -> Vec<i64> {
        self.inside.scanned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::entry::Entry;
    use elite_journal::prelude::{Allegiance, Economy};
    use galos_photometry::ClassLight;

    /// One entry, read the way the follower hands it over: tag and all.
    fn entry(json: &str) -> Entry<Event> {
        serde_json::from_str(json).expect("the entry should parse")
    }

    /// A galaxy aged from a fixed moment, so a Recency bucket is a fact about
    /// the fixture rather than about when the suite was run.
    fn galaxy() -> Galaxy {
        Galaxy::new(
            "2026-08-08T12:00:00Z".parse().expect("a moment to date from"),
        )
    }

    const JUMP: &str = r#"{
        "timestamp": "2026-08-08T12:00:00Z",
        "event": "FSDJump",
        "StarSystem": "Sol",
        "SystemAddress": 10477373803,
        "StarPos": [0.0, 0.0, 0.0],
        "Population": 22780919531,
        "SystemAllegiance": "Federation",
        "SystemEconomy": "$economy_Refinery;",
        "SystemSecurity": "$SYSTEM_SECURITY_high;"
    }"#;

    /// A scan of the star at the middle of Sol, as the game writes one.
    fn star_scan(class: &str, magnitude: f64, temperature: f64) -> String {
        format!(
            r#"{{
                "timestamp": "2026-08-08T12:01:00Z",
                "event": "Scan",
                "ScanType": "Detailed",
                "StarSystem": "Sol",
                "SystemAddress": 10477373803,
                "StarPos": [0.0, 0.0, 0.0],
                "BodyName": "Sol",
                "BodyID": 0,
                "StarType": "{class}",
                "Subclass": 2,
                "StellarMass": 1.0,
                "Radius": 696000000.0,
                "AbsoluteMagnitude": {magnitude},
                "Age_MY": 4600,
                "SurfaceTemperature": {temperature},
                "Luminosity": "V",
                "RotationPeriod": 2000000.0,
                "AxialTilt": 0.0,
                "DistanceFromArrivalLS": 0.0,
                "WasDiscovered": true,
                "WasMapped": false
            }}"#
        )
    }

    /// A scan of a planet in Sol, detailed or basic.
    ///
    /// The two shapes the game really writes: a close look, carrying the
    /// surface block, the materials on it, the tidal lock and the
    /// temperature; and the `AutoScan` it writes every time a ship
    /// re-enters a system it has already looked at, carrying none of them.
    fn body_scan(detailed: bool, at: &str, discovery: &str) -> String {
        let close = r#"
                "TidalLock": true,
                "SurfaceTemperature": 288.0,
                "AtmosphereType": "Nitrogen",
                "SurfacePressure": 101325.0,
                "Landable": true,
                "Atmosphere": "thick nitrogen atmosphere",
                "Volcanism": "minor water geysers",
                "TerraformState": "Terraformable",
                "Composition": { "Ice": 0.1, "Rock": 0.7, "Metal": 0.2 },
                "Materials": [{ "Name": "iron", "Percent": 20.0 }],"#;
        format!(
            r#"{{
                "timestamp": "{at}",
                "event": "Scan",
                "ScanType": "{}",
                "StarSystem": "Sol",
                "SystemAddress": 10477373803,
                "StarPos": [0.0, 0.0, 0.0],
                "BodyName": "Sol 3",
                "BodyID": 3,
                "BodyType": "Planet",
                "Parents": [{{ "Star": 0 }}],
                "PlanetClass": "Earthlike body",
                "MassEM": 1.0,
                "Radius": 6371000.0,
                "SurfaceGravity": 9.8,
                "SemiMajorAxis": 1.0,
                "Eccentricity": 0.0167,
                "OrbitalInclination": 0.0,
                "Periapsis": 114.2,
                "OrbitalPeriod": 31558000.0,
                "RotationPeriod": 86164.0,
                "AxialTilt": 0.41,
                "DistanceFromArrivalLS": 499.0,
                {}
                {discovery}
            }}"#,
            if detailed { "Detailed" } else { "AutoScan" },
            if detailed { close } else { "" },
        )
    }

    /// A jump names a system, places it, and says who runs it
    #[test]
    fn a_jump_is_a_system() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(JUMP)));

        let systems = galaxy.systems();
        assert_eq!(systems.len(), 1);
        assert_eq!(systems[0].id64, 10477373803);
        assert_eq!(systems[0].position, [0.0, 0.0, 0.0]);

        let names = galaxy.names();
        assert_eq!(names.len(), 1);
        assert_eq!(
            names[0].name, "SOL",
            "the index spells a system the way `galos_db` writes it",
        );

        let populated = galaxy.populated();
        assert_eq!(populated.len(), 1);
        assert_eq!(populated[0].population, 22780919531);
        assert!(
            populated[0].factions.is_empty(),
            "a journal numbered a faction it cannot number",
        );
    }

    /// A system nothing has been scanned in falls to the default light
    ///
    /// Two thirds of the published galaxy is in that state and this is more
    /// of it: a journal has no `primary_star_class` column to fall back on, so
    /// a system arrived in and not scanned is drawn as the M dwarf most
    /// systems turn out to be.
    #[test]
    fn an_unscanned_system_falls_to_the_default_class() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        let default = ClassLight::of("");
        assert_eq!(
            galaxy.systems()[0].absolute_magnitude,
            default.absolute_magnitude.0,
        );
    }

    /// A scanned star's magnitude is turned from bolometric to visual
    ///
    /// The game reports a star's whole output as one figure; the sky sees the
    /// part of it that is light. Left out, every hot star on the map is
    /// brighter than it is and a white dwarf badly so — and nothing about the
    /// picture would say which step was missing.
    #[test]
    fn a_scan_is_read_as_the_sky_sees_it() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&star_scan("G", 4.83, 5778.0)));

        let system = &galaxy.systems()[0];
        let visual = Magnitude(4.83).visual(Temperature(5778.0)).0;
        // Not exact, and the slack is the record rather than the arithmetic:
        // a scanned magnitude is held as the `f32` the database column holds
        // and read back out as an `f64`. The bolometric figure is a third of a
        // magnitude away here, which is thousands of times this.
        assert!(
            (system.absolute_magnitude - visual).abs() < 1e-5,
            "the scanned magnitude went in bolometric: {} against {visual}",
            system.absolute_magnitude,
        );
        assert_eq!(system.temperature, 5778.0);
    }

    /// The same body scanned twice is one body, and the later look wins
    #[test]
    fn a_rescan_replaces_rather_than_repeats() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&star_scan("G", 4.83, 5778.0)));
        galaxy.read(&entry(&star_scan("G", 4.83, 6000.0)));

        let inside = galaxy.bodies(10477373803);
        assert_eq!(inside.stars.len(), 1, "one star was filed twice");
        assert_eq!(inside.stars[0].temperature, 6000.0);
    }

    /// A basic scan after a detailed one keeps what it does not say
    ///
    /// The game writes an `AutoScan` every time a ship re-enters a system it
    /// has already looked at closely: ordered, later, and poorer. Replacing
    /// the record with it took away the surface, what can be picked up off
    /// it, the tidal lock and the temperature — everything the closer look
    /// was for.
    #[test]
    fn a_basic_scan_after_a_detailed_one_keeps_what_it_does_not_say() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        assert!(galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:01:00Z",
            r#""WasDiscovered": true, "WasMapped": true"#,
        ))));
        assert!(galaxy.read(&entry(&body_scan(
            false,
            "2026-08-08T12:05:00Z",
            r#""WasDiscovered": true, "WasMapped": false"#,
        ))));

        let inside = galaxy.bodies(10477373803);
        assert_eq!(inside.bodies.len(), 1, "one body was filed twice");
        let body = &inside.bodies[0];
        let surface = body.surface.as_ref().expect("the surface stands");
        assert_eq!(
            surface.materials.len(),
            1,
            "what can be picked up off it went with the basic scan",
        );
        assert_eq!(surface.pressure, 101325.0);
        assert!(body.tidal_lock, "a basic scan turned the body loose");
        assert_eq!(body.temperature, Some(288.0));
        assert!(body.mapped, "a rescan unmapped a mapped body");
    }

    /// Having been mapped is a fact about the galaxy, not a reading
    ///
    /// A scan that finds a star already mapped is knowledge; one that finds
    /// it unmapped is not evidence that it has stopped being so. The
    /// database's `was_mapped OR $n`, in the vocabulary this publishes.
    #[test]
    fn a_rescan_does_not_unmap() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:01:00Z",
            r#""WasDiscovered": true, "WasMapped": true"#,
        )));
        galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:05:00Z",
            r#""WasDiscovered": true, "WasMapped": false"#,
        )));

        assert!(galaxy.bodies(10477373803).bodies[0].mapped);
    }

    /// The earliest claim on a discovery is the one that stands
    ///
    /// A scan saying the body was already found says nothing about when, and
    /// its silence must not erase the scan that *was* the discovery.
    #[test]
    fn the_earliest_discovery_wins() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:01:00Z",
            r#""WasDiscovered": false, "WasMapped": false"#,
        )));
        galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:05:00Z",
            r#""WasDiscovered": true, "WasMapped": false"#,
        )));

        let found = galaxy.bodies(10477373803).bodies[0].discovered_at;
        assert_eq!(
            found.map(|at| at.to_rfc3339()),
            Some("2026-08-08T12:01:00+00:00".to_string()),
            "a later scan's silence took the discovery away",
        );
    }

    /// A scan arriving late overwrites the readings and not the clock
    ///
    /// Both halves of the rule in one place. EDDN carries scans from
    /// commanders in no order, and a journal directory holds sessions
    /// restored out of order: what a scan measured is taken as it comes,
    /// since that is what the database does with those columns, but the
    /// stamp the map reads as "updated" only ever goes forward.
    #[test]
    fn a_late_scan_does_not_put_the_clock_back() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&star_scan("G", 4.83, 6000.0)));
        // The same star, measured colder, by an entry stamped earlier.
        let older =
            star_scan("G", 4.83, 5778.0).replace("12:01:00Z", "12:00:30Z");
        galaxy.read(&entry(&older));

        let star = &galaxy.bodies(10477373803).stars[0];
        assert_eq!(
            star.temperature, 5778.0,
            "a reading is a reading whenever it arrives",
        );
        assert_eq!(
            star.updated_at.to_rfc3339(),
            "2026-08-08T12:01:00+00:00",
            "a late scan rewound the stamp the map reads",
        );
    }

    /// A scan that names no ancestor keeps the chain that stands
    ///
    /// What places a body inside its system. A scan carrying no `Parents` —
    /// which is what a primary's is, and what some uploaders strip — must
    /// not take the ancestry a fuller scan recorded.
    #[test]
    fn a_scan_that_names_no_ancestor_keeps_the_chain() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&body_scan(
            true,
            "2026-08-08T12:01:00Z",
            r#""WasDiscovered": true, "WasMapped": false"#,
        )));
        let orphaned = body_scan(
            true,
            "2026-08-08T12:05:00Z",
            r#""WasDiscovered": true, "WasMapped": false"#,
        )
        .replace(r#""Parents": [{ "Star": 0 }],"#, r#""Parents": [],"#);
        galaxy.read(&entry(&orphaned));

        let body = &galaxy.bodies(10477373803).bodies[0];
        assert_eq!(
            body.parents.len(),
            1,
            "a scan naming no ancestor took the chain away",
        );
    }

    /// A scan arriving after a jump does not take the jump's politics away
    ///
    /// The merge rule, and the one that would go unnoticed: a `Scan` names its
    /// system and says nothing about who runs it, so a system replaced rather
    /// than merged loses its population the moment anything in it is looked
    /// at.
    #[test]
    fn a_later_event_does_not_forget_an_earlier_one() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        galaxy.read(&entry(&star_scan("G", 4.83, 5778.0)));
        assert_eq!(galaxy.populated().len(), 1, "the population was dropped");
    }

    /// A jet cone is read off the star a ship drops in at
    #[test]
    fn a_neutron_star_supercharges() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        assert!(galaxy.boosts().is_empty(), "an unscanned system supercharged");

        galaxy.read(&entry(&star_scan("N", 12.0, 100_000.0)));
        let boosts = galaxy.boosts();
        assert_eq!(boosts.len(), 1);
        assert_eq!(boosts[0].boost, Boost::Neutron);
    }

    /// The class survives a trip through the byte a payload carries
    ///
    /// Which is the whole point of [`crate::meta::StarKind`]: a router asks
    /// what kind of star every system it expands has, and the answer has to
    /// be a byte beside a position rather than a lookup in a table of
    /// ninety-five million. So every kind has to come back out of its code
    /// as what went in, and a code this build does not know has to read as
    /// nothing having been said rather than as some other star.
    #[test]
    fn a_star_kind_goes_through_a_byte_unchanged() {
        use crate::meta::StarKind;

        for class in [
            "O",
            "B",
            "A",
            "F",
            "G",
            "K",
            "M",
            "L",
            "T",
            "Y",
            "N",
            "H",
            "DA",
            "W",
            "CS",
            "MS",
            "TTS",
            "AeBe",
            "M_RedGiant",
            "",
        ] {
            let kind = StarKind::of(class);
            assert_eq!(
                StarKind::from_code(kind.code()),
                kind,
                "{class} did not survive its byte",
            );
        }

        // A byte from a build that knew more kinds than this one.
        assert_eq!(StarKind::from_code(200), StarKind::Unknown);

        // And the two readings a route wants, off the byte rather than off
        // the class string.
        assert!(StarKind::of("K").scoops());
        assert!(!StarKind::of("DA").scoops());
        assert_eq!(
            StarKind::of("N").boost(),
            Some(crate::meta::Boost::Neutron)
        );
        assert_eq!(
            StarKind::of("DA").boost(),
            Some(crate::meta::Boost::WhiteDwarf)
        );
        assert_eq!(StarKind::of("G").boost(), None);

        // Nothing said reads as nothing said, and says nothing.
        assert_eq!(StarKind::of("").named(), None);
        assert!(!StarKind::Unknown.scoops());
    }

    /// A ship refuels at the main sequence and nowhere else
    ///
    /// The fact a fuel-aware route is built on, and the one a first letter
    /// gets wrong: the sky holds classes that begin with a scoopable letter
    /// and hold no hydrogen to scoop. Worth a test rather than a glance,
    /// because the failure is not a slower route — it is a ship stranded
    /// between stars.
    #[test]
    fn a_fuel_scoop_takes_hydrogen_off_the_main_sequence() {
        for class in [
            "K",
            "G",
            "B",
            "F",
            "O",
            "A",
            "M",
            "M_RedGiant",
            "M_RedSuperGiant",
            "K_OrangeGiant",
            "A_BlueWhiteSuperGiant",
            "F_WhiteSuperGiant",
        ] {
            assert!(
                crate::meta::scoopable(class),
                "{class} would not refuel a ship"
            );
        }

        for class in [
            // Too cool to have started fusing.
            "L",
            "T",
            "Y",
            // What is left after the hydrogen went.
            "D",
            "DA",
            "DAB",
            "N",
            "H",
            "SupermassiveBlackHole",
            // The wrong element, or the wrong kind of star, under a letter
            // that begins a scoopable class.
            "MS",
            "S",
            "C",
            "CN",
            "CJ",
            "AeBe",
            "TTS",
            "W",
            "WN",
            "WC",
            // And nothing at all.
            "",
        ] {
            assert!(
                !crate::meta::scoopable(class),
                "{class} would refuel a ship"
            );
        }
    }

    /// A route names systems the ship has never been to, and what burns in
    /// them
    ///
    /// The one place a journal states a class for a system nobody has scanned,
    /// which is what the published build's `primary_star_class` column is for.
    #[test]
    fn a_plotted_route_places_systems_ahead_of_the_ship() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "NavRoute",
                "Route": [
                    {
                        "StarSystem": "Wredguia WD-K d8-30",
                        "SystemAddress": 1044034375449,
                        "StarPos": [-101.0, 130.0, -21.0],
                        "StarClass": "N"
                    }
                ]
            }"#,
        ));

        let systems = galaxy.systems();
        assert_eq!(systems.len(), 1, "the route named nothing");
        assert_eq!(systems[0].position, [-101.0, 130.0, -21.0]);
        assert_eq!(
            galaxy.boosts().first().map(|it| it.boost),
            Some(Boost::Neutron),
            "the route's own class was not read",
        );
    }

    /// A system with no `StarPos` anywhere is not put in the tree
    ///
    /// There is nowhere to put it. It would be worse than useless: a system at
    /// the origin is a system drawn on top of Sol.
    #[test]
    fn a_placeless_system_is_left_out() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "FSSDiscoveryScan",
                "SystemName": "Somewhere",
                "SystemAddress": 42,
                "BodyCount": 12,
                "NonBodyCount": 3
            }"#,
        ));
        assert_eq!(galaxy.len(), 1, "the system was not recorded at all");
        assert!(galaxy.systems().is_empty(), "a placeless system was placed");
        assert!(galaxy.names().is_empty());
    }

    /// A scanned system reaches as far as [`crate::inside`] says
    ///
    /// The map sizes a system by this and draws the inside of it from the same
    /// records, so the two have to be one answer. Asked here only for the
    /// property that a scan produces one at all: a system with nothing on
    /// record has no entry, which is how the map tells "small" from "not
    /// known".
    #[test]
    fn a_scan_gives_the_system_a_size() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        assert!(galaxy.reaches().is_empty(), "an unscanned system had a size");

        galaxy.read(&entry(&star_scan("G", 4.83, 5778.0)));
        let reaches = galaxy.reaches();
        assert_eq!(reaches.len(), 1);
        assert!(reaches[0].reach > 0.0);
    }

    /// What has been touched is what a publish has to look at
    #[test]
    fn touching_is_forgotten_once_it_is_settled() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        assert_eq!(galaxy.touched().len(), 1);
        galaxy.settle();
        assert!(galaxy.touched().is_empty());
        assert_eq!(galaxy.len(), 1, "settling forgot the system itself");
    }

    /// A system placed by an event that did not name it publishes no name
    ///
    /// A nav beacon scan is the one arrival-shaped event whose system name is
    /// optional, and it carries a position. Published as a blank name it is a
    /// blank row in the map's search: a system the user can neither look up
    /// nor recognise, standing in the list between two they can. The tree
    /// draws it either way, out of the position the beacon did carry.
    #[test]
    fn a_nameless_system_is_not_named() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "NavBeaconScan",
                "SystemAddress": 42,
                "StarPos": [1.0, 2.0, 3.0],
                "NumBodies": 5
            }"#,
        )));
        assert_eq!(galaxy.systems().len(), 1, "the beacon's place was lost");
        assert!(galaxy.names().is_empty(), "a system was named blank");
    }

    /// A codex sighting puts a system on the map nobody has been to
    ///
    /// The divergence this closes. `galos_db::record` writes a positioned
    /// `systems` row for one of these and the accumulator used to fall
    /// through it, so a run filling both sinks off one feed disagreed with
    /// itself about which systems exist — and an `--index` with no `--db`
    /// under it was simply short of them, the honk that finds a codex entry
    /// being often the first thing anybody sends about a place.
    ///
    /// Names the system `System` and no other event does, which is the
    /// other half of what would go wrong quietly.
    #[test]
    fn a_codex_sighting_is_a_system() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "CodexEntry",
                "System": "Deciat",
                "SystemAddress": 6681123623626,
                "StarPos": [122.625, -0.8125, -47.28125],
                "EntryID": 2100201,
                "Name": "$Codex_Ent_L_Dwarf_Name;"
            }"#,
        )));

        let systems = galaxy.systems();
        assert_eq!(systems.len(), 1, "a codex sighting placed no system");
        assert_eq!(systems[0].id64, 6681123623626);
        assert_eq!(systems[0].position, [122.625, -0.8125, -47.28125]);
        assert_eq!(galaxy.names()[0].name, "DECIAT");
    }

    /// A surface scan names its body without describing one
    ///
    /// `SAASignalsFound` says which body the signals were on and nothing
    /// else about it — no orbit, no class, no radius. A record made from
    /// that would be a body the reach and the arrival star are derived
    /// from, which is a system drawn the wrong size around a star it does
    /// not have. The signals themselves are the database's to keep; the
    /// index has no column for them.
    #[test]
    fn a_surface_scan_makes_no_body() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "SAASignalsFound",
                "StarSystem": "Sol",
                "SystemAddress": 10477373803,
                "StarPos": [0.0, 0.0, 0.0],
                "BodyName": "Sol 3",
                "BodyID": 3,
                "Signals": [{ "Type": "$SAA_SignalType_Biological;", "Count": 3 }]
            }"#,
        )));

        assert_eq!(galaxy.systems().len(), 1, "the signals placed no system");
        let inside = galaxy.bodies(10477373803);
        assert!(inside.bodies.is_empty(), "a body was invented from signals");
        assert!(inside.stars.is_empty());
        assert!(
            galaxy.reaches().is_empty(),
            "a system with nothing scanned in it was given a size",
        );
    }

    /// A settlement is a station, and a station's politics are not the
    /// system's
    ///
    /// `ApproachSettlement` carries a government, an allegiance and a
    /// faction, and every one of them belongs to the station rather than to
    /// the system it stands in. Taken for the system's they would colour
    /// the sky by where the commander happened to land — a carrier or a
    /// rescue ship reads as a government of its own. The write path does
    /// not take them either: `ensure_system` is handed a name and a
    /// position and nothing else.
    #[test]
    fn a_settlement_does_not_govern_its_system() {
        let mut galaxy = galaxy();
        galaxy.read(&entry(JUMP));
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:01:00Z",
                "event": "ApproachSettlement",
                "Name": "Ross Installation",
                "StarSystem": "Sol",
                "SystemAddress": 10477373803,
                "StarPos": [0.0, 0.0, 0.0],
                "BodyID": 3,
                "BodyName": "Sol 3",
                "StationGovernment": "$government_Corporate;",
                "StationAllegiance": "Independent",
                "StationEconomies": [
                    { "Name": "$economy_Rescue;", "Proportion": 1 }
                ]
            }"#,
        )));

        let populated = galaxy.populated();
        assert_eq!(populated.len(), 1);
        assert_eq!(
            populated[0].allegiance,
            Some(Allegiance::Federation),
            "a station's allegiance was taken for its system's",
        );
        assert!(
            populated[0].government.is_none(),
            "a station's government was taken for its system's",
        );
        assert_eq!(
            populated[0].primary_economy,
            Some(Economy::Refinery),
            "a station's economy was taken for its system's",
        );
    }

    /// A signal batch naming only an address publishes nothing
    ///
    /// One rule for every report now, and it is the one a nav beacon
    /// already needed: what a report says is recorded, and publishing a
    /// system takes both a name and a place. So the position this carried is
    /// kept for whatever names the system later, and until then the system
    /// is in no table the map reads.
    ///
    /// The database cannot even do that much — `INSERT`ing a `systems` row
    /// wants a name, so `SystemReport`'s write path drops a nameless report
    /// where this one keeps it. The two agree on everything published,
    /// which is what has to be true; they differ on what they hold back,
    /// because one of them has a `NOT NULL` and the other does not.
    #[test]
    fn a_signal_batch_naming_no_system_publishes_nothing() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T12:00:00Z",
                "event": "FSSSignalDiscovered",
                "SystemAddress": 42,
                "StarPos": [1.0, 2.0, 3.0],
                "signals": [{ "SignalName": "$MULTIPLAYER_SCENARIO42_TITLE;" }]
            }"#,
        )));
        assert_eq!(galaxy.len(), 1, "the place the signal carried was lost");
        assert!(galaxy.names().is_empty(), "a system was named blank");
        assert!(
            galaxy.populated().is_empty(),
            "a system with no name was published as populated",
        );
    }

    /// A docking moves the clock and nothing else
    ///
    /// The write path's `systems` upsert stamps `updated_at =
    /// GREATEST(systems.updated_at, $n)` on every write including this one,
    /// so a docking moves the system's Recency and a catch-up re-derives
    /// it. Nothing else: a docking carries no position, so a system nothing
    /// has placed stays unplaced and unpublished on both sides.
    #[test]
    fn a_docking_moves_only_the_clock() {
        let mut galaxy = galaxy();
        let week_ago = JUMP.replace("2026-08-08", "2026-08-01");
        galaxy.read(&entry(&week_ago));
        let stale = galaxy.systems()[0];

        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T11:00:00Z",
                "event": "Docked",
                "StarSystem": "Sol",
                "SystemAddress": 10477373803,
                "StationName": "Abraham Lincoln",
                "StationType": "Orbis",
                "MarketID": 128016640,
                "StationGovernment": "$government_Corporate;",
                "StationAllegiance": "Independent"
            }"#,
        )));

        let docked_at: DateTime<Utc> =
            "2026-08-08T11:00:00Z".parse().expect("the docking's moment");
        let fresh = galaxy.systems()[0];
        assert_eq!(
            fresh.updated_at,
            docked_at.timestamp() as u32,
            "a docking did not move the system's clock",
        );
        assert!(
            fresh.age_bucket < stale.age_bucket,
            "a week-old system dock-visited today stayed a week old",
        );
        assert_eq!(fresh.position, stale.position);
        assert_eq!(
            galaxy.populated()[0].government,
            None,
            "a station's government was taken for its system's",
        );
    }

    /// A docking is not a place
    ///
    /// It carries no position, so a system a docking is the whole of what
    /// anybody has said about waits for whatever places it. The published
    /// build holds the same row and leaves it out of the map by the same
    /// rule, its every system query reading `position IS NOT NULL`.
    #[test]
    fn a_docking_places_nothing() {
        let mut galaxy = galaxy();
        assert!(galaxy.read(&entry(
            r#"{
                "timestamp": "2026-08-08T11:00:00Z",
                "event": "Docked",
                "StarSystem": "Colonia",
                "SystemAddress": 3238296097059,
                "StationName": "Jaques Station",
                "StationType": "Orbis",
                "MarketID": 3510250752
            }"#,
        )));
        assert_eq!(galaxy.len(), 1, "the docking was not recorded at all");
        assert!(
            galaxy.systems().is_empty(),
            "a system a docking never placed was placed",
        );
        assert!(galaxy.names().is_empty());
    }
}
