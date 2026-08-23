//! The on-disk payload: a cell's systems as a flat block of fixed-width rows.
//!
//! The payload is the bulk of the format — eighteen bytes a system, and most of
//! a galaxy is systems — and it is the part that never changes shape: an id, a
//! cell-relative position, and the two photometric bytes, laid end to end with
//! no index and no names. A cell's block is read whole and decoded into the
//! [`Point`]s the cache holds.
//!
//! Positions are cell-relative, so a block means nothing without the cell it
//! belongs to; the reader dequantizes each position against that cell when it
//! draws. That is what keeps precision at any distance from the origin and what
//! lets a block anchored to its cell go straight into a mesh.
//!
//! The cell *index* record — the aggregates the walks plan on — is the other
//! half of the format. Its byte layout settles with the last of the aggregate,
//! the filter marginals of the field step, and with the quantization tuned once
//! a real build has run; it is deliberately not frozen here while those fields
//! are still arriving. The payload is frozen because none of that touches it.

use crate::cache::{POINT_BYTES, Point};

/// Encode a cell's systems into its payload block.
pub fn encode_payload(points: &[Point]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(points.len() * POINT_BYTES);
    for point in points {
        bytes.extend_from_slice(&point.to_le_bytes());
    }
    bytes
}

/// Decode a cell's payload block into its systems.
///
/// Whole rows only: any trailing bytes that do not make a full row are ignored,
/// so a truncated block yields the systems it did carry rather than an error.
pub fn decode_payload(bytes: &[u8]) -> Vec<Point> {
    bytes
        .chunks_exact(POINT_BYTES)
        .map(|chunk| Point::from_le_bytes(chunk.try_into().unwrap()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point(id: u64, mag: f32) -> Point {
        Point { id64: id, pos: [10, 40000, 65535], magnitude: mag, temp_bucket: 3 }
    }

    /// A system survives the round trip through its bytes, to a hundredth of a
    /// magnitude and exactly in every other field.
    #[test]
    fn a_point_round_trips() {
        for mag in [-6.0, -1.5, 0.0, 4.83, 15.0] {
            let p = point(42, mag);
            let back = Point::from_le_bytes(&p.to_le_bytes());
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
        let bytes = encode_payload(&points);
        assert_eq!(bytes.len(), points.len() * POINT_BYTES);
        let back = decode_payload(&bytes);
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
        assert!(encode_payload(&[]).is_empty());
        assert!(decode_payload(&[]).is_empty());
        let mut bytes = encode_payload(&[point(1, 1.0)]);
        bytes.push(0xFF);
        assert_eq!(decode_payload(&bytes).len(), 1);
    }
}
