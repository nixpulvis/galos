//! The center of mass a close pair of bodies goes round
//!
//! Stored apart from [`crate::bodies`] because it is not a body. A scan of one
//! carries a system, an id and an orbit, and none of the mass, radius, class or
//! surface a body is described by. Nothing here is drawn: a barycenter is a
//! point that other things hang off, and what it is worth is that a body naming
//! it as an ancestor can be placed where it belongs.
use chrono::{DateTime, Utc};
use elite_journal::body::Orbit;

/// Clone for the same reason [`crate::bodies::Body`] is: it outlives the query
/// it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Barycenter {
    pub system_address: i64,
    pub id: i16,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    /// [`None`] for one that goes round nothing, which is what the barycenter
    /// at the root of a multi-star system is
    pub orbit: Option<Orbit>,
}

impl Eq for Barycenter {}

mod create;
mod fetch;
