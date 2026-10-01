# TODO

Left over from the reorganization into `galos_map::{map, ui}`, `galos_route`
and galos_index's layers.

## Naming the core types

- Settled: galos_index's `System` (a system as the build reads it, in
  `system`, beside `system::bodies`), `CellSystem` (the served record, in
  `tree::cell`), `Index` (the served tree, `tree::index`) and `Tree` (the live
  one). `galos::sink::Index` keeps its name, the sink's own.
- Still open: `Sky` is also the galos_sky crate, and galos_map imports
  `Source` as `IndexSource`.
- A system's identity is `id64: u64` in about a hundred places and
  `address: i64` in about eighty-five (`Tree::holds(address: i64)` beside
  `Tree::forget(id: u64)`). One newtype, and which sign, is the question.

## Fewer resources, fewer parameters

Bevy caps a system at sixteen parameters, and the map has run into it:
`SystemParam` bundles such as `Settings` (21 resources), `FilterBar` and
`walk::Worked` exist mostly to get under the cap, and 21
`#[allow(clippy::too_many_arguments)]` remain in galos_map, the widest being
`ask_bar` and `state_bar` (`ui/bar/mod.rs`), `bodies/spawn.rs` and
`galaxy/spawn.rs`. The route settings and the router are now one resource
each (`RouteSettings`, `Router`), and asking for a route takes one `Asking`
context in place of a dozen arguments; do the same elsewhere:

- Group what is always read together into one resource: the display toggles
  scattered over eight modules (`ShowNames`, `ShowBodyNames`, `ShowOrbits`,
  `ShowGrid`, `ShowMiddle`, `ShowPicked`, `NameLimit`, `NameRadius`, …) and
  the exposure dials (`StarExposure`, `FieldExposure`, `StarProfile`).
- Pass a bundle or a context down instead of taking it apart into arguments.
- Take `&T`/`&mut T` rather than `&Res<T>`/`&mut ResMut<T>` in helpers.
- The route request is still spelled three ways — `FetchIndex::Route`'s
  seven-field tuple, `Filter::Route`, `PlottedRoute` — with the range a
  `String` and a trip an `ARROW`-joined string parsed back apart. One `Trip`
  type carrying `RouteSettings`.

## Core parts stand alone, contributed parts add to them

The index's own parts are listed once (`galos_index::codec::parts::CorePart`)
and `--only` names them and each contributed table one to one. What is left:

- Which core parts are required and which the map can do without. Cells and
  names are held to agree on resume; a missing `populated.bin`,
  `reaches.bin` or `factions.bin` has not been worked through in the map's
  loader, which should treat each as a missing feature the way it treats a
  missing `boosts.bin`.
- A core-only build from the command line. Every writer hard-wires
  `galos::tables()`, and only `--from database` honours `--only`: the feed
  (`src/sink`) and the dump build (`src/read/cold.rs`) always write every
  contributed table. Needs a flag, or `--only` applied to every source.
- `galos_index::read::source::Part`, the map's change stamps, is a third
  list (`Index`, `Cell`, `Names`, `NamesDelta`, …, no bodies) that does not
  derive from `CorePart`.
- A map test that loads a directory built with no contributed tables.
- Factions are not core. `factions.bin` and the faction ids on each
  `PopulatedSystem` row want to be a contributed part, but `Table` only
  derives a row per system from its `System` record: an id-to-name table fed from
  the database's `factions`, and a column on another table, need a second
  kind of contribution. Until then `CorePart::Factions` stands in the list.

## Benchmarks

`galos_index/tests/{procedural,zooming}.rs` and `galos_route`'s `perf` module
measure a local `GALOS_PERF_DIR` and pass silently without one. Reorganize
them, with `galos_index/examples/names_bench.rs`, into criterion benchmarks.

## Reading the index faster

Measured over `.index/full`; see the commit that queued the payload reads.

- Pack the cell payloads into shards, as `codec/bodies` packs the bodies.
  Opening a payload file costs ~12 µs on macOS and does not go faster past
  four threads, so a wide view of 70k cells spends 0.6–0.9 s on `open` alone.
  A shard kept mapped reads a cell with no syscall. Needs a per-cell
  generation in place of the file's mtime (`read::source::Stamp`), a
  republish that stays atomic for a router holding a mapping, and a
  `galos index migrate` step.
- `reaches.bin` as mapped fixed-width columns, as the names table is. It is
  1.1 GB of MessagePack and ~1.3 s of the map's opening, and holds ~2 GB at
  peak while it decodes. `boosts.bin` the same.
- Every marked cell is read, to 16 points whatever its share, so that
  `screen::Empty` can light a dark patch of sky with the brightest system
  of a cell there. Measured at 60,000 ly, a still view opened 77,418 cells
  with it and 22,015 without, and finished reading at 3.68 s against 1.09 s,
  for 15 more stars drawn. Working the dark patches out from the index first
  and reading only the cells chosen would keep the stars and drop the opens.
- The map holds about twice the payload points it draws (260k against 154k
  at the flight's stop). Memory, not time.

## The map

- Settings are not persisted: the spawn budget, like every other setting,
  is back at its default on every launch.
- `galaxy::spawn::update` asks every drawn system every frame whether its row
  changed; a `Changed<System>` query would skip the rest.
- The flight harness (`galaxy/flight.rs`) awaits every read each frame and
  runs frames back to back, so it measures per-frame cost but not fill-in
  order or timing: with systems now built off the main thread its "fill-in
  frames" reads 28 where the running map fills a frame's budget every frame.
  Fill-in was checked with `dev/shot.rs` captures; how long it takes in the
  running map after the camera stops has not been measured end to end.
- 71 clippy warnings in galos_map, most of them the argument counts above
  and complex types; a few collapsible `if`s.

## Merged marks stand on a lattice

A wide view shows a regular grid of specks over the dense disc. The
suspect is the merged marks, not the glow: a cell whose contents fit in
`MERGE_PX` (4 px) is drawn as one `SMALLEST` (0.75 px) mark at its count
centroid (`paint/field.rs`, the blob loop). A filled cell's centroid is about
its box centre, and the merged cells under one patch of sky are one level, so
the marks sit on a lattice 2–4 px apart. The glow's lattice is at `SPLIT_PX`
with sigma half a cell, a 1.4 % ripple, and is not it.

- **Confirm first.** `GALOS_NO_BLOBS=1` leaves the blobs out of the field. If
  the grid goes, it is theirs.
- **Draw a blob at the system it stands for**, not the centroid. `blobs.rs`
  already says a merged mark is its brightest system, the head of the cell's
  own payload, since slices are magnitude-ordered. Jittering the centroid was
  ruled out: it draws a mark where no system is.
  - Index format 5: the `Cell` record gains the head's position, `[f32; 3]`,
    12 bytes on 262 (~4.6 %, ~2.5 MB on `.index/full`'s 204,466 cells).
    `f32` is thousandths of a light year at galaxy coordinates.
  - No per-frame cost: `index.bin` is read whole, and `BlobRef::at` is
    already carried off the aggregate; it takes its value from the new field.
    The merge test and the walk's binning keep the centroid.
  - A migration step, not a rebuild, as 4 was: read each cell's payload head.
    Bump `INDEX_VERSION`, add the step to `ops::upgrade::rewrite`.
  - The build and incremental ingest (`accumulate/merge`) set it wherever a
    cell's slice is written, or it goes stale.
  - A split hands over in place: the parent's slice draws as marks once it
    splits, and its head first, so the speck does not move.
  - By population a blob stands for the busiest system instead. Not carried;
    another 12 bytes a cell, and blobs are drawn the same there either way.
- **Then maybe antialias the speck.** At a 0.75 px radius a mark lights one
  pixel or four depending on its sub-pixel phase. A quad of at least ~1 px
  with brightness scaled by `(SMALLEST / r)²` keeps the light and loses the
  shimmer. After the lattice is gone, to see whether it is still wanted.
- Not wanted: fading blobs where they tile and leaving the density to the
  glow; the number of marks is what says the density.

## Smoother while moving

Measured with Tracy over `.index/full` in the running map, after systems
were built off the main thread. Still, the map is idle; these are what the
slowest tenth of frames spend while zooming out.

- `walk::reconcile`, ~15 ms. The per-cell prefix pass is O(marks) and runs
  every frame the plan moves. Its per-cell work could run across threads
  with a serial merge, or the pass could run every other frame while the
  camera moves.
- The render thread, ~16 ms, 7 of it `allocate_and_free_meshes`: the glow
  (up to ~150k quads, ~600k vertices) and the marks field are fresh meshes
  every frame the camera moves. Many glow quads are laid at peaks far
  under a display level; dropping those would shrink the upload, and is a
  change to how the glow looks.
- `paint::glow::build_glow`, ~6 ms, already across threads.
- The worst single frames (100–140 ms) were not looked into.
- `galaxy::System::build` names a system through `Names::get`, which builds
  a whole `NameEntry` to take its name; `name_of` is the cheaper answer.
  Off the main thread now, so it costs fill-in time rather than frames.

## Broken

- `cargo test --release -p galos_map` does not compile: `ui/bar/search.rs`
  and `ui/bar/selection.rs` import `ui::testing::draw_selected`, which the
  release test build does not have.
- `.index/7day` is at index format 2 and refuses to open until
  `galos index migrate` is run over it.
