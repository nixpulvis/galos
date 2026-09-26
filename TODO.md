# TODO

Left over from the reorganization into `galos_map::{map, ui}`, `galos_route`
and galos_index's layers.

## One point spread, not two

`galos_map::map::paint::sizing::instrument` and `galos_sky::Camera::psf` both
build the seeing core with the reference aureole behind it, out of
`galos_photometry::psf::AUREOLE_*`. Both say they are the same instrument, and
they are two copies of it (the sky's caps the halo share at 0.95 and stacks
several aureoles, the map's does neither). Move the stack into
`galos_photometry::psf`, with `Aureole`/`Effect` beside it, and have both
renderers call it.

## `galos_index/src/store/bodies.rs`

What was `pack.rs`, about 2,450 lines: the shard layout and index table, read,
write and remove, reclaiming dead records and punching holes, weighing and
sweeping, iteration, and the migration of loose files into shards. Split along
those lines into `store/bodies/`.

## Naming the core types — needs discussion

- `System`, `Index`, `Sky` and `Source` each mean something else nearby:
  galos_map and galos_db both have a `System`, the root crate imports `Index`
  as `ServedIndex`, galos_map imports `Source` as `IndexSource`, and `Sky` is
  also the galos_sky crate. Candidates floated: `SystemRecord`, `CellIndex`,
  `MappedGalaxy`, `Transport`. Every consumer changes, so agree on names first.
- A system's identity is `id64: u64` in about a hundred places and
  `address: i64` in about eighty-five (`Tree::holds(address: i64)` beside
  `Tree::forget(id: u64)`). One newtype, and which sign, is the question.

## Warnings

`cargo check --workspace --tests` warns 17 times, all
`panic message contains an unused formatting placeholder`: five in
`bin/galos/ingest.rs` and twelve in `src/read/mod.rs`. A lint newer than the
code; pass the value or drop the braces.

## Benchmarks

`galos_index/tests/{procedural,zooming}.rs` and `galos_route`'s `perf` module
measure a local `GALOS_PERF_DIR` and pass silently without one. Reorganize
them, with `galos_index/examples/names_bench.rs`, into criterion benchmarks.
