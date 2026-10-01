//! The cells' bytes: the index file's fixed records, a payload's columns,
//! and the version the index file is held to.
//!
//! The index file is a header and a run of fixed-width [`Cell`] records, each
//! spelled out with `record!` below down to the fields it is built of — a
//! [`CellId`] as level plus Morton key, an [`Aggregate`], the [`Moments`]
//! inside it.
//!
//! A cell's payload is columns rather than records — a 12-byte header, then
//! runs of position, star kind, address and when each was last heard from —
//! and a position is an integer count of [`POSITION_STEP`] off the cell's own
//! low corner, so a block is read *with* its cell rather than standing on its
//! own.
//!
//! Neither holds any light. How bright a cell and its systems are is the
//! photometry sidecar's, [`crate::codec::lights`], which is these files'
//! neighbour and is read only by the realistic view.
//!
//! Nothing is frozen yet: the version moves whenever a directory at the old
//! one has to be brought forward rather than rebuilt (see [`INDEX_VERSION`]),
//! and the index record keeps growing, as the aggregate gains the field
//! step's filter marginals and its quantization.

use crate::codec::bytes::{Decode, Encode, FixedCodec, record};
use crate::core::aggregate::{AGE_BUCKETS, Aggregate};
use crate::core::geometry::CellId;
use crate::core::moments::Moments;
use crate::core::star::StarKind;
use crate::tree::cell::Cell;
use crate::tree::cell::CellSystem;
use crate::tree::index::Index;

/// A cell's address: its level, then its Morton key.
impl Encode for CellId {
    fn encode(&self, out: &mut Vec<u8>) {
        self.level.encode(out);
        self.morton().encode(out);
    }
}

impl Decode for CellId {
    fn decode(cur: &mut &[u8]) -> Option<CellId> {
        let level = u8::decode(cur)?;
        Some(CellId::from_morton(level, u64::decode(cur)?))
    }
}

impl FixedCodec for CellId {
    const LEN: usize = u8::LEN + u64::LEN;
}

/// A star kind, as [`StarKind::code`].
impl Encode for StarKind {
    fn encode(&self, out: &mut Vec<u8>) {
        self.code().encode(out);
    }
}

impl Decode for StarKind {
    fn decode(cur: &mut &[u8]) -> Option<StarKind> {
        Some(StarKind::from_code(u8::decode(cur)?))
    }
}

impl FixedCodec for StarKind {
    const LEN: usize = 1;
}

record! {
    Moments {
        weight: f64,
        mean: [f64; 3],
        m2: f64,
    }
}

record! {
    Aggregate {
        count: u64,
        mass: Moments,
        aged: [u32; AGE_BUCKETS],
        kinds: [u32; StarKind::COUNT],
    }
}

record! {
    Cell {
        id: CellId,
        rank_lo: u64,
        rank_hi: u64,
        child_mask: u8,
        aggregate: Aggregate,
    }
}

/// The magic at the head of a cell's payload.
pub(crate) const PAYLOAD_MAGIC: [u8; 4] = *b"GPAY";

/// The payload layout this crate writes
///
/// Two since the magnitude and the temperature bucket left for the
/// photometry sidecar, the column that held them beside the moment now
/// holding the moment alone.
pub(crate) const PAYLOAD_VERSION: u16 = 2;

/// The header: magic, version, how many systems, how wide a position axis.
pub(crate) const PAYLOAD_HEADER: usize = 4 + 2 + 4 + 1 + 1;

/// The grid a system's position sits on, in light years
///
/// **Elite's coordinates are multiples of a thirty-second of a light
/// year.** Measured over `.index/full`: of 1,230,297 axes sampled, 415 —
/// 0.034% — were off that grid, the worst by 0.04 ly, which is what a
/// different source than the game's own gets you. So a position is an
/// integer count of these from its cell's low corner, and for a cell of
/// 1024 light years a `u16` holds it **exactly**: 1024 x 32 is 32,768
/// counts, with room to spare. A 2048 ly cell wants 65,537 and misses by
/// one, so it and everything coarser take a `u32` — 3,741 cells of the
/// galaxy's 204,466, and the ones holding fewest systems each.
///
/// Which is the difference between 6 bytes a position and the 24 of an `f64`
/// triple. The router's expansion loop measures every system in every
/// cell a sphere touches — 45.4 billion of them to relax 3.9 million — and
/// it reads a position and a star kind and nothing else, so the bytes it
/// streams are the whole cost. Seven against forty-one.
pub const POSITION_STEP: f64 = 1.0 / 32.0;

/// How wide a position axis is for a cell of this level, in bytes.
pub(crate) fn position_width(level: u8) -> u8 {
    let counts = crate::core::geometry::CellId::edge_at(level) / POSITION_STEP;
    if counts <= u16::MAX as f64 { 2 } else { 4 }
}

/// Where each column starts, for a payload of `count` systems whose
/// positions are `width` bytes an axis.
fn columns(count: usize, width: u8) -> [usize; 4] {
    let pos = PAYLOAD_HEADER;
    let kind = pos + count * width as usize * 3;
    let id64 = kind + count;
    let updated = id64 + count * 8;
    [pos, kind, id64, updated]
}

/// How long a payload of `count` systems is at `width` bytes an axis.
pub(crate) fn payload_len(count: usize, width: u8) -> usize {
    columns(count, width)[3] + count * 4
}

/// A cell's payload: its systems as columns rather than as records
///
/// **Columns, because the router and the drawing want different fields.**
/// An expansion reads a position and a star kind; drawing reads the address
/// and the moment as well. Laid as records, the expansion faults every byte
/// of a row to read seven of them, which is the same complaint the names
/// table makes about itself in its own header.
///
/// Positions are cell-relative integers on [`POSITION_STEP`], so the block
/// needs its cell to be read at all — which every reader has, the cell
/// being how the file was found.
pub fn payload_bytes(cell: CellId, points: &[CellSystem]) -> Vec<u8> {
    let width = position_width(cell.level);
    let mut out = Vec::with_capacity(payload_len(points.len(), width));
    PAYLOAD_MAGIC.encode(&mut out);
    PAYLOAD_VERSION.encode(&mut out);
    (points.len() as u32).encode(&mut out);
    width.encode(&mut out);
    0u8.encode(&mut out);

    let origin = cell.min_ly();
    for point in points {
        for (axis, origin) in point.position.iter().zip(origin) {
            let count = ((axis - origin) / POSITION_STEP)
                .round()
                .clamp(0., u32::MAX as f64) as u32;
            match width {
                2 => (count.min(u16::MAX as u32) as u16).encode(&mut out),
                _ => count.encode(&mut out),
            }
        }
    }
    for point in points {
        point.kind.encode(&mut out);
    }
    for point in points {
        point.id64.encode(&mut out);
    }
    for point in points {
        point.updated_at.encode(&mut out);
    }
    out
}

/// What a payload's header says: how many systems, how wide a position, and
/// where each column begins.
#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct PayloadHead {
    pub count: usize,
    pub width: u8,
    pub kinds: usize,
    pub ids: usize,
    pub updated: usize,
}

/// Read a payload's header, or [`None`] for bytes that are not one
///
/// The one place the layout is parsed, so the mapped reader and the
/// decoding reader cannot disagree about where a column starts. A block
/// carries its own magic and version, so a stale one is refused rather than
/// read as a plausible number of systems with every field out of the wrong
/// bytes.
pub(crate) fn payload_head(bytes: &[u8]) -> Option<PayloadHead> {
    let mut cur = bytes;
    if <[u8; 4]>::decode(&mut cur)? != PAYLOAD_MAGIC {
        return None;
    }
    if u16::decode(&mut cur)? != PAYLOAD_VERSION {
        return None;
    }
    let count = u32::decode(&mut cur)? as usize;
    let width = u8::decode(&mut cur)?;
    if width != 2 && width != 4 {
        return None;
    }
    let [_, kinds, ids, updated] = columns(count, width);
    Some(PayloadHead { count, width, kinds, ids, updated })
}

/// And back, or [`None`] for bytes that are not a payload of this layout
///
/// A block written by another version is refused rather than guessed at: it
/// carries its own magic and version, so a stale one cannot decode as a
/// plausible number of systems with every field read out of the wrong bytes.
pub(crate) fn payload_points(
    cell: CellId,
    bytes: &[u8],
) -> Option<Vec<CellSystem>> {
    let head = payload_head(bytes)?;
    let PayloadHead { count, width, kinds: kind, ids: id64, updated } = head;
    if bytes.len() < payload_len(count, width) {
        return None;
    }

    let pos = PAYLOAD_HEADER;
    let origin = cell.min_ly();
    let mut points = Vec::with_capacity(count);
    for at in 0..count {
        let mut axes = [0.; 3];
        for (axis, held) in axes.iter_mut().enumerate() {
            let from = pos + (at * 3 + axis) * width as usize;
            let mut cur = &bytes[from..];
            let counts = match width {
                2 => u16::decode(&mut cur)? as f64,
                _ => u32::decode(&mut cur)? as f64,
            };
            *held = origin[axis] + counts * POSITION_STEP;
        }
        points.push(CellSystem {
            id64: {
                let mut cur = &bytes[id64 + at * 8..];
                u64::decode(&mut cur)?
            },
            position: axes,
            updated_at: {
                let mut cur = &bytes[updated + at * 4..];
                u32::decode(&mut cur)?
            },
            kind: StarKind::from_code(bytes[kind + at]),
        });
    }
    Some(points)
}

/// The magic and version at the head of an index file.
pub(crate) const INDEX_MAGIC: [u8; 4] = *b"GIDX";
/// Five, and moved by what a directory needs brought forward
///
/// Each move is a step [`crate::ops::upgrade::rewrite`] takes, and a stale
/// directory is refused at `index.bin`, named by [`index_version`], and sent
/// there by name rather than rebuilt:
///
/// - **3** moved for the payload's layout: records with no header became
///   columns with one.
/// - **4** moved for the index record: the aggregate gained its star-kind
///   histogram.
/// - **5** moved for the order and the light. A cell's slice is its
///   subtree's first in [`standing`](crate::core::standing) order rather than
///   its brightest, so every payload holds different systems; and the
///   magnitude, the temperature and a cell's flux left the record and the
///   payload for the photometry sidecar. Nothing of a directory at 4 can be
///   rewritten into that in place — which systems a cell owns is the whole of
///   what moved — so the step raises the tree again from the resume point
///   beside the directory, which holds every system whole.
///
/// The columnar payload carries its own magic, version and count, and refuses
/// a stale one itself.
pub const INDEX_VERSION: u16 = 5;

/// The version an index file's header claims, or [`None`] for bytes that are
/// not an index file at all.
///
/// A legacy payload carries no header, so the width of its records is known
/// only from the version beside them. A reader that [`Index`]'s decode
/// refused asks this to say which format it met.
pub fn index_version(bytes: &[u8]) -> Option<u16> {
    let mut cur = bytes;
    if <[u8; 4]>::decode(&mut cur)? != INDEX_MAGIC {
        return None;
    }
    u16::decode(&mut cur)
}

/// The index file's bytes: a header naming `INDEX_MAGIC` and
/// [`INDEX_VERSION`], a count, and that many fixed-width [`Cell`] records.
/// Rewritten whole on every publish, and a file of another width or version
/// is refused rather than misread.
impl Encode for Index {
    fn encode(&self, out: &mut Vec<u8>) {
        out.reserve(
            INDEX_MAGIC.len() + u16::LEN + u32::LEN + self.len() * Cell::LEN,
        );
        INDEX_MAGIC.encode(out);
        INDEX_VERSION.encode(out);
        (self.len() as u32).encode(out);
        for cell in self.cells() {
            cell.encode(out);
        }
    }
}

impl Decode for Index {
    /// [`None`] for a header this does not read, and for a body that disagrees
    /// with it.
    ///
    /// The header says how many cells follow and a cell is a fixed width, so
    /// the length is a thing the file can be held to: anything but exactly
    /// `count * Cell::LEN` bytes of body was written by a different build of
    /// this code. Without that check a file whose header was not moved with
    /// its record passes the header, decodes one record's bytes as another's,
    /// and hands back a plausible-looking tree of nonsense. Refused here, it
    /// is an error instead of a wrong sky.
    ///
    /// A file of an earlier version is refused on the version alone, and
    /// [`crate::codec::cells`]'s `Index::read` names `galos index migrate` for
    /// it. See [`INDEX_VERSION`].
    fn decode(cur: &mut &[u8]) -> Option<Index> {
        if <[u8; 4]>::decode(cur)? != INDEX_MAGIC {
            return None;
        }
        if u16::decode(cur)? != INDEX_VERSION {
            return None;
        }
        let count = u32::decode(cur)? as usize;
        if cur.len() != count * Cell::LEN {
            return None;
        }
        let mut cells = Vec::with_capacity(count);
        for _ in 0..count {
            cells.push(Cell::decode(cur)?);
        }
        Some(Index::from_cells(cells))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell's whole index record survives the round trip exactly, aggregate
    /// and all; the moments are `f64` and lose nothing.
    #[test]
    fn a_cell_record_round_trips() {
        let agg = Aggregate::of_system([1.0, 2.0, 3.0], 2, StarKind::G)
            .merge(Aggregate::of_system([5.0, 6.0, 7.0], 5, StarKind::Neutron));
        let cell = Cell {
            id: CellId { level: 3, x: 5, y: 6, z: 7 },
            rank_lo: 512,
            rank_hi: 1024,
            child_mask: 0b1010_0001,
            aggregate: agg,
        };
        let mut buf = Vec::new();
        cell.encode(&mut buf);
        assert_eq!(buf.len(), Cell::LEN);
        let mut cur = &buf[..];
        assert_eq!(Cell::decode(&mut cur), Some(cell));
    }

    fn point(id: u64) -> CellSystem {
        CellSystem {
            id64: id,
            position: [10.5, -40000.25, 65535.0],
            updated_at: 1_757_260_000 + id as u32,
            kind: StarKind::G,
        }
    }

    /// A payload's columns decode back to the systems they were built from,
    /// and a position lands on the galaxy's own grid exactly
    ///
    /// Positions are cell-relative counts of a thirty-second of a light
    /// year ([`POSITION_STEP`]), which is the grid Elite's coordinates
    /// actually sit on — so a position on that grid comes back bit for bit,
    /// in six bytes rather than the twenty-four of an `f64` triple.
    #[test]
    fn a_payload_block_round_trips() {
        let cell = CellId { level: 11, x: 400, y: 300, z: 500 };
        let origin = cell.min_ly();
        let on_grid = |n: f64| {
            [
                origin[0] + n * POSITION_STEP,
                origin[1] + 32.0 + n,
                origin[2] + 7.5,
            ]
        };

        let mut points = vec![point(1), point(2), point(3)];
        for (n, held) in points.iter_mut().enumerate() {
            held.position = on_grid(n as f64 * 3.0);
            held.kind = StarKind::from_code(n as u8 + 5);
        }

        let bytes = payload_bytes(cell, &points);
        assert_eq!(bytes.len(), payload_len(points.len(), 2));
        // Seven bytes a system for the two fields a route reads, and twelve
        // more for the address and the moment: no light, which is the
        // sidecar's.
        assert_eq!((bytes.len() - PAYLOAD_HEADER) / points.len(), 19);

        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(back.len(), points.len());
        for (a, b) in points.iter().zip(&back) {
            assert_eq!(a.id64, b.id64);
            assert_eq!(a.position, b.position, "a position on the grid moved");
            assert_eq!(a.updated_at, b.updated_at);
            assert_eq!(a.kind, b.kind);
        }
    }

    /// A coarse cell takes the wider axis, and holds a position exactly too
    ///
    /// A `u16` of thirty-seconds covers 1024 light years, which is every
    /// cell but the 3,741 of 2048 and coarser in a galaxy of 204,466. Those
    /// take a `u32`, and the reader is told which by the block's own header
    /// rather than working it out.
    #[test]
    fn a_coarse_cell_widens_its_positions() {
        assert_eq!(position_width(11), 2, "a 64 ly cell");
        assert_eq!(position_width(7), 2, "a 1024 ly cell");
        assert_eq!(position_width(6), 4, "a 2048 ly cell misses by one");

        let cell = CellId { level: 3, x: 3, y: 3, z: 3 };
        let origin = cell.min_ly();
        let mut held = point(9);
        held.position = [origin[0] + 9000.0, origin[1] + 0.25, origin[2] + 3.0];

        let bytes = payload_bytes(cell, &[held]);
        assert_eq!(bytes.len(), payload_len(1, 4));
        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(
            back[0].position, held.position,
            "a coarse cell lost a position"
        );
    }

    /// An empty block is empty, and bytes that are not a payload are refused
    ///
    /// The block carries its own magic, version and count, so one written
    /// at another width is refused rather than decoded as a plausible number
    /// of systems with every field read out of the wrong bytes.
    #[test]
    fn bytes_that_are_not_a_payload_are_refused() {
        let cell = CellId { level: 11, x: 0, y: 0, z: 0 };
        let empty: &[CellSystem] = &[];
        let bytes = payload_bytes(cell, empty);
        assert_eq!(payload_points(cell, &bytes).unwrap().len(), 0);

        assert!(payload_points(cell, &[]).is_none(), "nothing read as a block");
        assert!(
            payload_points(cell, &[0xFF; 64]).is_none(),
            "rubbish read as a block",
        );

        // A block cut short is refused rather than half read.
        let whole = payload_bytes(cell, &[point(1), point(2)]);
        assert!(payload_points(cell, &whole[..whole.len() - 1]).is_none());

        // And one from another layout.
        let mut wrong = whole.clone();
        wrong[4] = 0xFE;
        assert!(payload_points(cell, &wrong).is_none(), "a stale version read");
    }

    use crate::core::aggregate::Aggregate;

    /// An index round-trips its cells, and a file that is not one is refused
    /// rather than misread.
    #[test]
    fn an_index_round_trips_and_rejects_a_bad_header() {
        let index = Index::from_cells([
            Cell {
                id: CellId::ROOT,
                rank_lo: 0,
                rank_hi: 512,
                child_mask: 0xFF,
                aggregate: Aggregate::of_system([0.0; 3], 0, StarKind::K),
            },
            Cell {
                id: CellId { level: 1, x: 0, y: 1, z: 1 },
                rank_lo: 512,
                rank_hi: 520,
                child_mask: 0,
                aggregate: Aggregate::ZERO,
            },
        ]);
        let bytes = index.to_bytes();
        assert_eq!(Index::from_bytes(&bytes), Some(index));
        assert_eq!(
            Index::from_bytes(b"nope and then some padding bytes"),
            None
        );
    }

    /// An index whose *cell* record is a different width is refused
    ///
    /// A header that was not moved with its record says the same magic and
    /// the same version over bytes of another width, and nothing in it can
    /// tell the two apart. Only its length can. Without this the
    /// decoder reads one record's bytes as another's and hands back a tree of
    /// plausible nonsense — the wrong sky, drawn with no complaint.
    ///
    /// Both directions, since a record may grow as easily as shrink.
    #[test]
    fn an_index_of_another_width_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system([0.0; 3], 0, StarKind::K),
        };
        let bytes = Index::from_cells([cell]).to_bytes();
        assert!(Index::from_bytes(&bytes).is_some(), "a good file reads");

        let mut wider = bytes.clone();
        wider.extend_from_slice(&[0u8; 32]);
        assert_eq!(Index::from_bytes(&wider), None, "a wider record read");

        let narrower = &bytes[..bytes.len() - 32];
        assert_eq!(Index::from_bytes(narrower), None, "a narrower record read");
    }

    /// An index at another format version is refused, and says which.
    ///
    /// A directory at the version before this one is well-formed at every
    /// other check its index can be put to — and its payloads hold systems a
    /// cell of this version does not own — so this is the only thing
    /// standing between it and a galaxy read out of the wrong order.
    #[test]
    fn an_index_at_another_version_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system([0.0; 3], 0, StarKind::K),
        };
        let bytes = Index::from_cells([cell]).to_bytes();
        assert_eq!(index_version(&bytes), Some(INDEX_VERSION));

        let mut stale = bytes.clone();
        stale[4..6].copy_from_slice(&1u16.to_le_bytes());
        assert_eq!(index_version(&stale), Some(1));
        assert_eq!(Index::from_bytes(&stale), None);

        assert_eq!(index_version(b"nope, not an index at all"), None);
    }
}
