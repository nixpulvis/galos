//! Bringing a built directory up to the format this build reads.
//!
//! One place for the migrations an operator runs on purpose, rather than one
//! command per format change: `galos index migrate` is what
//! [`crate::tree::index::Index::read`]'s refusal names, and what it does is
//! whatever the directory turns out to need.
//!
//! Not run at open, unlike [`crate::ops::migrate::migrate`]'s resharding. That
//! is a rename a file and this is every payload written again, which a reader
//! that wants to draw cannot spend without saying so.
//!
//! **A rebuild that is not a reimport.** Version 5 moved which systems a cell
//! owns — a slice is its subtree's first in
//! [`standing`](crate::core::standing) order rather than its brightest — and
//! took the light out of the cells into a sidecar. Nothing of an older
//! directory can be rewritten into that a cell at a time: every payload holds
//! different systems, and a payload is a lossy record of its systems, its
//! magnitude narrowed and its age dropped. But the resume point beside the
//! directory holds every system whole, and it is what a resumed cold build
//! takes its systems back out of anyway. So a stale directory is raised again
//! from it, by the same [`Build`] a resumed read uses, with nothing new read:
//! minutes over a galaxy, against the hours of the dump it was imported from.
//!
//! Every older version comes forward the same way, the legacy layouts
//! included: the resume point's records carry the star kind and everything
//! else a payload of any version was made from. A directory with no resume
//! point beside it cannot be brought forward and is said to be so, by path;
//! it is rebuilt from its source.
//!
//! The raise is a publish like any other: the old index comes down first and
//! the new one goes up last, so a run cut short leaves a directory with no
//! index, which every reader reads as nothing and which running this again
//! raises. Last, the contributed tables come forward as an open would bring
//! them, over a tree `Index::read` now accepts; a run cut short there leaves a
//! directory the next open finishes.

use crate::build::cold::{
    Build, Built, OnStop, ResumeMark, Start, region_budget,
};
use crate::build::snapshot::BuildParams;
use crate::codec::cells::format::{INDEX_VERSION, index_version};
use crate::codec::checkpoint::Checkpoint;
use crate::codec::layout::INDEX_FILE;
use crate::codec::tables::TableSet;
use std::io;
use std::path::Path;

/// What a rewrite came to.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rewrote {
    /// The version the directory was at when the rewrite found it.
    pub from: u16,
    /// Systems the tree was raised again over, out of the resume point; none
    /// for a directory already at this version.
    pub systems: u64,
    /// Cells the raised tree holds.
    pub cells: u64,
    /// Rows the contributed tables' own upgrades rewrote
    ///
    /// [`crate::codec::tables::Table::upgrade`] is a step of an open rather than
    /// of a build, and an open over a stale directory does nothing at all —
    /// [`crate::ops::migrate::migrate`] sets `upgrade` and returns, having
    /// touched nothing. So a directory brought forward by this command alone
    /// would still hold a table in a shape its owner has moved on from, and
    /// anything reading it without opening the galaxy first — the map's perf
    /// guard, for one — fails to decode a row.
    pub upgraded: u64,
}

/// Bring `dir` to the format this build reads, off the resume point at
/// `checkpoint`
///
/// Idempotent: a directory already at this version is left alone but for
/// its contributed tables, so a run interrupted part way is finished by
/// running it again. `said` hears how far it has got: once before the raise,
/// naming the version it found, and once at the end.
pub fn rewrite(
    dir: &Path,
    checkpoint: &Path,
    tables: &TableSet,
    stop: &(dyn Fn() -> bool + Sync),
    said: &mut dyn FnMut(&Rewrote),
) -> io::Result<Rewrote> {
    let mut wrote = Rewrote { from: version_of(dir)?, ..Rewrote::default() };
    said(&wrote);

    if wrote.from < INDEX_VERSION {
        let resumed = Checkpoint::read(checkpoint).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "{} is at index format version {}, which moved which \
                     systems every cell owns, so it is raised again from \
                     the resume point beside it, and {} could not be read \
                     ({err}): rebuild the directory from its source",
                    dir.display(),
                    wrote.from,
                    checkpoint.display(),
                ),
            )
        })?;
        let (by, cursor) = (resumed.by, resumed.cursor);
        drop(resumed);

        // Everything back out of the resume point and nothing read on top
        // of it: a resume that says nothing about where a read got to,
        // which is also what carries the names table forward rather than
        // starting it from nothing. See `ResumeMark::nowhere`.
        let build = Build::begin(
            dir,
            checkpoint,
            BuildParams::default(),
            region_budget(),
            Start::Resuming(ResumeMark::nowhere()),
            stop,
        )?;
        match build.finish(by, cursor, OnStop::Publish)? {
            Built::Index(summary) => {
                wrote.systems = summary.systems as u64;
                wrote.cells = summary.cells as u64;
            }
            Built::Stopped(abandoned) => {
                return Err(io::Error::new(
                    io::ErrorKind::Interrupted,
                    format!("{}: {abandoned}", dir.display()),
                ));
            }
        }
    }

    // The contributed tables, which an open would bring forward but an open
    // over a stale directory never reaches: `migrate` names this command
    // and returns without touching anything. After the index rather than
    // before it, because an upgrade may read the tree — the boost table
    // places its rows off the payloads — and a tree at a stale version is
    // one `Index::read` refuses.
    wrote.upgraded = crate::ops::migrate::upgraded(dir, tables)?
        .iter()
        .map(|&(_, rows)| rows as u64)
        .sum();
    said(&wrote);
    Ok(wrote)
}

/// The version `dir`'s index file claims, refusing one this build is older
/// than and a file that is not an index at all.
fn version_of(dir: &Path) -> io::Result<u16> {
    let path = dir.join(INDEX_FILE);
    let bytes = std::fs::read(&path)?;
    let refused = |said: String| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {said}", path.display()),
        )
    };
    match index_version(&bytes) {
        None => Err(refused("not an index file".to_owned())),
        Some(found) if found > INDEX_VERSION => Err(refused(format!(
            "version {found}, which is past the {INDEX_VERSION} this build \
             knows",
        ))),
        Some(found) => Ok(found),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build::snapshot::Snapshot;
    use crate::codec::checkpoint::Provenance;
    use crate::codec::layout::{checkpoint_beside, photometry_path};
    use crate::codec::{Directory, cells::Payload};
    use crate::core::star::StarKind;
    use crate::records::NameEntry;
    use crate::system::System;
    use crate::tree::index::Index;
    use crate::tree::lights::Lights;
    use std::ops::ControlFlow;

    /// A scratch directory unique to this run.
    fn scratch(what: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_upgrade_{what}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A galaxy where brightness and kind go together, as they do: so a
    /// tree ordered by brightness and one ordered by standing own different
    /// systems in every coarse cell.
    fn galaxy(n: u64) -> Vec<System> {
        (1..=n)
            .map(|id| {
                let (kind, magnitude) = match id % 5 {
                    0 => (StarKind::WhiteDwarf, 13.0),
                    1 => (StarKind::BrownDwarf, 18.0),
                    _ => (StarKind::M, 10.0 + id as f64 * 1e-4),
                };
                System {
                    id64: id,
                    position: [
                        (id % 23) as f64 * 37.0,
                        (id % 7) as f64 * 11.0,
                        (id % 31) as f64 * 53.0,
                    ],
                    absolute_magnitude: magnitude,
                    temperature: 3_000.0,
                    age_bucket: (id % 8) as u32,
                    updated_at: 1_700_000_000 + id as u32,
                    kind,
                }
            })
            .collect()
    }

    fn entry(system: &System) -> NameEntry {
        NameEntry {
            address: system.id64 as i64,
            name: format!("Sys {}", system.id64).into(),
            position: system.position.map(|axis| axis as f32),
        }
    }

    /// Put `dir`'s index back at `version`, which is the directory a
    /// migration meets: every reader refuses it until it is raised.
    fn written_at(dir: &Path, version: u16) {
        let path = dir.join(INDEX_FILE);
        let mut bytes = std::fs::read(&path).expect("an index");
        bytes[4..6].copy_from_slice(&version.to_le_bytes());
        std::fs::write(&path, bytes).expect("an old index");
    }

    /// A stale directory is raised from its resume point into exactly the
    /// tree a fresh build of the same systems is, light and all
    #[test]
    fn a_stale_directory_is_raised_from_its_resume_point() {
        let dir = scratch("raise");
        let checkpoint = checkpoint_beside(&dir);
        let systems = galaxy(9_000);
        let never = || false;
        let mut build = Build::begin(
            &dir,
            &checkpoint,
            BuildParams::default(),
            region_budget(),
            Start::Fresh,
            &never,
        )
        .expect("a build");
        for system in &systems {
            assert_eq!(
                build.push(*system, entry(system)).unwrap(),
                ControlFlow::Continue(())
            );
        }
        build.finish(Provenance::Database, None, OnStop::Abandon).unwrap();

        // What an older build left: an index at the version before, and no
        // light beside it.
        written_at(&dir, INDEX_VERSION - 1);
        std::fs::remove_file(photometry_path(&dir)).unwrap();
        assert!(Index::read(&dir).is_err(), "a stale index read");

        let wrote =
            rewrite(&dir, &checkpoint, &TableSet::new(), &never, &mut |_| {})
                .expect("the directory comes forward");
        assert_eq!(wrote.from, INDEX_VERSION - 1);
        assert_eq!(wrote.systems, systems.len() as u64);

        let index = Index::read(&dir).expect("the index reads at its version");
        let lights = Lights::read(&dir).expect("the light is beside it");
        let whole = Snapshot::build(&systems, &BuildParams::default());
        assert_eq!(index.len(), whole.index.len());
        for cell in whole.index.cells() {
            // Discretely the same cell; its moments are summed in the order
            // the regions came in, so they agree to the last bit or two.
            let ours = index.get(cell.id).expect("a cell of the build");
            assert_eq!(
                (ours.rank_lo, ours.rank_hi, ours.child_mask),
                (cell.rank_lo, cell.rank_hi, cell.child_mask),
                "{:?}",
                cell.id,
            );
            assert_eq!(ours.aggregate.count(), cell.aggregate.count());
            assert_eq!(ours.aggregate.kinds(), cell.aggregate.kinds());
            assert_eq!(
                Index::read_payload(&dir, cell.id).unwrap(),
                whole.payload(cell.id),
                "{:?} owns different systems",
                cell.id,
            );
            assert_eq!(
                Directory::at(&dir).read_lit(cell.id, usize::MAX).unwrap(),
                whole.lit(cell.id),
                "{:?} lights its systems differently",
                cell.id,
            );
            assert_eq!(
                lights.get(cell.id).m_min(),
                whole.lights.get(cell.id).m_min()
            );
        }
        assert!(
            Payload::open(&dir, whole.index.root().unwrap().id)
                .unwrap()
                .is_some()
        );

        // And a second run finds nothing to do.
        let again =
            rewrite(&dir, &checkpoint, &TableSet::new(), &never, &mut |_| {})
                .unwrap();
        assert_eq!((again.from, again.systems), (INDEX_VERSION, 0));
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_file(&checkpoint);
    }

    /// A stale directory with no resume point beside it is refused, naming
    /// both the version and where the resume point was looked for
    #[test]
    fn a_stale_directory_without_a_resume_point_is_refused() {
        let dir = scratch("orphan");
        let built = Snapshot::build(&galaxy(100), &BuildParams::default());
        built.write(&dir).unwrap();
        written_at(&dir, INDEX_VERSION - 1);
        let nowhere = dir.join("no.checkpoint");

        let err =
            rewrite(&dir, &nowhere, &TableSet::new(), &|| false, &mut |_| {})
                .expect_err("raised from nothing");
        let said = err.to_string();
        assert!(said.contains("no.checkpoint"), "{said}");
        assert!(
            said.contains(&format!("version {}", INDEX_VERSION - 1)),
            "{said}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
