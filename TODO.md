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
- The flight harness (`galaxy/flight.rs`) awaits every read each frame, so
  it measures per-frame cost but cannot see fill-in order or timing. Those
  were checked with `dev/shot.rs` captures instead.
- 71 clippy warnings in galos_map, most of them the argument counts above
  and complex types; a few collapsible `if`s.

## Broken

- `cargo test --release -p galos_map` does not compile: `ui/bar/search.rs`
  and `ui/bar/selection.rs` import `ui::testing::draw_selected`, which the
  release test build does not have.
- `.index/7day` is at index format 2 and refuses to open until
  `galos index migrate` is run over it.
