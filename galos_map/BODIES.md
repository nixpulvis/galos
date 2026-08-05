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

One notion, in `scale.rs` beside the sizing already there:

```rust
/// How large something of `size` looks from `distance` away, in radians
fn apparent(size: f64, distance: f64) -> f64;
```

It answers four things:

1. **Whether to fetch and spawn a system's contents.** Above `WORTH_FETCHING`
   (≈ `5e-3` rad) they are asked for; below `WORTH_KEEPING` (≈ `2e-3`) they are
   dropped. The gap is hysteresis, so a system on the line does not thrash.
   Before anything is known there is no extent to measure, so a **presumed
   extent** stands in — about a thousand light seconds — replaced by the measured
   one when contents land.
2. **When a system becomes a grid, and when the camera descends into it.** The
   same threshold, so these are one decision rather than three that can
   disagree.

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
3. **How large to draw the shell.** Between the visibility law it uses today,
   `4e-4·d + 8.5e-2` — already *angle × distance + floor* — and the system's true
   extent. Blended **geometrically**, `marker^(1-c) · extent^c`, not by a lerp:
   the ends are orders of magnitude apart, so a linear blend would hang near the
   marker for the whole ramp and then plunge.
4. **How solidly to draw it.** The same `closeness` on its material — bright and
   emissive as a marker, faint and translucent as a border being flown through.
   One number for both, so it cannot be the wrong size for how solid it looks.

Later, `apparent_brightness(luminosity, distance)` sits beside `apparent` and
gates stars the same way. `stars.absolute_magnitude` and `stars.luminosity` are
already in the schema waiting for it.

**Drop the `View::Bodies` TODO** at `scale.rs:60` rather than filling it in —
the point of all this is that there is no mode to switch to. Say so in the doc,
since it is a promise being deliberately broken.

**Tests:** `a_system_too_small_to_see_keeps_its_contents_to_itself`,
`flying_in_brings_a_systems_bodies_out`,
`a_system_on_the_threshold_does_not_flicker`,
`the_camera_descends_when_the_contents_arrive`,
`the_shell_shrinks_to_the_system_as_it_is_approached`,
`the_shell_is_never_smaller_than_what_it_encloses`,
`the_shell_thins_out_as_it_is_flown_into`.

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
