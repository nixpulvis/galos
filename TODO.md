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

- Settings are not persisted: every setting is back at its default on every
  launch.
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

## Merged marks

A merged mark stands at the brightest system its cell owns
(`Cell::brightest_at`, index format 6), which took away the lattice marks at
cell centroids drew over the dense disc. What is left:

- **A split does not hand over in place in every view.** Once a cell splits
  its slice draws as marks in the view's own order: brightest first in the
  sky, so the speck stays put, but by strata on the map, where the brightest
  may not be among the first drawn and the speck moves.
- By population a blob is named by the busiest system but stands at the
  brightest. Not carried; another 12 bytes a cell.
- **Maybe antialias the speck.** At a 0.75 px radius a mark lights one
  pixel or four depending on its sub-pixel phase. A quad of at least ~1 px
  with brightness scaled by `(SMALLEST / r)²` keeps the light and loses the
  shimmer.
- Not wanted: fading blobs where they tile and leaving the density to the
  glow; the number of marks is what says the density. Nor jittering the
  centroid: it draws a mark where no system is.

## The glow's volume

The glow's evenly filled cells are a volume now
([`galos_map/src/map/paint/volume.rs`](galos_map/src/map/paint/volume.rs)):
the basis method (Wald et al., 2017), ghosts of density nought past the
crowd's edge, one march a pixel through a table of the boxes, each box cut
wherever its field can turn, at half resolution and added in by the curve.
A filled cell hands its light from its splat to the volume across 4 to 8
logical pixels of edge; filaments, knots and the walk's cross-fades keep
their splats. The squared mask is fixed (`the_mask_is_drawn_once`), which
put the glow up a stop; nothing was retuned for it.

**How it measured.** Glow alone, looking down (`GALOS_SHOT_PITCH=-1.5`), at
the galaxy seen whole (150 k back, reach 90 k), the default (30 k back,
10 k) and close (6 k back at `X=-12000 Z=15000`); the Fourier peak at the
512 ly pitch over the mean.

- Window captures, sRGB, centre patch: before 3.0 %, 2.0 %, 14.3 %; the mask
  fix alone 0.3 %, 1.2 %, 5.9 %.
- Headless renders of the field target, linear, quiet patches: the mask fix
  alone 0.9 %, 3.2 %, 4.6 %; with the volume 0.9 %, 1.9 %, 2.2 %, and the
  close pose's lumps gone (high-pass rms 11 % of the mean to 5 %). Seen
  whole the leaves are 3 px, under the band, and nothing changes.
- The march matches the same field integrated by brute force on the CPU to
  0.1 %. Cut only at the boxes' centres it was out by up to 1.4 %, in thin
  lines; drawn a quad a box, summed by the blend, by a percent in lines at
  every box edge.

**What is left.**

- Not looked at in the window: the screen locked before the volume was in, so
  the pictures are headless renders of the field target, and the curve's
  reading of the half-resolution targets was checked on the GPU on its own.
- The tents are continuous but not smooth: a high-pass at the close pose
  shows faint creases on the cells' centre planes. The octant method is the
  smoother sibling, if they show.
- Cost, measured headless at 2560×1440: the march adds about 5 ms a frame
  seen whole and at the default, nothing close (at full resolution it was
  13 to 23 ms); cutting at the finer neighbours' turns is half of it.
  `volume::Built::of` is about 5 ms a moving frame at the default (21 k
  cells, 7.5 k ghosts, 720 k list entries), across threads.
- Seen whole the band's edge runs through the frame: 4.4 k cells and 22 k
  ghosts, for little light.
- The volume ends at the reach's sphere over a sixteenth of it, and a cell's
  volume light fades over the same band as its box leaves the sphere
  (`volume::rim`), so the walk dropping it takes nothing on screen with it;
  the splats still fade by how much of themselves the reach holds. Whether
  a channel fills its cell is asked of the cell's systems whole, not of what
  the marks left, so a cell no longer flips between box and Gaussian as
  marks come and go. Both were found off the field's own accounting, frame
  by frame, with the screen locked: not yet looked at in the window.
- Marks churn during a zoom: a cell's drawn count moves up and down by a few
  a frame as the share rises and the reach takes systems off its edge, and
  each system's light moves between its mark and its cell's residual. A
  residual of a few colonies changes shape with it. Small, but a step and
  not a fade.
- The tents attempt is still in `git stash list`.

## Smoother while moving

Measured with `profile.sh` over `.index/full`, the galaxy seen whole and
panning (`galaxypan`), zooming out (`out`) and in (`in`), frames from 30 %
of the way in; ms per frame.

| | before | after |
|---|---|---|
| `galaxypan` p50 / p90 | 36 / 55 | 24 / 37 |
| `out` p50 / p90 | 46 / 75 | 41 / 56 |
| `in` p50 / p90 | 29 / 61 | 25 / 39 |
| `walk::reconcile`, `galaxypan` | 12.1 | 5.5 |
| `blobs::weigh_blobs`, `galaxypan` | 3.8 | 0.9 |
| `walk::evict_payloads`, `galaxypan` | 1.4 | 0.2 |

What did it: the weighing order sorted on keys worked out once; the
payload sweep on its clock alone; the merged marks weighed and the cells
taken across the pool; reach and share worked out once a mark a pass; the
dark tiles' projection and the camera's axes worked out once a frame; no
verdict walk while nothing is asked; the two screen cuts walked side by
side; the populated table on `rustc_hash`. Stills are pixel for pixel what
they were.

What is left, on `galaxypan`:

- The main thread, ~19 ms a moving frame: `paint::glow::build_glow` 7.4
  (the splats 4.0, `volume::Built::of` 2.8), `walk::reconcile` 5.5 (asking
  each marked cell for its payload and strata, 2.0, is serial: a probe into
  a hundred thousand payloads apiece), `fetch` 1.8, `plan` 1.5.
- The render thread, ~14 ms, ~6 of it `allocate_and_free_meshes`: the glow
  and the marks field are fresh meshes every frame the camera moves, 168
  bytes a quad. Laid as instances out of a storage buffer, 28 bytes each,
  the upload and the join that builds it would shrink sixfold. Dropping
  quads laid far under a display level would too, and changes the look.
- The slowest tenth are frames the payload reads land in: `cell payloads`
  is 150 ms of CPU a frame across the pools then, two files opened and a
  `stat` a cell (`Directory::read_lit`, `Payload::open`, `FsSource::stamp`),
  and it starves the compute pool the glow and the walk spread over.
- `galaxy::System::build` names a system through `Names::get`, which builds
  a whole `NameEntry` to take its name; `name_of` is the cheaper answer.
  Off the main thread now, so it costs fill-in time rather than frames.

## Enhance

`map/enhance.rs`: every system in the view, read straight off the index
payloads on a thread of its own, projected through the map's own mirrored
lens (`plan::Lens`; the index's `View::projector` is right handed and the
map draws the galaxy mirrored) and its map light (`system_light` ×
`Hue::light`, clamped to the spyglass, what the filters exclude summed apart
and laid under at the dim) summed
per pixel; laid over the map as a flat picture `scale` windows across, a
window-sized piece at a time, on one log curve whose top is read off the
base. Run with `GALOS_ENHANCE=3 GALOS_ENHANCE_EXIT=1` and a pose held by
`GALOS_SHOT_WAIT=1000000`: a 3× picture is drawn in 1.1–2 s and a 6× in
1.5 s, the map's peak memory no higher than without it, the seams
invisible, the picture registered on the window's own view (it correlates
0.945 with the plain window shrunk to it, 0.896 mirrored), and a rerun
bit-identical. While a picture covers the window the map's own cameras
stand down (`Covered`) and the walk keeps planning the view as it stood, so
looking about in it spawns nothing: zoomed 3× into the dense middle the map
holds the 32,409 systems it held and draws at ~120 fps, where it had loaded
287,622 at 15. The camera stands still under a picture: the wheel, a drag,
`WASD` and `F`/`R` look about in it, and only close, escape and a resize of
the window put it away. Left to do, the first first:

- A system is a disc as large as the room around it on screen allows, up to
  the map's floor (`Crowding`, read off the cells' counts), and a point where
  the sky is crowded; its light is shared over the disc, so sparse sky is no
  brighter for being drawn in marks. The curve is white at the view's
  brightest and bends to put its middling lit pixel at a mid grey, so close
  and far views both read. A system's own size, close enough to see it, is
  still not drawn; nor is bloom, which is the next thing.
- By hand: the base standing on the window as it lands, the wheel and a drag
  in a shown picture with the map registered beneath it, cancel part way,
  close and Escape. Only the scripted run has been seen.
- The curve: a log of light topped at the base's 99.5th percentile
  (`WHITE_AT`). Wide views come out paler and sharper than the map, which
  lays its glow over the marks; no control for it yet.
- A cell straddling pieces is read once a piece: 6× reads 88.7M systems for
  the 37M in reach. Cheap at these sizes; a picture far past six would want
  the culling done down the tree rather than over every cell a part.
- The GPU holds every piece, RGBA8: 6× a 1440p window is half a gigabyte.
  Pieces drawn on demand while looking in would bound it.
- A filter change cancels through `Filters::is_changed`; check nothing writes
  the filters every frame, or a picture never finishes.
- Pointing at the picture: the full-window area that takes the wheel and the
  drag also takes clicks, so nothing on the picture can be hovered or picked,
  though the map underneath is registered to it and could answer.
- The EDAstro-style distribution maps this was for: a straight-on lens with
  top-down and side presets, which is what gives them a fixed scale.

## Broken

- `cargo test --release -p galos_map` does not compile: `ui/bar/search.rs`
  and `ui/bar/selection.rs` import `ui::testing::draw_selected`, which the
  release test build does not have.
- `.index/7day` is at index format 2 and refuses to open until
  `galos index migrate` is run over it.
