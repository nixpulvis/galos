//! The on-disk format: where each file lives and what is in the ones that
//! are not a table of records, down to the
//! [`Checkpoint`](checkpoint::Checkpoint) a writer resumes from and the
//! [`Lock`](lock::Lock) that keeps it alone.
//!
//! [`layout`] names every file and directory. [`payload`] is a cell's byte
//! layout and the version the index file is held to. [`msgpack`] is how a
//! metadata table is written and read back. [`checkpoint`] is the resume
//! point a writer keeps beside a directory, and [`lock`] is how one writer
//! at a time is kept to. [`parts`] names the index's own parts. The rest is
//! scratch a build writes and nothing serves.

pub mod checkpoint;
pub mod layout;
pub mod lock;
pub mod msgpack;
pub mod parts;
pub mod payload;
pub(crate) mod rows;
pub(crate) mod spill;
