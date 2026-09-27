// TODO: reorganize. This measures a local `GALOS_PERF_DIR` and passes
// silently without one, so it is not an integration test of anything a user
// runs. Revisit alongside `examples/names_bench.rs` when setting up proper
// criterion benchmarks.

//! What a zoom costs: the walk that decides what is drawn, and the read
//! that follows it.
//!
//! One walk per frame names the cells a view resolves, and every one of them
//! is read off the disk. Moving per-system data into the cells makes that
//! read bigger; sharding `cells/` makes it a different path. The walk is the
//! cheap half, descending a tree of its own ([`galos_index::prelude::Index`]),
//! and the read is what a zoom waits on, by four orders of magnitude: over
//! `.index/full`, 200,071,629 systems, the walk is 1–2 ms and the 151,619
//! payloads it marks at the wide zoom are 18.5 s and 6.3 GB to read.
//!
//! An integration test, everything it touches being this crate's public
//! surface. Routing is measured by `galos_route`'s `perf`, a unit-test module
//! of the router's own.
//!
//! Stands down without `GALOS_PERF_DIR` naming a built index directory:
//!
//! ```sh
//! GALOS_PERF_DIR=.index/full cargo test -p galos_index --test zooming -- --nocapture
//! ```
//!
//! The ceilings are loose, this running on whatever machine is to hand, so
//! what they catch is a change of *shape*.

use galos_index::codec::layout::payload_path;
use galos_index::prelude::{FsSource, Index, Mode, Source as _, View};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The directory to measure against, or [`None`] to stand down.
fn measured() -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var("GALOS_PERF_DIR").ok()?);
    if !dir.join("index.bin").exists() {
        eprintln!("{}: not a built index; standing down", dir.display());
        return None;
    }
    Some(dir)
}

/// A view of the galaxy from `distance` light years out, looking in.
///
/// Along the x axis at the galactic centre, where the mass is, so the walk
/// has the most work a view at that distance can give it.
fn looking_in(distance: f64) -> View {
    View {
        eye: [distance, 0.0, 25_000.0],
        forward: [-1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        fov_y: 0.8,
        viewport_height: 1080.0,
        aspect: 16.0 / 9.0,
    }
}

/// Zooming out: the walk that decides what is drawn, and the read that
/// follows it.
///
/// A zoom, not a sample: a system's-eye view, a neighbourhood, a region, and
/// the whole galaxy in frame. The last is every cell in the tree considered,
/// and is one scroll away.
#[test]
fn zooming_out_stays_quick() {
    let Some(dir) = measured() else { return };
    let index = Index::read(&dir).expect("the index should read");
    let source = FsSource::new(&dir);

    for distance in [10.0, 1_000.0, 25_000.0, 100_000.0] {
        let view = looking_in(distance);

        let at = Instant::now();
        let needed = index.needed(&view, Mode::Shell, None);
        let walked = at.elapsed();

        // What the walk asks the disk for: the half of a zoom that moving
        // per-system data into the cells would make heavier.
        let at = Instant::now();
        let mut points = 0;
        let mut bytes = 0;
        for mark in &needed.marks {
            let payload = pollster::block_on(source.payload(mark.id))
                .expect("a marked cell should read");
            points += payload.len();
            bytes += std::fs::metadata(payload_path(&dir, mark.id))
                .map_or(0, |it| it.len());
        }
        let read = at.elapsed();
        println!(
            "zoom {distance:>7} ly: walk {walked:>10.2?} \
             marks {:>6} blobs {:>6} splats {:>6} read {read:>10.2?} \
             points {points:>8} ({} KB)",
            needed.marks.len(),
            needed.blobs.len(),
            needed.splats.len(),
            bytes / 1024,
        );

        // A frame's budget at 60 fps is 16 ms and the walk is one of the
        // things in it, so the ceiling is a shape and not a frame: the walk
        // over the index's own flattened tree ([`galos_index::prelude::Index`])
        // is
        // 1.0–1.6 ms, and anything an order of magnitude over that is a walk
        // that has stopped reading the flattened tree.
        assert!(
            walked < Duration::from_millis(10),
            "a walk at {distance} ly took {walked:?}, which is a frame gone",
        );
    }
}
