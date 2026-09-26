//! A metadata table as a file: one MessagePack value, written beside its
//! path and renamed over it.
//!
//! Every sidecar the client reads beside the cells — the populated systems,
//! the reaches, the supercharges, the factions — and every small record a
//! builder keeps beside its resume point is one of these. The writer half is
//! [`write_meta`]; the reader half is [`read_meta`].

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::io;
use std::path::Path;

/// Serialize a metadata value to a file, MessagePack-encoded. The builder's
/// writer half; the reader half is [`read_meta`].
///
/// Written beside the file and renamed over it, as [`crate::format::checkpoint::Checkpoint`] is.
/// A metadata table carries no length, count or magic, so a torn write is
/// the one failure the format cannot detect. The rename is the only step
/// that touches `path`, so a builder killed mid-write leaves the table it
/// published last intact.
pub fn write_meta<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let bytes = encoded(value)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// One metadata value's bytes, for either writer.
fn encoded<T: Serialize>(value: &T) -> io::Result<Vec<u8>> {
    rmp_serde::to_vec(value)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Read a metadata value back from a file, MessagePack-decoded. The reader
/// half of [`write_meta`], and how a builder resuming onto a directory reads
/// its own published tables back.
pub fn read_meta<T: DeserializeOwned>(path: &Path) -> io::Result<T> {
    let bytes = std::fs::read(path)?;
    rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use elite_journal::body::{AtmosphereType, BodyType};

    /// The codec reads back the shared enums' `#[serde(untagged)]`
    /// `Unknown(String)` variants, which every scanned body with an
    /// unfamiliar class or atmosphere carries and a non-self-describing
    /// format cannot.
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

    /// A metadata table is renamed over rather than written through
    ///
    /// The published path is never the file being filled, so a reader working
    /// through the table goes on reading what was published, whole, however
    /// far the next write has got. A torn write is the one failure the format
    /// cannot detect: these tables carry no length, count or magic.
    #[test]
    fn a_metadata_write_does_not_touch_what_it_replaces() {
        use std::io::Read;

        let dir = std::env::temp_dir()
            .join(format!("galos_source_atomic_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        let path = crate::format::layout::boosts_path(&dir);

        let first: Vec<i64> = (0..1_000).collect();
        write_meta(&path, &first).expect("the first write");

        // A reader that has the table open, as the map does when a pass lands.
        let mut held =
            std::fs::File::open(&path).expect("the table opens for reading");

        let second: Vec<i64> = (0..50_000).collect();
        write_meta(&path, &second).expect("the second write");

        let mut bytes = Vec::new();
        held.read_to_end(&mut bytes).expect("the held table reads");
        let held: Vec<i64> = rmp_serde::from_slice(&bytes)
            .expect("the held table still decodes");
        assert_eq!(
            held, first,
            "a reader holding the table saw the write land in it"
        );

        let read: Vec<i64> = read_meta(&path).expect("the second read");
        assert_eq!(read, second, "the second write did not land whole");
        assert!(
            !path.with_extension("tmp").exists(),
            "the temporary was left beside the table"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
