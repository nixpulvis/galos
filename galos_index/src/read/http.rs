//! The HTTP side of the transport seam: what a served directory answers at
//! which path, with no HTTP library in it.
//!
//! [`Source`](super::source::Source) is one transport for cells and metadata,
//! and the filesystem is the only one there is. A hosted index is the other:
//! a server over a build directory and, on the far side, a reader that asks
//! it what [`FsSource`](super::source::FsSource) asks the disk. This is the
//! contract the two meet at, named once as [`crate::codec::layout`] names the
//! files, so the server and the reader to come cannot drift.
//!
//! **The bytes on the wire are the bytes on disk.** Every answer is the file
//! the directory holds, or one written in that file's format, so the reader
//! decodes with the decoders it already has. Two parts are cut on the way
//! out rather than sent whole: a cell's payload and its light are column
//! files, so the first `limit` of one is not a prefix of its bytes, and the
//! server reads the prefix and writes it back out as a file of `limit`
//! rows. A body file is packed into a shard and is written back out as the
//! loose file it once was.
//!
//! **A stamp is an `ETag`.** The stamp a part is answered with is the one
//! [`Source::stamp`](super::source::Source::stamp) would have given for it, so
//! a `HEAD` and an `If-None-Match` are the cheap question the trait asks
//! before anything is re-read.
//!
//! What is not here yet is the names table. It is mapped rather than read,
//! and 5.8 GB of it has no business crossing a wire; it wants a search
//! answered where it lies, which is its own route.

use crate::codec::Directory;
use crate::codec::cells::format::payload_bytes;
use crate::codec::layout::{
    INDEX_FILE, factions_path, lit_path, payload_path, photometry_path,
    populated_path, reaches_path,
};
use crate::codec::lights::lit_bytes;
use crate::core::geometry::{CellId, MAX_LEVEL};
use crate::read::source::Stamp;
use crate::tree::index::Index;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// One thing a served directory answers, by the path it is asked for at.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    /// `/index.bin`: the cell aggregates.
    Index,
    /// `/photometry.bin`: every cell's light.
    Lights,
    /// `/populated.bin`
    Populated,
    /// `/reaches.bin`
    Reaches,
    /// `/factions.bin`
    Factions,
    /// `/tables/{name}`: a contributed table.
    Table(String),
    /// `/cells/{level}/{x}/{y}/{z}?limit=N`: the first `limit` of one cell's
    /// payload, or all of it with no `limit`.
    Cell { id: CellId, limit: Option<usize> },
    /// `/lit/{level}/{x}/{y}/{z}?limit=N`: the first `limit` of one cell's
    /// systems' light.
    Lit { id: CellId, limit: Option<usize> },
    /// `/bodies/{address}`: the insides of one system.
    Bodies(i64),
}

impl Route {
    /// The route a request names, or [`None`] for one nothing answers.
    ///
    /// `path` without its query, and `query` what followed the `?`, if
    /// anything did. Only `limit` is read out of a query; anything else in
    /// it is ignored rather than refused, as a cache-busting parameter a
    /// browser adds would be.
    pub fn parse(path: &str, query: Option<&str>) -> Option<Route> {
        let limit = match query.map(limit_in) {
            Some(Err(())) => return None,
            Some(Ok(limit)) => limit,
            None => None,
        };
        let rest = path.strip_prefix('/')?;
        let mut parts = rest.split('/');
        let route = match parts.next()? {
            "index.bin" => Route::Index,
            "photometry.bin" => Route::Lights,
            "populated.bin" => Route::Populated,
            "reaches.bin" => Route::Reaches,
            "factions.bin" => Route::Factions,
            "tables" => {
                let name = parts.next()?;
                if !table_name(name) {
                    return None;
                }
                Route::Table(name.to_string())
            }
            "cells" => Route::Cell { id: cell_in(&mut parts)?, limit },
            "lit" => Route::Lit { id: cell_in(&mut parts)?, limit },
            "bodies" => Route::Bodies(parts.next()?.parse().ok()?),
            _ => return None,
        };
        match parts.next() {
            None => Some(route),
            Some(_) => None,
        }
    }

    /// The path and query this route is asked for at: what
    /// [`parse`](Route::parse) reads back as the same route.
    pub fn to_url(&self) -> String {
        let cell =
            |id: &CellId| format!("{}/{}/{}/{}", id.level, id.x, id.y, id.z);
        let limit = |limit: &Option<usize>| match limit {
            Some(limit) => format!("?limit={limit}"),
            None => String::new(),
        };
        match self {
            Route::Index => "/index.bin".into(),
            Route::Lights => "/photometry.bin".into(),
            Route::Populated => "/populated.bin".into(),
            Route::Reaches => "/reaches.bin".into(),
            Route::Factions => "/factions.bin".into(),
            Route::Table(name) => format!("/tables/{name}"),
            Route::Cell { id, limit: at } => {
                format!("/cells/{}{}", cell(id), limit(at))
            }
            Route::Lit { id, limit: at } => {
                format!("/lit/{}{}", cell(id), limit(at))
            }
            Route::Bodies(address) => format!("/bodies/{address}"),
        }
    }

    /// The file whose modification time stamps this route, or [`None`] for
    /// one that carries no stamp.
    ///
    /// The bodies carry none: a reader fetches them when a click opens a
    /// system and never holds them, which is why
    /// [`Part`](super::source::Part) has no variant for them either.
    fn stamped_by(&self, dir: &Path) -> Option<PathBuf> {
        Some(match self {
            Route::Index => dir.join(INDEX_FILE),
            Route::Lights => photometry_path(dir),
            Route::Populated => populated_path(dir),
            Route::Reaches => reaches_path(dir),
            Route::Factions => factions_path(dir),
            Route::Table(name) => Directory::at(dir).table_path(name),
            Route::Cell { id, .. } => payload_path(dir, *id),
            Route::Lit { id, .. } => lit_path(dir, *id),
            Route::Bodies(_) => return None,
        })
    }
}

/// What a route answers with.
pub enum Body {
    /// A file sent as it stands, opened before its stamp was taken so the two
    /// are of the same file: a part republished meanwhile is renamed over
    /// this one and leaves the handle reading what it opened.
    File(fs::File, u64),
    /// A file's worth of bytes written out for this request.
    Bytes(Vec<u8>),
    /// Nothing there: a cell that owns nothing, a sidecar an older build
    /// never wrote. A reader takes it as
    /// [`NotFound`](io::ErrorKind::NotFound), which is what the same part
    /// read off a disk answers.
    Missing,
}

/// One answer: the body, and the stamp it goes out under.
pub struct Answer {
    pub stamp: Option<Stamp>,
    pub body: Body,
}

/// The stamp `route` would be answered under, without reading it: a `HEAD`.
pub fn stamp(dir: &Path, route: &Route) -> io::Result<Option<Stamp>> {
    let Some(path) = route.stamped_by(dir) else {
        return Ok(None);
    };
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(stamp_of(&meta)?)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Answer `route` out of the build directory at `dir`.
///
/// The stamp of a part written out rather than sent is taken *before* it is
/// read, so a part republished between the two goes out under the older
/// stamp and is fetched again on the next poll, rather than going out under
/// the newer one and never being asked about again.
pub fn answer(dir: &Path, route: &Route) -> io::Result<Answer> {
    match route {
        Route::Cell { id, limit } => {
            let stamp = stamp(dir, route)?;
            let points = Index::read_payload_prefix(
                dir,
                *id,
                limit.unwrap_or(usize::MAX),
            )?;
            if stamp.is_none() && points.is_empty() {
                return Ok(Answer { stamp, body: Body::Missing });
            }
            Ok(Answer { stamp, body: Body::Bytes(payload_bytes(*id, &points)) })
        }
        Route::Lit { id, limit } => {
            let stamp = stamp(dir, route)?;
            if stamp.is_none() {
                return Ok(Answer { stamp, body: Body::Missing });
            }
            let lit = Directory::at(dir)
                .read_lit(*id, limit.unwrap_or(usize::MAX))?;
            Ok(Answer { stamp, body: Body::Bytes(lit_bytes(&lit)) })
        }
        Route::Bodies(address) => {
            let inside = Directory::at(dir).read_bodies(*address)?;
            let bytes = rmp_serde::to_vec(&inside)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            Ok(Answer { stamp: None, body: Body::Bytes(bytes) })
        }
        _ => {
            let path = route.stamped_by(dir).expect("a file-backed route");
            match fs::File::open(path) {
                Ok(file) => {
                    let meta = file.metadata()?;
                    Ok(Answer {
                        stamp: Some(stamp_of(&meta)?),
                        body: Body::File(file, meta.len()),
                    })
                }
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    Ok(Answer { stamp: None, body: Body::Missing })
                }
                Err(e) => Err(e),
            }
        }
    }
}

/// A stamp as an `ETag`: quoted, as the header wants it.
pub fn etag(stamp: Stamp) -> String {
    format!("\"{stamp:016x}\"")
}

/// The stamp an `ETag` carries, or [`None`] for one this did not write.
pub fn stamp_in(etag: &str) -> Option<Stamp> {
    let inner = etag.trim().strip_prefix("W/").unwrap_or(etag.trim());
    let inner = inner.strip_prefix('"')?.strip_suffix('"')?;
    Stamp::from_str_radix(inner, 16).ok()
}

/// A file's modification time as a stamp, in nanoseconds since the epoch.
///
/// The same reading [`FsSource`](super::source::FsSource) gives the same
/// file, so a stamp is the same number on either side of the wire.
pub(crate) fn stamp_of(meta: &fs::Metadata) -> io::Result<Stamp> {
    Ok(meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_nanos() as Stamp))
}

/// The `limit` in a query, [`None`] where there is none, and an error where
/// it is there and not a number.
fn limit_in(query: &str) -> Result<Option<usize>, ()> {
    for pair in query.split('&') {
        if let Some(value) = pair.strip_prefix("limit=") {
            return value.parse().map(Some).map_err(|_| ());
        }
    }
    Ok(None)
}

/// The cell named by the next four segments, level first.
fn cell_in<'a>(parts: &mut impl Iterator<Item = &'a str>) -> Option<CellId> {
    let level: u8 = parts.next()?.parse().ok()?;
    if level > MAX_LEVEL {
        return None;
    }
    let id = CellId {
        level,
        x: parts.next()?.parse().ok()?,
        y: parts.next()?.parse().ok()?,
        z: parts.next()?.parse().ok()?,
    };
    let side = 1u64 << level;
    if [id.x, id.y, id.z].iter().any(|&at| at as u64 >= side) {
        return None;
    }
    Some(id)
}

/// Whether `name` can name a contributed table: a file name in the
/// directory and never a path out of it.
fn table_name(name: &str) -> bool {
    !name.is_empty()
        && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::{BuildParams, Snapshot};
    use crate::codec::bytes::Decode;
    use crate::read::source::{FsSource, Part, Source};
    use crate::system::System;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_http_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn system(id: u64, at: f64) -> System {
        System {
            id64: id,
            position: [at, 900.0, 24400.0],
            absolute_magnitude: id as f64,
            temperature: 5000.0,
            age_bucket: 0,
            updated_at: 0,
            kind: crate::core::star::StarKind::G,
        }
    }

    fn bytes(body: Body) -> Vec<u8> {
        match body {
            Body::File(mut file, _) => {
                let mut out = Vec::new();
                io::Read::read_to_end(&mut file, &mut out).expect("a read");
                out
            }
            Body::Bytes(bytes) => bytes,
            Body::Missing => panic!("an answer with nothing in it"),
        }
    }

    /// Every route reads back as itself, and nothing outside them parses.
    #[test]
    fn a_route_reads_back_as_itself() {
        let id = CellId { level: 3, x: 1, y: 7, z: 0 };
        for route in [
            Route::Index,
            Route::Lights,
            Route::Populated,
            Route::Reaches,
            Route::Factions,
            Route::Table("sectors".into()),
            Route::Cell { id, limit: None },
            Route::Cell { id, limit: Some(64) },
            Route::Lit { id, limit: Some(64) },
            Route::Bodies(-42),
        ] {
            let url = route.to_url();
            let (path, query) = match url.split_once('?') {
                Some((path, query)) => (path, Some(query)),
                None => (url.as_str(), None),
            };
            assert_eq!(Route::parse(path, query), Some(route), "{url}");
        }
        for (path, query) in [
            ("/", None),
            ("/index.bin/more", None),
            ("/tables/../index", None),
            ("/tables/", None),
            ("/cells/3/8/0/0", None),
            ("/cells/3/1/1", None),
            ("/cells/3/1/1/1", Some("limit=many")),
            ("/bodies/sol", None),
        ] {
            assert_eq!(Route::parse(path, query), None, "{path}");
        }
    }

    /// What goes out decodes to what the disk reads, under the stamp the
    /// disk gives: a reader across the wire holds what a reader of the
    /// directory holds.
    #[test]
    fn an_answer_is_what_the_disk_reads() {
        pollster::block_on(answered());
    }

    async fn answered() {
        let dir = scratch("answer");
        let systems: Vec<_> =
            (1..40).map(|id| system(id, id as f64 * 3.0)).collect();
        Snapshot::build(&systems, &BuildParams::default())
            .write(&dir)
            .expect("the build should write");
        let source = FsSource::new(&dir);

        let index = answer(&dir, &Route::Index).expect("an answer");
        assert_eq!(
            index.stamp,
            source.stamp(Part::Index).await.expect("a stat"),
        );
        let read = Index::from_bytes(&bytes(index.body)).expect("an index");
        let on_disk = source.index().await.expect("an index");

        // Every cell that owns systems answers its payload, whole and cut.
        let mut cells = 0;
        for cell in read.cells() {
            let id = cell.id;
            let whole = source.payload(id).await.expect("a payload");
            if whole.is_empty() {
                continue;
            }
            cells += 1;
            let route = Route::Cell { id, limit: Some(5) };
            let sent = answer(&dir, &route).expect("an answer");
            assert_eq!(
                sent.stamp,
                source.stamp(Part::Cell(id)).await.expect("a stat"),
            );
            let points = crate::codec::cells::format::payload_points(
                id,
                &bytes(sent.body),
            )
            .expect("a payload");
            assert_eq!(points, whole[..whole.len().min(5)]);

            let lit = answer(&dir, &Route::Lit { id, limit: Some(5) })
                .expect("an answer");
            let lit = crate::codec::lights::lit_points(&bytes(lit.body), 5)
                .expect("a cell's light");
            assert_eq!(lit, source.lit(id, 5).await.expect("a cell's light"));
        }
        assert!(cells > 0, "the build should have put systems in a cell");
        assert_eq!(read.cells().count(), on_disk.cells().count());

        // A cell that owns nothing is not there, and neither is a table the
        // build never wrote.
        let empty = CellId { level: 10, x: 1, y: 2, z: 3 };
        let none = answer(&dir, &Route::Cell { id: empty, limit: None })
            .expect("an answer");
        assert!(matches!(none.body, Body::Missing));
        assert_eq!(none.stamp, None);
        let none =
            answer(&dir, &Route::Table("absent".into())).expect("an answer");
        assert!(matches!(none.body, Body::Missing));

        // A system nothing scanned answers its empty insides, as the disk
        // does, rather than nothing.
        let inside = answer(&dir, &Route::Bodies(7)).expect("an answer");
        let inside: crate::records::SystemBodies =
            rmp_serde::from_slice(&bytes(inside.body)).expect("bodies");
        assert_eq!(inside, source.bodies(7).await.expect("bodies"));

        let _ = fs::remove_dir_all(&dir);
    }

    /// An `ETag` carries its stamp out and back.
    #[test]
    fn an_etag_carries_its_stamp() {
        for stamp in [0, 1, u64::MAX, 1_791_641_239_023_135_000] {
            assert_eq!(stamp_in(&etag(stamp)), Some(stamp));
        }
        assert_eq!(stamp_in("W/\"00000000000000ff\""), Some(255));
        assert_eq!(stamp_in("unquoted"), None);
    }
}
