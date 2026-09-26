//! What a second look at a body does to what is on record.
//!
//! The peer of [`crate::accumulate::report`], one level down. That one merges what a
//! report says about a system; this merges what a scan says about the things
//! inside it — a star, a body, its surface, the barycentre a close pair goes
//! round.
//!
//! `galos_db` states the same rule column by column in the `ON CONFLICT DO
//! UPDATE` clauses of its `stars` and `system_bodies` upserts, and it has to:
//! the row it is merging against is a row Postgres holds, and reading it back
//! to merge it here would be a round trip and a lost update per scan. This is
//! the statement of the rule, that is a copy of it, and a conformance test in
//! `galos_db` is what keeps the two the same thing.
//!
//! ## The rule
//!
//! - A reading wins where the scan is one.
//! - What a scan does not state leaves what stands. A bare `AutoScan`
//!   carries no surface block, no materials and no tidal lock, and must not
//!   take away what a closer look found.
//! - The two facts about a thing's history only ever go one way: whether it
//!   has been mapped goes up, and when it was found goes back.
//! - The stamp only goes forward.
//!
//! Which is not the same as taking the later scan. The game writes a basic
//! `AutoScan` every time a ship re-enters a system it has already looked at
//! closely, so the poorer reading arrives second in one commander's own
//! ordered journal; EDDN carries scans from commanders in no order at all;
//! and a journal directory holds sessions restored out of order.
//!
//! ## One stored record over another
//!
//! The same rule has a second half, for when both sides are *stored*
//! records rather than a scan and a record: two directories folded into one
//! ([`crate::ops::absorb`]), or an event's populated row over the one a
//! directory publishes (`galos::sink::tables`). Each side there has already
//! merged every scan its own source saw, so there is no "did the scan
//! mention this" to ask — only which of two finished records is the later,
//! and what the earlier one still has to say. That is [`bodies_over`] and
//! [`populated_over`], and it is stated once, here.

use crate::records::{
    Barycenter, Body, Parent, PopulatedSystem, Star, Surface, SystemBodies,
};
use chrono::{DateTime, Utc};
use elite_journal::body::{
    Body as JournalBody, Orbit, Star as JournalStar, Surface as JournalSurface,
};
use elite_journal::entry::incremental::exploration::{
    Scan, ScanBaryCentre, ScanType,
};

/// File what a scan says under its key, over whatever is filed there.
///
/// The tables inside a system are short — a system is tens of bodies — so a
/// scan finds its record by walking rather than by hashing, and the record
/// is rebuilt from the held one rather than edited in place.
///
/// `make` is handed the held record where there is one, and is a `Fn` rather
/// than an `FnOnce` because the stores this runs against take an `FnMut` and
/// may in principle call it twice.
pub fn put<T, K: Eq>(
    table: &mut Vec<T>,
    key: K,
    keyed: impl Fn(&T) -> K,
    make: impl Fn(Option<&T>) -> T,
) {
    match table.iter().position(|held| keyed(held) == key) {
        Some(at) => {
            let made = make(Some(&table[at]));
            table[at] = made;
        }
        None => table.push(make(None)),
    }
}

/// The earliest claim on record, which is `LEAST`'s rule.
///
/// A scan that says nothing about when a thing was found leaves what stands,
/// rather than its silence winning.
pub fn earliest(
    held: Option<DateTime<Utc>>,
    said: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    match (held, said) {
        (Some(held), Some(said)) => Some(held.min(said)),
        (held, said) => held.or(said),
    }
}

/// Who a record is filed under, which is whoever spoke latest.
///
/// `>=` as the database's `CASE WHEN $7 >= updated_at` is: a tie goes to the
/// scan that has just arrived, there being nothing to choose between them and
/// one of the two having to win.
pub(crate) fn said_by(
    held: Option<(&str, DateTime<Utc>)>,
    by: &str,
    at: DateTime<Utc>,
) -> String {
    match held {
        Some((who, when)) if at < when => who.to_string(),
        _ => by.to_string(),
    }
}

/// An orbit as a rescan leaves it.
///
/// The two elements a scan may leave out are filled from what stands, and a
/// scan naming no orbit at all — which is what a primary star's scan is —
/// keeps the one already on record rather than taking it away.
pub fn orbit(said: Option<&Orbit>, held: Option<&Orbit>) -> Option<Orbit> {
    let Some(said) = said else { return held.cloned() };
    Some(Orbit {
        ascending_node: said
            .ascending_node
            .or_else(|| held.and_then(|it| it.ascending_node)),
        mean_anomaly: said
            .mean_anomaly
            .or_else(|| held.and_then(|it| it.mean_anomaly)),
        ..said.clone()
    })
}

/// When a scan says what it looked at was found, where it says at all.
///
/// `WasDiscovered` is a fact about the scan and not about the body: whether
/// somebody had got there before the commander who wrote it. Clear means this
/// scan *is* the discovery and the entry's own time is when it happened; set
/// means somebody was there earlier and the scan says nothing about when.
///
/// A nav beacon is not read for it. It answers for every body in the system
/// at once out of what it holds rather than out of a look anybody took, and
/// its scans carry the flag clear for bodies charted before the commander was
/// born. Sol arrives that way, which taken at its word makes one commander
/// the discoverer of the solar system.
pub fn discovered_at(scan: &Scan, at: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let beacon = scan.scan_type.as_ref().is_some_and(ScanType::is_beacon);
    (!beacon && !scan.target.discovery().discovered).then_some(at)
}

/// A scanned star as the index's record of one, over what stands.
///
/// Plain assignment for everything a scan always states, which is what the
/// database does with those columns too. The rest is the rule in the module
/// header: the orbit a primary's scan does not carry, the stamp that only
/// goes forward, the mapping that only goes up, and the discovery that only
/// goes back.
pub fn star(
    address: i64,
    star: &JournalStar,
    at: DateTime<Utc>,
    by: &str,
    found: Option<DateTime<Utc>>,
    held: Option<&Star>,
) -> Star {
    let parents = Parent::chain(&star.parents);
    Star {
        system_address: address,
        id: star.id,
        name: star.name.clone(),
        parents: match (parents.is_empty(), held) {
            (true, Some(held)) => held.parents.clone(),
            (_, _) => parents,
        },
        updated_at: held.map_or(at, |held| held.updated_at.max(at)),
        updated_by: said_by(
            held.map(|held| (held.updated_by.as_str(), held.updated_at)),
            by,
            at,
        ),
        absolute_magnitude: star.absolute_magnitude,
        age_my: star.age_my,
        distance_from_arrival_ls: star.distance_from_arrival_ls,
        luminosity: star.luminosity.clone(),
        star_class: star.star_class.clone(),
        stellar_mass: star.stellar_mass,
        subclass: star.subclass,
        orbit: orbit(
            star.orbit.as_ref(),
            held.and_then(|held| held.orbit.as_ref()),
        ),
        spin: star.spin.clone(),
        radius: star.radius,
        temperature: star.temperature,
        mapped: star.discovery.mapped || held.is_some_and(|held| held.mapped),
        discovered_at: earliest(
            held.and_then(|held| held.discovered_at),
            found,
        ),
    }
}

/// A scanned body as the index's record of one, over what stands.
///
/// Same rule as [`star`], and one more: a body's surface is a block the game
/// writes only where it looked at one, so a basic scan arriving after a
/// detailed one keeps the surface, the materials and the readings it does not
/// mention rather than taking them away.
pub fn body(
    address: i64,
    body: &JournalBody,
    at: DateTime<Utc>,
    by: &str,
    found: Option<DateTime<Utc>>,
    held: Option<&Body>,
) -> Body {
    let parents = Parent::chain(&body.parents);
    let stood = held.and_then(|held| held.surface.as_ref());
    Body {
        system_address: address,
        id: body.id,
        parents: match (parents.is_empty(), held) {
            (true, Some(held)) => held.parents.clone(),
            (_, _) => parents,
        },
        name: body.name.clone(),
        body_type: body
            .ty
            .clone()
            .or_else(|| held.and_then(|held| held.body_type.clone())),
        distance_from_arrival: body
            .distance_from_arrival
            .or_else(|| held.and_then(|held| held.distance_from_arrival)),
        updated_at: held.map_or(at, |held| held.updated_at.max(at)),
        updated_by: said_by(
            held.map(|held| (held.updated_by.as_str(), held.updated_at)),
            by,
            at,
        ),
        planet_class: body.planet_class.clone(),
        // A basic scan does not report it, so what a closer look found
        // stands; a body nothing has looked at closely is not tidally
        // locked as far as anything can say.
        tidal_lock: body
            .tidal_lock
            .unwrap_or_else(|| held.is_some_and(|held| held.tidal_lock)),
        mass: body.mass,
        radius: body.radius,
        gravity: body.gravity,
        temperature: body
            .temperature
            .or_else(|| held.and_then(|held| held.temperature)),
        surface: match &body.surface {
            Some(said) => Some(surface(said, stood)),
            None => stood.cloned(),
        },
        orbit: orbit(Some(&body.orbit), held.map(|held| &held.orbit))
            .expect("a scan states a body's orbit"),
        spin: body.spin.clone(),
        mapped: body.discovery.mapped || held.is_some_and(|held| held.mapped),
        discovered_at: earliest(
            held.and_then(|held| held.discovered_at),
            found,
        ),
    }
}

/// What a body with a surface has, as the index records it.
///
/// A scan that looked at a surface states the whole of what it measured —
/// the pressure, the atmosphere type, whether it can be landed on, what it
/// is made of and what can be collected there — so those are taken as they
/// come, the materials included: a list the scan does not repeat is a list
/// the body no longer carries. The three the game writes as an empty string
/// where it has nothing to say are the exception, being absences rather
/// than readings.
///
/// The one field that differs from the journal's shape: the index's
/// composition is optional, a body stored before the fractions were kept
/// having a surface and no reading of what it is made of. A scan always
/// carries one.
pub fn surface(said: &JournalSurface, held: Option<&Surface>) -> Surface {
    Surface {
        atmosphere_type: said.atmosphere_type.clone(),
        pressure: said.pressure,
        composition: Some(said.composition.clone()),
        landable: said.landable,
        atmosphere: said
            .atmosphere
            .clone()
            .or_else(|| held.and_then(|held| held.atmosphere.clone())),
        volcanism: said
            .volcanism
            .clone()
            .or_else(|| held.and_then(|held| held.volcanism.clone())),
        terraform_state: said
            .terraform_state
            .clone()
            .or_else(|| held.and_then(|held| held.terraform_state.clone())),
        materials: said.materials.clone(),
    }
}

/// The centre of mass a close pair goes round, over what stands.
///
/// Not a body and not drawn. It is kept so that a body naming it as an
/// ancestor can be placed where it belongs rather than at the middle of its
/// system, so the orbit is the whole of what it holds — and the stamp and the
/// name of whoever last spoke, by the same rule as everything else here.
pub fn barycenter(
    center: &ScanBaryCentre,
    at: DateTime<Utc>,
    by: &str,
    held: Option<&Barycenter>,
) -> Barycenter {
    Barycenter {
        system_address: center.system_address,
        id: center.body_id,
        updated_at: held.map_or(at, |held| held.updated_at.max(at)),
        updated_by: said_by(
            held.map(|held| (held.updated_by.as_str(), held.updated_at)),
            by,
            at,
        ),
        orbit: orbit(
            center.orbit.as_ref(),
            held.and_then(|held| held.orbit.as_ref()),
        ),
    }
}

// What follows is one *stored* record over another: two directories
// folded into one, or an event's row over what a directory publishes.
// Everything above is a scan over what is stored.

/// One populated row over another, the newer winning column by column.
///
/// **A thinner row must not erase a richer one.** A row derived from events
/// publishes an empty faction list by construction — a journal names
/// factions and numbers none of them — and the body counts arrive in their
/// own events rather than with the arrival, so writing such a row straight
/// over a database-derived one takes the faction ids off the system, and the
/// map colours and filters by exactly those.
///
/// With `newer` set this is an event's row over what a directory publishes,
/// which is what `galos::sink::tables` asks of it. The other direction is
/// what a merge of two directories needs and a feed does not: an older row
/// still fills in a column nothing has ever had a word for.
pub fn populated_over(
    held: &PopulatedSystem,
    said: PopulatedSystem,
    newer: bool,
) -> PopulatedSystem {
    let (win, lose) = match newer {
        true => (said, held.clone()),
        false => (held.clone(), said),
    };
    PopulatedSystem {
        security: win.security.or(lose.security),
        government: win.government.or(lose.government),
        allegiance: win.allegiance.or(lose.allegiance),
        primary_economy: win.primary_economy.or(lose.primary_economy),
        secondary_economy: win.secondary_economy.or(lose.secondary_economy),
        // Never stated by an event-derived row, so never taken away by one.
        factions: match win.factions.is_empty() {
            true => lose.factions,
            false => win.factions,
        },
        body_count: win.body_count.or(lose.body_count),
        non_body_count: win.non_body_count.or(lose.non_body_count),
        ..win
    }
}

/// Two stored records of one system's insides, folded into one.
///
/// The scan-over-stored merges above are the twin of this and state the rule
/// it restates: a reading wins where it is one, what is not stated leaves
/// what stands, the two facts about a thing's history only go one way, and
/// the stamp only goes forward. What differs is that both sides here are
/// *stored* records, each of which has already merged every scan its own
/// directory saw, so there is no "did the scan mention this" to ask — only
/// which of two finished records is the later, and what the earlier one
/// still has to say.
///
/// So, per thing, joined by its `id` within the system:
///
/// - A star, body or barycentre on one side only is taken whole.
/// - On both sides, the greater `updated_at` wins, a tie going to the
///   arriving record as everywhere else.
/// - `discovered_at` takes the **earliest** of the two that is `Some`
///   ([`earliest`]): when a thing was found does not change, and a
///   record that does not know leaves what does.
/// - `mapped` is **OR-ed**: it only ever goes up, and a side that never
///   heard of the mapping is not a side saying it was unmapped.
/// - A field the winner leaves **blank** — `None`, an empty string or an
///   empty list — falls back to the loser's. An orbit goes through
///   [`orbit`], so the two elements an uploader may drop are filled
///   in element by element rather than the whole orbit being taken or lost.
///
/// Everything else is a reading and the winner's stands: a magnitude, a
/// radius, a temperature, whether a body is tidally locked. Zero is a
/// reading there, not an absence.
pub(crate) fn bodies_over(
    held: SystemBodies,
    said: SystemBodies,
) -> SystemBodies {
    SystemBodies {
        stars: join(held.stars, said.stars, |it| it.id, star_over),
        bodies: join(held.bodies, said.bodies, |it| it.id, body_over),
        barycenters: join(
            held.barycenters,
            said.barycenters,
            |it| it.id,
            barycenter_over,
        ),
    }
}

/// Two tables of one system's things, joined by id.
///
/// A walk rather than a hash, as [`put`] is and for the same reason:
/// a system is tens of things, and a map of them costs more than the walk it
/// replaces. The held order is kept and what only the arriving side has goes
/// on the end, so a record written twice is written the same way twice.
fn join<T>(
    held: Vec<T>,
    said: Vec<T>,
    id: impl Fn(&T) -> i16,
    over: impl Fn(T, T) -> T,
) -> Vec<T> {
    let mut said: Vec<Option<T>> = said.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(held.len() + said.len());
    for stood in held {
        let key = id(&stood);
        let found = said
            .iter()
            .position(|it| it.as_ref().is_some_and(|it| id(it) == key));
        match found {
            Some(at) => {
                let arriving = said[at].take().expect("just found");
                out.push(over(stood, arriving));
            }
            None => out.push(stood),
        }
    }
    out.extend(said.into_iter().flatten());
    out
}

/// The winner's string where it said one, and the loser's where it left the
/// field empty.
///
/// The game writes an empty string where it has nothing to say, which is an
/// absence rather than a reading — [`surface`] calls out the three
/// fields it does this for.
fn stated(win: String, lose: String) -> String {
    match win.is_empty() {
        true => lose,
        false => win,
    }
}

/// Two stored stars of one system, folded. See [`bodies_over`].
fn star_over(held: Star, said: Star) -> Star {
    let (win, lose) = match said.updated_at >= held.updated_at {
        true => (said, held),
        false => (held, said),
    };
    let orbit = orbit(win.orbit.as_ref(), lose.orbit.as_ref());
    let mapped = win.mapped || lose.mapped;
    let discovered_at = earliest(win.discovered_at, lose.discovered_at);
    Star {
        name: stated(win.name, lose.name),
        parents: match win.parents.is_empty() {
            true => lose.parents,
            false => win.parents,
        },
        updated_by: stated(win.updated_by, lose.updated_by),
        luminosity: stated(win.luminosity, lose.luminosity),
        star_class: stated(win.star_class, lose.star_class),
        orbit,
        mapped,
        discovered_at,
        ..win
    }
}

/// Two stored bodies of one system, folded. See [`bodies_over`].
fn body_over(held: Body, said: Body) -> Body {
    let (win, lose) = match said.updated_at >= held.updated_at {
        true => (said, held),
        false => (held, said),
    };
    // A body's orbit is not optional — a scan always states one — so the
    // fill is only of the two elements an uploader may have dropped.
    let orbit =
        orbit(Some(&win.orbit), Some(&lose.orbit)).expect("two stated orbits");
    let mapped = win.mapped || lose.mapped;
    let discovered_at = earliest(win.discovered_at, lose.discovered_at);
    Body {
        parents: match win.parents.is_empty() {
            true => lose.parents,
            false => win.parents,
        },
        name: stated(win.name, lose.name),
        body_type: win.body_type.or(lose.body_type),
        distance_from_arrival: win
            .distance_from_arrival
            .or(lose.distance_from_arrival),
        updated_by: stated(win.updated_by, lose.updated_by),
        planet_class: stated(win.planet_class, lose.planet_class),
        temperature: win.temperature.or(lose.temperature),
        // A gas giant has no surface to record, and a record with one is a
        // record of a closer look: the winner's where it has one, and the
        // loser's rather than nothing where it has not.
        surface: match (win.surface, lose.surface) {
            (Some(win), Some(lose)) => Some(surface_over(win, lose)),
            (win, lose) => win.or(lose),
        },
        orbit,
        mapped,
        discovered_at,
        ..win
    }
}

/// Two stored surfaces of one body, folded. See [`bodies_over`].
fn surface_over(win: Surface, lose: Surface) -> Surface {
    Surface {
        composition: win.composition.or(lose.composition),
        atmosphere: win.atmosphere.or(lose.atmosphere),
        volcanism: win.volcanism.or(lose.volcanism),
        terraform_state: win.terraform_state.or(lose.terraform_state),
        materials: match win.materials.is_empty() {
            true => lose.materials,
            false => win.materials,
        },
        ..win
    }
}

/// Two stored barycentres of one system, folded. See [`bodies_over`].
///
/// The orbit is the whole of what one holds: a barycentre is not drawn and
/// is kept so a body naming it as an ancestor can be placed.
fn barycenter_over(held: Barycenter, said: Barycenter) -> Barycenter {
    let (win, lose) = match said.updated_at >= held.updated_at {
        true => (said, held),
        false => (held, said),
    };
    let orbit = orbit(win.orbit.as_ref(), lose.orbit.as_ref());
    Barycenter {
        updated_by: stated(win.updated_by, lose.updated_by),
        orbit,
        ..win
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::SystemBodies;
    use elite_journal::body::{Orbit, Spin};

    fn stamp(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).expect("a time")
    }

    fn a_star(id: i16, when: i64) -> Star {
        Star {
            system_address: 1,
            id,
            name: format!("Star {id}"),
            parents: Vec::new(),
            updated_at: stamp(when),
            updated_by: "a test".to_owned(),
            absolute_magnitude: 4.83,
            age_my: 4_600,
            distance_from_arrival_ls: 0.0,
            luminosity: "V".to_owned(),
            star_class: "G".to_owned(),
            stellar_mass: 1.0,
            subclass: 2,
            orbit: None,
            spin: Spin { period: 1.0, tilt: 0.0 },
            radius: 1.0,
            temperature: 5_778.0,
            mapped: false,
            discovered_at: None,
        }
    }

    fn a_body(id: i16, when: i64) -> Body {
        Body {
            system_address: 1,
            id,
            parents: Vec::new(),
            name: format!("Body {id}"),
            body_type: None,
            distance_from_arrival: Some(10.0),
            updated_at: stamp(when),
            updated_by: "a test".to_owned(),
            planet_class: "Icy body".to_owned(),
            tidal_lock: false,
            mass: 1.0,
            radius: 1.0,
            gravity: 1.0,
            temperature: Some(100.0),
            surface: None,
            orbit: Orbit {
                semi_major_axis: 1.0,
                eccentricity: 0.0,
                orbital_inclination: 0.0,
                periapsis: 0.0,
                orbital_period: 1.0,
                ascending_node: None,
                mean_anomaly: None,
            },
            spin: Spin { period: 1.0, tilt: 0.0 },
            mapped: false,
            discovered_at: None,
        }
    }

    /// Two stored records of one system are folded thing by thing: the
    /// later reading wins, the mapping only goes up, the discovery only
    /// goes back, a blank falls through, and a thing only one side has is
    /// kept.
    #[test]
    fn a_body_on_both_sides_takes_the_newer_reading() {
        let held = SystemBodies {
            stars: vec![
                Star {
                    temperature: 4_000.0,
                    mapped: true,
                    discovered_at: Some(stamp(500)),
                    ..a_star(0, 100)
                },
                a_star(2, 100),
            ],
            bodies: vec![a_body(1, 100)],
            barycenters: Vec::new(),
        };
        let said = SystemBodies {
            stars: vec![
                Star {
                    temperature: 5_100.0,
                    // The winner leaves it blank, so the loser's stands.
                    luminosity: String::new(),
                    mapped: false,
                    discovered_at: Some(stamp(300)),
                    ..a_star(0, 200)
                },
                a_star(3, 200),
            ],
            bodies: Vec::new(),
            barycenters: vec![Barycenter {
                system_address: 1,
                id: 4,
                updated_at: stamp(200),
                updated_by: "a test".to_owned(),
                orbit: None,
            }],
        };

        let merged = bodies_over(held, said);

        let star = |id: i16| {
            merged
                .stars
                .iter()
                .find(|it| it.id == id)
                .unwrap_or_else(|| panic!("star {id} was dropped"))
        };
        let primary = star(0);
        assert_eq!(primary.updated_at, stamp(200), "the later reading");
        assert_eq!(primary.temperature, 5_100.0, "the later reading");
        assert_eq!(primary.luminosity, "V", "a blank fell through");
        assert!(primary.mapped, "mapping only goes up");
        assert_eq!(
            primary.discovered_at,
            Some(stamp(300)),
            "discovery only goes back",
        );

        star(2);
        star(3);
        assert_eq!(merged.stars.len(), 3);
        assert_eq!(merged.bodies.len(), 1, "a body only one side had is kept");
        assert_eq!(merged.bodies[0].id, 1);
        assert_eq!(merged.barycenters.len(), 1);
        assert_eq!(merged.barycenters[0].id, 4);
    }

    /// The tie goes to the arriving record, as it does everywhere else the
    /// program merges by a stamp.
    #[test]
    fn a_tie_goes_to_the_arriving_body() {
        let held = SystemBodies {
            stars: vec![Star { temperature: 4_000.0, ..a_star(0, 100) }],
            ..SystemBodies::default()
        };
        let said = SystemBodies {
            stars: vec![Star { temperature: 5_100.0, ..a_star(0, 100) }],
            ..SystemBodies::default()
        };
        let merged = bodies_over(held, said);
        assert_eq!(merged.stars[0].temperature, 5_100.0);
    }

    /// A thinner populated row does not erase a richer one, in either
    /// direction: the newer wins where it says something, and the older
    /// fills only what nothing has ever said.
    #[test]
    fn a_thinner_populated_row_fills_rather_than_erases() {
        let stood = PopulatedSystem {
            address: 1,
            name: "SOL".into(),
            position: [0.0; 3],
            population: 22_780_919_531,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            factions: vec![1, 2, 3],
            body_count: Some(40),
            non_body_count: None,
        };
        let said = PopulatedSystem {
            population: 1,
            factions: Vec::new(),
            body_count: None,
            non_body_count: Some(7),
            ..stood.clone()
        };

        let newer = populated_over(&stood, said.clone(), true);
        assert_eq!(newer.population, 1, "the newer reading of the column");
        assert_eq!(newer.factions, vec![1, 2, 3], "never stated, never taken");
        assert_eq!(
            newer.body_count,
            Some(40),
            "a blank filled from what stood",
        );
        assert_eq!(newer.non_body_count, Some(7));

        let older = populated_over(&stood, said, false);
        assert_eq!(older.population, 22_780_919_531, "the standing reading");
        assert_eq!(older.factions, vec![1, 2, 3]);
        assert_eq!(older.body_count, Some(40));
        assert_eq!(older.non_body_count, Some(7), "still filled a blank");
    }
}
