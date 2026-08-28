# The sky

A second renderer over the same photometry: no 3D, no GPU, no database. It
draws one sky from one place, to a buffer, on the CPU.

It exists because `galos_photometry` currently has no way to be wrong. Every
function in it is tested against itself — flux and magnitude undo each other,
the main sequence is ordered hot before cool — and none of them is tested
against anything outside the repository. A second renderer and a real star
catalog are the two ways to change that, and they change it in different
places, which is why this reads as two threads rather than one.

The order below is the order the arguments settled, and several of them
settled by ruling something out. What this is *not* is as much of the design
as what it is.

## What it is for

**A human's eye.** Orion looks like Orion, or it does not. The Big Dipper is
recognisable, or the positions are wrong. Sirius is the brightest thing in the
sky, or the magnitude scale is inverted somewhere. These are checks a person
makes in a fifth of a second and no unit test expresses, and they are only
available once something draws a sky small enough to recognise.

**A laboratory for the response law.** Magnitude and flux to a drawn intensity
and radius, given an exposure and a point-spread function. Real mode needs it
and nothing in `galos_map` implements it yet. Developing it inside bevy means
a GPU, a window, a swapchain and an eyeball in the loop for every iteration.
Developing it here means a pure function, a deterministic buffer and a golden
PNG. The map then adopts the law rather than inventing a second one.

That second purpose carries a condition, and it is the load-bearing one: **the
response law lives in `galos_photometry`, called by both renderers.** If it
ends up baked into WGSL in the map and reimplemented here, every comparison
between the two is measuring the gap between two laws rather than between two
renderers, and it will never converge.

The line runs through the middle of what looks like one renderer's business,
and it is worth naming exactly, because the first cut at it was drawn in the
wrong place. The law is **how much light lands and where**:

| | whose | why |
|---|---|---|
| magnitude → energy | shared | `relative_exposure` |
| energy → spatial profile | **shared** | the PSF's normalization decides how much flux is in a star |
| profile → pixels | renderer's | a loop over pixels, or a quad and a fragment shader |
| linear → display | renderer's | bevy has its own tonemapper and should keep it |

The second row is the one that was got wrong. A point-spread function looks
like rendering — it is about pixels, it has a radius, it is what the rasterizer
spends its time in — but its *normalization* is what fixes how much of a star's
flux ends up in the picture at all. Two renderers that normalize a Gaussian
differently put different amounts of light in the same star, which is precisely
the quantity step 3 of the diffing ladder measures. A PSF in each renderer
makes that step unable to converge, for exactly the reason the exposure law in
each renderer would.

So `galos_photometry::psf` carries the profile, the normalization and the
radius, and `galos_sky` keeps only the loop that deposits it and the curve that
displays it. The cutoff below which a star stops being drawn is a *parameter*
of the radius rather than a constant inside it, because that one genuinely does
follow from the tone curve, and a CPU film response and a GPU tonemapper do not
have the same one.

## What it is not

**It is not the index's oracle.** The obvious-looking cross-test — brute-force
visible-star set against `Index::needed`'s octree walk — is a good test, and it
has nothing to do with rendering. The oracle for `needed()` is a loop over
stars asking which clear the eye limit from a viewpoint. Twenty lines, no
catalog, no pixels, no new crate. It belongs in `galos_index`'s own test suite,
beside the existing oracle that compares the live tree against a fresh
`Snapshot::build`, and it can be written today against synthetic stars.
Rendering contributes nothing to it.

So `galos_sky` and `galos_index` are unrelated. The one legitimate seam is
distant: if the sky is ever pointed at ED data — the case where it is looking
at the same stars the map draws — it needs somewhere to get 129 million
systems from, and that is the index files. But that is reading payloads as a
flat store, not calling `needed()` to decide what is visible. A data source,
never a visibility oracle. For a hundred-thousand-star catalog it needs neither.

**It is not a reason for an astrometry crate.** Three things looked like they
wanted one, and they turn out to have two natural homes, split by what each is
for rather than by subject matter — which is why the subject never wanted a
crate of its own.

- RA/Dec and parallax to cartesian, and the ED-to-galactic frame transform.
  Both are `galos_catalog`'s: getting foreign data into the units, frame and
  vocabulary the index speaks is the whole of that crate's job. HYG ships
  `x,y,z` in parsecs and needs neither; raw Hipparcos and Gaia need both.
- Projections. Mollweide for a whole sky, gnomonic for a patch. These are the
  renderer's own business the same way perspective projection is the map's,
  and they stay here.

And the first cross-check between ED and a real catalog needs no frame at all,
because **angular separation is frame-invariant**. Sirius to Procyon, seen from
Sol, is the same number in ED coordinates and equatorial and galactic. So
"does ED's local bubble match the real sky geometrically" is answerable with a
pairwise-separation comparison over the brightest nearby stars present in both,
and the frame matrix is what that comparison *derives*, not what it needs.
Frames and projections are modules here until a second consumer wants them.

## Two threads, and which validates what

The catalog and the renderer answer different questions and should not be
welded together.

**A real catalog validates the functions.** `blackbody_color` against measured
B−V. `apparent_magnitude` against a catalog's own V magnitude with a known
parallax. These are physics claims that hold in any galaxy, so a real one can
falsify them.

**ED's own scans validate the class table.** `class_light` exists to predict
*unscanned ED systems*, and what it should match is Frontier's stellar forge,
not the real main sequence. Pecaut & Mamajek is the prior; the `stars` table is
the data — `absolute_magnitude` and `surface_temperature` per scanned star,
keyed by `star_class`, in the millions. Hipparcos cannot speak to this at all.

## The honesty principle, and the one place it does not reach

A system draws with the light that is on record for it. A partially scanned
system is therefore dimmer than it truly is, and that is the behavior rather
than a defect: the map is honest about what exists. `DEFAULT_CLASS_LIGHT`
already says as much — guessing dim "keeps an unknown from crowding into a sky
it has no claim to."

There are three regimes, and the principle covers two of them cleanly.

1. **Fully scanned.** The real combined magnitude over the system's stars.
2. **Partially scanned.** Combined over what is known, dimmer than the truth.
   Honest in exactly the intended sense.
3. **No scanned star.** `class_light(primary_star_class)` — not measured light
   dimmed by ignorance but a figure invented from a letter, which can land on
   either side of the truth.

The third is where the principle has teeth, and it argues *against* fitting the
table to the centre of ED's scan distribution. If dimness is meant to encode
ignorance, the right value for a class is a conservative point in that
distribution rather than a median. An unscanned O-class system drawn at −4.0 is
visible across thousands of light years on the strength of one letter, and if
it is really a −2 the sky has a bright star in it nobody has ever looked at.

So the report below prints distributions, not central tendencies, and the
table is tuned by choosing a percentile deliberately.

## Validating the class table

A `classes` subcommand on `galos-db`, beside `index`. Informational, not a
test: the distribution moves as EDDN ingests scans, it needs a database, and
there is no assertion to make that a patch could not invalidate. It is read by
a person who then edits the table.

Per distinct `star_class`, over the `stars` table:

- Count, and the count of systems of that primary class with **no** scanned
  star. Two thirds of systems carry no scan, so the fallback decides most of
  the drawn sky — but unevenly, and this says which rows of the table are
  worth caring about. An error on O crowds the bright end; the same error on M
  changes nothing anybody sees.
- Percentiles of `absolute_magnitude` and `surface_temperature`, not just the
  median, so a conservative point can be chosen rather than fallen into.
- The delta against what `class_light` returns today, which is the whole point
  of running it.
- The **combined-minus-primary** delta among scanned systems: how much brighter
  a system's full complement of stars is than its primary alone, per class.
  Reported as a measurement of what the honesty principle costs — roughly how
  many magnitudes the unscanned bubble is dimmed by — and explicitly not as a
  correction to apply to the fallback. Measured rather than assumed.

## Visual diffing

Worth aspiring to, and reachable in three steps of increasing cost. Pixel-exact
comparison is not the goal at any of them; a 3D perspective GPU path and a 2D
CPU path will never agree bit for bit, and should not have to.

1. **Golden images inside `galos_sky` alone.** Deterministic CPU render, PNG
   checked in, compared on change. No GPU, no bevy, no harness. This catches
   every regression in the response law and is available the day the renderer
   runs.
2. **List diff, no pixels.** `galos_map` dumps its per-star screen positions
   and intensities for one frame under a fixed camera; `galos_sky` computes the
   same list through a perspective projection matching that camera. Compare.
   Catches everything but rasterization, and needs no image comparison.
3. **Aperture photometry on both renders.** `galos_map` rendered headless to an
   offscreen target, source extraction run on both images, the extracted star
   lists compared by centroid and integrated flux. This is what astronomers do
   to compare two images of the same field, and it is the right metric for the
   same reason: invariant to what legitimately differs between the two paths,
   sensitive to what should not — how much flux, and where it landed.

Steps 2 and 3 need a gnomonic projection matching a pinhole camera, which is
why projections are a module here rather than one hardcoded whole-sky view.

## What it found

Written after the first pictures, because the point of the exercise is to find
things and one turned up immediately.

**A star's colour changes how brightly it is drawn, by about 0.6 magnitudes.**
`blackbody_color` normalizes a tint so its brightest channel is one. That
carries hue exactly and luminance not at all: a white tint spreads across three
channels and a saturated one concentrates in a single one, so the tint's own
luminance runs from 0.57 at 3000 K through 0.90 at the Sun's temperature back
down to 0.52 at 30,000 K.

| temperature | linear RGB | luminance |
|---|---|---|
| 3000 K | 1.000, 0.479, 0.154 | 0.566 |
| 5772 K | 1.000, 0.878, 0.821 | 0.900 |
| 30000 K | 0.377, 0.512, 1.000 | 0.519 |

Multiply flux by that and a Sun-like star draws over half a magnitude brighter
than an equally luminous red giant or hot blue star, for no reason but its
colour. It is visible in the first render of Orion: Betelgeuse at magnitude
0.45 and Rigel at 0.18 should be near enough the same brightness, and
Betelgeuse comes out markedly dimmer.

It reaches `galos_map` too. `systems/aggregate.rs` builds a cell's tint as a
flux-weighted `blackbody_color`, so a cell of hot stars is drawn dimmer than a
white cell of the same luminosity — and the error is largest exactly where the
sky is most interesting.

The fix is to normalize to unit luminance rather than unit peak. That changes a
contract two renderers share, so it is recorded rather than applied:
`camera.rs` carries `a_stars_colour_should_not_change_how_bright_it_is`, which
asserts the defect and fails the day somebody corrects it.

This is what the second renderer was for, and it came out of looking at one
picture.

## The crates

Two, not one, and the second is the one that changes the shape of this.

### `galos_catalog`

**A catalog is a source of `System`s, exactly as the ED dataset is.** That is
the role, and it is the same role `galos_db/src/index.rs` plays: read my own
data, hand back `Vec<galos_index::System>`, know nothing about the tree. Seen
that way the earlier confusion with `galos_db` dissolves — a catalog crate is
not a peer of the *database*, it is a peer of the *bake*.

Which means it earns its place on a second consumer beyond the sky renderer:
building an index from catalog data and loading that into `galos_map`. A tree
of real stars, walked and drawn by the same client that draws the ED galaxy,
is both a genuine feature and the strongest test that `galos_index` is not
secretly ED-shaped.

It owns catalog parsing, units, and frames — everything between a foreign file
and the vocabulary the index speaks. Its own `Star` is **richer** than
`galos_index::System`, not a duplicate of it: a catalog row carries a name, a
catalog id, a V magnitude, a B−V colour index, a spectral type and proper
motion, and the validation work needs all of them. `System` is the lossy
projection that survives the bake, and the conversion to it is the crate's
output rather than its type.

```toml
[dependencies]
galos_photometry = { path = "../galos_photometry" }
galos_index = { path = "../galos_index", default-features = false, optional = true }

[features]
# The `From<Star> for galos_index::System` bridge and the tree build. Off by
# default so a consumer that only wants to read a catalog — the sky renderer —
# does not pull the index and its serde stack.
index = ["dep:galos_index"]
```

The feature is worth exactly one thing: it keeps `galos_sky` sitting just
above `galos_photometry` rather than above the index, which `galaxy.md`'s
ordering puts at step 2. If it turns out to be ceremony, collapse it — the
dependency is a matter of build surface, never of correctness coupling, since
nothing in the render path calls the index either way.

### `galos_sky`

```toml
[dependencies]
galos_photometry = { path = "../galos_photometry" }
galos_catalog = { path = "../galos_catalog" }
```

Projections and drawing. It defines no star type of its own — it reads
`galos_catalog::Star` and uses the three fields it needs. `galos_index` appears
nowhere in it, not even as a dev-dependency now that the tree-building exercise
has a better home in `galos_catalog`.

**On `age_bucket`.** `galos_index::System` carries one field that is neither
geometry nor photometry: which recency bucket a system's last write falls in,
feeding only the aggregate's `aged` histogram for the Recency colouring. A
catalog sets it to `0` and loses nothing, which is fine — but a second dataset
is what makes visible that the index's input vocabulary carries one field of
pure presentation and one dataset's notion of provenance. Worth noticing while
it is cheap.

**On `id64`.** ED addresses are large and structured; HYG's ids are small
integers. A tree built from a catalog and a tree built from ED cannot share an
id space without a namespace tag or an offset, and the map holds one
`ResidentIndex` and one `Transport`, so today it draws one dataset at a time.
Whether a catalog tree is a second index the map swaps to or a second index it
holds alongside is left open below; the crates above are the same either way.

## Order of work

Items 2, 3 and 4 are built, along with the `index` bridge from item 7.
`galos-db classes` was dropped as minor value — the class table matters for
Elite's unscanned systems, and neither of the two threads here depends on it.

1. ~~**`galos-db classes`.**~~ Dropped.
2. **Done.** `color_index_to_temperature` in `galos_photometry`, Ballesteros. Real
   catalogs give B−V, not effective temperature, so nothing can read a catalog
   without it — and it is a second independent route into `blackbody_color`.
3. **Done.** `galos_catalog`, HYG first: ~119k stars, `x,y,z` in parsecs plus
   `absmag`, `ci` and `spect` in one CSV, which is three columns to check
   against. The test is that the ten brightest come out in the right order at
   the right magnitudes.
4. **Done.** The renderer: a `Camera` placed and pointed, a pinhole
   projection, an energy-conserving PSF, a tone curve and a PNG. Pinhole
   rather than Mollweide first, because it is the projection a pinhole camera
   has and so the one the diffing ladder's steps 2 and 3 need. A whole-sky
   projection is still wanted for judging star counts.
5. **The response law** in `galos_photometry`, developed here against golden
   images, adopted by Real mode when it lands.
6. **The brute-force visibility oracle** in `galos_index`, which is unrelated
   to all of the above and can be done at any point, including first.
7. **A tree built from HYG**, through `galos_index` end to end — build, walk,
   payloads — on a dataset with no ED in it anywhere, behind `galos_catalog`'s
   `index` feature. Tests something none of the others do: whether
   `galos_index` is secretly ED-shaped.
8. **That tree loaded into `galos_map`**, which is what `galos_catalog` is
   ultimately for: the real sky drawn by the client that draws the ED galaxy,
   through the same walks and the same cells.

## To verify

- Whether ED's scanned population is a fair sample of its own forge within a
  class, or whether commanders scanning what is interesting biases the
  magnitude distribution the `classes` report reads. The report is only worth
  tuning against if it is not.
- Which percentile of a class's magnitude distribution is the honest
  conservative point. It is a judgement about how much an unscanned system may
  assert, not a figure to derive.
- Whether the combined-minus-primary delta is large enough to matter at the
  bright end, where the fallback decides whether a system is drawn at all.
- Whether a whole-sky Mollweide or a narrow gnomonic patch is the more useful
  first view. Recognising a constellation may want the patch; judging the
  overall star count wants the whole sky.
- What the ED-to-galactic matrix actually is, derived from the pairwise
  angular separations rather than assumed from Sagittarius A*'s coordinates
  alone.
- Whether ED's bubble is close enough to the real sky for the comparison to
  mean anything, or whether the forge diverges from the seeded stars fast
  enough that only the nearest few dozen are comparable.
- Whether `galos_map` can be rendered headless to an offscreen target
  cheaply enough for step 3 of the diffing ladder to run anywhere but a
  workstation.
- Whether interstellar extinction needs modelling for a real catalog to look
  right. ED does not model it; the real sky has it, and the Milky Way's band
  is where the difference would show.
- Whether a catalog tree is a second index `galos_map` swaps to or one it
  holds alongside the ED tree. Swapping is a control and costs nothing;
  holding both means an id namespace and two resident indexes, and is only
  worth it if seeing real stars among ED's is worth seeing.
- Whether `galos_catalog`'s `index` feature earns itself, or whether the
  dependency it defers is small enough that the flag is pure ceremony.
