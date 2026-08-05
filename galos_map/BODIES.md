# Rendering bodies inside a system

## Context

`big_space` is in (issue #28, closed), and `space.rs` already says what comes
next: *"Bodies are not placed in this grid; they belong to a finer one nested at
their own system."* `scale.rs` says the same about size: *"Once the camera is
close enough to see inside a system, its extent should come from what is in it…
The two want blending over the range where neither is right on its own, rather
than switching between them at a threshold."* This is issue #44.

Three things have to happen together for it to be worth anything: somewhere to
put bodies, a way to reach them from the UI, and a way to arrive that just works
as you zoom.

### What the investigation turned up

**The map cannot currently draw anything at system scale, at all.** The camera
had no explicit `Projection`, so Bevy's default `near: 0.1` applied — and a
world unit was a light year. Everything within a tenth of a light year of the
camera was clipped. Fixed in Stage 0, already landed.

**Nested grids cannot change units.** big_space composes grid transforms with
`DAffine3::from_rotation_translation(..)` — scale is deliberately dropped. So a
scaled grid converting light years to metres silently does nothing. There is one
unit for the whole hierarchy. What a nested grid *can* vary is its cell edge.

**That unit has to be the metre.** Bevy's PBR is in physical units, and the goal
is eventually real textures and surfaces. Light years would mean writing our own
lighting, which is the wrong trade. The database is already in metres, so this
also removes a conversion rather than adding one.

**big_space does the hard part on its own.** The render transform is computed
from an `i64` cell *difference* before anything becomes a float:

```rust
let cell_origin_relative = *local_cell - self.local_floating_origin().cell();
let grid_offset = self.cell_to_float(&cell_origin_relative);
```

So precision near the camera does not depend on distance from the galactic
centre, and nothing local does any per-frame bookkeeping. An entity gets a cell
and an offset when it spawns, and that is the end of its involvement.

**Precision does depend on the floating origin sharing a grid with what is
rendered.** The camera's own offset inside a galaxy cell is an `f32` — hundreds
of thousands of kilometres. So the camera has to descend into a system's grid to
see anything in it. That is the one genuinely new piece of machinery, and it
rides on the gate Stage 5 needs anyway.

### Decisions taken with the owner

Everything lands on one branch. Bodies are placed statically at their recorded
epoch, but behind an interface that takes a time, so a time knob drops in later
without a rewrite. Stars are read properly from the `stars` table rather than
faked; the system's centre stays where the galaxy grid says it is, and stars sit
inside it with bodies orbiting them.

A system keeps a drawn boundary — and that decision simplified the design rather
than adding to it. The sphere a system is already drawn as **is** that boundary,
not a marker to be faded out and replaced by one. One object, named **`Shell`**:
a surface enclosing a volume, which is what has to read both as the opaque ball
seen from light years off and as the edge you end up within. Elite has no term
to borrow — it has no system boundary at all, and its *bubble* means the
inhabited region of the galaxy, a sense this repo already uses at `camera.rs:49`.

## Style

The repo's register is unusual and matching it is half the work. Every module,
type, constant, function and test carries prose saying *why*, in plain English —
"A system is a place and a star is a thing in it". Constants are documented with
the measurement or reasoning behind their value. Tests are named as sentences
(`the_spyglass_decides_what_is_drawn`) with a doc comment on the case each pins
down. Read `space.rs`, `scale.rs` and `camera.rs` before writing a line.

---

## Stage 0 — See anything at all *(landed: `7613607`)*

`camera.rs` grew `focus_lens`, the sole writer of the camera's `Projection`,
holding `near = radius × NEAR_FRACTION` (`1e-4`) so the clip plane is always a
fixed fraction of the way to whatever is being looked at. `far` set past
everything, since the infinite reversed projection ignores it for the matrix but
`compute_frustum` still culls with it — the default `1000.` was quietly dropping
every star beyond a thousand light years.

`near_clip_plane` is deliberately untouched: its default *looks* like a stale
`0.1`, but `adjust_perspective_matrix_for_clip_plane` tests only the normal and
returns early, so it clips nothing. Giving it one would turn on the oblique
clipping meant for portals.

**Revisit in Stage 1:** `NEAR_FRACTION` is dimensionless and survives, but
`MIN_RADIUS`, `MAX_RADIUS` and `SIGHT` are light-year figures and become metres.

---

## Stage 1 — Metres, and two grids

**`galos_map/src/space.rs`** — the root grid is re-based to metres, and gains a
nested grid per system.

| | cell edge | cells used | precision | holds |
|---|---|---|---|---|
| galaxy grid | `2^53` m ≈ 0.952 ly | ~68,000 to the rim | ~270,000 km | systems |
| system grid | `1` m | ~`2.1e14` at 700,000 ls | ~30 nm | stars, bodies, orbits |

Both are far inside `i64`'s `9.2e18`. The galaxy grid's coarse precision is
ample, since it only ever places systems and their positions are quantised to
1/32 ly anyway.

**Cell edges are powers of two in metres.** `2^53` is exactly representable in
both `f32` and `f64`, so `cell × edge` is exact and no drift accumulates. A
literal light year (`9.4607e15`) is not, and over 68,000 cells the error skews
the star map by ~0.004 ly — small, but free to avoid.

Light years remain the map's *vocabulary*: `System::position`, the spyglass,
routes, "N Ly away". Metres appear only where a position becomes a grid cell.
This undoes a stated decision — `CELL_EDGE = 1.` is documented as keeping a
cell's coordinate and a light-year position the same number — so the new doc
comment should say plainly what is being given up and why.

**A system becomes a grid only while it needs to be one.** Not at spawn — the
`Grid` is inserted when its contents arrive and removed when they are dropped,
on the gate in Stage 5. big_space's local-origin pass describes its own
traversal as *"we will only look at siblings and parents, which will allow us to
visit the entire tree"*: the ordering buys precision, not pruning, so the
per-frame cost is proportional to how many grids exist. A grid per loaded system
would be ~14,000 of them at a 60 Ly spyglass and every system on record with it
opened wide, each costing an `f64` affine composition every frame. One or a
handful at a time costs nothing.

Order matters on the way out: despawn the contents first, *then* remove the
`Grid`, or there is a frame with `CellCoord` children under a non-grid.

Worth measuring rather than trusting: put a grid on every loaded system once,
with the spyglass wide, and see what it does to the frame time. If it turns out
cheap the lazy insert is still right, but the reasoning behind it would be wrong.

`Grid` has no hooks and no required components, so inserting it on an entity that
already exists is enough; and the `System` entity already carries the `CellCoord`
and `Transform` a nested grid wants. Children *without* a `CellCoord` "behave
like a normal single precision transform", so the shell, pointer target and label
are untouched.

The existing `Query<&Grid, With<BigSpace>>` lookups (`camera.rs:368`,
`spawn.rs:303`, `spawn.rs:507`) stay correct thanks to the `With<BigSpace>`
filter — check each rather than assuming.

**Rename `Star` → `Shell`.** The component at `spawn.rs:181` was never a star;
it is the sphere a whole system is drawn as. Once `stars` is read, `Star` means
a star. It cannot move onto the `System` entity and take *that* name either:
`spawn.rs:174-179` keeps it on a child because scale is inherited and this
sphere is drawn wildly exaggerated.

---

## Stage 2 — Reading what is in a system

**`galos_db`**

- Derive `Clone` on `bodies::Body` and `stars::Star`.
- **Write `Star::fetch_all(db, system_address)` fresh.** `stars/fetch.rs` is 197
  commented-out lines, but it is copy-pasted *bodies* code — it queries
  `FROM bodies` and builds a `Body`. It cannot be uncommented. Restore
  `mod fetch;` at `stars/mod.rs:40`.
- **`.sqlx` must be regenerated.** CI runs `SQLX_OFFLINE: true` and the cache has
  no `SELECT … FROM stars`. Run `cargo sqlx prepare --workspace` against a live
  database and commit the result. Nothing else here adds a query —
  `Body::fetch_all`, `Body::fetch_like_name` and `DbSystem::fetch(db, address)`
  all exist and are all cached.

**New `galos_map/src/systems/bodies/`** (`mod.rs`, `fetch.rs`, `orbit.rs`,
`spawn.rs`), a sibling of `route/`. `fetch.rs` keeps its own task map rather
than widening `FetchIndex`, which answers `Vec<DbSystem>` and only that:

```rust
#[derive(Resource, Default)]
pub struct ContentsTasks(HashMap<i64, Task<Contents>>);

pub struct Contents { stars: Vec<Star>, bodies: Vec<Body> }
```

Keyed by system address, since contents are asked for whole or not at all.
Polled in `MapSet::Populate`, as `spawn::spawn` polls its own.

---

## Stage 3 — Where each body sits

**`galos_map/src/systems/bodies/orbit.rs`** — pure maths, no ECS, testable
without a window or a database.

```rust
/// The path one body takes about whatever it goes round
pub struct Orbit { /* metres and radians */ }

impl Orbit {
    /// Where the body is `since` seconds after the epoch it was recorded at
    pub fn at(&self, since: f64) -> DVec3;
    /// The whole path, as `steps` points
    pub fn path(&self, steps: usize) -> Vec<DVec3>;
}
```

`at(0.)` is the static placement asked for now; a time knob later passes
something other than zero. That is the whole reason it takes a time it currently
ignores.

**Lengths need no conversion** — `radius` and `semi_major_axis` are already
metres, which is now the map's unit. Angles are degrees and become radians
(`axial_tilt` is already radians); periods are seconds.

Kepler's `E - e·sin E = M` by Newton from `E₀ = M + e·sin M`, capped at a fixed
iteration count so a near-parabolic orbit cannot spin. Then perifocal position
rotated by `R_z(Ω)·R_x(i)·R_z(ω)`. The reference plane is taken as the map's own
`y = 0`; the journal's frame is undocumented, so say so rather than implying
precision the data does not have.

**Parents.** `parent_id` has no foreign key and no index, and may name a star, a
body, or a barycentre that is not stored at all (issue #70). Resolve the chain
within the `Contents` just fetched; where the parent is not among them, orbit
the system centre — an honest shortcut, documented where taken.

**Tests:** `a_circle_is_solved_without_iterating`,
`kepler_returns_the_anomaly_it_was_given`,
`a_body_comes_back_to_where_it_started_after_one_period`,
`an_orbit_with_no_period_stands_still`, `degrees_become_radians`,
`a_body_whose_parent_is_not_on_record_orbits_the_system`,
`a_moon_is_placed_relative_to_the_planet_it_goes_round`.

---

## Stage 4 — Drawing them

```
Galaxy         BigSpace, Grid(2^53 m)
└── System     CellCoord, Transform, Grid(1 m)
    ├── Shell          no CellCoord — the system drawn as one thing
    ├── PointerTarget  no CellCoord
    ├── Name           no CellCoord
    ├── Body × n       CellCoord + Transform — stars, planets, moons
    └── Orbit  × n     CellCoord + Transform — one LineStrip each
```

- **Materials** follow the existing `SystemMaterials` pattern: palettes built
  once at startup, pointed at by handle, never mutated per entity. Stars
  emissive from `star_class`/`surface_temperature`.
- **Planets can be lit properly now.** Metres is exactly what makes a
  `PointLight` at a star mean something, so this is where it is worth trying
  rather than falling back to unlit materials keyed by `planet_class`. If the
  intensities prove awkward at these ranges, unlit is the retreat — say which
  was chosen and why in the module doc.
- **Meshes.** `SystemMesh` is `ico(1)` — twenty faces, fine for a two-pixel
  marker and plainly a solid once anything fills the screen. Bodies and the
  shell want their own handles at a higher subdivision.
- **Orbit lines** reuse `route::LineStrip`, hung off their own centre as
  `route/spawn.rs:60-68` documents, so vertices carry only the size of the orbit.
- **Picking**: `MeshPickingSettings { require_markers: true }`, so bodies need
  their own targets on the `pointer_target` model with
  `should_block_lower: false`.

---

## Stage 5 — Arriving

**The old law had the right shape. Its floor was standing in for the system.**

A shell is drawn at `4e-4·d + 8.5e-2` light years today: an angular term, which
holds it to a constant size on screen however far off it is, and a floor. The
floor is there because a system used to be one sphere with nothing inside it —
the angular term falls to nothing as the camera arrives, and without a minimum
there would be nothing left to look at. It has a consequence nobody wanted: the
sphere never shrinks below `8.5e-2` ly, so from that distance inward the camera
is inside it, and zooming into a system is not awkward but impossible.

Put the system's own size where the constant was and everything wanted falls
out of the same expression:

```rust
/// How large a system is drawn, in metres
///
/// A system drawn at its true size is invisible from the next one over, so
/// what is drawn is that size plus whatever angle keeps it on screen. Far off
/// the angle is the whole of it and a system holds a constant mark; the angle
/// falls away as the camera closes, and what is left is the system, a little
/// larger than life so that its outermost orbit sits comfortably inside.
///
/// Always larger than the system, and approaching it: the excess halves as the
/// distance halves. So a shell settles onto its system rather than arriving at
/// it, and there is no distance at which it does anything sudden.
fn shell(extent: f64, distance: f64) -> f64 {
    ANGULAR * distance + extent * MARGIN
}
```

Continuous everywhere, monotonic, and never smaller than what it encloses. No
maximum to put a corner in it, no ramp with two ends to name, no `closeness`,
no blend. The camera reaches the surface at `d ≈ extent × MARGIN` and passes
through a shell that is dissolving by then.

### What a system's size is before anybody has asked

`extent` has three states, not two: **not yet asked**, **asked and empty**, and
**asked and known**. The first two are the same picture — the map does not know
— so they are drawn the same way, from a stand-in extent of a typical system.
That makes an unasked system and one with nothing on record indistinguishable,
which is honest, since nothing about the difference is actionable.

An empty system therefore holds at the stand-in: its shell converges, stops, and
stays opaque, because nothing loaded means nothing to dissolve for. There is no
way through it. Zooming further is pointless rather than broken, which is about
as close to stopping the zoom as is worth building.

### Two ranges, and why the load one is generous

- **Load range, about 5 light years.** Fetch the rows and learn the true extent.
  Data only: no entities, no grid, no camera descent. One system at a time — the
  nearest — with hysteresis so a tie does not thrash, and one query per swap.
- **Spawn range, close in.** Create the bodies, insert the `Grid`, descend the
  camera. This is where the cost is, and it stays where there is something to
  see.

The load is generous **so that the extent changes while the angular term is
still most of the shell**. The test is not that the shell be too small to see —
at 5 ly it is a small sphere and should be — but that `ANGULAR · d` dominate
`extent × MARGIN`, because then swapping one extent for another barely moves it.
With `ANGULAR` at `4e-3`, about a fifth of a degree, and a stand-in of 5,000
light seconds:

| at 5 ly | shell | on a 1080p screen | change when the rows land |
|---|---|---|---|
| stand-in | `0.0202` ly | 4.7 px | — |
| Sol, 15,000 ls | `0.0206` ly | 4.8 px | **+1.9%** |
| compact, 1,000 ls | `0.0200` ly | 4.7 px | **−0.8%** |
| far tail, 700,000 ls | `0.0466` ly | 11 px | +131% |

An ordinary system does not visibly change at all. What the true extent actually
decides is **where the shell stops shrinking** — a smaller one pushes that
crossover closer in, so the shell goes on shrinking for longer and only begins
to grow on screen later. That is the whole of the difference, and it is a
difference in timing rather than an event.

It is also the same dial as the one that has to be settled by eye for a crowded
sky: `ANGULAR` is what holds a distant system on screen, and raising it is what
makes the 5 ly load smooth. One experiment answers both, and it comes first.

The stand-in does not have to err large. At 5 ly the camera would have to close
two and a half thousand times over before a single query returned in order to be
caught inside the shell, so a stand-in near the typical extent is right, keeping
corrections small in both directions rather than biased into one.

**Only the far tail moves visibly**, roughly doubling. Which the dissolve covers,
given the rule below.

### Opacity answers `d / shell`, and nothing else

Not time since loading, not an absolute distance. Keyed on where the camera is
relative to the shell it is approaching, two things come out right on their own:

- Flying in dissolves the shell, because `d / shell` falls. Tuned by eye.
- A system that gains bodies while the camera is already deep inside it grows
  its shell around the camera — and is transparent when it does, because
  `d / shell` is already tiny. What the viewer sees is the bodies arriving,
  which is what happened. Key it on time or on absolute distance instead and a
  solid sphere snaps shut around the camera.

**Bodies are not polled.** `Poll` exists because a system's row changes:
population, allegiance, who holds it. Orbital elements do not — what changes is
whether anyone has scanned them, and learning that mid-approach is rare enough
to leave to the next visit, when the camera is light years off and nothing
shows. That removes almost every mid-flight change of extent, and the rule above
covers the rest.

It answers three things:

1. **When to load, and when to spawn.** The two ranges above.
2. **When a system becomes a grid, and when the camera descends into it.** At
   the spawn range, so the grid, the bodies and the camera's frame arrive
   together and cannot disagree. Galaxy-grid precision is ~270,000 km, five
   orders below any distance the spawn range would sit at, so the descent has
   room either side and does not have to be exact.

   **Rendering is not affected by where the camera lives.** big_space composes
   every grid's transform relative to the floating origin, so a shell in the
   galaxy grid draws correctly with the camera nested inside a system — a shell
   60 Ly off lands with ~`3.4e10` m of precision against a `1e15` m sphere.
   What changes is only which arithmetic `orbit_camera` drives itself with.

   `OrbitCamera` keeps `focus` and `eye` as absolute galactic `DVec3` light
   years, exactly as now, so `visibility()`, `size_by_distance`,
   `fetch_spyglass` and every "N Ly away" are untouched — all galaxy-scale
   questions where 140 km is noise. This is the same split `System` already
   makes, and for the reason its doc gives: the undiminished answer is kept
   beside the grid placement rather than unpicked from it.

   The switch is only about which one *drives*. In the galaxy grid the `f64`
   focus drives and is split at the end, as today. Descended, the local metre
   position drives and the galactic `f64` is derived. The seam is the descent
   itself — `local = galactic_camera - galactic_system` inherits ~140 km at the
   rim — which is `2e-9` of the distance at the gate and does not compound,
   since flying to a body targets that body's exact local position.

   Still the fiddliest part of the change, and confined to one component.
3. **How large to draw the shell, and how solidly.** `shell(extent, d)` for the
   size, `d / shell` for the material — hand-tuned, since how a boundary should
   read as it is flown through is a thing to look at rather than to derive.

Later, `apparent_brightness(luminosity, distance)` sits beside this and decides
which stars are too faint to draw at all. `stars.absolute_magnitude` and
`stars.luminosity` are already in the schema waiting for it, and it is the same
shape of question: what a thing looks like from here, rather than how far away
it is.

**Drop the `View::Bodies` TODO** at `scale.rs:60` rather than filling it in —
the point of all this is that there is no mode to switch to. Say so in the doc,
since it is a promise being deliberately broken.

**Tests**, on the sizing itself, which is pure and wants sweeping rather than
sampling:

- `the_shell_is_never_smaller_than_the_system_it_holds` — the invariant the rest
  rests on. Swept over distances spanning the whole range and over extents from
  a compact system to the widest on record.
- `the_shell_settles_onto_its_system_rather_than_arriving` — halving the distance
  halves what is left over above the true size.
- `swapping_a_stand_in_for_the_truth_barely_moves_the_shell` — at the load range,
  and this is the one the whole two-range arrangement exists for.
- `a_smaller_system_only_moves_where_the_shell_stops_shrinking` — that the
  difference is in timing rather than in size.
- `a_system_with_nothing_on_record_holds_at_the_stand_in`.
- `a_shell_that_grows_around_the_camera_is_already_transparent` — the `d / shell`
  rule, which is what makes a system gaining bodies mid-flight harmless.
- `a_system_on_the_threshold_does_not_flicker`,
  `the_camera_descends_when_the_bodies_are_spawned`, over `MinimalPlugins`.

Write the size ones over a *compact* system as well as over Sol. A floor-shaped
mistake passes on Sol and fails on everything smaller, which is exactly how the
first version of this survived being designed.

**Walked end to end**, focus following the zoom, `near = radius × 1e-4`,
`shell = ANGULAR·d + extent × MARGIN` with `ANGULAR` at `4e-3` and `MARGIN` at
`1.2`, Sol's extent taken as Neptune's orbit — `4.5e12` m, so `5.4e12` with the
margin:

| focus | camera radius | near | grid | shell | on screen |
|---|---|---|---|---|---|
| galaxy | `4.7e20` m (50,000 ly) | `4.7e16` m | galaxy | `1.9e18` m | `4e-3` rad, a mark |
| Sol | `2.8e17` m (30 ly) | `2.8e13` m | galaxy | `1.1e15` m | `4e-3` rad, still a mark |
| Sol | `4.7e16` m (5 ly) | `4.7e12` m | galaxy | `1.9e14` m | rows land, shell moves 1.9% |
| Sol | `1.4e15` m (0.14 ly) | `1.4e11` m | galaxy | `1.1e13` m | the angle and the system are equal here |
| Sol | `9.5e13` m (0.01 ly) | `9.5e9` m | → system | `5.8e12` m | bodies spawned, camera descends, `3.5°` |
| Sol | `5.4e12` m | `5.4e8` m | system | `5.4e12` m | through the surface, `1` rad, nearly clear |
| Earth | `5e7` m (50,000 km) | `5e3` m | system | — | Earth a 7.3° disc |
| Luna | `5e6` m (5,000 km) | `500` m | system | — | Luna 20°, Earth 0.95° |

Fourteen orders of magnitude of camera radius, one continuous zoom, one grid
handover, and nothing anywhere in it that steps.

At the far end the camera's offset is good to 30 nm while Luna's mesh, `f32`
vertices scaled to `1.7e6` m, is good to about 0.1 m — the mesh becomes the
limit before the grid does, which is the argument for chunked terrain when
surfaces arrive rather than for finer cells.

### Still open: what should `extent` mean?

Taken as the outermost body it is set by whatever is furthest out, which is what
makes the tail so long. A system with everything inside a few thousand light
seconds and one thing at 700,000 gets a shell two hundred times wider than where
it actually lives, and what there is to see is a speck at the middle of it. A
high percentile instead of a maximum would frame what is there, and would
collapse the range of extents far enough that the tail's correction at the load
range stops being the one visible case.

It trades away containment: those few far bodies would be drawn outside their own
shell. Whether that is wrong or simply honest is a question about what the shell
is *for*, and it should be looked at rather than assumed.

**Shells at their true size cannot overlap.** The widest is about a fifth of a
light year against a spacing of four or more, twenty times clear, and the
ordinary ones are clear by thousands. Overlap belongs entirely to the angular
term, and is unbounded: at 50,000 ly with `ANGULAR` at `4e-3` every shell is
drawn 200 ly across against that same four, so millions of them interpenetrate.
That is what makes the galaxy read as a glow rather than as points, and it is
what the map already does — raising `ANGULAR` only makes more of it.

But it says the angular term is doing a point sprite's work with geometry. The
two ends of the map want different machinery — additive sprites carrying
brightness far off, a real sphere at a real size near to — rather than one law
stretched over both. That is where `stars.absolute_magnitude` and
`stars.luminosity` finally pay: a star too faint to contribute is not drawn at
all, instead of being a sphere too small to see. Left for its own change, and
the thing to test first, since it decides how a sky of millions reads.

---

## Stage 6 — Reaching a body

**Search (`search.rs`)** — one box, not two. A name is tried as a system, and if
nothing answers, as a body: `Body::fetch_like_name` then
`DbSystem::fetch(db, address)`, both existing and both already cached. So `Sol 3`
finds the third planet of Sol, and `SearchNote` says "No system or body named X"
when neither answers.

**Selection (`selection.rs`)** — `Selection` holds a `System` value because a
searched system is described before it is fetched (`selection.rs:57-62`). The
same holds for bodies, so a body is held *beside* its system rather than instead
of it. `position()` then answers where the body is, so "N Ly away", Center Camera
and the selection ring keep working unchanged.

**Panels (`info.rs`)** — `Subject::System` gains a bodies list filled by the
`fill_filters` idiom (`info.rs:277-312`): open now, fill next frame, say
"Looking…" between. Lines answer the same `Picked::{Select, Travel, Describe}`
gestures the filter list already answers. `Subject::Body(Body)` is the long form;
`galos_server/templates/body.html` shows which fields are worth having.

**Settings (`ui.rs`)** — a "System" section: checkboxes for stars, planets,
moons, orbit lines and the shell.

**Body filters are out of scope**, and the plan says so rather than leaving it
looking forgotten. A filter is a question asked of every *system*, answered from
what a `System` carries, dimming those that fail. "Systems with an Earth-like
world" needs new `galos_db` queries, another prepared-query regeneration, and a
`System` that carries something about its bodies. Worth its own issue.

---

## What to watch

- The camera's descent is the one piece with no precedent in the codebase. Build
  it behind the same gate as the contents so the two cannot disagree.
- `visibility()` (`systems/mod.rs:169`) hides a system by writing `Visibility` on
  the parent; bodies inherit it. Confirm that is wanted when the spyglass narrows
  while you are inside a system.
- `labels::choose_names` solves a screen-space packing over every candidate.
  Keep bodies out of it for now, or gate them on the same `apparent`.
- Turn on big_space's `BigSpaceValidationPlugin` while the nested grids are being
  built — nothing validates the hierarchy at runtime.
- Grids are per-frame work for big_space, so they are created and destroyed with
  a system's contents rather than living on every loaded system. Measure it, and
  say in the doc comment what the measurement was.
- The spyglass is not drawn — it is a culling radius `visibility()` applies by
  distance from the focus, and nothing on screen shows its reach. Worth a gizmo
  sphere one day; out of scope here, and not to be confused with the shell.
- `Body` is not `Clone` and `Star` has no read path: Stage 2 blocks everything
  after it.

## Verifying

```sh
git submodule update --init          # nothing builds until this is done
SQLX_OFFLINE=true cargo test --all   # what CI runs
DATABASE_URL=postgresql://…/galos cargo run -p galos_map
```

`cargo sqlx prepare --workspace` needs a live database and has to happen before
CI can build Stage 2.

End to end, by hand: search `Sol`, click the row to fly there, and keep zooming.
The shell should shrink and thin out rather than swallow the camera, closing
around you at the system's true extent, with the star and its planets inside it
where the ephemeris says. Then search `Sol 3`, confirm it is picked out and
described, and open its panel. `--features inspector` gives a world inspector if
a transform looks wrong.
