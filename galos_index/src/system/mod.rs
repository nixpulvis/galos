//! One system: where it is in the galaxy, and what is inside it.
//!
//! [`System`] is a system as a point in the galaxy, at the precision the
//! build needs. [`bodies`] is what is inside it — the arrangement of its
//! stars, bodies and barycentres: what goes round what, where each stands,
//! and how far the whole reaches — and [`bodies::orbit`] is the Kepler
//! arithmetic that places each of them.

pub mod bodies;

use crate::core::star::StarKind;
use serde::{Deserialize, Serialize};

/// One system as the index is built *from*: where it is, the photometry the
/// ordering and the aggregates need, and when it was last updated, all at
/// full precision.
///
/// **One of two records of a system, and the one that is written.** The
/// database, a catalog and the feed each hand the build a [`System`]; the live
/// [`crate::Tree`] holds one per system; a resume point is a galaxy of them on
/// disk. What the index is built *into* is a [`CellSystem`]: the same system
/// packed into a cell's payload, which is all a reader — the map, the router,
/// the walks — ever sees. [`CellSystem::of`] is the one conversion, and it
/// only goes this way:
///
/// | | [`System`] | [`CellSystem`] |
/// |---|---|---|
/// | id, position, `updated_at`, star kind | kept | kept, unchanged |
/// | absolute magnitude | `f64` | `f32` |
/// | temperature | kelvin | its [`TempBucket`] |
/// | `age_bucket` | kept | dropped: the cell's aggregate counts it |
/// | who holds it | the build, the live tree, the resume point | a cell's payload, and every reader |
///
/// Absolute magnitude and temperature are the finished figures from the
/// photometry fallback chain (scanned stars summed, else the primary's class,
/// else a default), not anything the build works out. `age_bucket` is the
/// Recency axis the caller has already binned; `updated_at` is the same fact
/// unbinned, Unix seconds, so the Recency filter has something finer than a
/// day to test. The caller bins one from the other off one reading.
///
/// What a [`CellSystem`] does not keep, and why the build cannot do without it:
/// the `f64` magnitude, which a cell's flux is summed from and the payload
/// is ordered by; the raw temperature, which the aggregate buckets itself;
/// and `age_bucket`, which is binned against the clock the report was read
/// at and cannot be worked out again from `updated_at` later.
///
/// The record is written to disk as its own bytes, so it is `repr(C)`,
/// sixty-four bytes, and padding-free; see [`crate::format::checkpoint`].
///
/// [`CellSystem`]: crate::tree::cell::CellSystem
/// [`CellSystem::of`]: crate::tree::cell::CellSystem::of
/// [`TempBucket`]: crate::core::aggregate::TempBucket
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct System {
    pub id64: u64,
    pub position: [f64; 3],
    pub absolute_magnitude: f64,
    pub temperature: f64,
    pub age_bucket: u32,
    pub updated_at: u32,
    /// What kind of star a ship arrives at
    ///
    /// Carried through the build so the payload can hold it: whether a ship
    /// refuels and whether it supercharges are this one byte, and the
    /// router reads it per expansion. See [`StarKind`].
    pub kind: StarKind,
}

/// The record width the resume point's format is written against. A field
/// added here without the format being told would read a checkpoint of one
/// galaxy back as another, so it fails the build instead.
///
/// Sixty-four with the star kind on it, where it was fifty-six: the byte
/// did not fit the `u32` pair's tail and took a word of its own. A resume
/// point written at the old width would be read as another galaxy, so
/// [`crate::format::checkpoint`]'s `VERSION` moved with it and a stale one is
/// refused rather than misread.
const _: () = assert!(std::mem::size_of::<System>() == 64);
const _: () = assert!(std::mem::align_of::<System>() == 8);
