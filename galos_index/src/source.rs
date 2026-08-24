//! The transport seam: the one place the client asks where cells and metadata
//! come from.
//!
//! Cells and metadata share a transport at all times. Today that is the
//! filesystem, [`FsSource`] over a build directory; tomorrow it is one HTTP
//! impl over the same layout. The trait is what lets the client swap the whole
//! transport at once rather than half of it, so a filesystem index is never
//! paired with an HTTP metadata service or the other way about.
//!
//! It is async for that reason and no other: the FS reads are blocking and
//! their futures resolve at once, but an HTTP impl to come is genuinely async,
//! and a sync trait now would force every call site to change when it lands.
//! Boxed through [`async_trait`] so a client can hold `Arc<dyn Source>` and pick
//! its transport at runtime.
//!
//! The path helpers are the file-layout contract the builder writes to and this
//! reads from, named once here so the two cannot drift.

use crate::cache::Point;
use crate::geometry::CellId;
use crate::meta::{Faction, NameEntry, PopulatedSystem, SystemBodies};
use crate::walk::Index;
use async_trait::async_trait;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::{Path, PathBuf};

/// The populated-systems table, resident once and read for every colour.
pub const POPULATED_FILE: &str = "populated.bin";
/// Every system's name and position: the search index and routing graph.
pub const NAMES_FILE: &str = "names.bin";
/// The faction id-to-name table, small and read whole.
pub const FACTIONS_FILE: &str = "factions.bin";
/// The subdirectory of per-system body files.
pub const BODIES_DIR: &str = "bodies";

/// The populated table's path within a build directory.
pub fn populated_path(dir: &Path) -> PathBuf {
    dir.join(POPULATED_FILE)
}

/// The names table's path within a build directory.
pub fn names_path(dir: &Path) -> PathBuf {
    dir.join(NAMES_FILE)
}

/// The factions table's path within a build directory.
pub fn factions_path(dir: &Path) -> PathBuf {
    dir.join(FACTIONS_FILE)
}

/// A system's body file within a build directory, keyed by address.
pub fn bodies_path(dir: &Path, address: i64) -> PathBuf {
    dir.join(BODIES_DIR).join(format!("{address}.bin"))
}

/// Serialize a metadata value to a file, MessagePack-encoded. The builder's
/// writer half; the reader half is [`read_meta`]. Both name the format in one
/// place so a write and a read cannot disagree on it.
pub fn write_meta<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = rmp_serde::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, bytes)
}

/// Read a metadata value back from a file, MessagePack-decoded.
fn read_meta<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Where the client reads cells and metadata from. One transport for both.
#[async_trait]
pub trait Source: Send + Sync {
    /// The resident tree of cell aggregates the walks plan on.
    async fn index(&self) -> io::Result<Index>;

    /// One cell's payload: its systems, positions cell-relative to `id`. Empty
    /// where the cell owns nothing.
    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>>;

    /// The populated-systems table, held resident for filtering and colour.
    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>>;

    /// Every system's name and position: the search index and routing graph.
    async fn names(&self) -> io::Result<Vec<NameEntry>>;

    /// The faction id-to-name table, read whole and cached by the caller.
    async fn factions(&self) -> io::Result<Vec<Faction>>;

    /// The bodies inside a system, fetched when a click opens it. Empty where
    /// the system has no scan on record.
    async fn bodies(&self, address: i64) -> io::Result<SystemBodies>;
}

/// A [`Source`] over a build directory on the local filesystem.
///
/// The directory holds `index.bin`, a `cells/` subdirectory of payloads, and
/// the metadata files this reads beside them. The reads are blocking; a client
/// drives them off its own task pool.
pub struct FsSource {
    dir: PathBuf,
}

impl FsSource {
    /// A source over the build directory at `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> FsSource {
        FsSource { dir: dir.into() }
    }

    /// The build directory this reads from.
    pub fn dir(&self) -> &Path {
        &self.dir
    }
}

#[async_trait]
impl Source for FsSource {
    async fn index(&self) -> io::Result<Index> {
        Index::read(&self.dir)
    }

    async fn payload(&self, id: CellId) -> io::Result<Vec<Point>> {
        Index::read_payload(&self.dir, id)
    }

    async fn populated(&self) -> io::Result<Vec<PopulatedSystem>> {
        read_meta(&populated_path(&self.dir))
    }

    async fn names(&self) -> io::Result<Vec<NameEntry>> {
        read_meta(&names_path(&self.dir))
    }

    async fn factions(&self) -> io::Result<Vec<Faction>> {
        read_meta(&factions_path(&self.dir))
    }

    async fn bodies(&self, address: i64) -> io::Result<SystemBodies> {
        match std::fs::read(bodies_path(&self.dir, address)) {
            Ok(bytes) => rmp_serde::from_slice(&bytes)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                Ok(SystemBodies::default())
            }
            Err(e) => Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use elite_journal::body::{AtmosphereType, BodyType};

    /// The metadata codec must read back the shared enums' `#[serde(untagged)]`
    /// `Unknown(String)` variants, which a non-self-describing format cannot and
    /// which every scanned body with an unfamiliar class or atmosphere carries.
    /// This is why the codec is MessagePack rather than postcard.
    #[test]
    fn untagged_enums_round_trip() {
        let cases = [
            AtmosphereType::Unknown("SomethingNew".into()),
            AtmosphereType::Oxygen,
            AtmosphereType::None,
        ];
        for want in cases {
            let bytes = rmp_serde::to_vec(&want).expect("encodes");
            let got: AtmosphereType =
                rmp_serde::from_slice(&bytes).expect("decodes");
            assert_eq!(want, got);
        }

        let want = BodyType::Unknown("Ringworld".into());
        let bytes = rmp_serde::to_vec(&want).expect("encodes");
        let got: BodyType = rmp_serde::from_slice(&bytes).expect("decodes");
        assert_eq!(want, got);
    }
}
