//! A star within a system
use crate::bodies::Parent;
use chrono::{DateTime, Utc};
use elite_journal::body::{Discovery, Orbit, Spin};

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
    pub discovery: Discovery,
}

impl Eq for Star {}

impl Star {
    /// The nearest ancestor, which is what the star's orbit is measured about
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}

mod create;
mod fetch;
