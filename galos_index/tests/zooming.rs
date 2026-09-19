//! What a zoom costs: the walk that decides what is drawn, and the read
//! that follows it.
//!
//! One walk per frame names the cells a view resolves, and every one of them
//! is read off the disk. Moving per-system data into the cells makes that
//! read bigger; sharding `cells/` makes it a different path. The walk is the
//! cheap half and has been since it descended a tree of its own — the read
//! is what a zoom waits on now, by four orders of magnitude.
//!
//! An integration test, everything it touches being this crate's public
//! surface. The routing half of what used to be one guard is `galos_map`'s
//! `systems::route::perf`, which cannot live out here: it measures settings
//! the map keeps to itself.
//!
//! Stands down without `GALOS_PERF_DIR` naming a built index directory:
//!
//! ```sh
//! GALOS_PERF_DIR=.index/full cargo test -p galos_index --test zooming -- --nocapture
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
//!
//! ## Measured 2026-09-16, where the screen walk's milliseconds went
//!
//! The screen walk was 22–29 ms a frame at 200 M systems — a ~35 fps floor
//! independent of everything routing — and the number everyone read as its
//! cost was the 151,619 cells it marks. **It is not the marks.** Ablated
//! over `.index/full`, 204,466 cells at 200,071,629 systems, release, from
//! 10 ly out looking in, one thing taken away at a time from a replica that
//! answers the same marks and the same field:
//!
//! | the walk | took |
//! |---|---|
//! | as it was, off the cell map | 23.1–28.0 ms |
//! | without the mark test | 17.8 ms |
//! | without the per-cell `Vec` of children | 26.0 ms |
//! | over the same cells in a flat array, same arithmetic | 2.7–3.3 ms |
//! | over a flat array of the figures it reads | **1.1–1.2 ms** |
//!
//! And the marks are not even what the clock tracks: **12,503 marks 100 kly
//! out cost 18.0–19.2 ms against 23.1 ms for 151,619** from inside the
//! bubble — twelve times the marks for a fifth more clock, and per cell
//! *visited* 131 ns against 114. What the walk visits is the whole tree at
//! every zoom inside 25 kly, because four fifths of it is 128–512 Ly cells
//! whose contents subtend hundreds of times the two pixels the split turns
//! on, so nothing stops short of a leaf:
//!
//! ```text
//! level     6     7     8     9    10    11    12    13
//! cells  2889 13002 54027 69689 48480 14112  1351    64
//! edge   2048  1024   512   256   128    64    32    16   Ly
//! ```
//!
//! **What it spent the milliseconds on was reaching the cells.** 408,932
//! lookups a walk — every cell twice, once as a child for its count and once
//! when it is popped — at 31–34 ns apiece, 13.3 ms of the 23 when replayed
//! on their own with no arithmetic between them. The same 408,932 addresses,
//! against three maps:
//!
//! | looked up in | each | in all |
//! |---|---|---|
//! | `HashMap<CellId, Cell>`, a 216-byte value, 44 MB | 32 ns | 13.3 ms |
//! | `HashMap<CellId, u32>`, the same hasher | 15 ns | 6.2 ms |
//! | `HashMap<CellId, u32>`, a multiply-shift hash | **3 ns** | **1.5 ms** |
//!
//! So three nanoseconds of a lookup is the arithmetic, twelve more are
//! SipHash, and seventeen are the cache lines of a 216-byte value. The mark
//! test's 9 ms is the same coin and not its cube roots: the identical
//! arithmetic over a packed array is 1.5 ms, where off the map it is what
//! pulls `Cell`'s second moments in for every leaf in the tree —
//! `contents_center` and `count_extent` being most of the record, and the
//! mark test being the only thing that reads them at a leaf.
//!
//! So the walks descend a tree of their own now
//! ([`galos_index::Index`]): the cells breadth-first with a cell's children
//! contiguous, carrying the figures each walk reads, none of which is a
//! function of the view. 88 bytes a node, 18 MB beside the map's 44, and
//! built where the map is — an index only ever being made from a whole set
//! of cells and never mutated after, which is what makes one derivation
//! enough. What that costs is the open: `Index::read` is **81 ms against
//! 30** over the same 44 MB file, against the second's ceiling the routing
//! guard holds it to.
//!
//! | zoom | before | after | marks |
//! |---|---|---|---|
//! | 10 ly out | 23.3 ms | **1.5 ms** | 151,619 |
//! | 1 kly | 23.2 ms | **1.5 ms** | 150,841 |
//! | 25 kly | 22.1 ms | **1.5 ms** | 97,891 |
//! | 100 kly | 18.4 ms | **1.0 ms** | 12,503 |
//!
//! Answer for answer, not just count for count: every row above was checked
//! cell for cell against the walk as it was written, the Shell splats still
//! summing to 1.000000 of the field, and `Mode::Real`'s two walks — 39,444
//! discrete stars and 171,650 glow cells from inside the bubble — coming
//! back with the same sets in **0.9–1.9 ms against 11.9–32.2**. The ladder
//! and the checks are `galos_index/examples/walk_cost.rs`.
//!
//! **And what a zoom costs is now the read, by four orders of magnitude.**
//! [`zooming_out_stays_quick`] reads what the walk marks, and at 200 M
//! systems that is 151,619 payloads: **18.5 s and 6.3 GB** at the wide zoom
//! against 1.05 s and 429 MB at the whole galaxy. The walk it follows is
//! 1.07–2.31 ms. Nothing above touches that half; it is the next thing in
//! the way.

use galos_index::walk::{Mode, View};
use galos_index::{FixedCodec as _, FsSource, Index, Point, Source as _};
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
             marks {:>6} blobs {:>6} splats {:>6} read {read:>10.2?} \
             points {points:>8} ({} KB)",
            needed.marks.len(),
            needed.blobs.len(),
            needed.splats.len(),
            bytes / 1024,
        );

        // A frame's budget at 60 fps is 16 ms and the walk is one of the
        // things in it, so the ceiling is a shape and not a frame: the walk
        // measured 18–25 ms off the cell map and 1.0–1.6 ms off the index's
        // own flattened tree ([`galos_index::Index`]), and anything an order
        // of magnitude over that is the map walk back.
        assert!(
            walked < Duration::from_millis(10),
            "a walk at {distance} ly took {walked:?}, which is a frame gone",
        );
    }
}
