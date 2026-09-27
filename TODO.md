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

The index's own parts are listed once (`galos_index::format::parts::CorePart`)
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
