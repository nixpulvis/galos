# The Galaxy Index

The on-disk format shared by `galos` and `edda`: one directory that both a
streaming map and a memory-mapped router read, written by one builder from one
database. The first half explains the format. The second half maps the EDGX,
AGG1 and EBEX formats onto it.

## The directory

One directory is the whole index. The database is the authority and it is a
server; this is a snapshot of it and it is a file format.

```
<dir>/
  index.bin                         the tree: every occupied cell, structure only
  cells/LL-<morton>.g<gen>.bin      what each cell owns: its points and their trailers
  agg/<column>.g<shape>.bin         one aggregation over the tree, one record per cell
  names/NNNNN.bin                   every system's name and position, in chunks
  factions.bin                      every faction, and the systems it holds
  stations/<market>.g<gen>.bin      one station: identity, board, outfitting, shipyard
  dict/commodities.bin              the commodity dictionary, in canonical order
  dict/modules.bin                  the module dictionary
  dict/ships.bin                    the ship dictionary
  bodies/<address>.bin              the insides of one system
  manifest.g<gen>.json              every file at one generation, with length and hash
  pack/<class>.g<gen>.ebex.zst      optional: one file class concatenated, for bulk download
```

Two numbers run through all of it. The **content generation** advances on
every publish. The **shape generation** advances only when the tree gains or
loses a cell. A file whose name carries a generation is written once and never
rewritten; a publish writes the files that changed under the new generation and
then the index, so a reader never sees an index naming a file that does not
exist. Old generations are removed after a grace period.

A client polls the two numbers and nothing else. If content moved, it re-reads
`index.bin` and refetches any resident cell whose recorded generation is older.
If shape moved, it re-reads the columns it holds.

## The tree

**The cube.** One cube stands over the galaxy, 131,072 ly on a side, centred
near the galactic core. That is the smallest power of two that holds a disc of
the Milky Way's diameter with margin on every side. Level zero is the cube.
Each level halves the edge, so a cell at level 13 is 16 ly across, and the tree
goes finer where the bubble is dense, down to level 21.

**A cell** is one such box at one level that has systems under it. It has an
address, its level and integer coordinates, and a Morton key that interleaves
those coordinates so cells near in space are near in the key. A cell has up to
eight children, the octants of its box. Only occupied cells exist.

**A slice.** Every system belongs to exactly one cell. The systems under a cell
are sorted by brightness, and the cell **owns** the brightest few hundred of
them that no ancestor took first. Those are the cell's slice. The rest fall to
its children the same way, until a cell with few enough systems owns them all.
So the root's slice is the brightest systems in the galaxy, a leaf's slice is
everything left in its box, and a coarse view draws bright systems without
loading anything below it.

**`index.bin`** is one record per occupied cell, in level-then-Morton order.
The record is the cell's structure and nothing else:

```
level, morton key
rank_lo, rank_hi          the slice: which run of the brightness order it owns
child_mask                which of the eight octants exist
count                     systems in the whole subtree
gen                       the content generation its cell file was written at
```

Its header carries the cube's edge and centre, the maximum level, the cell
count, both generations and the EDDN watermark the snapshot was taken at, so a
reader hardcodes nothing. The whole file is a few hundred kilobytes and is
resident on every client; every walk plans on it and it alone.

A cell's position in this file is its **ordinal**. Every column file is
indexed by it.

## What a cell owns

**`cells/LL-<morton>.g<gen>.bin`** is one cell's slice, fetched on demand when
the walk says the cell's systems separate on screen. It has two parts and no
header. The boundary between them is known from the index record's slice
length and from counting a flag.

**A point** is one system, fixed width, in brightness order. It carries what
every system has, which is everything a mark needs to be drawn, sized,
coloured and filtered without looking anything up:

```
address                   the id64
position                  three integers in 1/32 ly, exact for Elite
magnitude                 the combined visual absolute magnitude, fixed point
temperature               a colour bucket, six log-spaced steps
star class                the sixteen-code table below, plus flags:
                          scoopable companion, populated, has station
political word            allegiance, government, security as small codes,
                          with a code for unknown distinct from the game's None
population                log-scaled, for size and ordering
body count, belt count    from the discovery scan; a sentinel for never scanned
reach                     how far the system extends from its arrival star
updated at                when it was last heard from
```

Every point field has an aggregate shadow: position in the moments, magnitude
and temperature in the flux buckets, class in the stellar counts, the political
word in the political histograms. The point is the atom; the columns are sums
of it.

**A trailer** follows the points, one per point whose populated flag is set, in
the same order. It carries what only a populated system has and a panel wants
exact:

```
population                exact
economies                 primary and secondary
power                     the controlling power, if any
factions                  up to eight ids
```

A system on the feed changes one cell file and the aggregates of that cell's
ancestors. A political change is the same size of write as a star scan.

**The star-class codes**, shared with EDGX:

| Code | Class | Code | Class |
|---:|---|---:|---|
| 0 | Unknown | 8 | L |
| 1 | O | 9 | T |
| 2 | B | 10 | Y |
| 3 | A | 11 | Protostar: T Tauri, Herbig Ae/Be |
| 4 | F | 12 | Exotic: Wolf-Rayet, carbon, MS, S |
| 5 | G | 13 | White dwarf |
| 6 | K | 14 | Neutron |
| 7 | M | 15 | Black hole |

## Aggregations

An **aggregation** is one way of summarising a subtree so that a cell can
stand for everything beneath it. Each has the same four parts: a weight, the
position moments in that weight, a prune key, and buckets to colour by. The
moments give the centroid and spread that decide when a cell splits into its
children and how the two cross-fade. The buckets tint a splat. The prune key
says whether a subtree can matter to this view at all, and it is the one part
that does not add: a minimum magnitude, a maximum population, a set of flags.

The rule every aggregation obeys is exact composition. A region drawn coarse
and the same region drawn fine integrate to the same totals, and a splat over a
loaded slice draws the total less the slice's own contribution, so nothing is
counted twice and nothing falls between the marks and the glow. That is why
every bucket is a sum or a count, and why every prune key is answered on the
total and never on a residual.

**`agg/<column>.g<shape>.bin`** is one aggregation, one fixed record per cell
in ordinal order, with a header naming the column, its version, and the shape
generation it was built against.

| Column | Weight | Prune key | Buckets |
|---|---|---|---|
| density | count | count > 0 | none |
| light | flux | brightest magnitude | flux per temperature bucket |
| population | population | largest population | populated count |
| political | count | any system in the filter's buckets | allegiance, government, security, population per allegiance |
| recency | count | newest update | age buckets |
| stellar | count | any neutron, white dwarf, scoopable, station | count per star class |
| explored | count | any scanned | body sum, scanned count |
| markets | station count | any station | stations per commodity, in dictionary order |

A client loads the columns its open views ask for. The map by allegiance loads
density and political. The realistic sky loads light. The router loads
stellar. Adding a view is adding one column, with no change to the tree, the
points, or the walk.

## Beside the tree

**`names/`** is every system's name and position, chunked so a publish
rewrites the one chunk new systems landed in. It is resident whole because a
search reaches any name and a route steps between any two positions; the
router's graph is this table. A name-prefix ordering is derived at load, not
published.

**`factions.bin`** is every faction's id and name and the sorted addresses it
holds. A filter builds a set from it and tests points against the set. The
reverse map, which factions a system has, is read off the trailer.

**`stations/<market>.g<gen>.bin`** is one station: its identity, owning
system, type, pads and arrival distance; its commodity board with prices,
supply, demand and observation time; its outfitting and shipyard lists; and its
confiscated commodities. Commodities, modules and ships are named by their
index into the dictionaries. Presence, which stations sell what, is the
markets column and is aggregated; the board itself is here and is fetched when
a station is opened, or fetched whole by a client that wants every board.

**`dict/`** holds the three dictionaries, each a small table of id, symbol and
display name in a fixed canonical order. The markets column's buckets follow
`dict/commodities` position for position.

**`bodies/<address>.bin`** is one system's stars, planets and barycenters with
their orbits. A system is one sphere from anywhere but inside it, so this is
fetched on the way in, one system at a time, and never held.

**`manifest.g<gen>.json`** lists every file at one generation with its byte
length and SHA-256, the EDDN watermark, and a minimum reader version per file
class. A file absent from the manifest is absent as of the watermark.

**`pack/<class>.g<gen>.ebex.zst`** is optional and derived: every file of one
class at one generation concatenated in ordinal or id order, with a small
directory of offsets, in the EBEX container and compressed as one zstd frame.
It exists for a client that wants everything at once and would otherwise make
tens of thousands of requests. Nothing is in a pack that is not in the
directory.

## Reading it

A client holds the index and the columns its views want. Every frame the walk
turns the eye's position into the cells whose slices should draw as marks and
the cells that should splat, as a pure function of where the eye is. Drawing
takes what is needed and resident, loading fetches what is needed and absent,
prefetching fetches what the extrapolated eye will need, and eviction drops
what is resident and no longer needed.

A client that wants the whole galaxy at once fetches every cell file at one
content generation, or the cells pack, strips the trailers and concatenates in
ordinal order. That is a flat star array, and `index.bin` is its cell table.
On the next poll it fetches only the cells whose generation moved and
re-concatenates locally. Nothing is written twice to serve both clients, and
the flat array is as fresh as the streamed one.

---

# Mapping the EDDA formats

The three EDDA binary formats each land on one part of the directory. EDGX is
the tree and its cell files seen flat. AGG1 is the tree plus one column. EBEX
is the station and dictionary files, with its container kept as the pack
format.

## EDGX to the layout

| EDGX v2 / v3 | New layout |
|---|---|
| `stars.bin` header: magic, version, record count, `cell_ly` | `index.bin` header: magic, version, cell count, root edge and centre, max level, both generations, EDDN watermark |
| `stars.bin` records, sorted by cell key | `cells/*` at one generation, trailers stripped, concatenated in ordinal order; served as `pack/cells` |
| record: x, y, z f32 | point position, i32 x3 in 1/32 ly, or f32 if agreed |
| record: class nibble, flag bits | point class byte: same sixteen codes; flags scoopable companion, populated, has station |
| record: id64 | point address |
| record: name length, u40 name offset | not on the point; `names/` chunks, offsets derived by the pack writer |
| `cells.bin`: key, start, count | `index.bin` record: morton, rank_lo, slice length, plus level |
| packed 21-bit-per-axis cell key | morton at a level; same interleave, one binary search per level |
| `byname.bin` permutation | derived at load from `names/`, or emitted into the names pack |
| v3 companion distance buckets | one byte on the point, sourced from bodies; not present today |
| v3 `boost250` sub-index | the tree pruned by the `stellar` column; no separate sub-index |

What changes for an EDGX reader: a level column in the cell table, so a radius
query does one binary search per level; a directory of cell files rather than
one array, or a local concatenation of them; and the record's fields.
The query itself, cube-sphere pruning then a scan of the hit range, is
unchanged.

## AGG1 to the layout

| AGG1 | New layout |
|---|---|
| header: level count, per-level shift and node count | `index.bin` header and the level field on each cell |
| one node per occupied cell, leaves parallel to `cells.bin` | one `index.bin` record per occupied cell; columns parallel by ordinal |
| levels built by dropping 3 Morton bits | the tree's parent links, adaptive depth |
| node: star_count u16 saturating | cell count u64, exact |
| node flags: any scoopable, any neutron, any white dwarf | `agg/stellar` prune key |
| node flag: any station, reserved | `agg/stellar` prune key, set; also `agg/markets` station count |
| node: best boost class | derived from the neutron and white dwarf bits |
| derived locally, never published | published; a client that prefers to derive it still can |
| query: prune by flags and cube-sphere, scan leaf records | tree descent with a column predicate, then read the cell files |

## EBEX to the layout

| EBEX section | Record fields | New layout |
|---|---|---|
| header | snapshot sequence, creation time, EDDN watermark | content generation, `manifest.g<gen>.json`, watermark in the index header |
| 1 Systems, inhabited | address, x y z, population, observation time | point address, position, updated at; trailer population |
| | name string id | `names/` |
| | security, allegiance string ids | point political word |
| | power string id | trailer power |
| | flags | point flags and trailer |
| 16 Stars | address, main-star class, scoopable, observation time | point class byte and updated at |
| 2 Stations | market id, owning system, name, service flags, per-service observation times | `stations/<market>` header |
| 10 Station details | pads, arrival distance, type | `stations/<market>` header |
| 5 Markets | station, commodity, buy, sell, demand, supply, observation time | `stations/<market>` board rows, commodity by dictionary index |
| 11 Confiscation pairs | station, commodity | `stations/<market>` confiscation list |
| 7 Outfitting, 9 Shipyards | station to module or ship, snapshot directory | `stations/<market>` outfitting and shipyard lists |
| 4 Commodities, 6 Modules, 8 Ships | dictionaries | `dict/*.bin`, same canonical order |
| absent | | `factions.bin`, `bodies/`, every `agg/` column, trailer factions and economies |
| container: header, section directory, one zstd frame | | `pack/<class>.g<gen>.ebex.zst`, sections replaced by file classes |
| full baseline, absence means absent | | the manifest for one generation is the baseline; a file missing from it is absent |
| per-station stale skip on hydrate | | newest-wins upsert in the builder's database, once |
| manifest: product, version, schema, files with bytes and sha256 | | `manifest.g<gen>.json`, one product per file class |
| resumable range download | | applies to packs, not to cell or station files |

EBEX survives as the container for packs and not as a data model. Its header,
section directory, watermark and zstd frame are kept; its record schemas are
replaced by the directory's, one section per file class, so a section is the
concatenation of that class's files at one generation. A hydrating client
bootstraps from the packs and stays current from the directory, fetching only
the station and cell files whose generation moved. EBEX alone could not do
that, being a full baseline with no deltas.

## What the EDDA products become

| Product | Reads |
|---|---|
| `routing` | `index.bin`, `cells/`, `names/`, `agg/stellar`; or `pack/cells` and `pack/names` |
| `stars` | the point's class and updated-at fields, already in `cells/` |
| `community` | `stations/`, `dict/`, points and trailers; or `pack/stations` and `pack/dict` |
| `bootstrap` | the manifest and the packs at one generation |

A client that hydrates fetches the packs once and then only the files whose
generation moved. A client that streams never touches a pack. Both read the
same records, written once by one builder.

## What the merge adds to galos

- The EDGX star-class code table and the scoopable-companion flag on the point.
- A Powerplay field in the trailer.
- The EDDN watermark in the index header.
- `stations/`, `dict/`, the manifest, and the optional pack writer.
- Station files dirtied by market messages in the watch, the most frequent
  write on the feed.

## Open with the edda side

1. Reading the directory directly, or through a locally concatenated mirror.
2. Position representation: f32 against 1/32 ly integers.
3. Whether the name blob and by-name permutation are published or derived.
4. Whether AGG1's flag set is exactly the stellar column's prune key, or wants
   more bits.
5. A companion distance bucket on the point, if v3 routing needs it.
