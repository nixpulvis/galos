//! A system as somebody reported it, and what a second report does to it.
//!
//! The one shape every source states a system in. EDDN and a journal write
//! events, fifteen of which name a system and put the name, the place, the
//! politics or the counts in fifteen different fields; EDSM's and EDDB's
//! dumps write a file with the same columns read off it months ago. Both
//! meet here, and the two derivations of the index take it from here rather
//! than from the event.
//!
//! That is the whole reason this exists. `galos_db`'s write path and
//! [`crate::galaxy`]'s accumulator each need those fields off the event, and
//! plucking them out separately is two fifteen-arm matches that have to
//! agree about which events name a system and what each one says. They did
//! not: four events wrote a positioned row on one side and nothing at all
//! on the other. One match answers both.
//!
//! ## What a second report does
//!
//! [`SystemReport::over`], and it is the rule `galos_db`'s `ON CONFLICT DO
//! UPDATE` states column by column:
//!
//! - The newer report wins wherever it says anything.
//! - What it does not say leaves what stands, since a blank is not a reading
//!   and cannot contradict one.
//! - An older report still fills in what nothing has ever said, for the same
//!   reason.
//! - The stamp holds at the newest of the two either way, so a message
//!   delivered late does not put the reading back to when it was sent.
//!
//! It is not enough to take the later report. EDDN carries messages from
//! commanders in no order at all, a journal directory holds sessions restored
//! out of order, and the game writes a bare `AutoScan` on re-entering a
//! system it has already looked at closely — so the poorer reading arrives
//! second often enough that "last one wins" is a galaxy that forgets what it
//! knew.
//!
//! ## What is not here
//!
//! **Who reported it.** `updated_by` is a column on the database's row and
//! a field of every body an index publishes, and nothing either derivation
//! keeps above body level. So it travels beside a report as the `user`
//! every sink method already takes, and a galaxy of them does not pay a
//! string per system for something it will never serve.
//!
//! **Anything below system level.** A body is only ever stated by a `Scan`,
//! so `elite_journal`'s own [`Star`](elite_journal::body::Star) and
//! [`Body`](elite_journal::body::Body) *are* the report, and both
//! derivations hand them straight to [`crate::merge`]. A `BodyReport` would
//! be a shape with one source and nothing to reconcile.

use crate::meta::PopulatedSystem;
use crate::name::SystemName;
use chrono::{DateTime, Utc};
use elite_journal::entry::route::Destination;
use elite_journal::entry::{Entry, Event};
use elite_journal::prelude::{Allegiance, Economy, Government, Security};
use elite_journal::system::Coordinate;

/// A system as one report describes it, and as everything reported so far
/// adds up to.
///
/// The same type both ways round, which is what lets the rule be stated
/// once: [`SystemReport::over`] takes a report over a report and answers a
/// report, so what a source hands in and what an accumulator holds are not
/// two shapes with a conversion between them.
///
/// Every column is optional because every column is something a particular
/// report may not mention. A scan names a system and places it and says
/// nothing about who runs it; an arrival says all six political columns; a
/// honk says how much there is to find and nothing else. The address is the
/// exception: a report nothing can key is not a report.
#[derive(Clone, Debug, PartialEq)]
pub struct SystemReport {
    pub address: i64,
    /// When the reading was taken out in the galaxy, which is what the
    /// merge weighs and what the Recency axis reads. Not when it arrived.
    pub at: DateTime<Utc>,

    /// [`None`] where the report named only an address, which several of the
    /// game's events do.
    ///
    /// A system cannot be published without one — a blank row in the names
    /// table is a system the search can neither reach nor recognise — but it
    /// is still worth recording, since whatever names it later merges onto
    /// the place and the counts this report did carry.
    pub name: Option<SystemName>,
    /// [`None`] where the report did not place it, which the game does for
    /// events written inside a system it has already placed.
    pub position: Option<Coordinate>,

    pub population: Option<u64>,
    pub security: Option<Security>,
    pub government: Option<Government>,
    pub allegiance: Option<Allegiance>,
    pub primary_economy: Option<Economy>,
    pub secondary_economy: Option<Economy>,

    /// How many bodies the system holds, as the honk, the all-found tally or
    /// a nav beacon counted them.
    pub body_count: Option<i32>,
    /// The belts and rings, which only the honk counts.
    pub non_body_count: Option<i32>,

    /// The class of the star a ship drops in at, where a report states one
    /// for a system nobody has scanned.
    ///
    /// A plotted route is the only thing that does: it names a class for
    /// every stop ahead of the ship. `galos_db`'s `primary_star_class`
    /// column, and what [`crate::derive::lit`] falls back to when a system
    /// has no scanned star — which is two thirds of the galaxy.
    pub star_class: Option<String>,
}

impl SystemReport {
    /// A report of an address at a moment, saying nothing else yet.
    pub fn new(address: i64, at: DateTime<Utc>) -> SystemReport {
        SystemReport {
            address,
            at,
            name: None,
            position: None,
            population: None,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            body_count: None,
            non_body_count: None,
            star_class: None,
        }
    }

    /// What one journal entry says about the system it happened in.
    ///
    /// The one fan-out. Every event that names a system is an arm here and
    /// nowhere else, so an event taught to one derivation is taught to both
    /// and a new EDDN schema is one arm rather than two.
    ///
    /// [`None`] for an entry that says nothing about a system at all, which
    /// is most of a journal: combat, cargo, engineering and the rest are not
    /// facts about the sky. Not an error and not an omission.
    ///
    /// The three kinds of arm, and the whole of why the count is fifteen:
    ///
    /// - **An arrival** — `FSDJump`, `Location`, `CarrierJump` — states the
    ///   system in full, the commander standing in it reading the political
    ///   columns off.
    /// - **A look** — the scans, the barycentre, the counting events — names
    ///   it, places it and says how much there is.
    /// - **Something that merely happened there** — a codex sighting, a
    ///   signal, a settlement, a docking. The index has no column for the
    ///   thing itself and the database keeps it in a table of its own, but
    ///   both need the system on record first: Postgres for the foreign key
    ///   the row hangs off, and the index because the honk that finds a
    ///   signal is often the first thing anybody ever sends about a place.
    ///
    /// `NavRoute` is the one event that is not here. It states a system per
    /// stop rather than one, so it is [`SystemReport::plotted`] over the
    /// destinations, which both sides already loop.
    pub fn of(entry: &Entry<Event>) -> Option<SystemReport> {
        let at = entry.timestamp;
        let report = |address, name: Option<&str>, position| SystemReport {
            name: name.map(SystemName::new),
            position,
            ..SystemReport::new(address, at)
        };

        match &entry.event {
            // The fullest thing a journal says about a system: the commander
            // is in it, reading its politics off.
            Event::FsdJump(jump) => Some(Self::arrival(at, &jump.system)),
            Event::Location(here) => Some(Self::arrival(at, &here.system)),
            // A carrier jump says everything about a system that arriving
            // under your own power does.
            Event::CarrierJump(jump) => Some(Self::arrival(at, &jump.system)),

            Event::Scan(scan) => Some(report(
                scan.system_address,
                Some(&scan.star_system),
                scan.star_pos,
            )),
            Event::ScanBaryCentre(center) => Some(report(
                center.system_address,
                Some(&center.star_system),
                center.star_pos,
            )),

            // How much there is in a system, which is the other half of
            // knowing what has been found in it. Three events report the
            // same number under three different names, and only the honk
            // counts what is not a body.
            Event::FssDiscoveryScan(honk) => Some(SystemReport {
                body_count: Some(honk.body_count),
                non_body_count: Some(honk.non_body_count),
                ..report(
                    honk.system_address,
                    Some(&honk.system_name),
                    honk.star_pos,
                )
            }),
            Event::FssAllBodiesFound(all) => Some(SystemReport {
                body_count: Some(all.count),
                ..report(
                    all.system_address,
                    Some(&all.system_name),
                    all.star_pos,
                )
            }),
            // The one counting event that names its system the way
            // everything else does, and may name nothing at all.
            Event::NavBeaconScan(beacon) => Some(SystemReport {
                body_count: Some(beacon.num_bodies),
                ..report(
                    beacon.system_address,
                    beacon.star_system.as_deref(),
                    beacon.star_pos,
                )
            }),

            // Things that happened in a system without describing one. Names
            // the system `System`, which no other event does.
            Event::CodexEntry(codex) => Some(report(
                codex.system_address,
                Some(&codex.system_name),
                codex.star_pos,
            )),
            Event::SAASignalsFound(found) => Some(report(
                found.system_address,
                found.star_system.as_deref(),
                found.star_pos,
            )),
            Event::FssBodySignals(found) => Some(report(
                found.system_address,
                found.star_system.as_deref(),
                found.star_pos,
            )),
            Event::FssSignalDiscovered(found) => Some(report(
                found.system_address,
                found.star_system.as_deref(),
                found.star_pos,
            )),
            // A settlement is a station on a planet's surface. Its
            // government, its allegiance and its faction are the
            // *station's*, and are not read for the system's: a carrier or
            // a rescue ship reads as a government of its own, and taking
            // that for the system's would colour the sky by where the
            // commander parked.
            Event::ApproachSettlement(approach) => Some(report(
                approach.system_address,
                approach.system_name.as_deref(),
                approach.star_pos,
            )),
            // A docking says which system it is and not where: the game
            // writes no `StarPos` on one and `Docked` keeps none of the one
            // EDDN's augmenter adds. So it moves the moment and nothing
            // else for a system already placed, and a system a docking is
            // the whole of what is known about waits for whatever places
            // it. Its politics are the station's, as a settlement's are.
            Event::Docked(docked) => Some(report(
                docked.system_address,
                Some(&docked.system_name),
                None,
            )),

            _ => None,
        }
    }

    /// A system as an arrival event states it, which is in full.
    fn arrival(
        at: DateTime<Utc>,
        system: &elite_journal::system::System,
    ) -> SystemReport {
        SystemReport {
            name: Some(SystemName::new(system.name.clone())),
            position: system.pos,
            population: system.population,
            security: system.security,
            government: system.government,
            allegiance: system.allegiance,
            primary_economy: system.economy,
            secondary_economy: system.second_economy,
            ..SystemReport::new(system.address, at)
        }
    }

    /// A stop on the route the ship last plotted.
    ///
    /// A system nobody has been to, named, placed and with the class of the
    /// star at the middle of it — which is everything the tree needs, and
    /// the only place a journal states a class for an unscanned system. Not
    /// a visit: nobody flew anywhere, a route was laid in, so it says
    /// nothing about who runs the place or what is in it.
    pub fn plotted(at: DateTime<Utc>, stop: &Destination) -> SystemReport {
        SystemReport {
            name: Some(SystemName::new(stop.star_system.clone())),
            position: Some(stop.star_pos),
            star_class: Some(stop.star_class.clone()),
            ..SystemReport::new(stop.system_address as i64, at)
        }
    }

    /// Lay `said` over what this report holds.
    ///
    /// The merge rule, once. `galos_db` states the same thing in the `ON
    /// CONFLICT DO UPDATE` clauses of its `systems` upsert, which it has to
    /// — a row is merged by Postgres against a row Postgres holds, and
    /// reading it back to merge it here would be two round trips and a lost
    /// update between the two pools. That copy is pinned to this one by
    /// `galos_db`'s conformance test rather than by having been read
    /// carefully.
    pub fn over(&mut self, said: SystemReport) {
        debug_assert_eq!(self.address, said.address, "two systems merged");
        // Which direction every column below fills in. The newer reading
        // wins where it is a reading; the older one fills only what nothing
        // has ever said. `>=` as the database's `CASE WHEN $n >=
        // updated_at` is: a tie goes to the report that has just arrived,
        // there being nothing to choose between them and one having to win.
        let newer = said.at >= self.at;

        fill(&mut self.name, said.name, newer);
        fill(&mut self.position, said.position, newer);
        fill(&mut self.population, said.population, newer);
        fill(&mut self.security, said.security, newer);
        fill(&mut self.government, said.government, newer);
        fill(&mut self.allegiance, said.allegiance, newer);
        fill(&mut self.primary_economy, said.primary_economy, newer);
        fill(&mut self.secondary_economy, said.secondary_economy, newer);
        fill(&mut self.star_class, said.star_class, newer);

        // The two that cannot go stale, and so are not weighed by the stamps
        // at all. A system does not gain or lose bodies, and a timestamp
        // guard would throw nearly every count away: a system busy enough to
        // be honked at is busy enough to have been reported more recently by
        // something else. `galos_db::System::set_body_counts` says the same
        // and says why at greater length.
        fill(&mut self.body_count, said.body_count, true);
        fill(&mut self.non_body_count, said.non_body_count, true);

        // Only ever forward. The stamp is what the map reads as "updated",
        // and a message delivered late is still a reading of an older
        // moment.
        self.at = self.at.max(said.at);
    }

    /// The name as the galaxy spells it, where the report named one.
    ///
    /// Upper case because [`SystemName`] cannot be anything else, so this
    /// is a borrow rather than the `to_uppercase` per read it used to be —
    /// and the two derivations cannot publish two spellings of one galaxy,
    /// which is what the fold was for.
    pub fn named(&self) -> Option<&SystemName> {
        self.name.as_ref()
    }

    /// Where the system is, as the tree counts positions.
    pub fn placed(&self) -> Option<[f64; 3]> {
        let at = self.position?;
        Some([at.x, at.y, at.z])
    }

    /// This system's political columns, where anybody lives in it.
    ///
    /// [`None`] for a system with no population, no name or no place: the
    /// table is what the map colours and filters by, and a row it cannot
    /// draw or look up is not one.
    ///
    /// `factions` is empty. A faction's numeric id is `galos_db`'s, minted
    /// when the row is first written, and a report names factions and
    /// numbers nothing — so a system's faction ids keep coming from
    /// whatever is underneath rather than being invented here, where they
    /// would collide with real ones and colour the map by the wrong
    /// faction.
    pub fn populated(&self) -> Option<PopulatedSystem> {
        let population = self.population.filter(|&it| it > 0)?;
        let at = self.placed()?;
        Some(PopulatedSystem {
            address: self.address,
            name: self.named()?.clone(),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
            population,
            security: self.security,
            government: self.government,
            allegiance: self.allegiance,
            primary_economy: self.primary_economy,
            secondary_economy: self.secondary_economy,
            factions: Vec::new(),
            body_count: self.body_count,
            non_body_count: self.non_body_count,
        })
    }
}

/// One column, filled as the merge rule says.
///
/// `COALESCE` in whichever order the stamps put the two readings: a reading
/// wins where it is one, and where it is blank what already stands is not
/// contradicted by it.
fn fill<T>(held: &mut Option<T>, said: Option<T>, newer: bool) {
    if said.is_some() && (newer || held.is_none()) {
        *held = said;
    }
}
