//! A body within a star system
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq)]
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
    pub atmosphere_type: String,
    pub volcanism: Option<String>,

    pub mass: f32,
    pub radius: f32,
    pub surface_gravity: f32,
    pub surface_temperature: f32,
    pub surface_pressure: f32,
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
