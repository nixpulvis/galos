//! What a journal says, as the index says it.
//!
//! The peer of `galos_db::index`, and it is worth being exact about what that
//! means. That one reads Postgres — rows already merged from every commander
//! EDDN carries — and derives the tree and the metadata from them. This one
//! reads a directory of `.log` files, which is one commander's own game and
//! nobody else's, and derives the same things from the events themselves.
//! Between them sits `galos_index`, which knows neither.
//!
//! The fan-out is deliberately the same one `galos-sync`'s `journal/record.rs`
//! does, and for the reason stated there: a scan is a scan whether it arrives
//! off a socket or off a disk. What differs is where it lands. `record.rs`
//! writes fourteen tables and the index build reads them back; this keeps the
//! two shapes the index actually wants — [`galos_index::System`] and the
//! metadata records — and skips the round trip. So a body scanned in the game
//! is on the map a second later without a database having been asked.
//!
//! ## What a journal cannot say
//!
//! Three gaps, all of them stated here rather than papered over, because a
//! layer that quietly answered them wrongly would be worse than one that says
//! nothing:
//!
//! - **Factions have no ids.** A faction's numeric id is `galos_db`'s, minted
//!   when the row is first written; a journal names factions and numbers
//!   nothing. So [`Galaxy`] publishes no faction table and leaves
//!   [`PopulatedSystem::factions`] empty, and a system's factions keep coming
//!   from whatever is underneath. Filling that column with ids of its own
//!   would collide with real ones and colour the map by the wrong faction.
//! - **A system's own row is a visit.** EDDN hears about a system from
//!   everyone; a journal hears about it from one ship. What is here is where
//!   this commander has been, plus the stops on the route they last plotted.
//! - **Most systems have no scanned star.** The published build falls back to
//!   a `primary_star_class` column when nothing is scanned. A journal has no
//!   such column: it has the star classes it has scanned and the ones the
//!   route file names, and past those the default stands in — exactly as it
//!   does for a system EDDN knows nothing about either.
//!
//! ## Time
//!
//! The Recency axis is measured from a clock this crate holds
//! ([`Galaxy::now`]) rather than from a database's. That is the same rule
//! `galos_db::index` follows for the same reason — one clock compared against
//! itself — and here it is simpler, the events carrying their own timestamps
//! and no upsert standing between them and the reading.

use chrono::{DateTime, Utc};
use elite_journal::body::{
    Body as JournalBody, Star as JournalStar, Surface as JournalSurface,
};
use elite_journal::entry::incremental::exploration::{
    Scan, ScanTarget, ScanType,
};
use elite_journal::entry::route::Destination;
use elite_journal::entry::{Entry, Event};
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use elite_journal::system::{Coordinate, System as JournalSystem};
use galos_index::System;
use galos_index::meta::{
    Barycenter, Body, Boost, NameEntry, Parent, PopulatedSystem, Star, Surface,
    SystemBodies, SystemBoost, SystemReach,
};
use galos_photometry::{ClassLight, Magnitude, Temperature};
use std::collections::{BTreeMap, HashMap, HashSet};

/// The edges between the eight Recency buckets, in days since a system was
/// last heard from. The published build's own edges, named again rather than
/// shared: they are a property of the format, and the two must agree.
const AGE_EDGES: [i64; 7] = [1, 7, 30, 90, 365, 1095, 3650];

/// Who a journal's readings are filed under when nothing in it says.
///
/// `galos-sync journal` files them under the same word, and for the same
/// reason: these came from a journal and that is the whole of the claim.
pub const UNKNOWN: &str = "unknown";

/// Which Recency bucket an age in days falls in, `0..8`.
fn age_bucket(days: i64) -> usize {
    AGE_EDGES.iter().filter(|&&edge| days >= edge).count()
}

/// A system as the journal has described it, over however many events.
///
/// Merged rather than replaced, in the same spirit the database's write path
/// merges: a later event wins where it says anything, and where it says
/// nothing what is already known stands. An `FSDJump` names a system's
/// politics and a `Scan` in the same system names none of them, and the scan
/// arriving second must not take the politics away.
#[derive(Clone, Debug, Default)]
struct Visit {
    name: String,
    /// Where the system is, in light years. [`None`] where every event naming
    /// it left `StarPos` out, which the game does for events inside a system
    /// it has already placed.
    position: Option<[f64; 3]>,
    /// The latest timestamp of any event about this system: what the Recency
    /// axis reads and what the payload point carries.
    updated_at: Option<DateTime<Utc>>,
    population: u64,
    security: Option<Security>,
    government: Option<Government>,
    allegiance: Option<Allegiance>,
    primary_economy: Option<Economy>,
    secondary_economy: Option<Economy>,
    body_count: Option<i32>,
    non_body_count: Option<i32>,
    /// The arrival star's class, where the route file named it.
    ///
    /// A route is the one place a journal states a class for a system nobody
    /// has scanned, which is what the published build's `primary_star_class`
    /// column holds. A scan says it better and overrides this.
    routed_class: Option<String>,
}

/// Everything a journal directory has said, in the shapes the index wants.
///
/// Held whole in memory. A commander's journal is thousands of systems where
/// the galaxy is a hundred and twenty-nine million, so the tables here are
/// megabytes rather than gigabytes and every derivation over them is a pass
/// that costs nothing worth measuring — which is why the source over this
/// rebuilds rather than editing, and why there is no checkpoint, no cursor
/// and no incremental publish anywhere in this crate.
#[derive(Clone, Debug)]
pub struct Galaxy {
    /// What dates the Recency reading. Set once by the caller per pass, so
    /// every system in one publish is aged against the same moment.
    now: DateTime<Utc>,
    /// Who is flying, as the last `Commander` or `LoadGame` said.
    commander: String,
    systems: HashMap<i64, Visit>,
    inside: HashMap<i64, SystemBodies>,
    /// Systems touched since the last [`Galaxy::settle`].
    touched: HashSet<i64>,
}

/// The political columns a system carries.
///
/// Named as a group because two very different things hand them over: an
/// arrival event, where the commander is standing in the system reading them
/// off, and a published dump, where somebody else read them off months ago.
/// Both say the same six things and neither says any of them reliably, so
/// every one is optional and every one merges the same way — a reading wins
/// where it is a reading, and where it is blank what already stands is not
/// contradicted by it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Politics {
    pub population: Option<u64>,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,
}

impl Politics {
    /// What an arrival event says about the system it names.
    fn of(system: &JournalSystem) -> Politics {
        Politics {
            population: system.population,
            security: system.security,
            government: system.government,
            allegiance: system.allegiance,
            primary_economy: system.economy,
            secondary_economy: system.second_economy,
        }
    }
}

impl Default for Galaxy {
    fn default() -> Galaxy {
        Galaxy::new(Utc::now())
    }
}

impl Galaxy {
    /// An empty galaxy, aged against `now`.
    pub fn new(now: DateTime<Utc>) -> Galaxy {
        Galaxy {
            now,
            commander: UNKNOWN.to_string(),
            systems: HashMap::new(),
            inside: HashMap::new(),
            touched: HashSet::new(),
        }
    }

    /// Date the Recency reading from `now` from here on.
    ///
    /// A watch running for hours would otherwise go on measuring every
    /// system's age from the moment it started, and the youngest bucket is a
    /// day wide.
    pub fn dated(&mut self, now: DateTime<Utc>) {
        self.now = now;
    }

    /// Who the journal says is flying.
    pub fn commander(&self) -> &str {
        &self.commander
    }

    /// How many systems the journal has named.
    pub fn len(&self) -> usize {
        self.systems.len()
    }

    /// Whether the journal has named nothing at all.
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
    /// refused: a journal is mostly combat, cargo and docking, and none of
    /// that is a fact about the sky.
    pub fn read(&mut self, entry: &Entry<Event>) -> bool {
        let at = entry.timestamp;
        match &entry.event {
            Event::FsdJump(jump) => self.visit(at, &jump.system),
            Event::CarrierJump(jump) => self.visit(at, &jump.system),
            Event::Location(location) => self.visit(at, &location.system),
            Event::Scan(scan) => self.scan(at, scan),
            Event::ScanBaryCentre(center) => {
                self.seen(
                    at,
                    center.system_address,
                    &center.star_system,
                    center.star_pos,
                );
                let address = center.system_address;
                let barycenter = Barycenter {
                    system_address: address,
                    id: center.body_id,
                    updated_at: at,
                    updated_by: self.commander.clone(),
                    orbit: center.orbit.clone(),
                };
                put(
                    &mut self.inside.entry(address).or_default().barycenters,
                    barycenter,
                    |it| it.id,
                );
                true
            }
            Event::FssDiscoveryScan(honk) => {
                self.seen(
                    at,
                    honk.system_address,
                    &honk.system_name,
                    honk.star_pos,
                );
                let visit = self.visit_mut(honk.system_address);
                visit.body_count = Some(honk.body_count);
                visit.non_body_count = Some(honk.non_body_count);
                true
            }
            Event::FssAllBodiesFound(all) => {
                self.seen(
                    at,
                    all.system_address,
                    &all.system_name,
                    all.star_pos,
                );
                self.visit_mut(all.system_address).body_count = Some(all.count);
                true
            }
            Event::NavBeaconScan(beacon) => {
                // The one of the three counting events that names its system
                // the way everything else does, and may name nothing at all.
                self.seen(
                    at,
                    beacon.system_address,
                    beacon.star_system.as_deref().unwrap_or(""),
                    beacon.star_pos,
                );
                self.visit_mut(beacon.system_address).body_count =
                    Some(beacon.num_bodies);
                true
            }
            Event::NavRoute(route) => {
                let mut said = false;
                for stop in &route.destinations {
                    said |= self.plotted(at, stop);
                }
                said
            }
            Event::Commander(who) => {
                self.commander = who.name.clone();
                false
            }
            Event::LoadGame(game) => {
                if let Some(who) = &game.commander {
                    self.commander = who.name.clone();
                }
                false
            }
            _ => false,
        }
    }

    /// A system arrived in, which is the fullest thing a journal says about
    /// one.
    fn visit(&mut self, at: DateTime<Utc>, system: &JournalSystem) -> bool {
        self.seen(at, system.address, &system.name, system.pos);
        self.govern(system.address, Politics::of(system));
        true
    }

    /// A system as a published dump gives it: named, placed, and with the
    /// columns somebody else read off it, and nothing below system level.
    ///
    /// Not an event and never was — nobody flew anywhere, a file was
    /// published — so it does not touch a scan, a body or a star class. What
    /// it does is exactly what an arrival does minus the visit: it puts a
    /// system on the map with its politics, which for two thirds of the
    /// galaxy is everything anyone knows.
    ///
    /// `at` is when the dump says the reading was taken, not when it was
    /// read. That is what the Recency axis wants and it is why an EDDB dump
    /// lands in the oldest bucket where it belongs rather than looking like
    /// news.
    pub fn place(
        &mut self,
        at: DateTime<Utc>,
        address: i64,
        name: &str,
        position: Coordinate,
        politics: Politics,
    ) -> bool {
        self.seen(at, address, name, Some(position));
        self.govern(address, politics);
        true
    }

    /// Merge political columns into what a system already carries.
    ///
    /// A reading wins where it is one; a blank leaves what stands. The rule
    /// the database's own write path states, kept here because the two have
    /// to agree about what a `Scan` arriving after an `FSDJump` does to a
    /// system's population, which is nothing.
    fn govern(&mut self, address: i64, said: Politics) {
        let visit = self.visit_mut(address);
        visit.population = said.population.unwrap_or(visit.population);
        visit.security = said.security.or(visit.security);
        visit.government = said.government.or(visit.government);
        visit.allegiance = said.allegiance.or(visit.allegiance);
        visit.primary_economy = said.primary_economy.or(visit.primary_economy);
        visit.secondary_economy =
            said.secondary_economy.or(visit.secondary_economy);
    }

    /// A stop on the route the ship last plotted.
    ///
    /// A system nobody has been to, named, placed and with the class of the
    /// star at the middle of it — which is everything the tree needs and the
    /// only place a journal states a class for an unscanned system. It is not
    /// a visit, so it moves nothing but the name, the place and the class.
    fn plotted(&mut self, at: DateTime<Utc>, stop: &Destination) -> bool {
        let address = stop.system_address as i64;
        self.seen(at, address, &stop.star_system, Some(stop.star_pos));
        self.visit_mut(address).routed_class = Some(stop.star_class.clone());
        true
    }

    /// Whatever a scan looked at, and the system it looked at it from.
    fn scan(&mut self, at: DateTime<Utc>, scan: &Scan) -> bool {
        self.seen(at, scan.system_address, &scan.star_system, scan.star_pos);
        let address = scan.system_address;
        let by = self.commander.clone();
        let found = discovered_at(scan, at);
        let inside = self.inside.entry(address).or_default();
        match &scan.target {
            ScanTarget::Star(star) => put(
                &mut inside.stars,
                star_of(address, star, at, &by, found),
                |it| it.id,
            ),
            ScanTarget::Body(body) => put(
                &mut inside.bodies,
                body_of(address, body, at, &by, found),
                |it| it.id,
            ),
            // A belt cluster and a ring are numbered bodies of the system and
            // neither has a record in [`SystemBodies`], which carries what the
            // map draws inside a system. The scan still counts as having been
            // in the system, which `seen` has already recorded.
            ScanTarget::Cluster(_) | ScanTarget::Ring(_) => {}
        }
        true
    }

    /// A system named by any event at all: its name, its place if the event
    /// carried one, and the moment.
    fn seen(
        &mut self,
        at: DateTime<Utc>,
        address: i64,
        name: &str,
        pos: Option<Coordinate>,
    ) {
        self.touched.insert(address);
        let visit = self.systems.entry(address).or_default();
        if !name.is_empty() {
            visit.name = name.to_string();
        }
        if let Some(pos) = pos {
            visit.position = Some([pos.x, pos.y, pos.z]);
        }
        // The latest event about a system is what the Recency axis reads, and
        // journal files are read in order but a directory holds sessions
        // restored out of order often enough to be worth the max.
        visit.updated_at =
            Some(visit.updated_at.map_or(at, |held| held.max(at)));
    }

    /// The system's record, for an event that has already been `seen`.
    fn visit_mut(&mut self, address: i64) -> &mut Visit {
        self.touched.insert(address);
        self.systems.entry(address).or_default()
    }

    /// Every placed system, as the tree takes them.
    ///
    /// A system with no `StarPos` anywhere in the journal is left out: the
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
        let visit = self.systems.get(&address)?;
        let position = visit.position?;
        Some(self.system(address, visit, position))
    }

    /// One system's name and place, where it has been placed.
    pub fn name_of(&self, address: i64) -> Option<NameEntry> {
        let visit = self.systems.get(&address)?;
        let at = visit.position?;
        Some(NameEntry {
            address,
            name: visit.name.clone(),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
        })
    }

    /// How far one system reaches, where anything in it has been scanned.
    pub fn reach_of(&self, address: i64) -> Option<f32> {
        self.inside.get(&address)?.extent(address)
    }

    /// What one system's arrival star can supercharge, if anything.
    pub fn boost_of(&self, address: i64) -> Option<Boost> {
        Boost::of(&self.arrival_class(address)?)
    }

    /// One system's political columns, where anybody lives in it.
    pub fn populated_of(&self, address: i64) -> Option<PopulatedSystem> {
        let visit = self.systems.get(&address)?;
        if visit.population == 0 {
            return None;
        }
        let at = visit.position?;
        Some(PopulatedSystem {
            address,
            name: visit.name.clone(),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
            population: visit.population,
            security: visit.security,
            government: visit.government,
            allegiance: visit.allegiance,
            primary_economy: visit.primary_economy,
            secondary_economy: visit.secondary_economy,
            factions: Vec::new(),
            body_count: visit.body_count,
            non_body_count: visit.non_body_count,
        })
    }

    /// Whether anything at all is known about the system at `address`.
    pub fn holds(&self, address: i64) -> bool {
        self.systems.contains_key(&address)
    }

    /// One system's photometry and place, by the same fallback chain the
    /// published build uses.
    ///
    /// Its scanned stars if it has any: their visual magnitudes combine into
    /// one figure and the tint is the brightest of them, which dominates it.
    /// Failing that the arrival star's class, which only a plotted route
    /// states here. Failing that the default M dwarf, which is what the
    /// galaxy is mostly made of.
    fn system(
        &self,
        address: i64,
        visit: &Visit,
        position: [f64; 3],
    ) -> System {
        let stars = self.lit(address);
        let (absolute_magnitude, temperature) = match Magnitude::combine(
            stars.iter().map(|&(m, _)| Magnitude(m)),
        ) {
            Some(combined) => {
                let tint = stars
                    .iter()
                    .copied()
                    .min_by(|a, b| a.0.total_cmp(&b.0))
                    .map(|(_, temperature)| temperature)
                    .expect("a combined magnitude means at least one star");
                (combined.0, tint)
            }
            None => {
                let light =
                    ClassLight::of(visit.routed_class.as_deref().unwrap_or(""));
                (light.absolute_magnitude.0, light.temperature.0)
            }
        };
        let at = visit.updated_at.unwrap_or(self.now);
        System {
            id64: address as u64,
            position,
            absolute_magnitude,
            temperature,
            age_bucket: age_bucket((self.now - at).num_days()),
            updated_at: at.timestamp().clamp(0, u32::MAX as i64) as u32,
        }
    }

    /// A system's scanned stars as `(visual absolute magnitude, temperature)`.
    ///
    /// The scanned magnitude is bolometric — the star's whole output as one
    /// figure — so it is turned into the visual magnitude the sky sees before
    /// anything sums it. That is where a white dwarf keeps its faint scanned
    /// brightness and a neutron star falls to nothing, and it is the one step
    /// that would be easy to leave out and impossible to see the absence of.
    fn lit(&self, address: i64) -> Vec<(f64, f64)> {
        self.inside
            .get(&address)
            .map(|inside| {
                inside
                    .stars
                    .iter()
                    .map(|star| {
                        let t = star.temperature as f64;
                        let m = Magnitude(star.absolute_magnitude as f64);
                        (m.visual(Temperature(t)).0, t)
                    })
                    .collect()
            })
            .unwrap_or_default()
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
    /// [`galos_index::inside`]'s answer, which is the same call the published
    /// build makes: the map sizes a system by this and draws the inside of it
    /// from the same records, and a shell smaller than the orbits it contains
    /// is the one thing a reach cannot be.
    pub fn reaches(&self) -> Vec<SystemReach> {
        let mut table: Vec<SystemReach> = self
            .inside
            .keys()
            .filter_map(|&address| {
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
        let mut table: Vec<SystemBoost> = self
            .systems
            .keys()
            .filter_map(|&address| {
                Some(SystemBoost { address, boost: self.boost_of(address)? })
            })
            .collect();
        table.sort_by_key(|it| it.address);
        table
    }

    /// The class of the star a ship drops in at, as far as the journal says.
    fn arrival_class(&self, address: i64) -> Option<String> {
        let scanned = self.inside.get(&address).and_then(|inside| {
            inside
                .stars
                .iter()
                .min_by(|a, b| {
                    a.distance_from_arrival_ls
                        .total_cmp(&b.distance_from_arrival_ls)
                        .then(a.id.cmp(&b.id))
                })
                .map(|star| star.star_class.clone())
        });
        scanned.or_else(|| self.systems.get(&address)?.routed_class.clone())
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
        self.inside.get(&address).cloned().unwrap_or_default()
    }

    /// Every system with anything scanned in it.
    pub fn scanned(&self) -> impl Iterator<Item = (i64, &SystemBodies)> {
        self.inside.iter().map(|(&address, inside)| (address, inside))
    }
}

/// Put `it` in `table`, replacing whatever was already filed under its key.
///
/// A body is scanned more than once — a honk, then a proper look, then a
/// surface map — and the later scan is the fuller one. Replacing rather than
/// merging field by field is the right rule for a single commander's journal
/// read in order: the events are that commander's own successive looks at the
/// one body, so the last is the best.
fn put<T, K: Eq>(table: &mut Vec<T>, it: T, key: impl Fn(&T) -> K) {
    let k = key(&it);
    match table.iter().position(|held| key(held) == k) {
        Some(at) => table[at] = it,
        None => table.push(it),
    }
}

/// When a scan says what it looked at was found, where it says at all.
///
/// `galos-sync`'s rule, stated there at length and repeated in one line here:
/// `WasDiscovered` clear means this scan *is* the discovery and the entry's
/// own time is when it happened; set means somebody was there earlier and the
/// scan says nothing about when. A nav beacon is not read for it — it answers
/// for every body in the system out of what it holds rather than out of a
/// look anybody took.
fn discovered_at(scan: &Scan, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let beacon = scan.scan_type.as_ref().is_some_and(ScanType::is_beacon);
    (!beacon && !scan.target.discovery().discovered).then_some(at)
}

/// The ancestry a scan named, nearest first.
///
/// A scan writes each ancestor as a one-entry map of kind to id, and the walk
/// back to the star is what places the thing, so the order and the whole
/// chain are kept. `galos_db::bodies::Parent::chain`'s rule, in the index's
/// own vocabulary.
fn chain(named: &[BTreeMap<String, i16>]) -> Vec<Parent> {
    named
        .iter()
        .filter_map(|parent| {
            let (ty, id) = parent.iter().next()?;
            Some(Parent { ty: Some(ty.clone()), id: *id })
        })
        .collect()
}

/// A scanned star as the index's record of one.
fn star_of(
    address: i64,
    star: &JournalStar,
    at: DateTime<Utc>,
    by: &str,
    found: Option<DateTime<Utc>>,
) -> Star {
    Star {
        system_address: address,
        id: star.id,
        name: star.name.clone(),
        parents: chain(&star.parents),
        updated_at: at,
        updated_by: by.to_string(),
        absolute_magnitude: star.absolute_magnitude,
        age_my: star.age_my,
        distance_from_arrival_ls: star.distance_from_arrival_ls,
        luminosity: star.luminosity.clone(),
        star_class: star.star_class.clone(),
        stellar_mass: star.stellar_mass,
        subclass: star.subclass,
        orbit: star.orbit.clone(),
        spin: star.spin.clone(),
        radius: star.radius,
        temperature: star.temperature,
        mapped: star.discovery.mapped,
        discovered_at: found,
    }
}

/// A scanned body as the index's record of one.
fn body_of(
    address: i64,
    body: &JournalBody,
    at: DateTime<Utc>,
    by: &str,
    found: Option<DateTime<Utc>>,
) -> Body {
    Body {
        system_address: address,
        id: body.id,
        parents: chain(&body.parents),
        name: body.name.clone(),
        body_type: body.ty.clone(),
        distance_from_arrival: body.distance_from_arrival,
        updated_at: at,
        updated_by: by.to_string(),
        planet_class: body.planet_class.clone(),
        // A basic scan does not report it, and a body nobody has looked at
        // closely is not tidally locked as far as anything can say.
        tidal_lock: body.tidal_lock.unwrap_or(false),
        mass: body.mass,
        radius: body.radius,
        gravity: body.gravity,
        temperature: body.temperature,
        surface: body.surface.as_ref().map(surface_of),
        orbit: body.orbit.clone(),
        spin: body.spin.clone(),
        mapped: body.discovery.mapped,
        discovered_at: found,
    }
}

/// What a body with a surface has, as the index records it.
///
/// The one field that differs: the index's composition is optional, a body
/// stored before the fractions were kept having a surface and no reading of
/// what it is made of. A scan always carries one.
fn surface_of(surface: &JournalSurface) -> Surface {
    Surface {
        atmosphere_type: surface.atmosphere_type.clone(),
        pressure: surface.pressure,
        composition: Some(surface.composition.clone()),
        landable: surface.landable,
        atmosphere: surface.atmosphere.clone(),
        volcanism: surface.volcanism.clone(),
        terraform_state: surface.terraform_state.clone(),
        materials: surface.materials.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::entry::Entry;

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
        assert_eq!(names[0].name, "Sol");

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

    /// A scanned system reaches as far as `galos_index::inside` says
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
}
