//! A body within a star system
use chrono::{DateTime, Utc};
use elite_journal::body::AtmosphereType;

/// Clone because the map carries one into a component and into whatever
/// panel is describing it, and a body outlives the query it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    pub parent_id: Option<i16>,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub planet_class: String,
    pub tidal_lock: bool,
    pub landable: bool,
    pub terraform_state: Option<String>,
    pub atmosphere: Option<String>,
    pub volcanism: Option<String>,

    pub mass: f32,
    pub radius: f32,
    pub surface_gravity: f32,
    pub surface_temperature: f32,
    /// [`None`] for a gas giant, which has no surface to record
    pub surface: Option<Surface>,
    pub semi_major_axis: f32,
    pub eccentricity: f32,
    pub orbital_inclination: f32,
    pub periapsis: f32,
    pub orbital_period: f32,
    pub rotation_period: f32,
    pub axial_tilt: f32,
    pub ascending_node: f32,
    pub mean_anomaly: f32,

    pub was_mapped: bool,
    pub was_discovered: bool,
}

impl Eq for Body {}

mod create;
mod fetch;

/// What a body with a surface has, less the composition no column holds
#[derive(Clone, Debug, PartialEq)]
pub struct Surface {
    pub atmosphere_type: AtmosphereType,
    pub pressure: f32,
}

impl Surface {
    pub(crate) fn read(
        atmosphere_type: Option<String>,
        pressure: Option<f32>,
    ) -> Option<Self> {
        Some(Self {
            atmosphere_type: AtmosphereType::from(atmosphere_type?.as_str()),
            pressure: pressure?,
        })
    }
}
