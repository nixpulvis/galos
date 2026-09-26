//! Bringing a directory's layout up to date, before anything reads it.
//!
//! The migrations an open runs on its own: packing the loose body files into
//! their shards ([`crate::store::bodies::pack`]), moving loose payloads into
//! theirs ([`reshard_cells`]), folding the names table's old MessagePack
//! chunks into a mapped base, and giving the supercharge table the places its
//! rows always implied ([`place_boosts`]). All of them are content-blind and
//! idempotent, and all of them are what [`migrate`] runs in order.
//!
//! A directory whose *format* this build cannot read is another matter, and
//! not something an open can fix: that is [`crate::ops::upgrade`], which
//! [`migrate`] names rather than attempts.

use crate::core::record::Boost;
use crate::format::layout::{PAYLOAD_DIR, boosts_path};
use crate::format::msgpack::write_meta;
use crate::records::SystemBoost;
use std::fs;
use std::io;
use std::path::Path;

/// How far a reshard got: what it moved, and whether anything is left.
///
/// [`reshard_bodies`] and [`crate::ops::migrate::reshard_cells`] both answer this.
/// A caller that asked one to stop needs both halves: the count for the
/// line it logs, and `finished` to say whether the next open has the rest
/// of the directory to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Resharded {
    /// Loose files that are no longer loose: moved into their shard, or
    /// dropped where the shard already held that file.
    pub moved: usize,
    /// Whether nothing loose was left behind. A stop part way answers
    /// `false`; a directory that was already sharded answers `true`, having
    /// had nothing to do.
    pub finished: bool,
}

/// What a directory's layout migration came to, part by part.
///
/// The counts are what a caller logs; `finished` is whether the next open
/// has the rest of that part to do.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Migrated {
    /// The body files, walked into the packed shard files.
    pub bodies: crate::store::bodies::Packed,
    /// The cell payloads, or [`None`] where the bodies were abandoned part
    /// way and the payloads were never reached.
    pub cells: Option<Resharded>,
    /// Systems in the names table folded out of MessagePack chunks into a
    /// mapped base, or [`None`] where the directory had no chunks — which
    /// is every directory built since.
    pub names: Option<usize>,
    /// Supercharge rows given the place they had always implied, or
    /// [`None`] where the table already carried one.
    pub boosts: Option<usize>,
    /// The format version the directory claims, where this build cannot
    /// read it and a manual upgrade is what is wanted
    ///
    /// Nothing was done in that case: see [`migrate`]. The caller's job is
    /// to *say so*, naming `galos index migrate`, rather than to carry on
    /// and let the refusal fall out of the first cell anybody asks for.
    pub upgrade: Option<u16>,
}

/// Bring an existing directory's *layout* up to date, before anything reads
/// or writes it: [`crate::store::bodies::pack`], [`crate::ops::migrate::reshard_cells`],
/// and then the names chunks.
///
/// Nothing is versioned: a layout this cannot recognise is built again.
///
/// Interruptible, and the one thing at an open that has to be: a galaxy's
/// worth of loose body files is hours of them, which a run asked to stop
/// must not be held up by. What this abandons a later open takes up; a
/// reader falls back to the loose paths for whatever has not been packed,
/// so a directory left half migrated serves exactly what a finished one
/// does.
///
/// A stop during the bodies leaves the payloads alone rather than opening
/// a second `readdir` on a directory nothing is going to move anything in:
/// `cells` is [`None`] and the next open runs both halves.
///
/// The names fold is last and is *not* interruptible, because it cannot
/// serve half: a directory has either the chunks or a base, and until the
/// fold finishes the chunks are still what stands. A galaxy's worth of them
/// is one external sort — minutes, against the afternoon that derived them
/// — and it happens once, ever, per directory.
pub fn migrate(
    dir: &Path,
    stop: &(dyn Fn() -> bool + Sync),
) -> io::Result<Migrated> {
    // **Said rather than worked around.** Everything below moves files
    // about without reading what is in them, so it would run to completion
    // over a directory whose payloads this build cannot read — and the
    // refusal would surface later, out of whatever first asked for a cell,
    // as a failed open with no remedy attached. A layout this build does
    // not read is not something an open can fix: it is hours of re-encoding
    // and a sweep of the scan record, which is `galos index migrate`.
    if let Some(found) = crate::store::cells::stale(dir) {
        return Ok(Migrated {
            bodies: crate::store::bodies::Packed { moved: 0, finished: true },
            cells: None,
            names: None,
            boosts: None,
            upgrade: Some(found),
        });
    }

    let bodies = crate::store::bodies::pack(dir, stop)?;
    if !bodies.finished {
        return Ok(Migrated {
            bodies,
            cells: None,
            names: None,
            boosts: None,
            upgrade: None,
        });
    }
    let cells = crate::ops::migrate::reshard_cells(dir, stop)?;
    let names = crate::store::names::fold_chunks(dir)?;
    // After the fold, because it reads the names table and the fold is
    // what decides which generation that is.
    let boosts = place_boosts(dir)?;
    Ok(Migrated { bodies, cells: Some(cells), names, boosts, upgrade: None })
}

/// Move every loose `cells/*.bin` into its shard, stopping where asked.
///
/// [`crate::store::bodies::pack`]'s twin, and [`crate::ops::migrate::migrate`] is the
/// pair: a one-time migration, idempotent, a rename each. A directory
/// already sharded costs one `readdir`.
///
/// `stop` is asked before each move, and abandoning is safe wherever it
/// lands: [`legacy_payload_path`] is read where the sharded path is absent,
/// so a half-migrated directory serves every cell a finished one does, and
/// the next open takes the rest. A payload already standing in its shard
/// wins over the loose one, for the reason [`crate::store::bodies::pack`]
/// gives.
pub fn reshard_cells(
    dir: &Path,
    stop: &dyn Fn() -> bool,
) -> io::Result<Resharded> {
    let entries = match fs::read_dir(dir.join(PAYLOAD_DIR)) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Resharded { moved: 0, finished: true });
        }
        Err(e) => return Err(e),
    };
    let mut moved = 0;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        let Some(rest) = name.strip_suffix(".bin") else { continue };
        let Some((_, morton)) = rest.split_once('-') else { continue };
        let Ok(morton) = u64::from_str_radix(morton, 16) else { continue };
        if !entry.file_type()?.is_file() {
            continue;
        }
        if stop() {
            return Ok(Resharded { moved, finished: false });
        }
        let to = dir
            .join(PAYLOAD_DIR)
            .join(format!("{:03x}", morton & 0xfff))
            .join(name);
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent)?;
        }
        match fs::metadata(&to) {
            Ok(_) => fs::remove_file(entry.path())?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::rename(entry.path(), to)?;
            }
            Err(e) => return Err(e),
        }
        moved += 1;
    }
    Ok(Resharded { moved, finished: true })
}

/// The supercharge table's rows as they were published before they carried
/// a place.
///
/// Read only by [`place_boosts`], which is how a directory written by an
/// older builder is brought forward. Two fields, so it decodes exactly the
/// rows [`SystemBoost`]'s three cannot.
#[derive(serde::Deserialize)]
struct Unplaced {
    address: i64,
    boost: Boost,
}

/// Give the supercharge table the places its rows always implied,
/// answering how many rows were rewritten — or [`None`] where there was
/// nothing to do.
///
/// The table used to be addresses and classes, which left the router to
/// join four million of them against the names table's address column to
/// find out where the jet cones are: 4 GB of mapping faulted and 7.9 s
/// before a galactic route could start planning, once a session. The place
/// belongs in the published row, and this is the one pass that puts it
/// there — the same join, run once, by the side that publishes.
///
/// A row the names table cannot place is dropped rather than placed at the
/// origin, which would put a jet cone at the galactic centre and plan every
/// route through it. It is the rule the derivations already follow.
///
/// Not interruptible and it need not be: it is one read of the table, one
/// pass over the address column, and one write.
pub fn place_boosts(dir: &Path) -> io::Result<Option<usize>> {
    let path = boosts_path(dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    // Already placed, which is every table written since. Asked first, so
    // a current directory pays one decode and nothing else.
    if rmp_serde::from_slice::<Vec<SystemBoost>>(&bytes).is_ok() {
        return Ok(None);
    }
    let mut old: Vec<Unplaced> = rmp_serde::from_slice(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // The table is published in address order, and the join walks the
    // column in that order; sorted here rather than trusted, since a file
    // that says otherwise would answer places for the wrong systems.
    old.sort_unstable_by_key(|row| row.address);
    let addresses: Vec<i64> = old.iter().map(|row| row.address).collect();

    // **The places come from the payloads, one pass over the galaxy.**
    // They used to come from the names table's own position column, and
    // that column no longer exists: a system's place is in the cell that
    // owns it and nowhere else. Asking the tree per address would be a
    // sphere query apiece — milliseconds by four million rows — where the
    // cells hold every place already, in an order this does not care
    // about.
    let index = crate::Index::read(dir)?;
    let mut placed: Vec<SystemBoost> = Vec::with_capacity(old.len());
    for cell in index.cells() {
        for point in crate::Index::read_payload(dir, cell.id)? {
            let Ok(which) = addresses.binary_search(&(point.id64 as i64))
            else {
                continue;
            };
            placed.push(SystemBoost {
                address: old[which].address,
                boost: old[which].boost,
                position: [
                    point.pos[0] as f32,
                    point.pos[1] as f32,
                    point.pos[2] as f32,
                ],
            });
        }
    }
    // Address order, as every published table is, so the row a reader
    // binary-searches for is where it expects: the walk above is in cell
    // order, which is no order at all to a caller.
    placed.sort_unstable_by_key(|row| row.address);
    write_meta(&path, &placed)?;
    Ok(Some(placed.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::records::{Barycenter, SystemBodies};
    use std::path::PathBuf;

    /// An empty scratch directory named after the test using it.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("galos_migrate_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    /// A system's insides, told apart by the address they belong to.
    fn inside(address: i64) -> SystemBodies {
        SystemBodies {
            barycenters: vec![Barycenter {
                system_address: address,
                id: 0,
                updated_at: "2026-08-08T12:00:00Z".parse().expect("a moment"),
                updated_by: "a test".into(),
                orbit: None,
            }],
            ..SystemBodies::default()
        }
    }

    /// A directory this build cannot read is said so, not migrated
    ///
    /// Everything the migration does is content-blind — it moves files into
    /// shards and folds chunks — so it would run happily over payloads of
    /// another layout and leave the refusal to fall out of the first cell
    /// anybody asked for, as a failed open with no remedy attached. The
    /// remedy is hours of re-encoding (`galos index migrate`) and not
    /// something an open can do, so the migration's job here is to name the
    /// version it met and touch nothing.
    #[test]
    fn a_directory_of_another_layout_asks_for_an_upgrade() {
        use crate::format::layout::legacy_bodies_path;

        let dir = scratch("migratestale");
        let address = 2_412_116_659_890_i64;
        write_meta(&legacy_bodies_path(&dir, address), &inside(address))
            .expect("a loose file writes");

        // An index file of a layout this build does not read.
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"GIDX");
        bytes.extend_from_slice(&(crate::INDEX_VERSION - 1).to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        std::fs::write(dir.join(crate::format::layout::INDEX_FILE), &bytes)
            .expect("an index file writes");

        let done = super::migrate(&dir, &|| false).expect("the migration runs");
        assert_eq!(
            done.upgrade,
            Some(crate::INDEX_VERSION - 1),
            "the layout met was not named",
        );
        // And nothing was moved: the loose file is still loose.
        assert_eq!(done.bodies.moved, 0);
        assert!(
            legacy_bodies_path(&dir, address).exists(),
            "the migration moved files it could not read the index of",
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A migration stopped in the body files does not go on to the cells
    ///
    /// The two halves are one open's work, and the run has already said it
    /// wants out: reading the payload directory to move nothing would be
    /// another `readdir` over a galaxy's worth of files. `cells` says which
    /// it is — [`None`] for a half that was never reached, against a
    /// [`Resharded`] that found nothing to do.
    #[test]
    fn a_migration_stopped_in_the_bodies_leaves_the_cells() {
        use crate::format::layout::legacy_bodies_path;

        let dir = scratch("migratestop");
        for n in 0..4 {
            let address = 2_412_116_659_890_i64 + n * 7_919;
            write_meta(&legacy_bodies_path(&dir, address), &inside(address))
                .expect("a loose file writes");
        }
        let loose = dir.join(crate::format::layout::PAYLOAD_DIR);
        std::fs::create_dir_all(&loose).expect("the payload directory");
        let payload = loose.join("01-0000000000000001.bin");
        std::fs::write(&payload, b"\x90").expect("a loose payload writes");

        // Asked before each move, so the third question stops the third.
        // Atomic rather than a `Cell`: the packing deals its shards out to
        // threads, so what it asks about stopping is shared.
        let questions = std::sync::atomic::AtomicUsize::new(0);
        let stop = || {
            questions.fetch_add(1, std::sync::atomic::Ordering::Relaxed) >= 2
        };
        let part = super::migrate(&dir, &stop).expect("the migration runs");
        assert_eq!(part.bodies.moved, 2, "the bodies did not stop when asked");
        assert!(!part.bodies.finished, "an abandoned half claimed to be done");
        assert_eq!(part.cells, None, "the cells were run after a stop");
        assert!(payload.exists(), "a stopped migration moved a payload");

        let rest = super::migrate(&dir, &|| false).expect("the second open");
        assert!(rest.bodies.finished, "the bodies did not finish");
        assert_eq!(rest.bodies.moved, 2, "the second open left a body loose");
        assert_eq!(
            rest.cells,
            Some(super::Resharded { moved: 1, finished: true }),
            "the second open did not take the payload the first left",
        );
        assert!(!payload.exists(), "the payload was left loose");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
