//! What a second look at a body does to what is on record.
//!
//! The peer of [`crate::report`], one level down. That one merges what a
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

use crate::meta::{Barycenter, Body, Parent, Star, Surface};
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
pub fn said_by(
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
