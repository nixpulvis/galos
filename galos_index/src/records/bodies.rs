//! What a system holds inside it: its stars, bodies and barycentres.
//!
//! The record a click into a system pulls, one per system and read one at a
//! time — see [`SystemBodies`]. The scan fields mirror the `galos_db` structs
//! field for field and reuse the `elite_journal` enums, so the client
//! renders them through the same code it rendered database rows through,
//! changing only the type it names. Serde records rather than hand-rolled
//! `FixedCodec`, since they are variable, nested and read one system at a
//! time rather than a million points a frame.

use chrono::{DateTime, Utc};
use elite_journal::body::{
    AtmosphereType, BodyType, Composition, Material, Orbit, Spin,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// One ancestor of a body, as the scan named it, nearest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parent {
    pub ty: Option<String>,
    pub id: i16,
}

impl Parent {
    /// Whether this ancestor is a barycentre
    ///
    /// The journal names a barycentre parent `Null` and nothing else by that
    /// name, so the type alone answers it. Worth asking even where the
    /// barycentre has no row of its own: what kind of thing a chain names is
    /// known from the naming, and only its own orbit waits on its scan.
    pub fn is_barycenter(&self) -> bool {
        self.ty.as_deref() == Some("Null")
    }

    /// The ancestry a scan named, nearest first.
    ///
    /// A scan writes each ancestor as a one-entry map of kind to id, and the
    /// walk back to the star is what places the thing, so the order and the
    /// whole chain are kept. `galos_db::bodies::Parent::chain`'s rule, in
    /// the index's own vocabulary.
    pub fn chain(named: &[BTreeMap<String, i16>]) -> Vec<Parent> {
        named
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some(Parent { ty: Some(ty.clone()), id: *id })
            })
            .collect()
    }
}

/// What a body with a surface has, and a gas giant has none of.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Surface {
    pub atmosphere_type: AtmosphereType,
    pub pressure: f32,
    pub composition: Option<Composition>,
    pub landable: bool,
    pub atmosphere: Option<String>,
    pub volcanism: Option<String>,
    pub terraform_state: Option<String>,
    pub materials: Vec<Material>,
}

/// A star within a system, with the fields a scan and its photometry carry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Star {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    /// Every ancestor the scan named, nearest first, empty for the primary.
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
    /// [`None`] for the primary, which goes round nothing.
    pub orbit: Option<Orbit>,
    pub spin: Spin,
    pub radius: f32,
    pub temperature: f32,
    /// Whether anybody had mapped the star when it was scanned, which is a
    /// fact about the star rather than about the scan the way the discovery
    /// flag that used to sit beside this was.
    pub mapped: bool,
    /// When the star was found, where a scan on record says so.
    ///
    /// [`None`] where every scan on record found it already discovered, such
    /// a scan saying somebody had been there without saying when.
    pub discovered_at: Option<DateTime<Utc>>,
}

/// A body within a system.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    /// Every ancestor the scan named, nearest first.
    pub parents: Vec<Parent>,
    pub name: String,
    pub body_type: Option<BodyType>,
    /// How far from the arrival star, in light seconds.
    pub distance_from_arrival: Option<f32>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    pub planet_class: String,
    pub tidal_lock: bool,
    pub mass: f32,
    pub radius: f32,
    /// Measured at the cloud tops where there is no surface, so not in
    /// [`Surface`].
    pub gravity: f32,
    pub temperature: Option<f32>,
    /// [`None`] for a gas giant, which has no surface to record.
    pub surface: Option<Surface>,
    pub orbit: Orbit,
    pub spin: Spin,
    /// Whether anybody had mapped the body when it was scanned, which is a
    /// fact about the body rather than about the scan the way the discovery
    /// flag that used to sit beside this was.
    pub mapped: bool,
    /// When the body was found, where a scan on record says so.
    ///
    /// [`None`] where every scan on record found it already discovered, such
    /// a scan saying somebody had been there without saying when.
    pub discovered_at: Option<DateTime<Utc>>,
}

/// The center of mass a close pair of bodies goes round.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Barycenter {
    pub system_address: i64,
    pub id: i16,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    /// [`None`] for the barycenter at the root of a multi-star system.
    pub orbit: Option<Orbit>,
}

/// Everything a click into a system pulls: its stars, bodies and barycenters.
///
/// One file per system, keyed by address, so the map fetches exactly the
/// system a click opened and nothing else. Empty where a system has no scan on
/// record, which reads the same as a system whose file was never written.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SystemBodies {
    pub stars: Vec<Star>,
    pub bodies: Vec<Body>,
    pub barycenters: Vec<Barycenter>,
}

impl Star {
    /// The nearest ancestor, which is what the star's orbit is measured about.
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}

impl Body {
    /// The nearest ancestor, which is what the body's orbit is measured about.
    pub fn parent_id(&self) -> Option<i16> {
        self.parents.first().map(|parent| parent.id)
    }
}
