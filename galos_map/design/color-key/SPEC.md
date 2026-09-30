# galos_map: color key and filter redesign

A spec for implementation in `galos_map`. It merges "Color By" (today in the
settings pane) and the Filter tab of the bar into one design, and makes every
color of every category a toggle that hides the systems drawn in it.

Write the code in the repo's existing style: prose doc comments that say why,
not what, and tests named as statements (`the_chrome_is_lettered_in_one_width`).


## 1. What changes, in one paragraph

A **color mask** is added beside the existing filters. It is a set of hidden
values per category (allegiance, government, security) plus one flag for
uninhabited systems. It is edited in a **key** that lives at the top of the
bar's Filter tab, and whose category tabs replace the Color By setting. When
the form is closed, the key collapses to a **color row** at the top of the
bar's applied rows: one chip per entry, each a one-click toggle. Hovering the
row shows a **mini legend** naming the chips. A new **hide interface** toggle
hides the chrome and shows the mini legend above the rose instead.


## 2. Model

### 2.1 The mask

```rust
/// What the user has asked not to see, by the value a system is colored by
#[derive(Resource, Clone, Default)]
pub struct Mask {
    allegiance: BitSet<{ Allegiance::BUCKETS }>,   // bit per Bucketed bucket
    government: BitSet<{ Government::BUCKETS }>,
    security: BitSet<{ Security::BUCKETS }>,
    uninhabited: bool,
    /// Lifted without being forgotten, like `Entry::enabled`
    enabled: bool,
    revision: u32,
}
```

- Key the bits on `galos_index::read::inhabited::Bucketed::bucket`, not on
  `Hue`. Seven governments share red, and each must toggle on its own. A `u32`
  per category is enough (the largest is 18 buckets); a const-generic bitset
  is optional.
- **All three categories apply at once**, whichever one the map is colored by.
  Hiding Independent and then coloring by Security must not bring
  Independent back. The color row says so (see 4.1, `+N`).
- "Uninhabited" means a system with no row in `Populated`. It is one flag
  shared by all three categories.
- `enabled == false` admits everything the mask would hide, and keeps the
  bits.
- Bump `revision` on every change (see 2.3).

### 2.2 How it combines with the filters

Today, in `map/filter.rs`, picking filters (faction, route, systems) are
OR-ed, and `Recency` is AND-ed over them. The mask is another AND:

```
admitted = (no picking filter enabled || any picking filter admits)
           && recency admits
           && mask admits          // true when !mask.enabled
```

The mask narrows; it never adds. Put it in `Prepared::admits` beside the
`Recency` arm so there is still one predicate for drawn systems and LOD
points.

### 2.3 What a candidate has to carry

`filter::Candidate` today holds `address`, `factions`, `updated_at`. Add:

```rust
/// The system's buckets, [`None`] for a system nobody lives in
pub politics: Option<Politics>,   // Politics { allegiance: u8, government: u8, security: u8 }
```

It is built in two places, and both must fill it:

- `map/galaxy/walk.rs` `fn candidate`, from `populated.get(address)`
  (`PopulatedSystem` already has `allegiance`, `government`, `security`).
- `map/galaxy/mod.rs` `System::candidate`.

At `DimTo(0)`, masked systems are not loaded, exactly like filtered ones: never
spawned, and evicted if already on the map. The walk has to see the mask's
`revision` change the same way it sees `Filters::revision` today, or toggling a
chip will not evict anything.

### 2.4 The field and the marks

`paint/glow.rs` keeps an invariant: a system is worth the same light whether
it is drawn as its own mark or the field stands in for it (`mark_light`). The
mask has to hold that invariant too:

- `political()` skips masked buckets, across **all three** categories, not
  only the one being drawn. Because `Inhabited` holds three separate marginal
  histograms and not their joint, the field cannot remove exactly the systems
  hidden in the *other* categories. Accept that: remove the masked buckets of
  the category being drawn exactly, and scale the rest by the share of the
  cell the other categories' masks leave, computed from their own histograms.
  Note this approximation in a doc comment.
- Uninhabited systems are the backdrop (`Gains::backdrop`). Hiding them drops
  the backdrop term.
- A masked system drawn as a mark follows `DimTo`, like a filtered one; its
  light must leave the field when it does.
- `map/galaxy/blobs.rs` uses `political()` for merged marks, so it inherits
  the change. Check it.
- Existing tests `composition_is_additive` and `nobody_home_deposits_nothing`
  must still pass. Add one that a fully masked cell deposits nothing.

### 2.5 Tiers: how the key groups its rows

The key does not list every value flat. Each category has a top tier, and
groups underneath it.

**Allegiance** (the user's "three kinds"):

```
Federation                                  red
Empire                                      cyan
Alliance                                    green
Other  (collapsible, closed by default)
  Independent, Player pilots                yellow
  Pilots Federation, Frontline Solutions    orange
  Guardian                                  blue
  Thargoid                                  magenta
  Unaligned  (buckets: unreported + None)   gray
Uninhabited
```

Keep Independent's place a single `const` for now; it may move into the top
tier (open question 1).

**Government**: one group per hue, taken from `Hue::government`. A group of
one is drawn as a plain row. Groups are always open.

```
RED      Communism, Confederacy, Dictatorship, Feudal, Patronage, Prison, Prison Colony
         Corporate                                        cyan
BLUE     Democracy, Theocracy
         Anarchy                                          yellow
         Cooperative                                      orange
GREEN    Carrier, Megaconstruction, Private Ownership
         Engineer                                         magenta
         None  (buckets: unreported + None)               gray
Uninhabited
```

**Security**: flat. High (blue), Medium (cyan), Low (green), Anarchy (red),
None (gray, unreported + None), Uninhabited.

Build the government and security groups by walking `Bucketed::at` through
`Hue::government` / `Hue::security`, so the key and the map cannot disagree
about which color means what. Only the allegiance tier split (which three are
on top) is written by hand.

A row can cover several buckets (the gray rows do). Toggling a row sets or
clears all of its buckets.

### 2.6 ColorBy

`ColorBy` stays a resource. It is now set by the key's category tabs and
removed from the settings pane (`ui/settings.rs`, the "Color By" block).
`DimTo` stays in settings and now governs masked systems too; update its help
text to say so.


## 3. Where things live

The chrome keeps its zones (`ui/mod.rs`): Asking is the ask bar, Holding is
the rows under it, When is the time strip. Nothing new goes in the bottom
right except the mini legend while the interface is hidden.

```
┌─gear─┐ ┌ System │ Filter │ Route ─────────────┐
│  ⚙   │ │ ALLEGIANCE  GOVERNMENT  SECURITY       │   <- the key: Filter tab only
└──────┘ │ ■ Federation                  3,219    │
┌─eye──┐ │ ■ Empire                      2,442    │
│  ⊘   │ │ ■ Alliance                    1,036    │
└──────┘ │ ▸ ◕ Other                     4,514    │
         │ ─────────────────────────────────────  │
         │ ◌ Uninhabited               621,600    │
         │ all  none  invert       alt-click: solo│
         │ ────────────────────────────────────── │
         │ Faction  > search a faction            │   <- existing faction lookup
         │ Heard    ├────────●──┤ 30d             │   <- existing recency control
         └────────────────────────────────────────┘
          ☑ ALLEGIANCE  ■ ■ ■ ◕  ◌      1 hidden      <- color row (always shown)
          ☐ Faction     Mother Gaia
          ☑ Heard       within 30d
            through     703 / 12,210
```

`■` filled chip, `◕` the Other chip, `◌` Uninhabited (dashed circle).


## 4. Components

### 4.1 Color row (applied rows, `ui/bar/applied.rs`)

Always drawn, as the first row of the applied rows, whether the form is out or
not. It replaces any separate "Color · N hidden" row.

```
☑  ALLEGIANCE   ■ ■ ■ ◕   ◌        2 hidden +1
^  ^            ^         ^        ^
|  |            |         |        summary
|  |            |         uninhabited chip
|  |            one chip per top-tier entry of the current category
|  current category name
mask enabled
```

| Part | Behavior |
|---|---|
| Checkbox | `Mask::enabled`. Unchecked, the summary is struck through and muted; the chips keep showing what is set. |
| Category name | Click: open the Filter tab (`AskMode::Filter`) with the key showing. |
| Chip | One per top-tier entry: an item, or a whole group. Click toggles it (a group: hide all if any are shown, else show all). Alt, ctrl or cmd click: solo, hiding every other value in the category and Uninhabited too. |
| Uninhabited chip | Toggles `Mask::uninhabited`. |
| Summary | `all shown`, or `N hidden` for the current category, plus ` +M` when M values are hidden in the other categories. Amber when the mask is enabled and anything is hidden, muted otherwise. |
| Hover | Anywhere on the row, while the Filter tab is not out: show the mini legend popover (4.3). |

For Allegiance that is four chips plus Uninhabited. For Government it is one
per hue group (eight); for Security, five.

### 4.2 Key (Filter tab form, `ui/bar/filter.rs`)

The Filter tab form becomes: category tabs, key rows, a footer, a divider,
then the existing faction lookup and recency control, unchanged.

- **Category tabs**: `ALLEGIANCE  GOVERNMENT  SECURITY`, small caps, the
  active one underlined. Picking one sets `ColorBy`.
- **Item row**: swatch, name, count, right-aligned. Click toggles; alt, ctrl
  or cmd click solos. While hovered, every other value on the map drops to
  about a quarter of its opacity (a preview; it changes no state).
- **Group header (Other)**: a disclosure chevron (its own button), then a
  swatch, name, summed count, and `k/n` in amber when partly hidden. Clicking
  the header, not the chevron, toggles the whole group.
- **Group header (government hues)**: small-caps hue name with `all`, `none`
  or `k/n` at the right. Clicking toggles the group. No chevron.
- **Uninhabited**: its own row at the bottom, under a hairline, with a dashed
  circle swatch.
- **Footer**: `all` (show everything in this category, and Uninhabited),
  `none` (hide every value in this category, but leave Uninhabited alone),
  `invert` (this category only), then `alt-click: solo` as a hint.
- **Height**: the list scrolls past a fixed height (about 360 px at the
  current type size), so Government does not push the faction lookup off
  screen.
- **Counts**: the root `Inhabited` histograms (`Settled`), summed over a row's
  buckets. They are resident, so this costs nothing. Uninhabited is the
  index's system count minus the populated count. Format them with
  `ui::text::thousands`.
- `Shift-F` still opens the Filter tab; keep focus on the faction box, since
  that is still the only thing typed there.

### 4.3 Mini legend

One list, drawn in two places.

```
■ Federation
■ Empire
■ Alliance
◕ Other         6/7
click a chip to toggle
```

- One line per top-tier entry, in the color row's order, with the same
  swatch. Fully hidden entries are struck and muted. Partly hidden groups show
  `k/n` in amber. Government groups read `Red (7)`.
- **Popover**: framed, anchored under the color row, shown while the pointer
  is over the row and the Filter tab is not out. It has the footer hint.
- **Bare**: frameless, above the rose in the bottom right, with the category
  name over it in small caps, shown only while the interface is hidden. No
  hint line.

### 4.4 Hide interface

New. A second square button under the gear (an eye with a slash) and a key
binding (`F2` is free; `F1` is help and `F3` diagnostics). Hidden, the map
draws only: the rose, the bare mini legend (4.3), and a faint eye button in
the top left to bring the chrome back. `Esc` also brings it back. Names, grid
and the time strip keep their own switches; this hides the bar, gear, rows and
panels only. Add it to the bindings table in `galos_map/README.md`.

This is the lowest priority part; it can ship after the rest.


## 5. Visual spec

All egui, in the existing monospace chrome (`style.rs`). Sizes below are at
the bar's current type size (Body 11.5).

| Token | Value | Used for |
|---|---|---|
| panel fill | `#111319` | form frame, popover |
| panel stroke | `#2a2f3b`, 1 px | frames, dividers |
| hover fill | `#1b1f28` | hovered row, active ask tab |
| text | `#d7dae2` | names |
| muted | `#8b91a1` | captions, small caps, lifted state |
| hidden text | `#7d8494`, struck through | hidden row names |
| count | `#9aa0ae`, one step smaller | counts |
| attention | `#f0c060` | `N hidden`, `k/n` |

Hue colors come from `Hue::light()` through `style::color32`, never a second
table.

Swatch states (10 px in the key, 13 px in the color row, 2 px corner radius):

| State | Drawing |
|---|---|
| shown | filled with the hue |
| hidden | 1.5 px stroke of the hue, no fill, 50 % alpha |
| partly hidden (groups) | left half filled, 1.5 px stroke of the hue |
| Other chip | a pie of its members' hues, weighted evenly; hidden = gray stroke only; partial = the pie at 60 % alpha with a gray stroke |
| Uninhabited | 1.5 px dashed circle, `#8e94a4`; hidden = 40 % alpha |

egui has no conic fill: draw the Other chip as a `Shape::mesh` of triangle
fans, or fall back to vertical stripes if that is simpler. It is 13 px; either
reads.

Indents in the key: top-tier rows at 32 px, children of a collapsible group at
50 px, to line up under the chevron.


## 6. Files likely touched

| File | Change |
|---|---|
| `src/map/filter.rs` | `Mask` resource, `Politics` on `Candidate`, the AND in `Prepared::admits` and `Filters::admits`, revision handling |
| `src/map/galaxy/walk.rs` | fill `politics` in `candidate`, react to the mask's revision |
| `src/map/galaxy/mod.rs` | fill `politics` in `System::candidate` |
| `src/map/paint/glow.rs` | masked buckets in `political` / `composition`, backdrop off with Uninhabited |
| `src/map/galaxy/blobs.rs` | check merged marks through `political` |
| `src/map/galaxy/spawn.rs` | a function from category to tiers, built from `Hue::*`; `ColorBy` unchanged |
| `src/ui/bar/filter.rs` | the key at the top of `filter_body` |
| `src/ui/bar/applied.rs` | the color row first; the "through" count includes the mask |
| `src/ui/settings.rs` | remove Color By; update DimTo help text |
| `src/ui/mod.rs`, new `src/ui/legend.rs` | mini legend (popover and bare), hide interface |
| `src/map/keys.rs`, `README.md` | `F2` binding and its row in the table |


## 7. Plan

Each step builds, passes tests, and leaves the map usable.

1. **Model.** `Mask`, `Politics` on `Candidate`, the AND in `admits`. Unit
   tests: a masked allegiance is not admitted; masks in other categories still
   apply; `enabled == false` admits everything; Uninhabited hides only
   systems without a `Populated` row; the mask narrows but never adds (a
   faction filter plus a mask admits the intersection).
2. **Walk and eviction.** Revision plumbing, `DimTo(0)` eviction. Test that a
   masked system is evicted at a dim of zero and dimmed above it.
3. **Field.** `political` honors the mask. Test that a fully masked cell
   deposits nothing, and that the light a masked system takes out of the
   field equals its `mark_light`.
4. **Tiers.** The category-to-tiers function. Test that every bucket of every
   category appears in exactly one row, and that every row's swatch is the
   `Hue` the map paints its buckets in.
5. **Key.** The Filter tab form; remove Color By from settings.
6. **Color row.** Chips, solo, summary, checkbox; the "through" count.
7. **Mini legend popover.**
8. **Hide interface** and the bare legend. Optional for a first release.


## 8. Open questions

1. **Independent's tier.** Top tier beside the three powers, or under Other?
   By count it is the largest allegiance, so Other is mostly Independent.
2. **"Other as gray".** An option to paint every Other system one neutral
   gray on the map, so the map itself reads as three kinds. It was a lever in
   the mockups; not specified here.
3. **Unreported vs None.** The gray rows fold `Bucketed` bucket 0 (no reading)
   in with the explicit `None`. Split them if the difference matters.
4. **Persistence.** If filters are saved between sessions, the mask and the
   chosen category should be saved with them.
5. **Field approximation** (2.4). Acceptable, or should `Inhabited` grow a
   joint histogram? That costs index size and is probably not worth it.
