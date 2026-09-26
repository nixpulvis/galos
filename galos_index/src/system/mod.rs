//! Inside one system: where each star and body is, and how they are
//! arranged.
//!
//! [`orbit`] is Kepler's problem — an ellipse, a tilt and a moment turned
//! into a place. [`inside`] is the arrangement: one system's records read
//! as a tree of what goes round what, placed.

pub mod inside;
pub mod orbit;
