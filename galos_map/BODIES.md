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

**The floor goes, and the system's own size takes its place.**

A shell is drawn at `4e-4·d + 8.5e-2` light years today: an angular term, which
holds it to a constant size on screen however far off it is, and a floor. The
floor is there because a system used to be one sphere with nothing inside it —
the angular term falls to nothing as the camera arrives, and without a minimum
there would be nothing left to look at. It has a consequence nobody wanted: the
sphere never shrinks below `8.5e-2` ly, so from that distance inward the camera
is inside it, and zooming into a system is not awkward but impossible.

So the floor was always standing in for the size of the system, back when that
was not known. Now it is, and it can say so:

```rust
/// How large a system is drawn, in metres
///
/// Two answers, and the larger of them wins. A system drawn at its true size
/// is invisible from the next one over, so far off it is drawn at whatever
/// angle keeps it on screen; and that angle shrinks as the camera closes,
/// until it passes the size the system actually is and there is no longer any
/// reason to pretend.
fn shell(extent: f64, distance: f64) -> f64;   // ANGULAR * distance ⊕ extent
```

A soft maximum rather than a bare one — `(a^n + b^n)^(1/n)` with a small `n` —
so the handover is a knee rather than a corner. `n → ∞` is `max`, and if the
corner turns out not to show, `max` is the simpler thing to keep.

**This is what the sizing wanted all along**, and it costs nothing to reason
about. The camera is outside the shell exactly while `d > extent`, and inside
exactly when `d < extent` — which is what being inside a system means. There is
no distance at which it is trapped in a sphere larger than the thing that sphere
stands for, for any system, with no threshold deciding it.

It also takes the constants with it. There is no ramp, so nothing to name its
two ends; and no **presumed extent**, since nothing has to guess a system's size
in order to decide anything. `closeness` is just `extent / shell` — nothing far
away, one once the extent has taken over — and the same number drives both the
size and the material.

Three distances follow from it, each meaning something:

| | Sol (`extent` 0.00048 ly) | a compact system (0.000032 ly) |
|---|---|---|
| contents fetched | ~1 ly | ~1 ly |
| handover, `extent / ANGULAR` | 1.19 ly | 0.079 ly |
| camera passes the surface, `extent` | 0.00048 ly | 0.000032 ly |

Two things to watch, both consequences of the floor going rather than of
anything added:

- **A system with nothing on record has no extent, so its shell shrinks to a
  point and goes.** That is honest — it says what is known — but it is a visible
  change in behaviour and the alternative, a small floor of its own, is one
  constant away.
- **`ANGULAR` becomes load-bearing.** At `4e-4` it is about `0.023°`, half a
  pixel at 1080p, and with the floor gone it is the only thing keeping a distant
  system on screen; bloom is carrying it today. Expect to raise it once it can
  be seen — and settle it against a sky of millions rather than against one
  star, since what it really sets is how a crowd of overlapping shells reads.

It answers four things:

1. **Whether to fetch and spawn a system's contents.** In absolute light years,
   about one, with hysteresis — not from apparent size. Whether to load a
   system's data is a question about proximity and budget rather than about how
   large it looks, and it has to be answerable before anything is known about
   the system. About one system stands within that, so nothing needs a budget.
   It is also roughly ten times the handover distance for a typical system,
   which is the lead time the fetch wants to have landed by.
2. **When a system becomes a grid, and when the camera descends into it.** At
   the handover — `extent / ANGULAR`, where the shell stops standing for the
   system and starts being it. One moment for both, so the two cannot disagree,
   and it is the moment the map itself already marks.

   Galaxy-grid precision there is ~270,000 km against a distance of `0.079` ly
   for even a compact system, five orders below, so the descent has room either
   side and does not have to be exact.

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
3. **How large to draw the shell.** `shell(extent, d)` above — the angular term
   until it falls past the extent, the extent after.
4. **How solidly to draw it.** `closeness = extent / shell` on its material —
   bright and emissive while it stands for a system, faint and translucent once
   it is one, a border about to be flown through. The same number as the size,
   so it cannot be the wrong size for how solid it looks.

Later, `apparent_brightness(luminosity, distance)` sits beside this and decides
which stars are too faint to draw at all. `stars.absolute_magnitude` and
`stars.luminosity` are already in the schema waiting for it, and it is the same
shape of question: what a thing looks like from here, rather than how far away
it is.

**Drop the `View::Bodies` TODO** at `scale.rs:60` rather than filling it in —
the point of all this is that there is no mode to switch to. Say so in the doc,
since it is a promise being deliberately broken.

**Tests:** `a_system_too_small_to_see_keeps_its_contents_to_itself`,
`flying_in_brings_a_systems_bodies_out`,
`a_system_on_the_threshold_does_not_flicker`,
`the_camera_descends_where_the_shell_becomes_the_system`,
`the_shell_holds_one_angle_until_it_meets_the_system`,
`the_shell_stops_shrinking_at_the_size_of_what_it_encloses`,
`the_shell_thins_out_as_it_is_flown_into`,
`a_system_with_nothing_on_record_dwindles_to_a_point`, and the invariant the
whole thing rests on —
`the_camera_is_outside_the_shell_until_it_is_inside_the_system`, swept over
distances spanning the range and over extents from a compact system to the
widest on record. That last one has to be written over a *compact* system as
well as over Sol: a floor-shaped mistake passes on Sol and fails on everything
smaller, which is how the first one survived being designed.

**Walked end to end**, focus following the zoom, `near = radius × 1e-4`,
`shell = 4e-4·d ⊕ extent`, Sol's extent taken as Neptune's orbit, `4.5e12` m:

| focus | camera radius | near | grid | shell | on screen |
|---|---|---|---|---|---|
| galaxy | `4.7e20` m (50,000 ly) | `4.7e16` m | galaxy | `1.9e17` m (20 ly) | a point, `4e-4` rad |
| Sol | `2.8e17` m (30 ly) | `2.8e13` m | galaxy | `1.1e14` m (0.012 ly) | a point, `4e-4` rad |
| Sol | `4.7e16` m (5 ly) | `4.7e12` m | galaxy | `1.9e13` m | contents fetched |
| Sol | `1.1e16` m (1.19 ly) | `1.1e12` m | → system | `4.5e12` m | handover: the shell is the system |
| Sol | `4.5e12` m (15,000 ls) | `4.5e8` m | system | `4.5e12` m | through the surface, `1` rad |
| Earth | `5e7` m (50,000 km) | `5e3` m | system | — | Earth a 7.3° disc |
| Luna | `5e6` m (5,000 km) | `500` m | system | — | Luna 20°, Earth 0.95° |

Fourteen orders of magnitude of camera radius, one continuous zoom, one grid
handover. The shell holds `4e-4` rad the whole way down until it meets Sol's
own size, then holds that and grows on screen as it is approached — which is
what a thing does when you fly at it.

At the far end the camera's offset is good to 30 nm while Luna's mesh, `f32`
vertices scaled to `1.7e6` m, is good to about 0.1 m — the mesh becomes the
limit before the grid does, which is the argument for chunked terrain when
surfaces arrive rather than for finer cells.

**Where the fetch distance has to sit.** The handover is at `extent / ANGULAR` —
a *camera distance*, not a size. It is `0.079` ly for a compact system and
`1.19` ly for Sol, so a few light years covers both with room.

It does not cover the tail. A system with bodies at 700,000 light seconds hands
over when the camera is 55 ly off, and Alpha Centauri, whose reach runs to about
a fifth of a light year, at some 500. Those are far outside any sane fetch
distance, so they take their true size when their contents land rather than at
the handover, and grow in one step — from `0.002` to `0.022` ly for the first of
those, a tenfold jump that is a quarter of a degree on screen. Honest enough, a
system being drawn at the size that keeps it visible until the map has asked what
size it really is, but worth watching before it is called fine.

### A system's size is learned too late — open, and to be tested first

The shell's size wants `extent`, and `extent` comes out of the database, so
between a system appearing and its contents landing the map is drawing something
whose size it does not know. Where the handover falls further out than the fetch,
the shell has been drawn too small for the whole approach and jumps to the truth
the moment the answer arrives. Nothing is wrong afterwards; it is the arriving
that shows.

**Test before building anything.** `ANGULAR` is the leverage: the distance the
fetch must reach is `extent / ANGULAR`, so raising it shrinks the problem in
proportion. At `4e-4` a system reaching 700,000 light seconds wants 55 ly; at
`4e-3` it wants 5.5, which is about where the fetch would sit anyway. Since
`ANGULAR` has to be settled by eye regardless — it is what holds a distant system
on screen, and what decides how a crowd of them reads — that experiment may
answer this one for nothing. Do it first.

If something is still wanted after that, three things, cheapest first, and each
useful without the others:

1. **Ease the size rather than assigning it.** Hold what is drawn and approach
   the answer over a few hundred milliseconds. `camera.rs:144`'s `approach` is
   exactly this and is already frame-rate independent and tested. It does not
   make the size right any sooner — it stops the wrongness from being a flinch,
   and it covers everything, including a system whose extent changes later
   because more of it has been scanned since.
2. **Learn the extents early and cheaply, without the bodies.** One aggregate —
   `max(semi_major_axis)` grouped by `system_address` over the addresses the
   spyglass already holds — answers a float per system rather than every row of
   every one. Batched on the `FetchTasks` model, cached by address for the
   session since a system's reach does not change. That removes the cause rather
   than the symptom, and it scales to the outliers, which no fetch distance can.
3. **Store it on the `systems` row.** The same number, kept rather than derived.
   `DbSystem` is already fetched for everything in the spyglass, so the shell
   would never be drawn at a size it had to guess at, at any distance. Costs a
   migration, a backfill over `bodies`, and a line in the sync path — worth it
   once, not worth it on suspicion. Adjacent to issue #69.

**And a question underneath all three: what should `extent` mean?** Taken as the
outermost body it is set by whatever is furthest, which is what makes the tail so
long — a system with everything inside a few thousand light seconds and one thing
at 700,000 gets a shell two hundred times wider than where it actually lives, and
what there is to see is a speck at the middle of it. A high percentile instead of
a maximum would frame what is there, and would collapse the range of extents so
far that the handover falls inside any sensible fetch for almost every system.

It trades away containment: those few far bodies would be drawn outside their own
shell. Whether that is wrong or simply honest is a question about what the shell
is *for*, and it should be looked at rather than assumed.

**Shells at their true size cannot overlap.** The widest is about a fifth of a
light year against a spacing of four or more, twenty times clear, and the
ordinary ones are clear by thousands. Overlap belongs entirely to the angular
term, and is unbounded: at 50,000 ly every shell is drawn 20 ly across against
that same four, so millions of them interpenetrate. That is what makes the
galaxy read as a glow rather than as points, and it is the current behaviour
rather than anything introduced here.

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
