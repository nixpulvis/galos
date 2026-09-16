//! The on-disk format: the traits and the byte layout of every record the
//! builder writes and the client reads.
//!
//! Four traits, in layers. [`Encode`] appends a value's bytes and [`Decode`]
//! reads them back off a cursor; a [`Codec`] is a type that does both. A
//! [`FixedCodec`] is a codec whose byte length is known up front, so its records
//! compose - a width is the sum of its parts' and a decode is a run of reads in
//! the same order a write took, which is why no width is a magic number and no
//! decode counts offsets. The index file and a cell's payload block are codecs
//! too, variable-length, each composing the fixed records below it.
//!
//! The layout is explicit rather than a derived serialization because these
//! bytes are a contract both sides hold across versions. The primitives get
//! their bytes from their own `to_le_bytes`; a record spells its fields out,
//! transforms and all (a cell's `id` as level plus Morton key, an aggregate's
//! `m_min` as a NaN-sentinel `f32`), which is the part a derive could not
//! express.
//!
//! The payload is columns rather than records — a 12-byte header, then runs
//! of position, star kind, address and photometry — and a position is an
//! integer count of [`POSITION_STEP`] off the cell's own low corner, so a
//! block is read *with* its cell rather than standing on its own. Nothing
//! is frozen yet: the version moves when a record's width does and the
//! check cannot catch it (see [`INDEX_VERSION`]), and the index record
//! keeps growing, as the aggregate gains the field step's filter marginals
//! and its quantization.

use crate::aggregate::{Aggregate, Cell};
use crate::cache::Point;
use crate::geometry::CellId;
use crate::meta::StarKind;
use crate::walk::Index;

/// Append this value's on-disk bytes to a buffer; [`to_bytes`](Self::to_bytes)
/// is the standalone form, for writing a whole value to a file.
pub trait Encode {
    fn encode(&self, out: &mut Vec<u8>);

    fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        self.encode(&mut out);
        out
    }
}

/// Read this value off the front of a cursor, advancing it, or [`None`] if the
/// bytes there are not a valid encoding of one. The inverse of [`Encode`];
/// [`from_bytes`](Self::from_bytes) reads it from a whole slice.
pub trait Decode: Sized {
    fn decode(cur: &mut &[u8]) -> Option<Self>;

    fn from_bytes(mut bytes: &[u8]) -> Option<Self> {
        Self::decode(&mut bytes)
    }
}

/// A value that round-trips through bytes: both [`Encode`] and [`Decode`], any
/// length. The index file and a cell's payload block are codecs; so is every
/// [`FixedCodec`]. A marker, implemented for anything that is both halves.
pub trait Codec: Encode + Decode {}

impl<T: Encode + Decode> Codec for T {}

/// A [`Codec`] whose byte length is a compile-time constant. `LEN` is the sum of
/// a record's fields', so no width is a magic number, and it is what lets
/// records compose - an array is that many in a row, and a container sizes
/// itself from it.
pub trait FixedCodec: Codec {
    const LEN: usize;
}

/// [`FixedCodec`] for the little-endian primitives, straight from their own
/// `to_le_bytes`/`from_le_bytes`. `LEN` is `size_of`, which for a scalar is
/// exactly its encoded width, so adding a primitive is one token in the list.
macro_rules! le_fixed {
    ($($t:ty),* $(,)?) => {$(
        impl Encode for $t {
            fn encode(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_le_bytes());
            }
        }
        impl Decode for $t {
            fn decode(cur: &mut &[u8]) -> Option<Self> {
                let (head, rest) = cur.split_at_checked(<$t as FixedCodec>::LEN)?;
                *cur = rest;
                Some(<$t>::from_le_bytes(head.try_into().unwrap()))
            }
        }
        impl FixedCodec for $t {
            const LEN: usize = std::mem::size_of::<$t>();
        }
    )*};
}

le_fixed!(u8, u16, u32, u64, i16, f32, f64);

/// A fixed-size array is that many records, end to end.
impl<T: FixedCodec, const N: usize> Encode for [T; N] {
    fn encode(&self, out: &mut Vec<u8>) {
        for item in self {
            item.encode(out);
        }
    }
}

impl<T: FixedCodec, const N: usize> Decode for [T; N] {
    fn decode(cur: &mut &[u8]) -> Option<[T; N]> {
        let items: [Option<T>; N] = std::array::from_fn(|_| T::decode(cur));
        if items.iter().any(Option::is_none) {
            return None;
        }
        Some(items.map(|item| item.unwrap()))
    }
}

impl<T: FixedCodec, const N: usize> FixedCodec for [T; N] {
    const LEN: usize = N * T::LEN;
}

/// Generate a [`FixedCodec`] impl for a record from one field list, so its
/// `LEN`, `encode`, and `decode` cannot drift apart. A field is `name: Type`
/// where `Type: FixedCodec`; `name: Type as Wire` where the value round-trips through
/// a wire type for a quantized or repacked field (`Wire: From<Type>` on the way
/// out, `Type: From<Wire>` on the way back); or `pad N` for N reserved zero
/// bytes. Everything runs in field order, so `encode` and `decode` mirror.
macro_rules! record {
    ($name:ident { $($body:tt)* }) => {
        impl Encode for $name {
            fn encode(&self, out: &mut Vec<u8>) {
                record!(@encode self, out, $($body)*);
            }
        }
        impl Decode for $name {
            fn decode(cur: &mut &[u8]) -> Option<Self> {
                record!(@decode cur, {} {} $($body)*)
            }
        }
        impl FixedCodec for $name {
            const LEN: usize = record!(@len $($body)*);
        }
    };

    (@len) => { 0 };
    (@len pad $n:literal $(, $($rest:tt)*)?) => { $n + record!(@len $($($rest)*)?) };
    (@len $f:ident : $t:ty as $w:ty $(, $($rest:tt)*)?) => {
        <$w as FixedCodec>::LEN + record!(@len $($($rest)*)?)
    };
    (@len $f:ident : $t:ty $(, $($rest:tt)*)?) => {
        <$t as FixedCodec>::LEN + record!(@len $($($rest)*)?)
    };

    (@encode $s:ident, $o:ident,) => {};
    (@encode $s:ident, $o:ident, pad $n:literal $(, $($rest:tt)*)?) => {
        for _ in 0..$n { Encode::encode(&0u8, $o); }
        record!(@encode $s, $o, $($($rest)*)?);
    };
    (@encode $s:ident, $o:ident, $f:ident : $t:ty as $w:ty $(, $($rest:tt)*)?) => {
        Encode::encode(&<$w>::from($s.$f), $o);
        record!(@encode $s, $o, $($($rest)*)?);
    };
    (@encode $s:ident, $o:ident, $f:ident : $t:ty $(, $($rest:tt)*)?) => {
        Encode::encode(&$s.$f, $o);
        record!(@encode $s, $o, $($($rest)*)?);
    };

    (@decode $c:ident, {$($lets:tt)*} {$($names:tt)*}) => {
        { $($lets)* Some(Self { $($names)* }) }
    };
    (@decode $c:ident, {$($lets:tt)*} {$($names:tt)*} pad $n:literal $(, $($rest:tt)*)?) => {
        record!(@decode $c,
            {$($lets)* for _ in 0..$n { <u8 as Decode>::decode($c)?; }}
            {$($names)*} $($($rest)*)?)
    };
    (@decode $c:ident, {$($lets:tt)*} {$($names:tt)*} $f:ident : $t:ty as $w:ty $(, $($rest:tt)*)?) => {
        record!(@decode $c,
            {$($lets)* let $f = <$t>::from(<$w as Decode>::decode($c)?);}
            {$($names)* $f,} $($($rest)*)?)
    };
    (@decode $c:ident, {$($lets:tt)*} {$($names:tt)*} $f:ident : $t:ty $(, $($rest:tt)*)?) => {
        record!(@decode $c,
            {$($lets)* let $f = <$t as Decode>::decode($c)?;}
            {$($names)* $f,} $($($rest)*)?)
    };
}
pub(crate) use record;

record! {
    Point {
        id64: u64,
        pos: [f64; 3],
        magnitude: f32,
        temp_bucket: u8,
        updated_at: u32,
        kind: StarKind,
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
pub const PAYLOAD_MAGIC: [u8; 4] = *b"GPAY";

/// The payload layout this crate writes.
pub const PAYLOAD_VERSION: u16 = 1;

/// The header: magic, version, how many systems, how wide a position axis.
pub const PAYLOAD_HEADER: usize = 4 + 2 + 4 + 1 + 1;

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
/// Which is the difference between 6 bytes a position and the 24 an `f64`
/// triple took. The router's expansion loop measures every system in every
/// cell a sphere touches — 45.4 billion of them to relax 3.9 million — and
/// it reads a position and a star kind and nothing else, so the bytes it
/// streams are the whole cost. Seven against forty-one.
pub const POSITION_STEP: f64 = 1.0 / 32.0;

/// How wide a position axis is for a cell of this level, in bytes.
pub fn position_width(level: u8) -> u8 {
    let counts = crate::geometry::edge_ly(level) / POSITION_STEP;
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
pub fn payload_len(count: usize, width: u8) -> usize {
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
pub fn payload_bytes(cell: CellId, points: &[Point]) -> Vec<u8> {
    let width = position_width(cell.level);
    let mut out = Vec::with_capacity(payload_len(points.len(), width));
    PAYLOAD_MAGIC.encode(&mut out);
    PAYLOAD_VERSION.encode(&mut out);
    (points.len() as u32).encode(&mut out);
    width.encode(&mut out);
    0u8.encode(&mut out);

    let origin = cell.min_ly();
    for point in points {
        for (axis, origin) in point.pos.iter().zip(origin) {
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
pub struct PayloadHead {
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
/// carries its own magic and version, which the record blocks this replaced
/// did not: a stale one is refused rather than read as a plausible number
/// of systems with every field out of the wrong bytes.
pub fn payload_head(bytes: &[u8]) -> Option<PayloadHead> {
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
/// carries its own magic and version, so unlike the record blocks this
/// replaced, a stale one cannot decode as a plausible number of systems
/// with every field read out of the wrong bytes.
pub fn payload_points(cell: CellId, bytes: &[u8]) -> Option<Vec<Point>> {
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
        points.push(Point {
            id64: {
                let mut cur = &bytes[id64 + at * 8..];
                u64::decode(&mut cur)?
            },
            pos: axes,
            magnitude: f32::decode(&mut lit)?,
            temp_bucket: u8::decode(&mut lit)?,
            updated_at: u32::decode(&mut lit)?,
            kind: StarKind::from_code(bytes[kind + at]),
        });
    }
    Some(points)
}

/// How wide a record was in the payloads written before the columns
///
/// Read by the migration that rewrites them and by nothing else: the old
/// block was `id64`, three `f64` axes, a magnitude, a temperature bucket
/// and a moment, laid end to end with no header to say so.
pub const LEGACY_POINT_LEN: usize = 8 + 24 + 4 + 1 + 4;

/// The systems a payload written before the columns held
///
/// Whole records only, so a trailing partial row is dropped rather than
/// failed. The star kind was not among them, so every system comes back as
/// [`StarKind::Unknown`] and the migration fills it from the scan record.
pub fn legacy_payload_points(bytes: &[u8]) -> Vec<Point> {
    let mut points = Vec::with_capacity(bytes.len() / LEGACY_POINT_LEN);
    let (rows, _) = bytes.as_chunks::<LEGACY_POINT_LEN>();
    for row in rows {
        let mut cur = &row[..];
        let Some(id64) = u64::decode(&mut cur) else { continue };
        let Some(pos) = <[f64; 3]>::decode(&mut cur) else { continue };
        let Some(magnitude) = f32::decode(&mut cur) else { continue };
        let Some(temp_bucket) = u8::decode(&mut cur) else { continue };
        let Some(updated_at) = u32::decode(&mut cur) else { continue };
        points.push(Point {
            id64,
            pos,
            magnitude,
            temp_bucket,
            updated_at,
            kind: StarKind::Unknown,
        });
    }
    points
}

/// The magic and version at the head of an index file.
const INDEX_MAGIC: [u8; 4] = *b"GIDX";
/// Two, and moved by the payload record's width
///
/// It stood at zero while the format settled, and a record changed width
/// under it more than once — the age buckets went from `u64` to `u32` — on
/// the argument that the length check in [`Index`]'s own `decode` catches a
/// stale file by its size, so a rebuild is the fix and rebuilding is cheap
/// against inputs already to hand.
///
/// That argument holds for the index file and not for the payload. A block
/// of points carries no magic, no version and no count, so nothing about it
/// can be held to a width: [`Vec<Point>`]'s decode takes whole records until
/// fewer than one remains, and a file written at another width decodes as a
/// plausible number of systems with every field read out of the wrong bytes.
/// The index beside it cannot tell either, `Cell::LEN` being unchanged. So a
/// change to [`Point`]'s width has to be caught in the one header there is,
/// and this is it: a stale directory is refused at `index.bin`, named by
/// [`index_version`], and rebuilt.
///
/// Which means a bump costs a full rebuild of the cells, and is worth it
/// only for a width change the payload cannot catch itself. A change to the
/// index record alone still rides on the length check.
pub const INDEX_VERSION: u16 = 3;

/// The version an index file's header claims, or [`None`] for bytes that are
/// not an index file at all.
///
/// A payload block carries no header, so the width of its records is known
/// only from the version beside them. A reader that [`Index`]'s decode
/// refused asks this to say which format it met.
pub fn index_version(bytes: &[u8]) -> Option<u16> {
    let mut cur = bytes;
    if <[u8; 4]>::decode(&mut cur)? != INDEX_MAGIC {
        return None;
    }
    u16::decode(&mut cur)
}

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
    /// this code. That is the check that makes the index record's width safe
    /// to change without moving [`INDEX_VERSION`] — without it a stale index
    /// passes the header, decodes one record's bytes as another's, and hands
    /// back a plausible-looking tree of nonsense. Refused here, it is a
    /// rebuild instead of a wrong sky.
    ///
    /// The payload has no such check — it carries no count to be held against,
    /// and drops a short tail rather than failing — so a change to *its* width
    /// is caught here instead, by the version this refuses on. See
    /// [`INDEX_VERSION`].
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

    fn point(id: u64, mag: f32) -> Point {
        Point {
            id64: id,
            pos: [10.5, -40000.25, 65535.0],
            magnitude: mag,
            temp_bucket: 3,
            updated_at: 1_757_260_000,
            kind: crate::meta::StarKind::G,
        }
    }

    /// A system survives the round trip through its bytes exactly, every
    /// field, the magnitude included.
    #[test]
    fn a_point_round_trips() {
        for mag in [-6.0, -1.5, 0.0, 4.83, 4.831_234_5, 15.0] {
            let p = point(42, mag);
            let mut buf = Vec::new();
            p.encode(&mut buf);
            assert_eq!(buf.len(), Point::LEN);
            let mut cur = &buf[..];
            let back = Point::decode(&mut cur).unwrap();
            assert_eq!(back.id64, p.id64);
            assert_eq!(back.pos, p.pos);
            assert_eq!(back.temp_bucket, p.temp_bucket);
            assert_eq!(back.updated_at, p.updated_at);
            assert_eq!(back.magnitude, p.magnitude);
        }
    }

    /// Two magnitudes closer together than a hundredth come back apart.
    ///
    /// The wire carried a fixed-point hundredth once, which rounded both of
    /// these to the same number and cost 0.92 % of a system's flux. The
    /// payload's own ordering is by the full value, so the encoding is the
    /// only place the distinction could be lost.
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
    /// actually sit on — so a position on that grid comes back bit for bit
    /// where an `f64` triple came back bit for bit, in six bytes instead of
    /// twenty-four.
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
            held.pos = on_grid(n as f64 * 3.0);
            held.kind = StarKind::from_code(n as u8 + 5);
        }

        let bytes = payload_bytes(cell, &points);
        assert_eq!(bytes.len(), payload_len(points.len(), 2));
        // Seven bytes a system for the two fields a route reads, against
        // the forty-one a record cost.
        assert_eq!((bytes.len() - PAYLOAD_HEADER) / points.len(), 24);

        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(back.len(), points.len());
        for (a, b) in points.iter().zip(&back) {
            assert_eq!(a.id64, b.id64);
            assert_eq!(a.pos, b.pos, "a position on the grid moved");
            assert_eq!(a.temp_bucket, b.temp_bucket);
            assert_eq!(a.updated_at, b.updated_at);
            assert_eq!(a.magnitude, b.magnitude);
            assert_eq!(a.kind, b.kind);
        }
    }

    /// A coarse cell takes the wider axis, and holds a position exactly too
    ///
    /// A `u16` of thirty-seconds covers 2048 light years, which is every
    /// cell but the 852 coarser ones in a galaxy of 204,466. Those take a
    /// `u32`, and the reader is told which by the block's own header rather
    /// than working it out.
    #[test]
    fn a_coarse_cell_widens_its_positions() {
        assert_eq!(position_width(11), 2, "a 64 ly cell");
        assert_eq!(position_width(7), 2, "a 1024 ly cell");
        assert_eq!(position_width(6), 4, "a 2048 ly cell misses by one");

        let cell = CellId { level: 3, x: 3, y: 3, z: 3 };
        let origin = cell.min_ly();
        let mut held = point(9, 1.5);
        held.pos = [origin[0] + 9000.0, origin[1] + 0.25, origin[2] + 3.0];

        let bytes = payload_bytes(cell, &[held]);
        assert_eq!(bytes.len(), payload_len(1, 4));
        let back = payload_points(cell, &bytes).unwrap();
        assert_eq!(back[0].pos, held.pos, "a coarse cell lost a position");
    }

    /// An empty block is empty, and bytes that are not a payload are refused
    ///
    /// The block carries its own magic, version and count, which the record
    /// blocks it replaced did not: one written at another width used to
    /// decode as a plausible number of systems with every field read out of
    /// the wrong bytes, and the only guard against it was the index file's
    /// version beside it.
    #[test]
    fn bytes_that_are_not_a_payload_are_refused() {
        let cell = CellId { level: 11, x: 0, y: 0, z: 0 };
        let empty: &[Point] = &[];
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

    /// The payloads written before the columns still read, for the migration
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
        assert_eq!(back[0].pos, [1.5, -2.0, 3.25]);
        assert_eq!(back[0].temp_bucket, 3);
        assert_eq!(
            back[0].kind,
            StarKind::Unknown,
            "the old payload cannot have held a kind",
        );
    }

    /// A cell's whole index record survives the round trip exactly, aggregate
    /// and all; the moments are `f64` and lose nothing.
    #[test]
    fn a_cell_record_round_trips() {
        let agg = Aggregate::of_system([1.0, 2.0, 3.0], 4.83, 5772.0, 2)
            .merge(Aggregate::of_system([5.0, 6.0, 7.0], -1.0, 12000.0, 5));
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
                aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
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

    /// An index written when a *cell* record was a different width is refused
    ///
    /// [`INDEX_VERSION`] does not move for a change to the index record, so
    /// a stale file carries the same magic and the same version and the
    /// header cannot tell it apart. Only its length can. Without this the
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
            aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
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
    /// The payload's record width rides on this version and on nothing
    /// else, a block of points carrying no header of its own. A directory
    /// from before the magnitude went to `f32` is well-formed at every
    /// other check, so this is the only thing standing between it and a
    /// galaxy decoded out of the wrong bytes.
    #[test]
    fn an_index_at_another_version_is_refused() {
        let cell = Cell {
            id: CellId::ROOT,
            rank_lo: 0,
            rank_hi: 512,
            child_mask: 0xFF,
            aggregate: Aggregate::of_system([0.0; 3], 1.0, 5000.0, 0),
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
