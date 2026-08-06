# The night sky

A plan for drawing the sky as it looks from inside a star system, using the
stars the database knows about. Deferred until the basic bodies work
(TODO(#44)) lands; written down now so the decisions are not re-argued later.

## The physics that shapes the design

Every star outside the camera's own system is a point. The Sun from one
parsec subtends ~0.009 arcseconds; nothing the eye or a sensor resolves.
Three consequences drive everything below:

1. **Occlusion is irrelevant.** Two stars along nearly the same line of
   sight do not eclipse; their light adds. Starlight from distinct stars is
   incoherent, so intensities sum linearly. Sky stars must therefore render
   additively (`AlphaMode::Add`, no depth write) rather than occluding.

2. **All blur is in the receiver.** With no atmosphere, a star's image is
   `flux × PSF`, where the point spread function (diffraction, aberration,
   scatter) has the *same shape for every star*. Bright stars look bigger
   only because more of a fixed-shape PSF clears the visibility threshold —
   apparent radius grows roughly logarithmically with flux. The map's HDR
   camera with `Bloom::NATURAL` already is such a PSF: render every star at
   pixel scale with emissive ∝ flux and bloom produces the size illusion
   for free. Nobody draws big circles.

3. **Inside a system, the sky is static.** Moving 100 ls shifts a star 4 ly
   away by ~0.16 arcsec — invisible. A rendered star computed for an eye
   position `Δ` away from the current one is off by `Δ/d` radians in
   direction and `~2.2·(Δ/d)` magnitudes in brightness. Staleness is
   measurable, and tolerance scales with distance.

Photometry: `m = M + 5·log10(d_pc/10)`, flux `∝ 10^(-0.4·m)`, combined
magnitude of an unresolved pair `-2.5·log10(10^(-0.4·m1) + 10^(-0.4·m2))`.
A dark-adapted eye in space reaches roughly magnitude +8. Stars fainter
than ~+2 are seen by rods and appear colorless; only bright stars show
their blackbody tint.

## Two orthogonal axes

**Presentation mode** is a user toggle, successor to the `View` resource:

- **Real** — photometric points: blackbody color from temperature, flux
  from absolute magnitude and distance, additive blending, bloom as PSF.
  Constellations come out right from any vantage.
- **Shell** — the map's translucent ball, colored by `ColorBy`
  (allegiance/government/security), grown enough to see and click.

**Camera context** is a continuous scalar: how deep inside a system the
camera sits. It drives *sizing only*, never mode. Both modes are defined
at every camera position; nothing modal happens when the camera crosses
into a system. ("Shell" here means the drawn ball — the bodies branch
already names the component `Shell` — not the distance bands below.)

## The sizing law

The bodies branch already writes the law in the right shape:
`scale = ANGULAR·d + FLOOR`, an angle a shell holds on screen plus a
size, with `FLOOR` documented as a stand-in for the system's true size
until its bodies have been read, giving way to the real extent once they
have. The stand-in is what makes near neighbors enormous from inside a
system — at 8.5e-2 ly it puts Alpha Centauri, seen from Sol, at ~1.1°,
two full moons — and the handover already planned there fixes most of
it: with a true system size (light hours) in place of the guess, a
neighbor settles to about `ANGULAR` ≈ 1.4 arcmin on its own.

What the sky adds is a context blend on the angular term itself:

    angular_radius(d) = angular(context) + size(system)/d
    scale             = angular_radius(d) · d

- Map context (camera light-years from everything): `angular = 4e-4`, as
  today.
- Sky context (camera inside a system): `angular` eases down to the sky
  scale — ~`5e-5`–`1e-4` rad for Shell (a marker, grown a bit),
  effectively one pixel for Real (bloom does the rest).
- Context: smoothstep over `log10(distance to nearest system)`, full sky
  inside ~0.1 ly, full map beyond ~2 ly, eased in log space. The
  nearest-system distance comes from the already-fetched region.

Outside, shells shrink with distance toward the map angle as today;
entering a system they shrink quickly to the sky scale so neighbors read
as bright dots. `PointerTarget` is sized independently by `pointing`, so
sky-scale dots keep a fat hit target.

The `Systems`/`Stars` split in `View` becomes vestigial once sizing is
context-driven (`Stars` is the law with `boost = 0` everywhere); fold both
into `Mode { Real, Shell }` unless the uniform view is still wanted.

## Camera 2.0: aiming and exposure

Looking at the sky is pointing a camera: an aim, a lens, and an exposure.
The map's camera grows those three things, and none of them is a new
camera — they are regimes and dials of the orbit camera it already has.

**First person is the orbit camera at radius zero.** The pose math
already holds there: `rotation` is stored explicitly rather than derived
from positions, so `eye = center + rotation·Z·radius` degenerates
cleanly, drag keeps rotating through the very line it rotates through
today, the pan rate already scales by `radius` and so dies exactly on
arrival, and `PITCH_LIMIT` stops a milliradian short of the zenith, which
is less than a bloom width. There is no mode enum; the regime is
`radius == 0`, pinned by `snap`.

**Stand here.** The way in is not a move but a re-parametrization:
`center ← eye`, `radius ← 0`, rotation kept. The eye has not moved and
the view direction has not changed — the world holds still to the pixel,
so instant *is* smooth, and anything animated is affordance (a reticle,
the dials) rather than the camera. It works from any orbit: wherever the
eye happens to be is where aiming starts. Setting it also pins
`target_center` and `target_radius`, so the easing never finishes a
stale approach underneath the new standpoint.

**Aiming.** Drag rotates in place. Sensitivity scales with the field of
view, as on any real camera: telephoto aiming needs a finer hand.

**One magnification axis.** Far out, the wheel dollies, as today. At the
radius floor it hands off to focal length: scrolling in narrows the
field of view (telephoto — frame a binary, resolve a neighbor), scrolling
out widens it back to normal. Run the field of view at the same
`ZOOM_RATE` in e-folds and magnification per notch is continuous across
the handoff; nobody finds the seam. The billboard shader and the sizing
law already read the field of view as a uniform, so telephoto genuinely
magnifies the sky — the projection is live.

**There is no switch back, by design.** After aiming around, the old
center is stale; restoring it would snap the view to a former interest,
so no inverse is kept. Leaving is a forward choice, made two ways.
Wheel-out past the widest field of view dollies backward along the view
axis, orbiting the spot just stood on — backing away from the tripod.
Or **orbit that**: point at something and take it as the new center —
`center ← target`, `radius ← distance to it`, rotation kept. The target
lies along the view ray, so the pose identity holds exactly and again
nothing on screen moves. This rides machinery the map already has:
`PointedAt`, `Selection`, and the double-click that today means "that
one."

**Exposure.** One dial, EV100 — shutter, iso and aperture trade against
motion blur and depth of field, and there is neither, so one knob is the
honest number. Auto-metering by default (Bevy ships histogram
auto-exposure): with brightness on an honest photometric scale, swinging
the view away from the local star makes the exposure climb and the stars
fade in over a second — dark adaptation, emergent rather than scripted.
Manual override remains for shooting the sky properly, which is an
underexposed foreground. The scale stays honest: at an exposure that
holds a sunlit planet, the constellations are gone, exactly as they are
from the day side of a real window. A compressed-range cheat can be a
toggle later if honesty proves annoying; it is not the default.

## Distance bands (Real mode only)

The sky is kept honest by a refresh policy whose cost scales with
`velocity/distance`. Sky stars are grouped into concentric distance bands,
each remembering the **baked eye position** it was fetched around. A band
goes stale in proportion to camera movement: a star computed for an eye
`Δ` away is off by `Δ/d` radians and `~2.2·(Δ/d)` magnitudes, so
tolerance scales with `d_inner`. A system is ~1e-4 ly across, so flight
inside one never trips any band: the "camera in a system looking out"
case costs zero recomputation as a limit, not as a detected mode. Band
membership needs hysteresis at the edges, as `SWITCHING_THRESHOLD` does
for grid cells.

**Bands are meshes, not entities.** Entities exist only within the
spyglass, where Shell mode needs picking and labels anyway; Real mode
rematerializes those same entities photometrically. Beyond the spyglass a
band bakes to one point-cloud mesh — far stars need no per-star identity,
and per-frame cost becomes a handful of draw calls whatever the count. A
whole sky is ~10–50k stars (magnitude 8 from Earth is ~40k, and the DB is
sparser than reality beyond the bubble), so the meshes are small. Vertices
are anchored to the cell of the baked eye; f32 offsets keep sub-tolerance
precision at every band's distance.

**The projection is live; only the star list is a snapshot.** Each star's
quad carries its center position, absolute magnitude and color as vertex
attributes, and a billboard vertex shader expands it in view space each
frame — computing direction (rasterization), angular size (the sizing
law), and flux (1/d² from the current eye) live on the GPU. Rotation and
zoom are therefore exact at all times; nothing about a bake privileges a
view. What can go stale is only *set membership* — which stars belong in
a magnitude-limited band changes as the camera travels — and membership
error is invisible by construction, since a star enters or leaves the set
at the visibility floor. The shader is also where later polish lives
(scotopic desaturation, spikes are per-star, distance-dependent effects),
so it is part of the rendering phase, not an optimization.

**Refetch is the only real cost, so give it a margin.** Fetch each band
with ~10× spatial margin around its baked eye. Then most invalidations
are answered by what is already cached (cheap mesh rebuilds, milliseconds
for the whole sky on `AsyncComputeTaskPool`, swap when ready), and the
database is only asked again when the camera outruns the margin. Far
bands are magnitude-limited: at 5,000 ly only bright giants clear
naked-eye visibility, so row counts stay sane.

**The policy is speed-tiered**, which is where "take a photo of this"
becomes literal:

- Parked or in-system: every band valid, zero cost.
- Slow drift: inner bands churn cheaply from cache; outer bands barely
  notice. Live for any speed at which someone is actually reading the sky.
- Fast travel (a fly-to crosses the galaxy at thousands of ly/s,
  invalidating everything every frame): stop chasing. Freeze the
  composite — show the last good photo — and rebake the cascade on
  arrival, inner bands first. The camera already signals settling
  (`Travel` completes, `snap` pins the center), so the photo develops
  when you stand still without new detection machinery. Physically
  honest, too: a long exposure during a slew gives star trails, not a
  sharper sky.

Shell mode needs none of this. Its reach is the spyglass, as today; the
band meshes and the invalidation system exist only while Real is on.

## The far background: a cubemap over an aggregate table

The mesh remembers positions; a cubemap remembers directions. A band mesh
is a catalog re-projected live every frame, so translation works. A
cubemap is an image indexed by direction alone — rotation is exact, but
the depth is gone, so it behaves as if everything in it were infinitely
far. That flaw is harmless exactly where `Δ/d` stays under tolerance for
any plausible travel, which is why the cubemap is reserved for the
outermost layer: the Milky Way band, the summed glow of stars each
individually below threshold. An image is the natural container for
"integrated flux per direction"; a point list is the natural container
for discrete sources. Bakes happen offline for a grid of reference
positions; crossing hundreds of ly swaps to the nearest bake.

**The background renders the record.** It is an accurate picture of what
the database holds, survey bias included — a bright corridor along a
popular exploration route is information, and the map is a map. A galaxy
density model for a prettier Milky Way is a separate future feature; it
would feed the same bake as another flux source, changing nothing below.

Baking cannot scan every row, so the database grows an aggregate layer.
The bake is a sum of `L/(4πd²)` over far stars — the same 1/d² sum as
N-body gravity — so the structure is a Barnes–Hut-style luminosity
octree: a sparse hierarchy of 3D cells, each holding the photometric
aggregate of everything inside it.

    star_flux_cells (
        level, cx, cy, cz,     -- sparse integer cell coords per level
        luminosity real[],     -- linear flux units, ~6 temperature buckets
        centroid   real[3],    -- luminosity-weighted mean position
        spread     real,       -- luminosity-weighted RMS radius
        count      bigint,
    )

- Luminosity in linear units, never magnitudes: magnitudes do not add.
  Convert `10^(-0.4·M)` at ingest and sum.
- Temperature buckets keep the background's color structure (warm bulge,
  blue arms) without per-star storage.
- The weighted centroid, not the cell center, is where a cell's light
  splats from, or the glow quantizes into grid-sized blobs.
- The spread renders each cell as a Gaussian splat rather than a point,
  which is what makes a coarse cell look like a star field.

A bake walks the hierarchy from a reference point with an opening-angle
test (`size/d` under a fraction of a degree: splat; otherwise descend),
bottoming out at the band radius. Cost is governed by the opening angle,
not the star count — tens of thousands of indexed reads rather than a
full scan. Coarser levels roll up from the fine one by `GROUP BY`; the
weighted moments compose exactly.

**Bright and faint split at ingest**, by absolute magnitude. Stars above
the cutoff (a fraction of a percent) stay out of the aggregates in an
individually-queryable bright-star table; everything fainter contributes
to cells and is never queried individually at range again. This keeps the
far bands' magnitude-limited queries off the full table *and* prevents
double counting: a giant drawn live in a band must not also be baked into
the glow behind it. The mesh/cubemap boundary becomes one ingest-time
constant.

Maintenance is incremental because sums are additive: a scan event
upserts its star's flux into one fine cell per level. New discoveries
change any cell's flux by nothing the eye can see, so baked cubemaps may
lag the table by weeks; rebaking is a cron job. The table is worth
designing so `galos_server` can share it — "total recorded luminosity in
a region" comes for free.

## Data

**Shell mode requires zero database changes.** Hue (three enums on the
`systems` row), brightness, dim, and size all derive from what the region
fetch already returns.

**Real mode pays for the join, only when on.** A lightweight query —
position, `absolute_magnitude`, `surface_temperature`, `star_class` via
`LEFT JOIN stars`, plus `primary_star_class` from the systems side —
keyed by band, issued through a new `FetchIndex` variant when Real mode
turns on or a band invalidates. Magnitude-limited at range. This
resurrects the commented-out `galos_db::stars` fetch code.

Fallback chain per system: `stars` rows (sum the fluxes; one point at
these distances) → `primary_star_class` lookup table → default class.
The table maps Elite's classes (O B A F G K M, TTS, L/T/Y, D*, N, H, …)
to a typical absolute magnitude and temperature.

Photometry caches on the entity as an optional `Photometry` component
beside `System`, filled lazily by the first Real fetch that covers it, so
toggling modes refetches nothing. `System` stays the political row it is;
the sizing law reads neither. Stale photometry (a `stars` row arriving
after annotation) is refreshed by the same poll that re-fetches a region —
an absolute magnitude is not news the way a faction flip is.

## Phases

1. **Photometry core.** Pure functions: apparent magnitude, flux,
   combined magnitude, blackbody color, the class fallback table. Every
   function is a one-line physics claim with a unit test.
2. **Sizing-law refactor.** Rides the bodies branch's angle-plus-size
   expression and its true-size handover; adds the context blend on the
   angular term. Shell mode is complete here, database untouched.
3. **Camera 2.0.** Stand here, orbit that, the wheel handoff to focal
   length, drag scaled by field of view, the exposure dial with
   auto-metering. Works in Shell mode too, so it lands independently of
   everything photometric.
4. **Real-mode data.** The join query, `Photometry` component, lazy fetch
   keyed by mode + band with coverage margin, baked-eye invalidation,
   the speed-tiered policy with rebake-on-settle.
5. **Real-mode rendering.** Within the spyglass, binned photometric
   materials on the existing entities (quarter-magnitude × ~8 temperature
   buckets keeps the shared-handle pattern of `SystemMaterials`). Beyond
   it, band meshes with the billboard vertex shader. Additive blending,
   bloom tuning against the exposure scale.
6. **The far background.** The `star_flux_cells` aggregate table and
   ingest split, the offline bake walk, reference-grid cubemaps.
7. **Polish**, each droppable: scotopic desaturation below a flux
   threshold, optional diffraction spikes, star trails during fast
   travel. A galaxy density model feeding the same bake is a separate
   future feature, not part of this plan.

## Coordination with the bodies work

The bodies branch owns the near field: the current system's own stars and
planets at real geometry. The sky is the complementary far field, excludes
the current system, and — by consequence 3 above — never parallaxes while
the camera flies within one. The two compose without knowing about each
other; they meet in only two places. One is the sizing law's context
scalar, which the bodies view will want for the same blend the `scale.rs`
module doc already asks for (and which should also drive today's ambient
light down to black inside a system, where PBR needs a dark sky). The
other is the photometric scale: each local star lights the bodies as a
point light whose color and intensity come from the same phase-1
functions, anchored to the same EV100 exposure the sky renders under, so
lit surfaces and emissive stars sit on one believable brightness axis.
The bodies branch draws the map in metres — one of its stated reasons
being that bevy's lighting speaks physical units — so the phase-1
functions feed the lights real values, candela from a star's luminosity
and color from its temperature, with no unit shim in between.
