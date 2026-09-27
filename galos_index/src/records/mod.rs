//! The serving records: what a reader reads beside the cells, by the row
//! ([`PopulatedSystem`], [`SystemReach`]) or by the system ([`SystemBodies`]).
//!
//! Two families. The tables ([`PopulatedSystem`], [`SystemReach`],
//! [`NameEntry`], [`Faction`]) are one row a system, read resident or
//! searched; the bodies ([`SystemBodies`] and what it holds) are one record
//! a system, read when a click opens it. [`derive`](mod@derive) is the rules
//! both derivations of the index answer alike from those records.
//!
//! A dependent's own table is neither: its row is its own type, derived from
//! a system's [`crate::system::System`] ([`crate::store::tables::Table`]).

mod bodies;
pub mod derive;
mod tables;

pub use bodies::{Barycenter, Body, Parent, Star, Surface, SystemBodies};
pub use tables::{Economies, Faction, NameEntry, PopulatedSystem, SystemReach};
