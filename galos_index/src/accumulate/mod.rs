//! Events and reports, accumulated into what the index wants.
//!
//! [`galaxy`] takes events one at a time — from a feed or a commander's own
//! files — and keeps the [`System`](crate::System) records and the metadata
//! rows they come to. [`report`] is the one shape every source states a
//! system in, [`merge`] what a second report or scan does to what stands,
//! and [`bodies`] where a system's insides are kept between one scan and the
//! next.

pub mod bodies;
pub mod galaxy;
pub mod merge;
pub mod report;
