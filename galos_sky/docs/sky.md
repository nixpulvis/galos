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
renderers, and it will never converge. The law is physics-shaped — flux in,
pixels out — so it belongs in the physics crate on its own merits, and the
diffing below depends on it being there.

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
wanted one and none of them survives:

- RA/Dec to cartesian. The HYG catalog ships `x,y,z` in parsecs already; the
  conversion is a column the format has. From raw Hipparcos it is
  parallax-to-distance and two trig calls, private to the loader.
- Projections. Mollweide for a whole sky, gnomonic for a patch. These are the
  renderer's own business the same way perspective projection is the map's.
- The ED-to-galactic frame transform. One 3×3 matrix and Sol's offset — a
  constant with a test.

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

## The crate

```toml
[dependencies]
galos_photometry = { path = "../galos_photometry" }
```

And, for now, nothing else. `galos_sky` sits just above the physics crate and
below everything derived — `galaxy.md` puts photometry at step 1 and the index
at step 2, and a photometry testbed belongs at 1.5. It defines its own `Star`:
position, absolute magnitude, temperature. The same three fields
`galos_index::System` carries, without the `id64` and without the `age_bucket`.

There is no `galos_catalog` crate and no `galos_astrometry` crate. The loader
is a module here, producing `Vec<Star>`; frames and projections are modules
here. Either is promoted the day a second consumer wants it and not before.

`galos_index` appears only as a `dev-dependency`, and only for the separate
exercise below of building a tree from a real catalog. It is never in the
render path.

**On `age_bucket`.** `galos_index::System` carries one field that is neither
geometry nor photometry: which recency bucket a system's last write falls in,
feeding only the aggregate's `aged` histogram for the Recency colouring. A
catalog sets it to `0` and loses nothing, which is fine — but a second dataset
is what makes visible that the index's input vocabulary carries one field of
pure presentation and one dataset's notion of provenance. Worth noticing while
it is cheap.

## Order of work

1. **`galos-db classes`.** Self-contained, needs no new crate, and it is the
   one thing here that can tell you a number currently carried on faith is
   wrong. Do it before more is built on the bake.
2. **`color_index_to_temperature`** in `galos_photometry`, Ballesteros. Real
   catalogs give B−V, not effective temperature, so nothing can read a catalog
   without it — and it is a second independent route into `blackbody_color`.
3. **The catalog loader**, HYG first: ~119k stars, `x,y,z` in parsecs plus
   `absmag`, `ci` and `spect` in one CSV, which is three columns to check
   against. The test is that the ten brightest come out in the right order at
   the right magnitudes.
4. **The renderer**: projection, splat, tone-map, PNG. A Mollweide whole sky
   from Sol as the first golden image, and the first time anyone can look at
   this and say whether it is right.
5. **The response law** in `galos_photometry`, developed here against golden
   images, adopted by Real mode when it lands.
6. **The brute-force visibility oracle** in `galos_index`, which is unrelated
   to all of the above and can be done at any point, including first.
7. **A tree built from HYG**, through `galos_index` end to end — build, walk,
   payloads — on a dataset with no ED in it anywhere. Tests something none of
   the others do: whether `galos_index` is secretly ED-shaped.

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
