//! The stores a served directory is made of, each read and written here.
//!
//! [`cells`] is the payload files, one a cell. [`bodies`] is every system's
//! insides, packed a shard to a file. [`names`] is the names table, mapped
//! and searched. [`sidecars`] holds the metadata tables open across a run.

pub mod bodies;
pub mod cells;
pub mod names;
pub mod sidecars;
