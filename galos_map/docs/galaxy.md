# The galaxy

One spatial hierarchy over every system, and everything that reads it: the
map's level of detail, the night sky's discrete stars, and the glow behind
both. It is also the client's on-disk format, which is what lets the map draw
the galaxy without a database.

This reads in the order the choices were forced. Measurements first, because
the shape of the tree follows from them, then the physics, because it decides
what order the tree is in. Those two settle the invariants, the invariants
settle the cell, and the cell is what the three walks read. Then what those
walks draw, the map's marks and field before the sky's stars and glow, and
last where the bytes come from and in what order the work lands.

Two requirements sit under all of it, and most of what follows is answering one
of them.

There are 129 million systems and a client that can hold a few million points.
So a region has a far representation as well as a near one, and refinement is
what moves it from the first to the second.

The server is fed by EDDN and answers every client at once. A request has to be
one a server can serve, which means a static file or an indexed read and never
a scan, and the map has to stay honest while one is in flight for a second or
two. Both fall out of putting the aggregates in the index rather than with the
payload. A region draws before it loads, so latency decides how much of it has
condensed into discrete marks and never whether anything is on screen.

## Measurements

Taken 2026-08-15. The live database is bubble-weighted, and the dump samples
are the head of a file ordered by id64, so they over-represent catalog stars
and well-scanned systems.

### The live database

4382 MB, 1,351,884 systems, 1,323,689 with a position.

| Table | Total | Heap | Indexes | Rows | Bytes/row |
|---|---|---|---|---|---|
| commodities | 1119 MB | 594 MB | 525 MB | 5.6M | 211 |
| body_materials | 869 MB | 438 MB | 431 MB | 7.6M | 120 |
| outfitting | 847 MB | 374 MB | 473 MB | 4.2M | 213 |
| systems | 641 MB | 251 MB | 391 MB | 1.3M | 499 |
| bodies | 516 MB | 387 MB | 129 MB | 1.2M | 464 |
| stars | 136 MB | 96 MB | 39 MB | 377k | 377 |

Indexes are 2050 MB of the 4382, and 378 MB has never been scanned.
`stats_reset` is null, so those counters cover the life of the database.

### The Spansh dump

`galaxy.json.gz` is 115.59 GB, decompressing at 5.45x to about 630 GB of
JSON. A 200 MB slice parsed to 234,710 complete systems.

Per system: 2.27 stars, 2.86 planets, 0.32 barycenters, 14.4 material
entries, 0.48 rings, 0.20 body signals. Mean name length 17.1 bytes.

Scaling by 551.2 gives 129.4M systems, 293M stars, 370M planets, 1.87B
material rows, 2.8B rows total. Multiplied by the bytes/row above that is
about 600 GB in Postgres, or 450 to 550 GB after a sorted load with indexes
built afterward.

The total is anchored to a real file size, so it is more robust than the
per-table parts. Head-of-file bias means the system count is probably a
floor and bodies-per-system probably a ceiling.

### Density

Occupancy on the live positions, which is what settles adaptive versus
uniform subdivision.

| Cell | Occupied | Median | p99 | Max |
|---|---|---|---|---|
| 4096 ly | 603 | 266 | 44,787 | 120,857 |
| 1024 ly | 8,825 | 21 | 1,818 | 44,267 |
| 256 ly | 93,519 | 4 | 159 | 11,960 |
| 64 ly | 514,927 | 1 | 25 | 890 |
| 16 ly | 1,044,315 | 1 | 6 | 127 |

At 1024 ly the median cell holds 21 systems and the worst holds 44,267. A
uniform grid at any size is either useless in the bubble or almost entirely
empty outside it.

Counted outward from Sol, the densest case:

| Radius | Systems | Density |
|---|---|---|
| 10 ly | 12 | 0.00286 /ly^3 |
| 25 ly | 171 | 0.00261 |
| 50 ly | 1,234 | 0.00236 |
| 100 ly | 8,009 | 0.00191 |
| 200 ly | 37,959 | 0.00113 |

0.0029 /ly^3 is the real solar-neighborhood stellar density, and 12 systems
is what it predicts inside 10 ly, so the database is complete there and mean
spacing is about 7 ly. The falloff further out is survey incompleteness
rather than structure, which is what makes the density at 10 ly the right
figure to extrapolate a complete galaxy from.

### Extent

```
x  -41,974 .. 40,504     extent 82,478 ly
y   -3,492 ..  5,319     extent  8,811 ly
z  -16,845 .. 65,630     extent 82,475 ly
```

A 131,072 ly cube (2^17) centered near (0, 900, 24,400) covers it. 2^16 does
not. The cube wastes most of its y extent, which a sparse tree charges
nothing for.

### Names

98.40% of sampled systems carry procedural names. The rest are catalog
stars: `HD 225160`, `HIP 77716`, `XTE J1856+053`, `BD+60 2522`. Naive
scaling says 2.1M hand-named galaxy-wide, but the real ceiling is the size
of the catalogs Frontier imported, so a few hundred thousand.

### The dynamic set

96k systems by `population > 0`, 117k with any station, and 126k implied by
the size of `galaxy_populated.json.gz`. Three estimates agreeing that
everything which changes attaches to under 0.1% of systems.

### Churn

`galaxy_1day.json.gz` is 0.94% of the full dump. `galaxy_7days.json.gz` is
only 2.7x the one-day file rather than 7x, so most daily churn is the same
systems being revisited.

## The physics

Every star outside the camera's own system is a point. The Sun from one
parsec subtends about 0.009 arcseconds, which nothing the eye or a sensor
resolves. Three consequences drive everything below.

1. **Occlusion is irrelevant.** Two stars along nearly the same line of
   sight do not eclipse; their light adds. Starlight from distinct stars is
   incoherent, so intensities sum linearly. Sky stars therefore render
   additively (`AlphaMode::Add`, no depth write) rather than occluding.

2. **All blur is in the receiver.** With no atmosphere a star's image is
   `flux * PSF`, where the point spread function of diffraction, aberration
   and scatter has the same shape for every star. Bright stars look bigger
   only because more of a fixed-shape PSF clears the visibility threshold,
   so apparent radius grows roughly logarithmically with flux. The map's HDR
   camera with `Bloom::NATURAL` already is such a PSF: render every star at
   pixel scale with emissive proportional to flux and bloom produces the
   size illusion for free. Nobody draws big circles.

3. **Inside a system, the sky is static.** Moving 100 ls shifts a star 4 ly
   away by about 0.16 arcsec, which is invisible. A star computed for an eye
   position `e` away from the current one is off by `e/d` radians in
   direction and about `2.2*(e/d)` magnitudes in brightness. Staleness is
   measurable, and tolerance scales with distance.

Photometry: `m = M + 5*log10(d_pc/10)`, flux proportional to `10^(-0.4*m)`,
and the combined magnitude of an unresolved pair is
`-2.5*log10(10^(-0.4*m1) + 10^(-0.4*m2))`. A dark-adapted eye in space
reaches roughly magnitude +8. Stars fainter than about +2 are seen by rods
and appear colorless; only bright stars show their blackbody tint.

## The two invariants

Everything below follows from these, and neither can be retrofitted.

### Ordering is by absolute magnitude

Not by notability, population, or discovery date. A star is visible when
`M + 5*log10(d_pc/10) <= m_limit`, so a bright giant 5,000 ly out belongs in
the sky while a hundred dim dwarfs at 50 ly do not. Any other ordering drops
stars that should be drawn, and the failure is invisible: a plausible sky
that is wrong.

Magnitude ordering also happens to be right for the map. Pull the camera
back and the brightest stars are what you want to keep.

### Refinement is additive

A node at level L stores ranks `[N_(L-1), N_L)` of its subtree, so a node
holds only what its ancestors did not. Drawing a node together with its
loaded ancestors is exactly the union, with no duplicate possible.

A slice is top-K by magnitude plus a spatial uniformity quota, a blue-noise
sample of the remainder sized so no sub-volume is empty at coarse detail.
Bright stars cluster, so magnitude alone leaves voids where a region holds
nothing bright, and those voids fill in later as a visible patch. The quota
is free photometrically: a faint star promoted to a coarse level renders at
its true flux, which is under the threshold at that distance, so it
contributes nothing to Real mode and exists only so map mode has no holes.

This is the Potree and 3D Tiles scheme. It buys three things at once. There
is no double-draw to detect, because a system appears in exactly one node. A
child still in flight leaves a strict subset on screen rather than a hole,
so the failure mode is sparser and never blank. And because the ordering is
magnitude, the stars a child adds are always the faintest in that cell, so
they arrive at the visibility floor where nothing is left to hide.

## The cell

Sparse, adaptive, split when a cell exceeds ~4,096 systems. Root at level 0
is the 131,072 ly cube; each level halves.

```
cells (
    level, cx, cy, cz,      -- sparse integer coords per level

    -- the additive slice this cell owns
    rank_lo, rank_hi,

    -- discrete star culling
    m_min       real,       -- brightest absolute magnitude in the subtree

    -- background splat
    luminosity  real[],     -- linear flux, ~6 temperature buckets
    centroid    real[3],    -- luminosity weighted
    spread      real,       -- luminosity weighted RMS radius
    count       bigint,

    -- map mode aggregate
    mass        real[3],    -- count weighted centroid
    extent      real,       -- count weighted RMS radius
    mix         int[][],    -- one marginal per ColorBy axis
    aged        int[],      -- counts by age bucket, for Recency
)
```

Every aggregate here is over the cell's whole subtree, and what a cell stores
is that total, `T(c)`, and not the residual. With the payload absent, `T(c)`
is drawn as-is and no shells competed with it. With the payload present, the
residual is `T(c)` minus the loaded slice's own moments, computed from the
records just arrived, and drawing the total without that subtraction would
double count every shell on screen. The subtraction is stable where it
matters, since an internal node's slice is 512 systems against possibly
millions in its subtree, and the one fragile case, a slice that is most of
its subtree, is a leaf, where the residual is zero by definition and nothing
is subtracted. Storing the residual instead would have no answer at all when
the payload is absent, and having an answer then is the whole point.

`m_min` is the brightest single star, and is not derivable from
`luminosity`, which is a sum. Both are needed and they answer different
walks.

Luminosity is in linear units and never magnitudes, because magnitudes do
not add: convert `10^(-0.4*M)` at build time and sum. The temperature buckets
keep the glow's color structure, a warm bulge and blue arms, without
per-star storage. The spread is what renders a cell as a Gaussian splat
rather than a point, which is what makes a coarse cell look like a star
field instead of a dot.

Two centroids because the weightings differ. A luminosity weighted centroid
is what keeps the glow from quantizing into grid sized blobs, and map mode
wants the same thing weighted by count, which diverges wherever the bright
stars sit off center.

Index and payload ship separately, and **the aggregates live in the index**,
not with the payload. An aggregate exists so a region can be drawn without
being loaded, so shipping it with the payload would mean fetching a cell's
systems in order to draw the glow that stands in for not fetching them. It
is also what makes a slow request harmless: the answer is already on screen
at lower detail while the payload is in flight.

The two cuts make this concrete. The shell cut is set by the point budget,
which at 46,000 shells and 512-system slices lands near 90 cells for a
full-galaxy view, and 90 splats is far too coarse a glow. The aggregate walk
descends past it on the opening-angle test, so it reaches cells whose
payloads were never requested.

Quantized, a cell record is about 90 bytes: Morton key (8), rank range (8),
payload offset and length (12), child mask (1), subtree count (4),
luminosity in 6 log buckets (12), luminosity centroid and spread (8), count
centroid and spread (8), `m_min` (2), the star class marginal in 8 hue
buckets (8), and eight age buckets (16). The political marginals add 22
bytes to the minority of cells holding a governed system. At 97,000 cells
that is 9.8 MB raw and about 5 MB compressed, downloaded once.

Beside it sits the populated table, roughly 96,000 systems carrying
position, the political columns and a name, so about 3.5 MB. It is needed
only inside the separability crossover, near 1,950 ly, which is exactly
where a user has zoomed in far enough to want it. Beyond that the index
alone draws the political view, and draws it correctly rather than as a
stand-in, since individual marks would not be separable there anyway.

The index is also what lets the traversal plan before any payload arrives.
The photometric walk prunes on `m_min` without fetching, which is most of
the work at range, and knowing how much is outstanding needs it too: `M` is
the size of the `needed` set and `N` how much of it is resident, in cells
and, through `rank_hi - rank_lo`, in systems.

Payload per system in a cell is 17 bytes: id64 (8), position as three u16
quantized within the cell (6), magnitude as a fixed-point i16 (2) and a
temperature bucket (1). No name, no political columns, no index.

Positions are cell-relative rather than absolute. Absolute f32 at 40,000 ly
from origin has an ulp of 0.005 ly, about 300 AU. A u16 per axis inside a
16 ly leaf resolves 0.00024 ly, about 15 AU, in half the bytes, and
compresses better for having lost a shared exponent.

Slice sizes differ by role. Internal nodes carry about 512 systems, because
budget granularity matters most at coarse levels where one expansion moves
many points. Leaves carry up to 4,096, where bulk transfer efficiency
matters more than granularity.

Post-import that is 65,000 to 85,000 leaves at 15 to 20 KB compressed,
9,000 to 12,000 internal nodes, and about 1.5 GB compressed in total. Depth
runs from about 5 in sparse space to 13 or 14 in the bubble core.

## The three walks

One traversal function, three tests, all reading the same tree.

| Walk | Test | Draws |
|---|---|---|
| Screen space | projected extent in pixels over a threshold | map mode |
| Photometric | `m_min + 5*log10(d_min/10) <= m_limit` | Real mode discrete stars |
| Opening angle | `size/d` under a fraction of a degree | background splats |

`d_min` is the distance from the eye to the nearest point of the cell's
bounding box, which makes the photometric test conservative. It never drops
a visible star.

Culling is `m_min` and nothing else. The build-time cut described under the
glow is about not painting a star twice, and is not a culling mechanism.

## One predicate, three consumers

```
fn needed(camera, mode) -> Vec<NodeId>

renderer  draws     needed & resident
loader    requests  needed - resident
evictor   drops     resident - needed(with margin)
```

Drawing and fetching are separate today. Fetching is keyed by
`FetchIndex::Region(center, radius, ..)`, and drawing is gated by the
containment test in `galos_map/src/systems/mod.rs:239`. That gap is where
flicker would live, so the two collapse into one function whose result feeds
both.

Region keys also make the loader re-serve what it already holds.
`Region(center, radius)` grows on a zoom out, and a wider radius is no refresh
of a narrower survey (`refreshes` in `galos_map/src/systems/fetch.rs`), so the
whole sphere is read and its systems rebuilt off-thread again — most of them
already on the map from the inner region. The interim keeps this off the
frame: the spawn queue (`PendingSpawns` in `galos_map/src/systems/spawn.rs`)
dedups arrivals against the resident set, so a re-delivered system is dropped
before it is queued rather than spawned as a no-op re-insert, and the queue
reads as the new annulus rather than the whole sphere. What that does not
spare is the read and the rebuild, which the task pays over the whole region
regardless. The `needed()` collapse above is what ends it: keyed per cell
against a resident cell cache (`galos_index::cache::Resident`, built but unused
by the map today), `needed - resident` is the annulus of cells a zoom out
newly reaches, so neither the transport nor the build touches a cell already
held.

Eviction uses the same predicate with a wider margin, and evicts from memory
only. Cells on disk are kept, since a position never changes.

Eviction is a frame-cost bound before it is a memory one, and today there is
no evictor at all. The spyglass `clear` hides a system out of reach
(`Visibility::Hidden`, the containment test at
`galos_map/src/systems/mod.rs:239`) but never despawns it, so a session's
resident set is the high-water mark of everywhere the camera has looked.
`big_space` walks the whole grid every frame the camera moves
(`galos_map/src/space.rs`), recomputing a `GlobalTransform` for each resident
system whether or not it is drawn, so the cost of turning the camera is set by
how much has ever been loaded rather than by what is on screen. Measured after
folding the shell onto the system entity and frustum-gating the per-frame
sizing (`galos_map/src/systems/scale.rs`): a still view over ~360k held
systems holds about 18 ms a frame, and rotating that same view, which moves
the floating origin and forces the whole walk, runs several times that and
climbs with everything a zoom-out ever pulled in. Frustum culling cannot touch
it, the walk being keyed on grid membership rather than on visibility. The
`needed()` predicate with a margin is what bounds it, and it falls out of the
structure above rather than being bolted on: beyond the spyglass a cell is one
point-cloud mesh instead of thousands of entities, and the evictor drops what
the camera has left behind, so the transform walk runs over a bounded resident
set instead of the session's high-water mark.

## The metric is screen space, and the control is a point budget

```
px = node_size / d * viewport_height / (2 * tan(fov / 2))
```

Camera 2.0 below hands the wheel off to focal length at the radius floor, so
zooming changes the field of view without moving the eye. A distance keyed
level of detail would sit still while the image magnifies and then pop when
the camera dollies instead. The billboard shader already reads the field of
view as a uniform, so both halves read one number.

Projected size is the priority, not the cut. A per-cell pixel threshold
bounds the total in a 2D map, where tiles do not stack, but not here: the
depth extent of the visible volume is unbounded by the screen, so a
threshold met by every cell along a view ray says nothing about how much was
drawn. From 30,000 ly a level 5 cell subtends 128 px, which is 15 by 8
across a 1080p screen and another 20 deep behind them, so about 2,400 cells
and several million points.

So map mode traverses from the root against a point budget, expanding
highest projected size first and stopping when the budget is spent. The
budget is the tuned number. A pixel threshold survives only as a floor,
below which a cell's contents land on one pixel and cannot add anything, and
that floor prunes very little: a 16 ly leaf reaches 1 px only at 15,000 ly,
and a 128 ly cell not within the galaxy at all.

The budget is set so that it cannot bind while marks are still separable,
which is what makes completeness a consequence rather than a separate rule.

Call two marks distinguishable when their separation exceeds `w / p`, for a
mark diameter `w` and a coverage fraction `p` near 0.3. Each one then owns
`(w/p)^2` pixels, so the count that can possibly be distinguished on a
1920x1080 screen is bounded. At Bevy's default 45 degree field of view,
`k = 1303` pixels per radian:

| Mark diameter | Min separation | Separable points |
|---|---|---|
| 1.04 px, `ANGULAR` today | 3.5 px | 172,000 |
| 2 px | 6.7 px | 46,000 |
| 3 px | 10 px | 21,000 |

Size it from what renders correctly rather than from what `ANGULAR` does
today, which is a defect with an issue against it (TODO(#72) in
`galos_map/src/systems/scale.rs:184`): at half a pixel a mark aliases across
the samples a pixel is drawn from, and on a 600 line window it disappears at
some positions. So the budget is tens of thousands, not hundreds.

Set it to that bound and the traversal cannot run out while anything is
still individually visible, because exceeding it means the marks already
overlap. There is no completeness radius to pick.

The budget bounds cost and nothing else. What a cell *draws* is decided
separately, by whether its own systems are separable at this distance,
because systems cluster and a total under the budget says nothing about
local density. Marks where they separate, a splat where they do not. The two
tests answer different questions and neither substitutes for the other.

The magnitude 8 limit puts a whole sky at 10 to 50k stars, which sits right
at the 46,000 bound rather than under it. That is the correct answer and not
a collision: a dark sky is at the edge of resolving individual stars, and
the Milky Way band is exactly where they stop being separable.

A leaf stays atomic: never draw part of one. Cells inside the spyglass
expand regardless, but for a different reason, which is that picking and
labels need entities. At the default 10 ly opening that is 12 systems.

Priority is weighted toward attention as well as projected size.
`labels.rs` already carries that judgement as `CENTER_WEIGHT`,
`POINTED_WEIGHT` and `SELECTED_WEIGHT`, and refinement should follow where
the viewer is looking for the same reasons a name does.

The budget never removes a resident cell. It gates new expansions only.
Otherwise camera movement reallocates it, regions visibly thin, and
refinement reads as data loss.

For a sanity check on where the cut lands, holding on-screen star density
constant gives `T = sqrt(C / density)`, so a 4,096 slice at 0.1 stars per
pixel sits near 200 px. The cut belongs in the low hundreds of pixels, which
is worth knowing when reading budget behavior.

Real mode has no pixel criterion. A giant at 5,000 ly clearing the magnitude
limit occupies far less than a pixel of cell footprint and must still be
drawn, so photometry alone decides and the walk terminates because `m_min`
prunes. The 10 to 50k stars above threshold are their own budget.

One pixel is the right bound in exactly two places, both of them in the sky:
the background splat's opening angle, since a splat narrower than a pixel is
a point, and the PSF core, which is drawn at pixel scale with bloom
supplying apparent size.

## Three regimes, and only two of them draw shells

The blob at far zoom is not a sizing bug. It is the absence of a far
representation: every fetched system gets a shell at any distance, so a wide
view composites all of them into mush.

1. **Full descent.** Every system in the cell, individually.
2. **Sampled.** Additive slices, individually, at roughly constant density
   on screen.
3. **Aggregate.** One splat per cell, and no shells at all.

Regime 3 is the map-mode counterpart of the glow below, on the same cells.

Aggregating beats drawing more marks because with many systems to a pixel
every compositing rule lies. Additive saturates to white and the category is
gone. Alpha blending averages, so a red allegiance and a blue one make
purple, which denotes a faction that is not there, and inventing an answer
is worse than losing one. Depth testing picks arbitrarily and flickers as
the camera moves. A cell holding a histogram can say something true instead:
colored by the dominant category, intensity by count, and stippled by
histogram proportion so a region's mix reads as texture.

Regime 2 on its own would hold drawn density constant on screen, which fixes
the blob but destroys what makes a galaxy legible, since the Milky Way's
shape is density. So the aggregate is not a cheap substitute for regime 2.
It carries what regime 2 cannot, and both draw: sampled individuals over an
aggregate underlay.

Populated systems never aggregate. There are about 117,000 of them against
129 million, they are resident anyway, and they are what a user navigates
by. Draw them as individuals at every zoom, over the aggregate, which is
what `prominence` scaling the mark by population is already reaching for.

### One system, one unit of weight

Every system carries exactly one unit of weight, and at any cut that weight
sits in exactly one place: a discrete shell, or a cell's splat. Never both,
never neither. It is easiest to see in the narrow case, a giant drawn live
that must not also be folded into the glow behind it, but it holds at every
cut in either mode.

So a cell's aggregate covers the systems in its subtree that live in deeper
slices, not its whole subtree. Since slices partition:

    R(parent) = sum over children of [ S(child) + R(child) ]

Descending replaces one parent splat with its children's slices drawn as
shells plus their residual splats, for identical total weight. Refinement
moves weight from smooth to discrete and neither creates nor destroys any,
which is what makes shells and the field one quantity in two
representations rather than two things to reconcile.

The two refinements run in opposite directions on the same tree. Discrete
individuals refine additively, a child adding what its ancestors lacked. The
aggregate field refines by replacement, a child's splat standing in for the
parent's. They stay consistent only through the residual above.

Cross-fading is safe because the aggregates compose exactly. Counts, flux
buckets and histograms are sums, the count weighted centroid is a weighted
mean, and the RMS radius composes through the parallel axis theorem. Parent
and children therefore integrate to the same totals, so blending between
them cannot pump brightness.

The cross-fade does not begin until every child is resident. Both the
loading ramp and the LOD blend are weight scalars and multiply, but
conservation only holds once the children exist, so starting early fades the
parent out into nothing and opens a hole where the field was. The parent
holds full weight while children are in flight, which is one thing the N of
M readout is for, and is why a request that takes a second or two costs
detail rather than correctness.

### Accumulate, then resolve

Splats are never blended against each other. They accumulate additively with
no depth write into an offscreen field, each contributing weight across its
Gaussian footprint into per category channels, and one resolve pass
normalizes: composition from the channels over the total, intensity from the
total.

Addition is order independent, so neighbors sum into a continuous field with
no seam and no draw order artifact. A fine cell beside a coarse one needs no
stitching either, since conservation means the total is right whatever level
each region resolved at. There is no crack because it is a field and not a
mesh.

Today's shells are `AlphaMode::Blend` (`galos_map/src/systems/spawn.rs:180`),
which is order dependent averaging, so the far view's color depends on draw
order. This replaces that with a defined weighted mean.

The field is low frequency, so it accumulates at quarter resolution and
bilinear upsamples in the resolve. That is nearly free, and the upsample is
what turns discrete splats into a smooth gradient. It also affords a channel
per `ColorBy` category, so the histogram survives to the resolve and the mix
can read honestly: dominant category as hue, normalized entropy driving
desaturation, so a contested region reads gray rather than as a color no
faction has.

Both modes share the structure, Real mode accumulating flux per temperature
bucket where map mode accumulates count per category. The field resolves
into the linear HDR path the camera already carries (`Hdr` and
`Bloom::NATURAL` at `galos_map/src/camera.rs:407` and `:428`), so bloom acts
on it as the PSF for free.

Field first, shells over it in the forward pass. The field holds only the
residual, so a shell never sits on top of its own contribution.

This closes TODO(#72) structurally rather than by clamping. Nothing renders
below the floor, because the transition hands off to the cell containing the
mark instead of shrinking the mark toward nothing.

### Filtering the aggregate

Nothing is built per filter combination. The cell stores marginals per axis
and a stratified sample, and every filter is composed from those at draw
time, so a mask is a dot product in the resolve.

The filters split by cardinality, and the split lands either side of the
aggregate boundary.

`Route` and `Systems` (`galos_map/src/systems/filter.rs:151` and `:162`)
carry explicit address lists tens to hundreds long. They never touch the
aggregate, and `FetchIndex::Systems` is already the path they ride.

`Faction` reaches only populated systems, about 96,000 against 129 million,
and those are resident and drawn individually at every zoom regardless. A
faction filter never leaves the always-individual set.

`Recency` is a scalar axis, so eight age buckets answer any span exactly by
prefix sum, for 16 bytes a cell.

Categorical `ColorBy` masks are per-axis marginals on the cells that hold
any of the systems concerned. `ColorBy` is `Allegiance | Government |
Security` (`galos_map/src/systems/spawn.rs:190`), all three null for 97.2%
of systems, so only populated cells carry them: 22 categories across the
three axes, as u8 fractions, on the 40 to 60 thousand cells that have any,
which is about 1.1 MB.

They are needed despite that small domain, because governed systems cluster
far harder than they are few. One 256 ly cell holds 5,805 of them, and at
full galaxy zoom, 42.7 ly to the pixel, that cell is 6 px across and holds
470 systems to the pixel. A global count under the budget says nothing about
whether they can be told apart where they actually sit.

Bubble-core spacing works out near 10 ly once the import lands, so a 2 px
mark at `p = 0.3` needs 6.7 px of separation and the crossover falls at
about 1,950 ly. Inside that the political view is marks; beyond it, splats.

The galaxy-scale axis is `primary_star_class`, with 94.5% coverage and 36
values against the 8 `Hue` offers, so its marginal buckets to 8 bytes on
every cell.

A backdrop of ungoverned systems behind a political view is a density
question rather than a composition one, so it reads `count` and splats one
uncolored channel.

Marginals cannot answer a conjunction, and independence is not an acceptable
stand-in: allegiance and government correlate hard in Elite, so a product of
marginals would confidently draw systems that do not exist. The escape is
the spatial uniformity quota already in every slice. It is a stratified
sample with a known inclusion probability, so splatting each loaded system
at weight `1/p` estimates filtered density without bias under any predicate
at all, including axes nobody has invented yet. Poisson disk rather than
pure random makes it lower variance, and it sharpens on its own as `p` rises
toward 1 with depth.

Noise never bites where it hurts, for the reason that keeps recurring: a
conjunction narrows, a narrow filter has a small true population, and a
small population is drawable as individuals. The estimate matters least
exactly where it is worst.

Filtering sharpens the loading numbers rather than breaking them. Marginals
make the traversal filter-aware, so a cell with no admitted systems is
pruned before it is fetched, `needed()` shrinks, and `N` and `M` stay
exactly computable. Under the sampled estimator exact pruning is lost, but
conservatively: some empty cells get fetched and nothing is drawn wrong.

The build therefore knows almost nothing about filtering. A new `ColorBy`
axis works immediately through the sampled estimator and exactly after the
next nightly build adds its marginal.

## Flicker

Additive refinement and magnitude ordering handle double-draw, holes, and
pop by construction. Two things remain.

A presence scalar per node ramps 0 to 1 over about 200 ms and multiplies
into flux, so an arriving cell fades rather than appears. At the visibility
floor this is close to physically correct.

Each system's ramp starts at an offset hashed from its id64, spread across
that window. A cell whose systems all ramp together brightens as a block,
which is the one arrival artifact bad enough to notice; dithered, it
stipples in. Magnitude slices help here on their own, since a slice is its
subtree's faintest members scattered through the whole cell volume rather
than a contiguous sub-block, so cell boundaries are already hard to see.

Hysteresis on the descend test, entering at `T_in` and collapsing at
`0.7 * T_in`, keeps a parked camera from toggling. `SWITCHING_THRESHOLD` is
the existing pattern for this.

Prefetch runs `needed()` against the eased pose a few hundred ms ahead and
unions the result into the request set. Under fast travel the freeze policy
below takes over and prefetch stops chasing.

## Names

Names are not a tile payload.

Hand-named systems are one resident table, a few hundred thousand entries at
about 20 bytes, under 10 MB, held always. It is the table the search box
wants anyway.

Everything else is generated from the id64, which encodes boxel coordinates,
mass code, and index, plus a name fragment table of a few tens of KB. This
needs verifying: `elite_journal` has no id64 decoding, and EDTS-style
tooling is the reference. If it does not hold, the fallback is a name
sidecar per leaf.

Loading is already decided by existing code. `choose_names`
(`galos_map/src/systems/labels.rs:502`) is held to whichever is nearer of
`NameRadius` and the spyglass, works in screen pixels, and greedily packs a
few hundred rectangles. `worth_naming` (`labels.rs:264`) requires `stands`,
so pointed-at and selected systems need a drawn mark, which exists only
inside the spyglass. Names are needed for leaves intersecting
`min(NameRadius, spyglass)`, default 100 ly, and nowhere else.

## Two axes: mode and context

Everything so far is the tree and what the map draws off it. The rest of the
drawing is the sky, which reads the same cells through the photometric walk
and differs only in what it makes of them. It starts with what the user
chooses and where the camera stands, because those two decide sizing, and
sizing is the one thing both modes share.

**Presentation mode** is a user toggle, successor to the `View` resource:

- **Real** is photometric points: blackbody color from temperature, flux
  from absolute magnitude and distance, additive blending, bloom as PSF.
  Constellations come out right from any vantage.
- **Shell** is the map's translucent ball, colored by `ColorBy`
  (allegiance/government/security), grown enough to see and click.

**Camera context** is a continuous scalar: how deep inside a system the
camera sits. It drives sizing only, never mode. Both modes are defined at
every camera position; nothing modal happens when the camera crosses into a
system. "Shell" here means the drawn ball, since the bodies work already
names the component `Shell`.

Shell mode needs nothing photometric. Its hue, brightness and dim come from
the populated table and the cell marginals, both of which the index already
carries, and its size from the law below.

The `Systems`/`Stars` split in `View` becomes vestigial once sizing is
context-driven, since `Stars` is the sizing law with `boost = 0` everywhere.
Fold both into `Mode { Real, Shell }` unless the uniform view is still
wanted.

## The sizing law

The bodies work already writes the law in the right shape:
`scale = ANGULAR*d + FLOOR`, an angle a shell holds on screen plus a size,
with `FLOOR` documented as a stand-in for the system's true size until its
bodies have been read, giving way to the real extent once they have. The
stand-in is what makes near neighbors enormous from inside a system. At
8.5e-2 ly it puts Alpha Centauri, seen from Sol, at about 1.1 degrees, two
full moons, and the handover already planned there fixes most of it: with a
true system size in light hours in place of the guess, a neighbor settles to
about `ANGULAR`, roughly 1.4 arcmin, on its own.

What the sky adds is a context blend on the angular term itself:

    angular_radius(d) = angular(context) + size(system)/d
    scale             = angular_radius(d) * d

- Map context, the camera light-years from everything: `angular = 4e-4`, as
  today.
- Sky context, the camera inside a system: `angular` eases down to the sky
  scale, roughly 5e-5 to 1e-4 rad for Shell, a marker grown a bit, and
  effectively one pixel for Real, where bloom does the rest.
- Context is a smoothstep over `log10(distance to nearest system)`, full sky
  inside about 0.1 ly, full map beyond about 2 ly, eased in log space. The
  nearest-system distance comes from the resident tree.

Outside, shells shrink with distance toward the map angle as today. Entering
a system they shrink quickly to the sky scale so neighbors read as bright
dots. `PointerTarget` is sized independently by `pointing`, so sky-scale
dots keep a fat hit target.

## Camera 2.0: aiming and exposure

Looking at the sky is pointing a camera: an aim, a lens, and an exposure.
The map's camera grows those three things, and none of them is a new camera.
They are regimes and dials of the orbit camera it already has.

**First person is the orbit camera at radius zero.** The pose math already
holds there. `rotation` is stored explicitly rather than derived from
positions, so `eye = center + rotation*Z*radius` degenerates cleanly, drag
keeps rotating through the very line it rotates through today, the pan rate
already scales by `radius` and so dies exactly on arrival, and `PITCH_LIMIT`
stops a milliradian short of the zenith, which is less than a bloom width.
There is no mode enum; the regime is `radius == 0`, pinned by `snap`.

**Stand here.** The way in is not a move but a re-parametrization:
`center <- eye`, `radius <- 0`, rotation kept. The eye has not moved and the
view direction has not changed, so the world holds still to the pixel and
instant is smooth. Anything animated is affordance, a reticle or the dials,
rather than the camera. It works from any orbit: wherever the eye happens to
be is where aiming starts. Setting it also pins `target_center` and
`target_radius`, so the easing never finishes a stale approach underneath
the new standpoint.

**Aiming.** Drag rotates in place. Sensitivity scales with the field of
view, as on any real camera, since telephoto aiming needs a finer hand.

**One magnification axis.** Far out, the wheel dollies, as today. At the
radius floor it hands off to focal length: scrolling in narrows the field of
view to frame a binary or resolve a neighbor, scrolling out widens it back
to normal. Run the field of view at the same `ZOOM_RATE` in e-folds and
magnification per notch is continuous across the handoff, so nobody finds
the seam. The billboard shader and the sizing law already read the field of
view as a uniform, so telephoto genuinely magnifies the sky. The projection
is live.

**There is no switch back, by design.** After aiming around, the old center
is stale, and restoring it would snap the view to a former interest, so no
inverse is kept. Leaving is a forward choice, made two ways. Wheel-out past
the widest field of view dollies backward along the view axis, orbiting the
spot just stood on, which is backing away from the tripod. Or **orbit
that**: point at something and take it as the new center, so `center <-
target`, `radius <- distance to it`, rotation kept. The target lies along
the view ray, so the pose identity holds exactly and again nothing on screen
moves. This rides machinery the map already has: `PointedAt`, `Selection`,
and the double-click that today means "that one."

**Exposure.** One dial, EV100. Shutter, iso and aperture trade against
motion blur and depth of field, and there is neither, so one knob is the
honest number. Auto-metering is the default, and Bevy ships histogram
auto-exposure: with brightness on an honest photometric scale, swinging the
view away from the local star makes the exposure climb and the stars fade in
over a second, so dark adaptation is emergent rather than scripted. Manual
override remains for shooting the sky properly, which is an underexposed
foreground. The scale stays honest: at an exposure that holds a sunlit
planet the constellations are gone, exactly as they are from the day side of
a real window. A compressed-range cheat can be a toggle later if honesty
proves annoying; it is not the default.

**The instrument.** The point spread function is part of the camera too, and
its parameters are dials beside the exposure in one camera-controls panel:
core width, wing strength, later spikes. The form is a Moffat, `I(theta)`
proportional to `(1 + (theta/alpha)^2)^(-beta)`, a Gaussian-ish core with
power-law wings, and the wings are where "bright looks bigger" lives. The
radius above threshold grows as `F^(1/(2*beta))`, the gentle law the eye
expects, where a pure Gaussian clips every star to one size. One PSF for
every star, energy-normalized so summed fluxes stay honest. Which shape to
use is an instrument choice, never a per-star one.

Its values come three ways, composable. Computed from a declared instrument,
an Airy core from an aperture, or for the dark-adapted eye the CIE
disability-glare function, roughly `10/theta^3 + 5/theta^2` over 0.1 to 100
degrees, which is a citable and parameter-complete wing. Fitted from a
reference photograph whose look is wanted, which is the inverse problem
astronomy tooling like AstroPhot solves, run backwards as our parameter
source. Or calibrated against the renderer, since the drawn PSF is the
shader's core composed with bloom's fixed kernel: a test renders a star,
reads the radial profile off the framebuffer, and tunes bloom until the
encircled energy matches the target, once, rather than by eye.

## Drawing the sky

The unit is the cell, as everywhere else. The photometric walk returns the
cells whose `m_min` clears the limit, `needed()` turns that into requests,
and the resident cache is the same one map mode reads.

**Cells are meshes beyond the spyglass, entities inside it.** Entities exist
only within the spyglass, where Shell mode needs picking and labels anyway,
and Real mode rematerializes those same entities photometrically. Beyond it
a cell becomes one point-cloud mesh, since far stars need no per-star
identity, and per-frame cost becomes a handful of draw calls whatever the
count. A whole sky is 10 to 50k stars, so the meshes are small. The payload
is already quantized cell-relative, so a mesh anchored to its cell's origin
keeps sub-tolerance precision at any distance from the galactic origin.

**The projection is live; only the star list is a snapshot.** Each star's
quad carries its center position, magnitude and temperature bucket as vertex
attributes, and a billboard vertex shader expands it in view space each
frame, computing direction, angular size and flux from the current eye live
on the GPU. Rotation and zoom are therefore exact at all times, and nothing
about a resident cell privileges a view. What can go stale is only set
membership, which stars clear the limit as the camera travels, and
membership error is invisible by construction, since a star enters or leaves
the set at the visibility floor. By consequence 3 of the physics, a system
is about 1e-4 ly across, so flight inside one changes no membership at all:
the "camera in a system looking out" case costs nothing as a limit rather
than as a detected mode. Tolerance scales with distance, so the coarse cells
holding distant stars are the last to change.

The fragment half draws the PSF core integrated over each pixel's footprint,
an erf difference per axis, rather than sampled at its center, so a star
crossing a pixel boundary hands its energy over smoothly instead of
shimmering. That is the same sampling discipline photometry packages apply
when fitting the other way. The shader is also where later polish lives,
scotopic desaturation and per-star spikes among them, so it is part of the
rendering phase and not an optimization.

**The policy is speed-tiered**, which is where "take a photo of this"
becomes literal:

- Parked or in-system: everything valid, zero cost.
- Slow drift: mesh rebuilds run on `AsyncComputeTaskPool` and swap when
  ready, milliseconds for the whole sky. Live for any speed at which someone
  is actually reading the sky.
- Fast travel, where a fly-to crosses the galaxy at thousands of ly/s and
  invalidates everything every frame: stop chasing. Freeze the composite,
  showing the last good photo, and rebuild the cascade on arrival, nearest
  cells first. The camera already signals settling, since `Travel` completes
  and `snap` pins the center, so the photo develops when you stand still
  without new detection machinery. Physically honest, too: a long exposure
  during a slew gives star trails, not a sharper sky.

Inside the spyglass, where entities exist, photometry is cached on the
entity as an optional `Photometry` component beside `System`, so toggling
modes rebuilds nothing. `System` stays the political row it is, and the
sizing law reads neither. Materials are binned, quarter-magnitude by about
8 temperature buckets, which keeps the shared-handle pattern of
`SystemMaterials`.

## The glow

Below the discrete stars is the summed light of everything under threshold,
which is the Milky Way band. It is regime 3 in Real mode: the same cells,
the same opening-angle walk, accumulated as flux per temperature bucket
instead of count per category, resolving through the same field. The
luminosity centroid is where a cell's light splats from, the spread is its
Gaussian footprint, and the residual rule keeps a star that is drawn
discretely out of the glow behind it.

**The glow renders the record.** It is an accurate picture of what the
database holds, survey bias included. A bright corridor along a popular
exploration route is information, and the map is a map. A galaxy density
model for a prettier Milky Way is a separate future feature; it would feed
the same field as another flux source, changing nothing here.

The index is resident and about 5 MB compressed, so the walk that draws the
glow touches no payload and no server. That is what makes the glow the one
thing never absent and never patchy: it depends on nothing being loaded, and
what refines is only how much of it has condensed into discrete marks.

A precomputed cubemap is an option and not part of the spine. A mesh remembers
positions and a cubemap remembers directions, so rotation is exact but the
depth is gone and everything in it behaves as infinitely far. That flaw is
harmless only in the outermost layer, where `e/d` stays under tolerance for
any plausible travel, which is the layer the cubemap would cover: one
precompute per reference position on a grid, swapping to the nearest as the
camera crosses hundreds of ly. It buys nothing while the index is resident and
the splats are cheap, so it stays on the shelf until the walk is measured. The
one thing that would want it is a galaxy density model, which has no cells
of its own and for which an image is the natural container.

The cut between glow and discrete stars is fixed at build time rather than
derived, and this is the one place the residual subtraction cannot help.
Subtracting a drawn star from a live splat is arithmetic on moments, but
subtracting one from a precomputed image is not possible at all, so anything
the image integrates must be below the level at which the client ever draws
individuals. Where the glow is drawn live from the index, the cut is the
residual and there is no constant. Where an image is precomputed, the constant
is the level that image bottoms out at, a property of the precompute rather
than of the schema.

## Coordination with the bodies work

The bodies work owns the near field: the current system's own stars and
planets at real geometry. The sky is the complementary far field, excludes
the current system, and by consequence 3 of the physics never parallaxes
while the camera flies within one. The two compose without knowing about
each other, and they meet in only two places.

One is the sizing law's context scalar, which the bodies view will want for
the same blend the `scale.rs` module doc already asks for, and which should
also drive today's ambient light down to black inside a system, where PBR
needs a dark sky.

The other is the photometric scale. Each local star lights the bodies as a
point light whose color and intensity come from the same photometry
functions, anchored to the same EV100 exposure the sky renders under, so lit
surfaces and emissive stars sit on one believable brightness axis. The
bodies work draws the map in meters, one of its stated reasons being that
Bevy's lighting speaks physical units, so those functions feed the lights
real values, candela from a star's luminosity and color from its
temperature, with no unit shim in between.

## Building the index

Ordering by absolute magnitude needs an absolute magnitude for every system,
including the two-thirds with no `stars` rows. The fallback chain is `stars` rows, summing
the fluxes since one point is all these distances resolve -> the
`primary_star_class` lookup table -> a default class. The table maps Elite's
classes (O B A F G K M, TTS, L/T/Y, D*, N, H, ...) to a typical absolute
magnitude and temperature. This is what the commented-out
`galos_db::stars` fetch code is for.

Doing this at build time is what leaves the client with no join to make. The
payload's three bytes of magnitude and temperature bucket per system
are the finished answer, so Real mode at range issues no query at all.

Maintenance is incremental because sums are additive: a scan event upserts
its star's flux into one fine cell per level, and coarser levels roll up
from the fine one by `GROUP BY`, since the weighted moments compose exactly.
New discoveries change any cell's flux by nothing the eye can see. The
aggregate is worth designing so `galos_server` can share it, since "total
recorded luminosity in a region" comes for free.

The build is a Morton sort of 129M records and about 75k file writes, well
under an hour. Nightly incremental rewrites touch the 0.94% of leaves that
changed, and coarse levels barely move, since a newly discovered faint
system rarely displaces anything from a brightest-first prefix.

## Serving

The build runs where the database is. Ingest and the database belong on one
machine because `galos-sync eddn` is a serial loop
(`src/bin/galos-sync/eddn.rs:52`) at 31 messages a second in a busy hour,
and each message fans out into many statements. Round-trip latency, not CPU,
is what decides whether it keeps up, so it runs next to its database and
that machine never accepts an inbound connection.

What clients talk to is two things, and neither can be asked an expensive
question. Cells are immutable files in object storage behind a CDN, so a
request is a static read that never touches Postgres. The dynamic set, under
0.1% of systems, goes to a small API server fed by logical replication of
just those tables, so its queries run over 96,000 rows rather than 129
million. There is no endpoint that scans, and no request whose cost depends
on how much galaxy the camera can see.

That leaves latency, and the design spends it rather than hiding it. Cold
start is the index and the populated table, about 8 MB: the complete galaxy
as a density field at full spatial resolution, with correct density
structure and composition everywhere, before a single payload arrives. Every
political view is in there in full, filters included, since those read the
populated table rather than the tree. So a second or two of round trip
changes how much of a region has condensed into discrete marks and never
whether the region is drawn, and the map is complete at galaxy scale on an
8 MB download, which is the answer for everyone who will never take the full
1.5 GB.

Local routing does not need the full download either. A* along a corridor
touches a thin tube of leaves, so a 500 ly route explores perhaps 2 to 24 MB
of cells that the cache pulls as it goes. The full download is for offline
use and arbitrary routes.

### Follow-up: making a build directory servable

The builder writes the format a client reads, but three gaps stand between a
build directory and one a static server can put behind a CDN. None of them
touches the byte layout; all are in how the files are written, named, and
placed.

1. **Writes have to be atomic.** `Snapshot::write`, `Snapshot::write_diff` and
   `Tree::publish` (`galos_index/src/store.rs`) truncate in place with
   `fs::write` and delete with `fs::remove_file`, so a server reading the
   directory while a `--watch` publish is mid-flight can serve a torn
   `index.bin` or a half-written cell. Each write should land in a temporary
   file and rename over its target, which is atomic on one filesystem, and a
   removal should rename out of the way rather than unlink in place.

2. **Caching needs a generation.** A cell's filename is its Morton key and
   stays put, but its contents change every time a system in it moves, so the
   immutability the serving model above leans on does not hold for a live
   directory: `Cache-Control: immutable` on a stable name would pin stale
   bytes. Two ways out, and the index file is where either is cheapest to
   coordinate, being small and refetched whole: revalidate cells by
   `Last-Modified`/`ETag` on a conditional GET, or stamp a generation into the
   names so a rebuild writes fresh ones and the old expire. A full build
   uploaded to object storage can keep its names immutable per generation; the
   `--watch` directory served in place cannot, and that is the difference
   between the two paths.

3. **Output belongs outside the source tree.** The `index` command defaults its
   directory to `galos_index`, which is the crate's own source directory, so a
   build drops `index.bin` and `cells/` into it (both are `.gitignore`d for
   exactly this reason). The default should be a dedicated directory, or the
   tool should refuse to write into a crate root, so a served directory is
   never the source tree.

## Before the import

`updated_by` is 65 bytes wide in `systems`, `bodies` and `stars`, and
`systems` holds only 23,197 distinct values of it. Post-import that column
lands on 792M rows. A u32 key into an uploaders table saves about 48 GB,
which is 8% of the import, and it is far easier before loading 800M rows
than after.

`body_materials` is 224 GB of the projection, is written by
`galos_db/src/bodies/create.rs:218`, is joined by all five queries in
`galos_db/src/bodies/fetch.rs`, and is read by nothing outside `galos_db`.
As a `jsonb` column on `bodies` it costs perhaps 35 GB instead, taking the
import to about 375 GB.

`systems_position_idx` has never been scanned, despite `ST_3DDWithin` at
`galos_db/src/systems/fetch.rs:325` and the `<<->>` operator at line 256.
At 1.35M rows a sequential scan may simply be winning. Run
`EXPLAIN (ANALYZE, BUFFERS)` before the import, because at 129M rows it will
not be.

## The crates

The work above is one dataset in Postgres, one derived index over it, and one
client that draws the index without ever reaching for the database. That split
is the crate boundary, and one requirement fixes it: the map draws the galaxy
without a database, so `galos_map` must not link `galos_db`, sqlx or Postgres,
even transitively. A crate that wrapped the database would carry all three into
the client and lose the thing the format was for.

- **`galos_photometry`** is the physics: apparent magnitude, flux, combined
  magnitude, blackbody color, and the class fallback table. Pure functions over
  plain numbers, no database and no renderer, so both the build and the client
  read it and neither pulls in the other. It is step 1 and it gates the build.

- **`galos_index`** is the derived structure: the cell record and its
  aggregates, the Morton keys and cell-relative quantization, the on-disk
  format, the three walks and `needed()`, the aggregate composition with its
  residual, and the resident cache the client reads through. It depends on
  `galos_photometry` and nothing heavier. This is what makes the client
  database-free, and `galos_server` shares its read side, since "total recorded
  luminosity in a region" falls out of the same aggregates.

- **`galos_db`** stays what it is, the full authoritative dataset in Postgres,
  and grows the one thing this needs: building `galos_index` files from its own
  tables. The build reads systems and stars, fills absolute magnitude through
  the photometry fallback chain, Morton-sorts and writes the cells. Where that
  builder lives — inside `galos_db`, a sibling crate, a feature-gated binary —
  is left open, since it changes nothing above it.

- **`elite_journal`** is the shared domain model, rather than a new crate for
  one. It already carries systems, bodies, stars, factions, stations and
  markets with their enums and orbits, all with `serde`, and already speaks to
  the database through its `with-postgis-sqlx` feature. `galos_db`'s structs are
  thin persistence wrappers over it, and the server's JSON is these same types,
  so the client and the database share one vocabulary rather than three.

- **`galos_map`** ends with no `galos_db` dependency at all. It draws from
  `galos_index` cell files and asks a small server, over HTTP, for the metadata
  a click needs — a populated system's political columns, a body's scan, a
  faction, a name, a route. Its end-state dependencies are `galos_index`,
  `elite_journal` and an HTTP client. `Allegiance`, `Government` and `Security`
  already come from `elite_journal`, so the political coloring survives the cut
  untouched.

How the client gets the cell files — object storage behind a CDN, a bundled
download, something else — is the serving question below, and is likewise left
open. Neither it nor where the builder lives blocks the index: the cell schema
is what both the builder and the reader agree on, and it is the same whatever
answers those two.

## Order of work

1. **Photometry core** (`galos_photometry`). Pure functions: apparent
   magnitude, flux, combined magnitude, blackbody color, the class fallback
   table. Every function is a one-line physics claim with a unit test. It
   gates the build, because sorting by absolute magnitude needs the fallback
   for systems with no `stars` rows.
2. **The cell schema and the build** (`galos_index`), against the current
   1.35M-system database. A full build takes seconds at that size and the
   tree fits in memory, so the format, the walks, additive refinement and the
   flicker behavior are all validated before the import exists.
3. **The client walk.** `needed()`, the resident cache, the presence ramp.
   Map mode only. This is where the map stops querying Postgres to draw, and
   where `galos_map` drops its `galos_db` dependency, reading the index files
   and asking the server only for the metadata a click needs.
4. **The field.** Accumulate then resolve, the residual subtraction, the
   filter marginals and the sampled estimator. Map mode is complete here,
   and TODO(#72) closes with it.
5. **The sizing law.** The context blend on the angular term, riding the
   angle-plus-size expression the bodies work already writes and its
   true-size handover. Shell mode is complete here, and nothing photometric
   is needed for it.
6. **Camera 2.0.** Stand here, orbit that, the wheel handoff to focal
   length, drag scaled by field of view, and the camera-controls panel with
   the exposure dial and the instrument's PSF parameters beside it. Works in
   Shell mode, so it lands independently of everything photometric.
7. **Real mode.** The photometric walk on `m_min`, `Photometry` and binned
   materials inside the spyglass, cell meshes with the billboard vertex
   shader and the pixel-integrated PSF core beyond it, additive blending,
   the one-time encircled-energy calibration of bloom against the
   instrument's target profile, and the speed-tiered freeze.
8. **The import**, preceded by the three schema changes above, since they are
   far cheaper before 800M rows exist than after. This is what takes the tree
   from 1.35M systems to 129M, and nothing above it changes shape when it
   lands.
9. **Serving.** Object storage and the CDN for cells, the small API server
   for the dynamic set by logical replication.
10. **Polish**, each droppable: scotopic desaturation below a flux threshold,
    optional diffraction spikes, star trails during fast travel. A galaxy
    density model feeding the same field is a separate future feature, not
    part of this.

Steps 5 and 6 depend on nothing in 2 through 4 and can run alongside them.

## To verify

- id64 decoding to boxel coordinates and procedural name, against known
  systems.
- Whether the head-of-file bias moves the density distribution enough to
  change the leaf cap.
- Leaf cap and internal slice size, which trade cold-start size and request
  count against budget granularity, and should be tuned once the build runs
  rather than guessed now.
- The smallest mark that draws stably, which is what TODO(#72) is really
  asking, and which sets the budget through the table above.
- The coverage fraction `p` that decides when two marks stop reading as two.
  With the mark size it sets the budget, and it is a judgement about what a
  star field should look like, so it wants looking at rather than deriving.
- How large the spatial quota must be before voids stop reading as voids.
- Whether the opening-angle walk over the resident index draws the glow
  cheaply enough per frame, which is what decides whether a precomputed
  cubemap is wanted at all.
