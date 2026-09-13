//! What the map must not get slower at, measured against a real directory.
//!
//! Two things the index work of `TODO-scale.md` could wreck, neither of them
//! a build:
//!
//! - **Zooming out.** One `walk_screen` per frame decides what is drawn and
//!   the cells it marks are read off the disk. Moving per-system data into
//!   the cells makes that read bigger; sharding `cells/` makes it a
//!   different path.
//! - **Routing.** The router walks the resident names table, so anything
//!   that stops it being resident lands on the router first.
//!
//! A unit-test module rather than a file in `tests/`: the routing half
//! measures `route::graph`'s `Routing` and `Drive`, both `pub(crate)`, which
//! an integration test could reach only by widening the router's API.
//!
//! Both stand down without `GALOS_PERF_DIR` naming a built index directory:
//!
//! ```sh
//! GALOS_PERF_DIR=.galos_index cargo test -p galos_map --lib perf -- --nocapture
//! ```
//!
//! The ceilings are loose, this running on whatever machine is to hand, so
//! what they catch is a change of *shape*.
//!
//! ## Measured 2026-09-11, before any of wall 4's work
//!
//! Against a 2,780,323-system directory, release, warm but for the first
//! read:
//!
//! | | |
//! |---|---|
//! | `walk_screen`, every zoom | 0.28–0.34 ms |
//! | cells the walk marks | 1,641 (whole galaxy) – 3,168 (close in) |
//! | reading them | 22–51 ms warm, 362 ms cold; 55–98 MB |
//! | building the jump graph | 227 ms over 2.78 M systems |
//! | a 22-jump route, 50 ly range | 5.9 ms `Direct`, 6.8 ms `Quick` |
//!
//! The walk is nothing and the *read* is the cost of a zoom: per-cell names
//! and sidecars add to that.
//!
//! ## Measured again, `cells/` sharded
//!
//! Over a freshly built 2,780,941-system directory, 3,430 payloads over
//! 1,540 shard directories: walk 0.24–0.33 ms, read 22.4/46.1/51.4 ms warm
//! and 377 ms cold, graph 208 ms, route 4.9–5.7 ms. Within the noise of the
//! flat layout above.
//!
//! ## Measured again, the magnitude as `f32`
//!
//! The payload record went 39 B to 41 B, so a zoom reads 5.1 % more. Over a
//! freshly built 2,885,249-system directory, 3,559 payloads: walk
//! 0.28–0.57 ms, read 21.8/45.1/53.9 ms warm and 358 ms cold, graph 212 ms,
//! route 5.4–6.5 ms. The extra bytes are inside the run-to-run spread.

#![cfg(test)]

use crate::systems::route::graph::{Drive, JumpGraph, Routing};
use crate::{Boosts, Names};
use galos_index::meta::NameEntry;
use galos_index::walk::{Mode, View};
use galos_index::{FixedCodec as _, FsSource, Index, Point, Source as _};
use std::path::PathBuf;
use std::sync::Arc;
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
        let needed = index.needed(&view, Mode::Shell);
        let walked = at.elapsed();

        // What the walk asks the disk for: the half of a zoom that moving
        // per-system data into the cells would make heavier.
        let at = Instant::now();
        let mut points = 0;
        let mut bytes = 0;
        for &id in &needed.marks {
            let payload = pollster::block_on(source.payload(id))
                .expect("a marked cell should read");
            points += payload.len();
            bytes += payload.len() * Point::LEN;
        }
        let read = at.elapsed();
        println!(
            "zoom {distance:>7} ly: walk {walked:>10.2?} \
             marks {:>5} splats {:>5} read {read:>10.2?} \
             points {points:>8} ({} KB)",
            needed.marks.len(),
            needed.splats.len(),
            bytes / 1024,
        );

        assert!(
            walked < Duration::from_millis(250),
            "a walk at {distance} ly took {walked:?}, which is a frame gone",
        );
    }
}

/// Routing: building the jump graph, and crossing the galaxy with it.
///
/// The two halves are timed apart because they fail differently. The graph
/// is built once from the whole names table and is what a per-cell names
/// format would have to replace; the search is what a commander waits on.
#[test]
fn routing_stays_quick() {
    let Some(dir) = measured() else { return };
    let source = FsSource::new(&dir);
    let entries: Vec<NameEntry> =
        pollster::block_on(source.names()).expect("the names should read");
    let boosts = match pollster::block_on(source.boosts())
        .expect("the boosts should read")
    {
        Some(rows) => Boosts::of(rows),
        None => Boosts::default(),
    };

    // The two ends, chosen from the data rather than named: the system
    // nearest the origin, and the one nearest a point a thousand light
    // years off it. Both ends are then in the part of the galaxy anybody
    // has actually visited, so the search crosses inhabited space and comes
    // back with a route — where the two extremes of the table are as likely
    // to be an isolated pair with no chain between them at all, which times
    // an exhausted search rather than a real one.
    let nearest = |to: [f64; 3]| {
        entries
            .iter()
            .min_by(|a, b| {
                let away = |it: &NameEntry| {
                    let [x, y, z] = it.position;
                    (x as f64 - to[0]).powi(2)
                        + (y as f64 - to[1]).powi(2)
                        + (z as f64 - to[2]).powi(2)
                };
                away(a).total_cmp(&away(b))
            })
            .expect("the table should hold a system")
            .address
    };
    let start = nearest([0.0, 0.0, 0.0]);
    let end = nearest([700.0, 0.0, 700.0]);

    let held = Names { entries: Arc::new(entries), ..Names::default() };

    let at = Instant::now();
    let graph = JumpGraph::new(&held.entries, &boosts);
    let built = at.elapsed();
    println!("route graph: {} systems in {built:.2?}", graph.len());
    assert!(
        built < Duration::from_secs(30),
        "building the jump graph took {built:?}",
    );

    for how in [Routing::Quick, Routing::Direct] {
        let at = Instant::now();
        let route = graph.route(start, end, 50.0, how, Drive::Unaided, None);
        let plotted = at.elapsed();
        println!(
            "route {how:?}: {} jumps in {plotted:.2?}",
            route.map_or(0, |it| it.len()),
        );
        assert!(
            plotted < Duration::from_secs(120),
            "a {how:?} route took {plotted:?}",
        );
    }
}
