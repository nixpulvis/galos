//! A star within a system
use crate::bodies::Parent;
use chrono::{DateTime, Utc};
use elite_journal::body::{Orbit, Spin};

/// Clone because the map carries one into a component and into whatever
/// panel is describing it, and a star outlives the query it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Star {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    /// Every ancestor the scan named, nearest first, and empty for the primary
    pub parents: Vec<Parent>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub absolute_magnitude: f32,
    pub age_my: i32,
    pub distance_from_arrival_ls: f32,
    pub luminosity: String,
    pub star_class: String,
    pub stellar_mass: f32,
    pub subclass: i16,

    /// [`None`] for the primary, which goes round nothing
    pub orbit: Option<Orbit>,
    pub spin: Spin,
    pub radius: f32,
    pub temperature: f32,
    /// Whether anybody had mapped the star when it was scanned
    pub mapped: bool,
    /// When the star was found, where a scan on record says so
    ///
    /// [`None`] for a star every scan of which found it already discovered:
    /// such a scan says somebody got there earlier without saying when.
    pub discovered_at: Option<DateTime<Utc>>,
}

impl Eq for Star {}

impl Star {
    /// The nearest ancestor, which is what the star's orbit is measured about
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}

/// Turn a row of `stars` into one
///
/// Every query below selects the same columns and differs only in what it
/// selects by, so the mapping between a row and a [`Star`] is written once
/// here. `sqlx::query!` gives each query an anonymous row type of its own,
/// so this is a macro rather than a function: there is no one type to name
/// in a signature. It expands where it is called, so the caller is the one
/// holding the imports it reads.
macro_rules! star {
    ($row:expr) => {{
        let row = $row;
        Star {
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            // Read back rather than answered with, since the row may hold
            // an ancestry the message that wrote it did not name.
            parents: ancestry(row.parent_ids, row.parent_types),
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,

            absolute_magnitude: row.absolute_magnitude,
            age_my: row.age_my,
            distance_from_arrival_ls: row.distance_from_arrival_ls,
            luminosity: row.luminosity,
            star_class: row.star_class,
            stellar_mass: row.stellar_mass,
            subclass: row.subclass,

            orbit: orbit::read(
                row.semi_major_axis,
                row.eccentricity,
                row.orbital_inclination,
                row.periapsis,
                row.orbital_period,
                row.ascending_node,
                row.mean_anomaly,
            ),
            spin: Spin { period: row.rotation_period, tilt: row.axial_tilt },
            radius: row.radius,
            temperature: row.temperature,
            mapped: row.was_mapped,
            discovered_at: row.discovered_at.map(|at| at.and_utc()),
        }
    }};
}

mod create;
mod fetch;
