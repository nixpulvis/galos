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
into a system. ("Shell" here means the drawn ball, as in `Hue::color`'s
"translucent ball with a glow" — not the distance bands below.)

## The sizing law

Today `size_by_distance` uses `scale = 4e-4·d + 8.5e-2`, i.e. angular
radius `4e-4 + 0.085/d`. The constant term makes near neighbors enormous
from inside a system: Alpha Centauri from Sol subtends ~1.1°, two full
moons. Generalize to one law with context-blended coefficients:

    angular_radius(d) = floor(context) + boost(context)/d
    scale             = angular_radius(d) · d

- Map context (camera light-years from everything): `floor = 4e-4`,
  `boost = 0.085` — today's numbers exactly, a pure refactor.
- Sky context (camera inside a system): `boost → 0`, and `floor` drops to
  the sky scale — ~`5e-5`–`1e-4` rad for Shell (a marker, grown a bit),
  effectively "one pixel" for Real (bloom does the rest).
- Context: smoothstep over `log10(distance to nearest system)`, full sky
  inside ~0.1 ly, full map beyond ~2 ly, coefficients lerped in log space.
  The nearest-system distance comes from the already-fetched region.

Outside, shells shrink with distance toward the map floor as today;
entering a system they shrink quickly to the sky floor so neighbors read
as bright dots. `PointerTarget` is sized independently by `pointing`, so
sky-scale dots keep a fat hit target.

The `Systems`/`Stars` split in `View` becomes vestigial once sizing is
context-driven (`Stars` is the law with `boost = 0` everywhere); fold both
into `Mode { Real, Shell }` unless the uniform view is still wanted.

## Distance bands (Real mode only)

The sky is kept honest by a refresh policy whose cost scales with
`velocity/distance`. Sky stars are grouped into concentric distance bands,
each remembering the **baked eye position** its directions, magnitudes and
material bins were computed for. A band is valid while

    |eye_now − eye_baked| < θ_tol · d_inner

with `θ_tol` ≈ 1 mrad (half a bloom width; the photometric tolerance is
looser, so one test covers both). Tolerated movement before refresh:

| band     | movement  |
|----------|-----------|
| 10 ly    | ~0.01 ly  |
| 100 ly   | ~0.1 ly   |
| 1,000 ly | ~1 ly     |

A system is ~1e-4 ly across, so flight inside one never trips any band:
the "camera in a system looking out" case costs zero recomputation as a
limit, not as a detected mode. A 20 ly jump refreshes only the cheap inner
bands. Refreshes run on `AsyncComputeTaskPool` (the `fetch.rs` pattern)
and swap when ready; the stale band shown meanwhile is under tolerance by
construction. Band membership needs hysteresis at the edges, as
`SWITCHING_THRESHOLD` does for grid cells.

Far bands are magnitude-limited: at 5,000 ly only bright giants clear
naked-eye visibility, so row counts stay sane. The outermost layer — the
Milky Way band, i.e. the summed flux of everything unresolved — is a
cubemap baked offline (from the full table or a density model) for a
handful of reference positions; its tolerance is hundreds of ly of travel.

Shell mode needs none of this. Its reach is the spyglass, as today; the
band entities and the invalidation system exist only while Real is on.

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
2. **Sizing-law refactor.** Context-blended coefficients; map context
   reproduces today's constants exactly. Shell mode is complete here —
   beach-ball fix included, database untouched. Independently landable.
3. **Real-mode data.** The join query, `Photometry` component, lazy fetch
   keyed by mode + band, baked-eye invalidation.
4. **Real-mode rendering.** Binned photometric materials (quarter-
   magnitude × ~8 temperature buckets keeps the shared-handle pattern of
   `SystemMaterials`), additive blending, bloom/exposure tuning, exposure
   as the one user dial.
5. **Polish**, each droppable: scotopic desaturation below a flux
   threshold, optional diffraction spikes, the baked galaxy background.

## Coordination with the bodies work

The bodies branch owns the near field: the current system's own stars and
planets at real geometry. The sky is the complementary far field, excludes
the current system, and — by consequence 3 above — never parallaxes while
the camera flies within one. The two compose without knowing about each
other; they meet only in the sizing law's context scalar, which the bodies
view will want for the same blend the `scale.rs` module doc already asks
for.
