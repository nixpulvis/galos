//! The codex: a kind of thing, found somewhere
//!
//! One row per kind of thing per system, however many times it is found there.
//! Whether the commander who sent it was first to it is not recorded and
//! cannot be: EDDN strips that as personal data, so this says a thing was
//! found in a place, not that it was discovered.
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq)]
pub struct CodexEntry {
    pub system_address: i64,
    /// The game's own id for the kind of thing, stable across sightings
    pub entry_id: i64,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub name: Option<String>,
    pub category: Option<String>,
    pub sub_category: Option<String>,
    pub region: Option<String>,

    pub body_id: Option<i16>,
    pub body_name: Option<String>,
    pub nearest_destination: Option<String>,
    /// Where on a surface it was found, for the ones found on one
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

mod create;
mod fetch;
