# Market data UI

A standalone egui app for reading what the galaxy trades, written so the
drawing moves into `galos_map` once it earns a place there.

## Shape

- `galos_market` is the whole of it, and nothing outside that folder knows it
  exists. A workspace of its own: the root manifest lists no member for it,
  the root lock file has never heard of eframe, and the tracked `.sqlx` holds
  no query of its own. It is a prototype and the repository is not carrying it
  yet, so the other work here stays committable.
- An `eframe` shell around draw functions that take `&mut egui::Ui` and the
  state they read. Nothing in the drawing knows what a window is.
- egui 0.34, which is what `bevy_egui` 0.40 draws the map's chrome with. The
  version is pinned to the map's on purpose, so what is written here compiles
  there unchanged.
- `Markets::draw` takes a `Ui` and lays its panes out with `show_inside`, so
  the three of them hold together inside whatever they are given: a window of
  their own here, and a window or a panel of the map's later. Every top level
  `Panel::show` is deprecated in this egui anyway, which `galos_map::ui` ran
  into from the other direction.

## What it shows

Three panes, left to right, each answering the one beside it.

- **Commodities**, every name traded anywhere (398 of them), with how many
  markets carry it and what they charge on average. A box filters the list.
- **Quotes**, everywhere the picked commodity trades: system, station, what
  the station sells it for and holds in stock, what it pays and how much it
  wants. Sortable by any column, and narrowable to markets that will actually
  sell to you or buy from you.
- **Board**, everything the picked market trades, which is the same rows read
  the other way round. Picking a commodity there moves the left pane to it.

Prices read against the galactic mean: under it in green where you are
buying, over it in green where you are selling.

## Finding a trade

A fourth thing the pane does, in `trade.rs` and `trades.rs`. A trade is one
commodity, a market that sells it, and one that buys it for more. The whole
cross product is billions of pairs, so it is never built. Which question gets
asked depends on how much has been pinned:

- **Nothing pinned** is the galaxy board. Cheap because the buy side is only
  278k of 2.5M rows, and because a commodity's best selling price anywhere is
  one number: four hundred of those bound every buyable row without any pair
  being enumerated. 1.3s.
- **A system typed in the `near` box** is the question a trader actually has:
  I am here, what is worth carrying around here. Both ends inside the radius,
  so the whole run is local. 300ms at 25 ly, 2.4s at 100.
- **A source pinned** narrows that to one station: what this market sells
  that is worth more within N light years. 330ms.
- **Both ends pinned** is not a search. Two markets, what each would pay the
  other, both directions. 12ms.

The radius on a `near` search is measured from the system, not along the run,
so two markets on opposite edges of one bubble are up to twice it apart. Each
row carries what it would actually cost to fly, which is why that column is
there.

Pins are set from the board pane. Clicking a row of a search pins both its
ends, which turns it into the comparison of the pair it just proposed.

Distance is not a filter added at the end. Unbounded, the best margin from a
given market was 22,000 ly away; bounded to 100 ly it was 16 ly away and
worth 39M a run. Rows carry the light years between their ends, sort by it,
and go red past 500. Profit per minute waits for the map's routing, which is
where jumps rather than straight lines can be had.

`hold` turns a margin into what a run is actually worth. It is what stops the
board being a list of four-ton fortunes, and it divides rows already fetched,
so it asks the database nothing.

### What makes a trade real

The filters are not polish, they are what makes the number mean anything.
Unfiltered, every one of the top margins is a Thargoid tissue sample at fifty
million a ton, one ton of it, on a fleet carrier, priced by its owner.

Carriers are found by market id, not by name or type. A market message names
a station without saying what kind it is, so 12,936 stations on record have no
type at all. Frontier gives carriers their own id range, and on this database
it separates them exactly: all 1,269 known carriers inside, everything known
to be anything else outside, 1,510 caught in all. The callsign pattern that
looks like it should work misses 175 of them, which were exactly the ones at
the top of the board.

## The database

`galos_market::market` reads the same `commodities` and `markets` tables the
EDDN listener writes through `galos_db`, with a pool of its own. Not through
`galos_db` because that crate's pool is private to it, so a crate outside
cannot borrow the connection, and adding a method to it would put this
prototype's files in a folder that is tracked. Reads only; nothing here
writes a row.

- `Summary::fetch_all` groups `commodities` by name. Full aggregate over 2.7M
  rows, ~200ms warm, asked once at startup.
- `Quote::fetch_all` joins `commodities` to `markets` for one name. Up to
  ~11k rows for the common ones, which the table draws virtualised.
- `Commodity::fetch_all` reads one market's whole board.

The queries are checked when they run rather than when they compile.
`sqlx::query!` would want its offline data written into the repository's
`.sqlx`, which is tracked. That trade is worth revisiting the moment this
earns a place in the map: the reads are then what moves to
`galos_db::markets` first, and the macros come back with them.

`commodities` is keyed `(market_id, name)` and has no index on `name` alone,
so every search above is a sequential scan of 2.5M rows. That is where the
remaining second of the galaxy board goes. Two partial indexes would answer
it, matching the two halves a trade is made of:

```sql
CREATE INDEX ON commodities (name) WHERE stock > 0 AND buy_price > 0;
CREATE INDEX ON commodities (name, sell_price DESC) WHERE demand > 0;
```

Not added, because a migration lives in `galos_db/migrations`, which is
tracked, and this crate is arranged to leave the repository alone. Apply them
by hand to a development database if the wait starts to matter. Nothing here
depends on them.

Two things already learned from the plans, neither of which needed an index
to find:

`DISTINCT ON (name)` over a million rows sorts them by `(name, price)` and
spills 20MB to disk, which took 14 seconds. `max(sell_price) GROUP BY name`
says the same thing with a hash and no order at all, then a second pass finds
which market quoted it. 14s to 1s.

Joining what a bubble sells against what it buys, commodity by commodity, is
a few hundred thousand pairs and took 16 seconds. None of them are needed:
the best run for a commodity is the cheapest anyone sells it for against the
dearest anyone pays, and each of those is one pass. 16s to 300ms. Carrying a
PostGIS geometry through the middle of that cost most of what was left, so
the markets in reach are collected as bare ids and the two positions that
matter are looked up at the end.

The `near` search is where an index would earn most: it reads a quarter of
`commodities` twice, once per side of the run, and that is the whole of the
remaining second at a wide radius.

Buying and selling are the game's way round: `buy_price` is what the station
charges and wants `stock` to be there, `sell_price` is what it pays and wants
`demand`.

## Async

`galos_market::db` holds an `Ask` and an `Answer` and a channel between them,
with `async_std` running the queries off the UI thread. The map already polls
`bevy::tasks::Task`s the same way in `systems::fetch`, so porting is a matter
of swapping the spawner, not the questions.

## What it says when something is wrong

`main` installs `env_logger` at warn. Egui reports what it thinks is wrong
through `log` and nowhere else, so without a logger there is no sign anything
was said. `RUST_LOG` overrides it.

One of egui's own checks is turned off, in `quiet_scrolled_tables`: a
virtualised table hands one row's rectangle to the next every time it
scrolls, which reads to `warn_if_rect_changes_id` as widgets swapping
identities, and it draws a red border round most of the rows on screen. Egui
knows, and the fix is open upstream. See `ISSUE-egui-table-id-warnings.md`.

## Testing

Everything runs from inside the folder, since the crate is not a member of
the workspace: `cd galos_market`, then `cargo run`, `cargo test`.

The drawing gets egui tests in the map's style: run a bare `egui::Context`,
tessellate what was painted, read the text back off the shapes.

`examples/smoke.rs` drives the whole of it without a window, which is what
says the parts hold together: real database, real answers folded in, real
panes painted, and what they say printed. It is also the only thing that
checks the SQL, which no longer fails to compile when it is wrong.

A bare context is not a window, and there is at least one thing it cannot
show: driving it with wheel events scrolls the table but never reproduces the
borders above, because no row lands exactly where a row stood before. What
that costs is written down in the issue note rather than papered over with a
test that passes for the wrong reason.

## Not done

- Nothing writes. The UI reads what the EDDN listener has already recorded.
- No routing or profit-per-ton between two markets. The rows to build that
  from are all here, but a trade finder is its own piece of work.
- Commodity names arrive lowercase and unseparated (`fruitandvegetables`).
  They are shown as they are stored rather than guessed apart.
