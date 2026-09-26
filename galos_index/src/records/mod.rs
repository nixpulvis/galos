//! The serving records: what the client reads beside the cells, by the row.
//!
//! Two families. The tables ([`PopulatedSystem`], [`SystemReach`],
//! [`SystemBoost`], [`NameEntry`], [`Faction`]) are one row a system, read
//! resident or searched; the bodies ([`SystemBodies`] and what it holds) are
//! one record a system, read when a click opens it. [`derive`](mod@derive) is
//! the rules both derivations of the index answer alike from those records.

mod bodies;
pub mod derive;
mod tables;

pub use bodies::{Barycenter, Body, Parent, Star, Surface, SystemBodies};
pub use tables::{
    Economies, Faction, NameEntry, PopulatedSystem, SystemBoost, SystemReach,
};
