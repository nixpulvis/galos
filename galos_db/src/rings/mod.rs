//! A ring, which belongs to a body and is numbered as one
//!
//! Two things are true of a ring and this table holds one of them.
//!
//! It is a feature of the body that carries it, and what it is made of is
//! reported that way: a planet's own scan lists its rings with a class, a mass
//! and an inner and outer radius. None of that is here, because nothing reads
//! it yet.
//!
//! It is also a body in the system's numbering, scanned in its own right and
//! named as an ancestor by what lies in it. Every belt cluster on record names
//! a ring as its nearest parent, by the id a row here is keyed on, so without
//! these rows that walk back to the star stops at a number with nothing behind
//! it. That is the same reason [`crate::barycenters`] stands apart.
//!
//! Stored beside [`crate::clusters`] rather than in it. A cluster lies in a ring
//! and carries no orbit; a ring goes round a body and always carries one, which
//! is how a scan of each is told from the other.
use chrono::{DateTime, Utc};
use elite_journal::body::Orbit;

/// Clone for the same reason [`crate::bodies::Body`] is: it outlives the query
/// it came back in.
#[derive(Clone, Debug, PartialEq)]
pub struct Ring {
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
    pub distance_from_arrival: Option<f32>,
    pub discovered: bool,
    pub mapped: bool,
    /// Nearest ancestor first, as [`crate::bodies`] keeps them. The first is
    /// the body the ring goes round.
    pub parent_ids: Vec<i16>,
    pub parent_types: Vec<String>,
    /// Never absent, being what tells a ring from a belt cluster
    pub orbit: Orbit,
}

impl Eq for Ring {}

mod create;
mod fetch;
