//! The byte traits every fixed-width record is written through.
//!
//! Three traits, in layers. [`Encode`] appends a value's bytes and [`Decode`]
//! reads them back off a cursor. A [`FixedCodec`] is a value that does both
//! and whose byte length is known up front, so its records compose - a width
//! is the sum of its parts' and a decode is a run of reads in the same order
//! a write took, which is why no width is a magic number and no decode counts
//! offsets. The index file and a cell's payload block are variable-length
//! encodings of the fixed records below them; their layouts are
//! [`crate::format::payload`] and [`crate::read::index`].
//!
//! The layout is explicit rather than a derived serialization because these
//! bytes are a contract both sides hold across versions. The primitives get
//! their bytes from their own `to_le_bytes`; a record spells its fields out,
//! transforms and all (a cell's `id` as level plus Morton key, an aggregate's
//! `m_min` as a NaN-sentinel `f32`), which is the part a derive could not
//! express. Each record states its own layout with [`record!`] beside the
//! type, so the width and the type cannot be read apart.

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

/// A value that round-trips through bytes — both [`Encode`] and [`Decode`] —
/// and whose byte length is a compile-time constant. `LEN` is the sum of
/// a record's fields', so no width is a magic number, and it is what lets
/// records compose - an array is that many in a row, and a container sizes
/// itself from it.
pub trait FixedCodec: Encode + Decode {
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
