//! The photometry sidecar's bytes: every cell's light, and each system's.
//!
//! Two kinds of file, beside the two the cells are, and shaped like them:
//!
//! - `photometry.bin` beside `index.bin`: a header and a run of fixed-width
//!   records, a cell's address and its [`Photometry`] — the brightest
//!   magnitude as a NaN-sentinel `f32`, the flux per temperature bucket, and
//!   the flux-weighted [`Moments`](crate::core::moments::Moments). Rewritten
//!   whole whenever the index is.
//! - `photometry/<shard>/<cell>.bin` beside each payload: a header and two
//!   columns, each system's magnitude and temperature bucket, **in the
//!   payload's own order**, so the `at`th of one is the `at`th of the other
//!   and the two need no join. Written whenever the payload is.
//!
//! Neither is read by anything that does not ask how bright the sky is. The
//! map draws, the router routes and the filters filter off the cells alone.
//!
//! Every file is written beside its path and renamed over it, as a payload
//! is, so a reader holding one never sees it torn.

use crate::codec::Directory;
use crate::codec::bytes::{Decode, Encode, FixedCodec, record};
use crate::codec::layout::{PHOTOMETRY_DIR, lit_path, photometry_path};
use crate::codec::naming;
use crate::core::geometry::CellId;
use crate::core::photometry::{Lit, Photometry, TempBucket};
use crate::tree::lights::Lights;
use std::fs;
use std::io;
use std::path::Path;

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
    Photometry {
        m_min: Option<f32> as BrightestMag,
        flux: [f64; TempBucket::COUNT],
        light: crate::core::moments::Moments,
    }
}

/// One record of `photometry.bin`: whose light, and the light.
struct Entry {
    id: CellId,
    light: Photometry,
}

record! {
    Entry {
        id: CellId,
        light: Photometry,
    }
}

/// The magic at the head of `photometry.bin`.
const LIGHTS_MAGIC: [u8; 4] = *b"GLUM";

/// The magic at the head of a cell's light.
const LIT_MAGIC: [u8; 4] = *b"GLIT";

/// The layout both files are written at.
const VERSION: u16 = 1;

/// A cell's light's header: magic, version, how many systems.
const LIT_HEADER: usize = 4 + 2 + 4;

/// `photometry.bin`'s bytes: a header naming [`LIGHTS_MAGIC`] and the
/// version, a count, and that many fixed-width records, in address order so
/// the same tree writes the same file.
impl Encode for Lights {
    fn encode(&self, out: &mut Vec<u8>) {
        let mut entries: Vec<(CellId, Photometry)> =
            self.iter().map(|(id, light)| (id, *light)).collect();
        entries.sort_unstable_by_key(|(id, _)| (id.level, id.morton()));
        out.reserve(4 + 2 + 4 + entries.len() * Entry::LEN);
        LIGHTS_MAGIC.encode(out);
        VERSION.encode(out);
        (entries.len() as u32).encode(out);
        for (id, light) in entries {
            Entry { id, light }.encode(out);
        }
    }
}

impl Decode for Lights {
    /// [`None`] for a header this does not read, and for a body that is not
    /// exactly the count of records it says — as [`crate::tree::index::Index`]
    /// holds its own file to.
    fn decode(cur: &mut &[u8]) -> Option<Lights> {
        if <[u8; 4]>::decode(cur)? != LIGHTS_MAGIC {
            return None;
        }
        if u16::decode(cur)? != VERSION {
            return None;
        }
        let count = u32::decode(cur)? as usize;
        if cur.len() != count * Entry::LEN {
            return None;
        }
        let mut lights = Lights::default();
        for _ in 0..count {
            let Entry { id, light } = Entry::decode(cur)?;
            lights.insert(id, light);
        }
        Some(lights)
    }
}

impl Lights {
    /// Read `photometry.bin` from a build directory
    ///
    /// A directory without one answers [`io::ErrorKind::NotFound`], which a
    /// reader takes for a sky with no light on record rather than a failure.
    pub fn read(dir: &Path) -> io::Result<Lights> {
        let path = photometry_path(dir);
        let bytes = fs::read(&path).map_err(naming(&path))?;
        Lights::from_bytes(&bytes).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("{}: not a photometry file", path.display()),
            )
        })
    }

    /// Write `photometry.bin`, beside it and renamed over it.
    pub fn write(&self, dir: &Path) -> io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = photometry_path(dir);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, self.to_bytes())?;
        fs::rename(&tmp, &path)
    }
}

/// A cell's systems' light as its file holds it: the header, then every
/// magnitude, then every temperature bucket.
pub fn lit_bytes(lit: &[Lit]) -> Vec<u8> {
    let mut out = Vec::with_capacity(LIT_HEADER + lit.len() * 5);
    LIT_MAGIC.encode(&mut out);
    VERSION.encode(&mut out);
    (lit.len() as u32).encode(&mut out);
    for one in lit {
        one.magnitude.encode(&mut out);
    }
    for one in lit {
        one.temp_bucket.encode(&mut out);
    }
    out
}

/// The first `limit` of a cell's systems' light, or [`None`] for bytes that
/// are not one of these files, or that are cut short of what they say.
pub fn lit_points(bytes: &[u8], limit: usize) -> Option<Vec<Lit>> {
    let mut cur = bytes;
    if <[u8; 4]>::decode(&mut cur)? != LIT_MAGIC {
        return None;
    }
    if u16::decode(&mut cur)? != VERSION {
        return None;
    }
    let count = u32::decode(&mut cur)? as usize;
    if bytes.len() < LIT_HEADER + count * 5 {
        return None;
    }
    let magnitudes = LIT_HEADER;
    let buckets = magnitudes + count * 4;
    Some(
        (0..count.min(limit))
            .map(|at| {
                let from = magnitudes + at * 4;
                Lit {
                    magnitude: f32::from_le_bytes(
                        bytes[from..from + 4].try_into().unwrap(),
                    ),
                    temp_bucket: TempBucket::new(bytes[buckets + at]),
                }
            })
            .collect(),
    )
}

impl Directory<'_> {
    /// Write one cell's systems' light, opening its shard directory the first
    /// time anything lands there, beside the file and renamed over it.
    pub(crate) fn write_lit(self, id: CellId, lit: &[Lit]) -> io::Result<()> {
        let path = lit_path(self.root(), id);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, lit_bytes(lit))?;
        fs::rename(&tmp, &path)
    }

    /// Remove one cell's systems' light, the cell owning nothing now.
    pub(crate) fn remove_lit(self, id: CellId) -> io::Result<()> {
        match fs::remove_file(lit_path(self.root(), id)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }

    /// The first `limit` of one cell's systems' light, in the payload's own
    /// order; empty where the cell has none on record.
    pub fn read_lit(self, id: CellId, limit: usize) -> io::Result<Vec<Lit>> {
        match fs::read(lit_path(self.root(), id)) {
            Ok(bytes) => Ok(lit_points(&bytes, limit).unwrap_or_default()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(e),
        }
    }

    /// Where the per-cell light files live, for a sweep to walk.
    pub(crate) fn lit_dir(self) -> std::path::PathBuf {
        self.root().join(PHOTOMETRY_DIR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cell's light round-trips, and a magnitude keeps every bit of its
    /// `f32`: two closer together than a hundredth come back apart.
    #[test]
    fn a_cells_light_round_trips() {
        let lit = [
            Lit { magnitude: 4.833, temp_bucket: TempBucket::new(3) },
            Lit { magnitude: 4.831, temp_bucket: TempBucket::new(5) },
            Lit { magnitude: -6.5, temp_bucket: TempBucket::new(0) },
        ];
        let bytes = lit_bytes(&lit);
        assert_eq!(lit_points(&bytes, usize::MAX).unwrap(), lit);
        assert_eq!(lit_points(&bytes, 2).unwrap(), lit[..2]);
        assert!(lit_points(&bytes[..bytes.len() - 1], 9).is_none());
        assert!(lit_points(b"rubbish, and more", 9).is_none());
    }

    /// Every cell's light round-trips, and a file of another length is
    /// refused rather than misread.
    #[test]
    fn the_lights_round_trip() {
        let lights: Lights = [
            (
                CellId::ROOT,
                Photometry::of_system([1.0, 2.0, 3.0], 4.83, 5772.0).merge(
                    Photometry::of_system([5.0, 6.0, 7.0], -1.0, 12000.0),
                ),
            ),
            (CellId { level: 3, x: 5, y: 6, z: 7 }, Photometry::ZERO),
        ]
        .into_iter()
        .collect();
        let bytes = lights.to_bytes();
        assert_eq!(Lights::from_bytes(&bytes), Some(lights));
        assert_eq!(Lights::from_bytes(&bytes[..bytes.len() - 1]), None);
    }
}
