//! Tables a dependent publishes beside the index's own.
//!
//! The index keeps what a system *is* — where it sits and what kind of star a
//! ship arrives at, both in every payload. What a dependent makes of that is
//! its own business: the router's supercharge table is a jet cone per system
//! that has one, which is a fact about ships rather than the sky, and so it
//! is `galos_route`'s type and not this crate's.
//!
//! A dependent says what its table is by implementing [`Table`]: a name, a
//! row, and how a system's [`Arrival`] comes to one. The index does the
//! rest — holds the table open across a run, writes it beside its own,
//! carries it through a merge of two directories, and hands it back to a
//! reader — through [`crate::store::tables`], without knowing what a row
//! says.

use crate::core::record::StarKind;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fmt;
use std::io;
use std::path::Path;

/// What the index knows about one system that a contributed table derives
/// its row from.
///
/// The same three facts every payload carries: the address, the star a ship
/// arrives at, and where the system sits. Made only by
/// [`crate::records::derive::arrival`], for a system the index has placed
/// and has been told something about: a system nothing has said anything
/// of has no arrival, and a table is not asked about it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Arrival {
    pub address: i64,
    /// The star a ship arrives at, by
    /// [`crate::records::derive::arrival_kind`]. Never
    /// [`StarKind::Unknown`].
    pub kind: StarKind,
    /// Where the system sits, in light years, at the `f32` the tables
    /// publish places at.
    pub position: [f32; 3],
}

/// A table a dependent publishes beside the index: one row a system at most,
/// a function of the system's [`Arrival`].
///
/// Written as one MessagePack array of rows in address order, at
/// `<NAME>.bin`, like the index's own tables. A directory without the file
/// is one that cannot say, told apart from one holding an empty table.
pub trait Table: Send + Sync + 'static {
    /// What the table is called: its file is `<NAME>.bin`, and a log line, a
    /// verify and `--only` name it this. Unique among a directory's tables.
    const NAME: &'static str;

    /// What the table is for, in a line: said where it is named, as the
    /// help for `--only` beside the index's own parts.
    const ABOUT: &'static str;

    /// One system's row.
    type Row: Serialize
        + DeserializeOwned
        + Clone
        + PartialEq
        + fmt::Debug
        + Send
        + Sync
        + 'static;

    /// The system a row is about.
    fn address(row: &Self::Row) -> i64;

    /// The row for a system that arrives as `arrival`, or [`None`] where the
    /// table has nothing to say about it — which takes out a row that stood
    /// for it, the arrival being a statement about the system.
    fn derive(arrival: &Arrival) -> Option<Self::Row>;

    /// Bring forward a table an older build of the dependent wrote in
    /// another shape, answering how many rows it rewrote, or [`None`] where
    /// the table is current or absent.
    ///
    /// Run on every open of a directory ([`crate::ops::migrate`]) and by
    /// `galos index upgrade`, so it has to be cheap on a current table.
    fn upgrade(dir: &Path) -> io::Result<Option<usize>> {
        let _ = dir;
        Ok(None)
    }
}
