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
//! `m_min` as a NaN-sentinel `f32`, a point's magnitude as fixed-point `i16`),
//! which is the part a derive could not express.
//!
//! The payload is thirty-five bytes a system, its position carried in full as
//! three `f64`, so a block stands on its own without its cell. Nothing is
//! frozen yet: the version is zero while the format settles, and the index
//! record keeps growing, as the aggregate gains the field step's filter
//! marginals and its quantization.

use crate::aggregate::{Aggregate, Cell};
use crate::cache::Point;
use crate::geometry::CellId;
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

/// A magnitude on the wire: kept to a hundredth as a signed 16-bit int, finer
/// than the photometry it comes from.
struct Centimag(i16);

impl Encode for Centimag {
    fn encode(&self, out: &mut Vec<u8>) {
        self.0.encode(out);
    }
}

impl Decode for Centimag {
    fn decode(cur: &mut &[u8]) -> Option<Centimag> {
        Some(Centimag(i16::decode(cur)?))
    }
}

impl FixedCodec for Centimag {
    const LEN: usize = i16::LEN;
}

impl From<f32> for Centimag {
    fn from(m: f32) -> Centimag {
        Centimag(
            (m * 100.0).round().clamp(i16::MIN as f32, i16::MAX as f32) as i16
        )
    }
}

impl From<Centimag> for f32 {
    fn from(c: Centimag) -> f32 {
        c.0 as f32 / 100.0
    }
}

record! {
    Point {
        id64: u64,
        pos: [f64; 3],
        magnitude: f32 as Centimag,
        temp_bucket: u8,
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

/// A cell's payload block: its systems laid end to end.
///
/// Whole records only, so a trailing partial row is dropped rather than failed,
/// and a truncated block yields the systems it did carry.
impl Encode for [Point] {
    fn encode(&self, out: &mut Vec<u8>) {
        out.reserve(self.len() * Point::LEN);
        for point in self {
            point.encode(out);
        }
    }
}

impl Decode for Vec<Point> {
    fn decode(cur: &mut &[u8]) -> Option<Vec<Point>> {
        let mut points = Vec::with_capacity(cur.len() / Point::LEN);
        while let Some(point) = Point::decode(cur) {
            points.push(point);
        }
        Some(points)
    }
}

/// The magic and version at the head of an index file.
const INDEX_MAGIC: [u8; 4] = *b"GIDX";
/// Zero while the format is pre-1.0 and free to change; bumped once the first
/// cut is settled.
const INDEX_VERSION: u16 = 0;

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
    /// [`None`] if the header is not one this reads. A tail short of a full
    /// record is dropped rather than failed, as the payload is.
    fn decode(cur: &mut &[u8]) -> Option<Index> {
        if <[u8; 4]>::decode(cur)? != INDEX_MAGIC {
            return None;
        }
        if u16::decode(cur)? != INDEX_VERSION {
            return None;
        }
        let count = u32::decode(cur)? as usize;
        let mut cells = Vec::with_capacity(count);
        for _ in 0..count {
            match Cell::decode(cur) {
                Some(cell) => cells.push(cell),
                None => break,
            }
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
        }
    }

    /// A system survives the round trip through its bytes, to a hundredth of a
    /// magnitude and exactly in every other field.
    #[test]
    fn a_point_round_trips() {
        for mag in [-6.0, -1.5, 0.0, 4.83, 15.0] {
            let p = point(42, mag);
            let mut buf = Vec::new();
            p.encode(&mut buf);
            assert_eq!(buf.len(), Point::LEN);
            let mut cur = &buf[..];
            let back = Point::decode(&mut cur).unwrap();
            assert_eq!(back.id64, p.id64);
            assert_eq!(back.pos, p.pos);
            assert_eq!(back.temp_bucket, p.temp_bucket);
            assert!((back.magnitude - p.magnitude).abs() <= 0.005);
        }
    }

    /// A block is a whole number of fixed-width rows, and decodes back to the
    /// systems it was built from.
    #[test]
    fn a_payload_block_round_trips() {
        let points = vec![point(1, 2.0), point(2, -3.5), point(3, 9.25)];
        let bytes = points.as_slice().to_bytes();
        assert_eq!(bytes.len(), points.len() * Point::LEN);
        let back = Vec::<Point>::from_bytes(&bytes).unwrap();
        assert_eq!(back.len(), points.len());
        for (a, b) in points.iter().zip(&back) {
            assert_eq!(a.id64, b.id64);
            assert_eq!(a.pos, b.pos);
            assert_eq!(a.temp_bucket, b.temp_bucket);
            assert!((a.magnitude - b.magnitude).abs() <= 0.005);
        }
    }

    /// An empty block is empty, and a block with a stray trailing byte yields
    /// only its whole rows rather than failing.
    #[test]
    fn partial_rows_are_dropped_not_failed() {
        let empty: &[Point] = &[];
        assert!(empty.to_bytes().is_empty());
        assert!(Vec::<Point>::from_bytes(&[]).unwrap().is_empty());
        let mut bytes = [point(1, 1.0)].as_slice().to_bytes();
        bytes.push(0xFF);
        assert_eq!(Vec::<Point>::from_bytes(&bytes).unwrap().len(), 1);
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
}
