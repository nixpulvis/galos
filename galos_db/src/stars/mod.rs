//! A body within a star system
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq)]
pub struct Star {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    pub parent_id: Option<i16>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub absolute_magnitude: f32,
    pub age_my: i32,
    pub distance_from_arrival_ls: f32,
    pub luminosity: String,
    pub star_class: String,
    pub stellar_mass: f32,
    pub subclass: i16,

    pub ascending_node: f32,
    pub axial_tilt: f32,
    pub eccentricity: f32,
    pub mean_anomaly: f32,
    pub orbital_inclination: f32,
    pub orbital_period: f32,
    pub periapsis: f32,
    pub radius: f32,
    pub rotation_period: f32,
    pub semi_major_axis: f32,
    pub surface_temperature: f32,

    pub was_mapped: bool,
    pub was_discovered: bool,
}

impl Eq for Star {}

mod create;
// mod fetch;
