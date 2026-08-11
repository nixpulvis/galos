//! A stretch of a belt, scanned as a body and measured as nothing
//!
//! Stored apart from [`crate::bodies`] because it is not a body. A scan of one
//! carries a name, an id, the ring it lies in and whether it had been found
//! before, and none of the class, mass, radius or temperature a body is
//! described by. There is no single object there to weigh.
//!
//! A quarter of the scans EDDN carries are these, and what they are worth is
//! the system they name: before there was anywhere to put one, the whole
//! message was dropped and the system went unrecorded with it.
use chrono::{DateTime, Utc};

/// Clone for the same reason [`crate::bodies::Body`] is: it outlives the query
/// it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Cluster {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    pub distance_from_arrival: Option<f32>,
    pub discovered: bool,
    pub mapped: bool,
    /// Nearest ancestor first, as [`crate::bodies`] keeps them. The first is
    /// the ring the cluster lies in.
    pub parent_ids: Vec<i16>,
    pub parent_types: Vec<String>,
}

impl Eq for Cluster {}

mod create;
mod fetch;
