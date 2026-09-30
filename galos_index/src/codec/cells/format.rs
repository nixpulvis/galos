//! The cells' bytes: the index file's fixed records, a payload's columns,
//! and the version the index file is held to.
//!
//! The index file is a header and a run of fixed-width [`Cell`] records, each
//! spelled out with `record!` below down to the fields it is built of — a
//! [`CellId`] as level plus Morton key, an [`Aggregate`] with its `m_min` as
//! a NaN-sentinel `f32`, the [`Moments`] inside it.
//!
//! A cell's payload is columns rather than records — a 12-byte header, then
//! runs of position, star kind, address and photometry — and a position is
//! an integer count of [`POSITION_STEP`] off the cell's own low corner, so a
//! block is read *with* its cell rather than standing on its own.
//!
//! Nothing is frozen yet: the version moves whenever a directory at the old
//! one has to be brought forward rather than rebuilt (see [`INDEX_VERSION`]),
//! and the index record keeps growing, as the aggregate gains the field
//! step's filter marginals and its quantization.

use crate::codec::bytes::{Decode, Encode, FixedCodec, record};
use crate::core::aggregate::{AGE_BUCKETS, Aggregate, TempBucket};
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

/// A temperature bucket, as its index. Both ways in clamp, so a byte out of
/// range reads as the hottest bucket.
impl Encode for TempBucket {
    fn encode(&self, out: &mut Vec<u8>) {
        (self.index() as u8).encode(out);
    }
}

impl Decode for TempBucket {
    fn decode(cur: &mut &[u8]) -> Option<TempBucket> {
        Some(TempBucket::new(u8::decode(cur)?))
    }
}

impl FixedCodec for TempBucket {
    const LEN: usize = 1;
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

/// A brightest magnitude on the wire, with `NaN` standing for none: a real
/// magnitude is never NaN, so the sentinel cannot collide with a value.
struct BrightestMag(f32);

impl Encode for BrightestMag {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl Decode for BrightestMag {
    fn decode(cur: &mut &[u8]) -> Option<BrightestMag> {
        Some(BrightestMag(f32::decode(cur)?))
    }
}

impl FixedCodec for BrightestMag {
    const LEN: usize = f32::LEN;
}

impl From<Option<f32>> for BrightestMag {
    fn from(m: Option<f32>) -> BrightestMag {
        BrightestMag(m.unwrap_or(f32::NAN))
    }
}

impl From<BrightestMag> for Option<f32> {
    fn from(m: BrightestMag) -> Option<f32> {
        (!m.0.is_nan()).then_some(m.0)
    }
}

record! {
    Aggregate {
        m_min: Option<f32> as BrightestMag,
        count: u64,
        flux: [f64; TempBucket::COUNT],
        light: Moments,
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

/// The payload layout this crate writes.
pub(crate) const PAYLOAD_VERSION: u16 = 1;

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
    let lit = id64 + count * 8;
    [pos, kind, id64, lit]
}

/// How long a payload of `count` systems is at `width` bytes an axis.
pub(crate) fn payload_len(count: usize, width: u8) -> usize {
    columns(count, width)[3] + count * (4 + 1 + 4)
}

/// A cell's payload: its systems as columns rather than as records
///
/// **Columns, because the router and the drawing want different fields.**
/// An expansion reads a position and a star kind; drawing reads the
/// magnitude, the temperature bucket and the address. Laid as records, the
/// expansion faults all forty-one bytes of a row to read seven of them,
/// which is the same complaint the names table makes about itself in its
/// own header.
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
        point.magnitude.encode(&mut out);
        point.temp_bucket.encode(&mut out);
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
    pub lit: usize,
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
    let [_, kinds, ids, lit] = columns(count, width);
    Some(PayloadHead { count, width, kinds, ids, lit })
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
    let PayloadHead { count, width, kinds: kind, ids: id64, lit } = head;
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
        let mut lit = &bytes[lit + at * 9..];
        points.push(CellSystem {
            id64: {
                let mut cur = &bytes[id64 + at * 8..];
                u64::decode(&mut cur)?
            },
            position: axes,
            magnitude: f32::decode(&mut lit)?,
            temp_bucket: TempBucket::decode(&mut lit)?,
            updated_at: u32::decode(&mut lit)?,
            kind: StarKind::from_code(bytes[kind + at]),
        });
    }
    Some(points)
}

/// How wide a record is in a legacy payload, the headerless record layout
///
/// Read by the migration that rewrites them and by nothing else: a legacy
/// block is `id64`, three `f64` axes, a magnitude, a temperature bucket and
/// a moment, laid end to end with no header to say so.
pub(crate) const LEGACY_POINT_LEN: usize = 8 + 24 + 4 + 1 + 4;

/// The systems a legacy payload holds
///
/// Whole records only, so a trailing partial row is dropped rather than
/// failed. A legacy record has no star kind, so every system comes back as
/// [`StarKind::Unknown`] and the migration fills it from the scan record.
pub(crate) fn legacy_payload_points(bytes: &[u8]) -> Vec<CellSystem> {
    let mut points = Vec::with_capacity(bytes.len() / LEGACY_POINT_LEN);
    let (rows, _) = bytes.as_chunks::<LEGACY_POINT_LEN>();
    for row in rows {
        let mut cur = &row[..];
        let Some(id64) = u64::decode(&mut cur) else { continue };
        let Some(pos) = <[f64; 3]>::decode(&mut cur) else { continue };
        let Some(magnitude) = f32::decode(&mut cur) else { continue };
        let Some(temp_bucket) = TempBucket::decode(&mut cur) else { continue };
        let Some(updated_at) = u32::decode(&mut cur) else { continue };
        points.push(CellSystem {
            id64,
            position: pos,
            magnitude,
            temp_bucket,
            updated_at,
            kind: StarKind::Unknown,
        });
    }
    points
}

/// The magic and version at the head of an index file.
pub(crate) const INDEX_MAGIC: [u8; 4] = *b"GIDX";
/// Four, and moved by what a directory needs brought forward
///
/// Each move is a step [`crate::ops::upgrade::rewrite`] knows how to take, and
/// a stale directory is refused at `index.bin`, named by [`index_version`],
/// and sent there by name rather than rebuilt:
///
/// - **3** moved for the payload's layout. A legacy payload is a block of
///   records with no magic, no version and no count, so nothing about it can
///   be held to a width: its decode takes whole records until fewer than one
///   remains, and a file written at another width decodes as a plausible
///   number of systems with every field read out of the wrong bytes. The
///   index beside it could not tell either, `Cell::LEN` being the same, so
///   the header had to. The step reads the record blocks and writes them as
///   columns.
/// - **4** moved for the index record: the aggregate gained its star-kind
///   histogram, 64 bytes a cell. The length check in [`Index`]'s own `decode`
///   would have refused the old file on its own, but as "not an index file",
///   which is a rebuild — hours off a dump — for a record whose one new field
///   is derivable from the payloads beside it. So the version says which
///   record the file holds ([`CELL_LEN_BEFORE_KINDS`] up to 3), and the step
///   fills the histogram in from the payloads' kind column.
///
/// The columnar payload carries its own magic, version and count, and refuses
/// a stale one itself.
pub const INDEX_VERSION: u16 = 4;

/// The first version whose index record carries [`Aggregate`]'s star kinds.
pub(crate) const FIRST_WITH_KINDS: u16 = 4;

/// How wide an index record was before [`FIRST_WITH_KINDS`]: the record less
/// its star-kind histogram.
pub(crate) const CELL_LEN_BEFORE_KINDS: usize =
    Cell::LEN - StarKind::COUNT * u32::LEN;

/// An index record written before [`FIRST_WITH_KINDS`], its star kinds zero
///
/// The histogram is the last field of the aggregate and the aggregate the
/// last of the record, so an old record is exactly a current one cut short of
/// it: padded back out with zeros, the current decode reads it. [`None`] for
/// fewer than [`CELL_LEN_BEFORE_KINDS`] bytes.
pub(crate) fn cell_before_kinds(record: &[u8]) -> Option<Cell> {
    let mut whole = [0u8; Cell::LEN];
    whole[..CELL_LEN_BEFORE_KINDS]
        .copy_from_slice(record.get(..CELL_LEN_BEFORE_KINDS)?);
    Cell::decode(&mut &whole[..])
}

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
        let agg =
            Aggregate::of_system([1.0, 2.0, 3.0], 4.83, 5772.0, 2, StarKind::G)
                .merge(Aggregate::of_system(
                    [5.0, 6.0, 7.0],
                    -1.0,
                    12000.0,
                    5,
                    StarKind::Neutron,
                ));
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

    /// A record from before the star kinds reads as the same cell with no
    /// kinds, and is the width the files of that version were written at
    ///
    /// Which is what `galos index migrate` stands on: it reads every older
    /// record this way and fills the kinds in after.
    #[test]
    fn a_record_from_before_the_kinds_reads_without_them() {
        assert_eq!(CELL_LEN_BEFORE_KINDS, 198, "the version 3 record width");
        let agg =
            Aggregate::of_system([1.0, 2.0, 3.0], 4.83, 5772.0, 2, StarKind::G)
                .merge(Aggregate::of_system(
                    [5.0, 6.0, 7.0],
                    -1.0,
                    12000.0,
                    5,
                    StarKind::Neutron,
                ));
        let cell = Cell {
            id: CellId { level: 3, x: 5, y: 6, z: 7 },
            rank_lo: 512,
            rank_hi: 1024,
            child_mask: 0b1010_0001,
            aggregate: agg,
        };
        let mut buf = Vec::new();
        cell.encode(&mut buf);

        let mut wanted = cell;
        wanted.aggregate.kinds = [0; StarKind::COUNT];
        assert_eq!(
            cell_before_kinds(&buf[..CELL_LEN_BEFORE_KINDS]),
            Some(wanted)
        );
        assert_eq!(cell_before_kinds(&buf[..CELL_LEN_BEFORE_KINDS - 1]), None);
    }

    fn point(id: u64, mag: f32) -> CellSystem {
        CellSystem {
            id64: id,
            position: [10.5, -40000.25, 65535.0],
            magnitude: mag,
            temp_bucket: TempBucket::new(3),
            updated_at: 1_757_260_000,
            kind: StarKind::G,
        }
    }

    /// Two magnitudes closer together than a hundredth come back apart.
    ///
    /// A fixed-point hundredth would round both of these to the same number
    /// and cost 0.92 % of a system's flux. The payload's own ordering is by
    /// the full value, so the encoding is the only place the distinction
    /// could be lost.
    #[test]
    fn magnitudes_finer_than_a_centimag_stay_apart() {
        let dim = point(1, 4.831);
        let bright = point(2, 4.833);
        let cell = CellId { level: 11, x: 0, y: 0, z: 0 };
        let bytes = payload_bytes(cell, &[bright, dim]);
        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(back[0].magnitude, 4.833);
        assert_eq!(back[1].magnitude, 4.831);
        assert!(back[0].magnitude != back[1].magnitude);
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

        let mut points = vec![point(1, 2.0), point(2, -3.5), point(3, 9.25)];
        for (n, held) in points.iter_mut().enumerate() {
            held.position = on_grid(n as f64 * 3.0);
            held.kind = StarKind::from_code(n as u8 + 5);
        }

        let bytes = payload_bytes(cell, &points);
        assert_eq!(bytes.len(), payload_len(points.len(), 2));
        // Seven bytes a system for the two fields a route reads, against
        // the forty-one of a legacy record.
        assert_eq!((bytes.len() - PAYLOAD_HEADER) / points.len(), 24);

        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(back.len(), points.len());
        for (a, b) in points.iter().zip(&back) {
            assert_eq!(a.id64, b.id64);
            assert_eq!(a.position, b.position, "a position on the grid moved");
            assert_eq!(a.temp_bucket, b.temp_bucket);
            assert_eq!(a.updated_at, b.updated_at);
            assert_eq!(a.magnitude, b.magnitude);
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
        let mut held = point(9, 1.5);
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
        let whole = payload_bytes(cell, &[point(1, 1.0), point(2, 2.0)]);
        assert!(payload_points(cell, &whole[..whole.len() - 1]).is_none());

        // And one from another layout.
        let mut wrong = whole.clone();
        wrong[4] = 0xFE;
        assert!(payload_points(cell, &wrong).is_none(), "a stale version read");
    }

    /// A legacy payload of records reads, for the migration
    #[test]
    fn a_payload_from_before_the_columns_still_reads() {
        let mut row = Vec::new();
        7u64.encode(&mut row);
        [1.5f64, -2.0, 3.25].encode(&mut row);
        4.5f32.encode(&mut row);
        3u8.encode(&mut row);
        1_700_000_000u32.encode(&mut row);
        assert_eq!(row.len(), LEGACY_POINT_LEN);

        // A stray trailing byte yields the whole rows and drops the rest.
        let mut bytes = row.clone();
        bytes.push(0xAB);
        let back = legacy_payload_points(&bytes);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].id64, 7);
        assert_eq!(back[0].position, [1.5, -2.0, 3.25]);
        assert_eq!(back[0].temp_bucket, TempBucket::new(3));
        assert_eq!(
            back[0].kind,
            StarKind::Unknown,
            "the old payload cannot have held a kind",
        );
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
                aggregate: Aggregate::of_system(
                    [0.0; 3],
                    1.0,
                    5000.0,
                    0,
                    StarKind::K,
                ),
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
            aggregate: Aggregate::of_system(
                [0.0; 3],
                1.0,
                5000.0,
                0,
                StarKind::K,
            ),
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
    /// The width of a legacy payload rides on this version and on nothing
    /// else, a block of records carrying no header of its own. A legacy
    /// directory is well-formed at every other check, so this is the only
    /// thing standing between it and a galaxy decoded out of the wrong bytes.
    #[test]
    fn an_index_at_another_version_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system(
                [0.0; 3],
                1.0,
                5000.0,
                0,
                StarKind::K,
            ),
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
