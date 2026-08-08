//! What hangs in a system without being a body
//!
//! Stations, megaships, installations, beacons, and the unidentified sources
//! that come and go.
//!
//! A row is what is there now rather than a log of sightings: the same handful
//! of signals is reported over and over by everyone who passes through, and
//! keeping each report would say nothing the last one did not.
//!
//! Nothing here expires. The journal says how long a transient source has
//! left and EDDN strips it, so the age of a row is the only evidence there is.
//! [`SystemSignal::is_station`] is the exception worth trusting: what it marks
//! is permanent.
use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemSignal {
    pub system_address: i64,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    pub signal_type: Option<String>,
    /// Permanent where [`Some(true)`]
    pub is_station: Option<bool>,
    pub uss_type: Option<String>,
    pub spawning_state: Option<String>,
    pub spawning_faction: Option<String>,
    pub spawning_power: Option<String>,
    pub opposing_power: Option<String>,
    pub threat_level: Option<i32>,
}

mod create;
mod fetch;
