//! A galaxy on disk, for the tests that route over one.
//!
//! The router reads the cell payloads where they lie
//! ([`galos_index::Sky`]), so a test that asks for a route needs a built
//! directory and not a list of places. Which is the right shape for a test
//! to have: what it exercises is then the same mapping, the same descent
//! and the same records the map reads, rather than a second implementation
//! of them that happens to agree.

use galos_index::BuildParams;

use galos_index::Sky;

use galos_index::Snapshot;

use galos_index::System;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// A directory removed with the test that made it.
pub struct Scratch(pub PathBuf);

impl Scratch {
    /// An empty directory named for `what` and this process.
    pub fn new(what: &str) -> Scratch {
        static SEQ: std::sync::atomic::AtomicU32 =
            std::sync::atomic::AtomicU32::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let at = std::env::temp_dir()
            .join(format!("galos-map-{what}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&at);
        std::fs::create_dir_all(&at).expect("a scratch directory");
        Scratch(at)
    }

    /// Where it is.
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Build a directory holding these systems, and map it.
///
/// The magnitudes ascend with the address so the tree's ordering is
/// unambiguous, and nothing else about a system matters to a route.
pub fn sky(dir: &Path, places: &[(i64, [f64; 3])]) -> Arc<Sky> {
    built(dir, places, &BuildParams::default())
}

/// The same, divided until no two systems share a cell
///
/// The build divides on *count*: a cell splits when more systems fall in it
/// than the cap, and a cell keeps the brightest slice of what it owned. So
/// a handful of systems is one root payload under the published caps,
/// however far apart they lie — which leaves nothing to say about a query
/// that has to cross cells. Cut to one apiece, the same places build a tree
/// several levels deep and every system has a cell of its own.
pub fn sky_apart(dir: &Path, places: &[(i64, [f64; 3])]) -> Arc<Sky> {
    built(dir, places, &BuildParams { internal_slice: 1, leaf_cap: 1 })
}

/// Write the tree `params` makes of `places`, and map what was written.
fn built(
    dir: &Path,
    places: &[(i64, [f64; 3])],
    params: &BuildParams,
) -> Arc<Sky> {
    let systems: Vec<System> = places
        .iter()
        .map(|&(address, position)| System {
            id64: address as u64,
            position,
            absolute_magnitude: address as f64 * 0.001 - 3.0,
            temperature: 5000.0,
            age_bucket: 0,
            updated_at: 0,
            kind: galos_index::StarKind::G,
        })
        .collect();
    Snapshot::build(&systems, params).write(dir).expect("a built galaxy");
    Arc::new(Sky::open(dir).expect("the galaxy maps"))
}

/// The same over places given as the names table carries them, `f32`.
/// The address of the class `A` boxel `place` falls in
///
/// **A fixture places a system by giving it the right address.** The names
/// table holds no position since a name became a function of one, and what
/// it answers with is the middle of the boxel the address names — so a test
/// whose sky puts a system at a place and whose table gives it an unrelated
/// address is a test where the two disagree about where it is, and the
/// router resolves its ends against the wrong neighbourhood.
///
/// The real mapping, so any place has an address. The middle of the boxel
/// is within five light years of the place asked for, and the grid's period
/// is ten, so a fixture asking for multiples of ten gets its distances
/// exactly.
pub fn boxel_at(place: [f64; 3]) -> i64 {
    let axis = |at: f64, which: usize| {
        let from = at - elite_journal::boxel::ORIGIN[which];
        let sector = (from / elite_journal::boxel::SECTOR_LY).floor();
        let within = from - sector * elite_journal::boxel::SECTOR_LY;
        (sector as u8, (within / 10.0).floor() as u32)
    };
    let (sx, x) = axis(place[0], 0);
    let (sy, y) = axis(place[1], 1);
    let (sz, z) = axis(place[2], 2);
    elite_journal::Boxel {
        mass: 0,
        sector: [sx, sy, sz],
        ordinal: x + 128 * y + 128 * 128 * z,
        index: 0,
    }
    .address()
    .expect("a boxel inside the grid")
}

pub fn sky_of(
    dir: &Path,
    entries: &[galos_index::records::NameEntry],
) -> Arc<Sky> {
    let places: Vec<(i64, [f64; 3])> = entries
        .iter()
        .map(|entry| {
            (
                entry.address,
                [
                    entry.position[0] as f64,
                    entry.position[1] as f64,
                    entry.position[2] as f64,
                ],
            )
        })
        .collect();
    sky(dir, &places)
}
