//! The on-disk format: where each file lives and what is in the ones that
//! are not a table of records.
//!
//! [`layout`] names every file and directory. [`payload`] is a cell's byte
//! layout and the version the index file is held to. [`msgpack`] is how a
//! metadata table is written and read back. [`checkpoint`] is the resume
//! point a writer keeps beside a directory, and [`lock`] is how one writer
//! at a time is kept to. The rest is scratch a build writes and nothing
//! serves.

pub mod checkpoint;
pub mod layout;
pub mod lock;
pub mod msgpack;
pub mod payload;
pub(crate) mod rows;
pub(crate) mod spill;
