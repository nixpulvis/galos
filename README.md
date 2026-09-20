# Galos
-----
[![CI](https://github.com/nixpulvis/galos/actions/workflows/ci.yml/badge.svg)](https://github.com/nixpulvis/galos/actions/workflows/ci.yml)

Somewhere between reality and the space/flight sim E:D.

Elite's galaxy arrives as events and is kept in Postgres. A builder derives one
spatial index from it, and the [`galos-map`](./galos_map) program draws the
galaxy from that index alone, with no database. Beside them sits the sky as it
is measured from Earth, read from published star catalogs and compared against
what the game says.

The galaxy the index is built from is everyone else's game, forwarded through
EDDN. A commander's own is written to a directory of journal files on their
own machine, and `galos ingest` reads both into one place: naming the
feed and the journal together merges the world and the commander as they are
written, so the map draws the pair out of one directory with no database in
the path.

[doc/ARCHITECTURE.md](./doc/ARCHITECTURE.md) is the map of it: what each of the nine
crates is for, which way the data runs, what crosses the seam between the
database and the index, and which module header to open for a given decision.

`galos` is one command: `galos ingest` fills the database, the directory the
map draws, or both from one reading; `galos index` and `galos db` are what is
asked of each store once it holds something, and `galos search` and `galos
route` answer basic queries from the CLI. The map has its own
[README](./galos_map/README.md) for the mouse and keys.

## Prerequisites

### Rust

Install the toolchain with [rustup](https://rustup.rs). The workspace tracks
`stable` (see `rust-toolchain`), so a plain install is enough.

### Submodules

`elite_journal`, `eddn`, `eddb`, and `edsm` are submodules, pulled into the
build through `[patch.crates-io]`. They must be checked out or the patches stop
applying and the published copies come down beside the working tree.

```sh
git submodule update --init
```

### System libraries

The database columns are PostGIS geometries, so a PostgreSQL with PostGIS is
required. Building galos_map on Linux additionally needs the ALSA and udev
development headers.

## Configuration

`galos` and `cargo sqlx` read the connection from `DATABASE_URL`, taken
from the environment or a `.env` file in the working directory or one above
it. The `db` verbs, the `search` and `route` queries, and a `galos ingest`
run that names `--db` or reads `--from database` are what want it; the
`index` group is inspection and repair of a directory and never touches it,
and a copy built without the `db` feature has neither of those two flags nor
a client to open a connection with.

```sh
# .env
DATABASE_URL=postgresql://postgres@localhost/galos_development
```

## Database Setup

`galos db migrate` carries the migrations inside it and runs whichever the
database has not, so a server needs the binary and nothing else:

```sh
createdb galos_development

# `SQLX_OFFLINE` for this one build: the checked query macros verify
# themselves against `DATABASE_URL` as they compile, and the database this
# is about to migrate has no schema for them to check against yet. `.sqlx/`
# is the cached metadata they use instead.
SQLX_OFFLINE=true cargo run --bin galos -- db migrate

# The version it left, and what is in there.
cargo run --bin galos -- db status
```

`sqlx-cli` is for *writing* a migration rather than running one — `cargo
sqlx migrate add`, and `cargo sqlx prepare` to refresh the cached query
metadata in `.sqlx/` after changing a checked query:

```sh
cargo install sqlx-cli --locked --version "$(cargo pkgid sqlx | sed 's/.*@//')"
```

Resetting the database, and the template a test builds its own from, live
with the database crate, [`galos_db`](./galos_db).

To build or test without a database, use that cached metadata:

```sh
SQLX_OFFLINE=true cargo build

# Or build with no database client in it at all: `ingest --index` and the
# `index` verbs, which is what a machine that only serves the map wants.
cargo build --bin galos --no-default-features
```

## Running

One verb fills; two groups ask. `galos ingest` reads a publisher into
Postgres (`--db`), into an index directory (`--index [DIR]`), or into both at
once. `galos db` and `galos index` are what is asked *of* each store once it
holds something. Sources are named with `--from`, which repeats: `eddn`,
`spool=DIR`, `journal=PATH`, `edsm=PATH`, `edsm-api=NAME`, `eddb=PATH`,
`spansh=PATH`, and `database`.

**Naming both sinks is the point of the pair.** One reading of one publisher
into the two stores is, over EDDN, one subscription rather than two carrying
the same galaxy — and it is the run an operator keeping both current wants.
That used to be two processes side by side with a handoff flag to start the
directory level with the rows instead of level with whatever the feed had
mentioned since; it is now `galos ingest --from eddn --db --index`, and the
handoff is simply what that run does before it goes live.

The sinks are still not interchangeable, and a machine that only serves the
map has no Postgres on it: built with `--no-default-features` the binary is
`ingest --index` and the `index` group, and carries no database client at
all.

`galos db` has `status migrate verify catalog stats`; `galos index` has
`status diff pack sweep verify migrate sectors`. Neither of them fills
anything. What used to be two fill verbs — a live one and a regional one, and
which to reach for was a question about memory — is one verb that answers the
memory question from what the run *is*. The index sink holds a live tree,
because something may be reading the directory while it is written; the
regional build holds one region at a time and publishes nothing until it is
done, which is the only way a two hundred million system dump fits. A finite
dump, an `--index` and no `--db` has nothing to fan out to and no reader
waiting on a half-written directory, so it takes the regional route; anything
else holds the tree. **The run says which one it took when it starts**,
because it is the difference between minutes and hours and between 7 GB and
200 GB.

```sh
# Populate the database. `galos ingest --help` lists the flags.
cargo run --release --bin galos -- ingest --from eddn --db
cargo run --release --bin galos -- ingest --from edsm=systems.json --db
cargo run --release --bin galos -- ingest --from journal="$JOURNAL" --db

# Keep reading the journal while the game writes it.
cargo run --release --bin galos -- \
    ingest --from journal="$JOURNAL" --db --watch

# Derive the index from the rows instead, or follow the rows and republish
# as they move. `--index` is `.galos_index` wherever DIR is left off.
cargo run --release --bin galos -- ingest --from database --index
cargo run --release --bin galos -- ingest --from database --index --watch 5
cargo run --release --bin galos -- \
    ingest --from database --index --only reaches

# The same publishers into an index directory, with no database anywhere.
cargo run --release --bin galos -- ingest --from eddn --index
cargo run --release --bin galos -- \
    ingest --from journal="$JOURNAL" --index --watch

# Or both publishers into one directory: everybody else's galaxy and this
# commander's, merged as they are written, which is what the map draws.
cargo run --release --bin galos -- \
    ingest --from eddn --from journal="$JOURNAL" --index --watch

# Or both stores from one read of the feed — one process, one subscription.
# The directory is brought level with the rows first, then goes live.
cargo run --release --bin galos -- ingest --from eddn --db --index

# A galaxy-sized dump into a directory takes the regional route: one region
# held at a time, rather than a tree of every system read so far. No flag
# asks for it and the run says when it does it.
cargo run --release --bin galos -- \
    ingest --from spansh=galaxy.json --index .galos_index

# The same dump into the database, which is an import that can be re-run from
# its source: --bulk leaves commits unflushed, and --shard I/N takes one
# process's share of a file so N of them cover it exactly once between them.
cargo run --release --bin galos -- \
    ingest --from spansh=galaxy.json --db --bulk
for i in 0 1 2 3; do
    cargo run --release --bin galos -- \
        ingest --from spansh=galaxy.json --db --bulk --shard "$i/4" &
done; wait
```

`$JOURNAL` is where the game writes its logs, typically
`~/Saved Games/Frontier Developments/Elite Dangerous`.

The index resumes from `<dir>.checkpoint` beside the directory it writes,
unless `--checkpoint` names one. The resume point holds the whole editable
tree of that directory, and says which derivation wrote it: a directory built
from the database and one written from a feed are not the same artefact, and
neither is resumed onto the other's work.

With `galos ingest --db --index` the directory is brought level with the
database before it takes live events, and everything read in the meantime is
buffered and applied after — the overlap is duplicate work, and applying an
event twice lands exactly where applying it once did. With `--index` alone
the directory is whatever the feed has said since somebody started it. One
process per directory: a run writing one takes `<dir>.lock` and a second is
refused.

Ctrl-C asks the run to stop rather than killing it, so the last publish, the
whole directory and its resume point are written before it exits. A first
full build has nothing published to keep: asked to stop, it leaves the
directory as it found it and the next run builds it again. A second Ctrl-C
stops it where it stands.

```sh
# Query from the CLI.
cargo run --bin galos -- --help

# Open the 3D map. See galos_map/README.md. `--index DIR` or GALOS_INDEX
# names the directory it draws from, `.galos_index` under the working
# directory with neither.
cargo run --release -p galos_map
cargo run --release -p galos_map -- --index /srv/galos_index
GALOS_INDEX=/srv/galos_index cargo run --release -p galos_map

# The same, built to be profiled: bevy's spans and the map's, shipped to a
# Tracy profiler of the version the client speaks. See galos_map/README.md.
cargo run --release -p galos_map --features tracy

# What a built index directory holds, without writing anything anywhere.
cargo run --bin galos -- index status .galos_index
```

`RUST_LOG` selects what a run logs (e.g. `RUST_LOG=debug`), info and above
by default.

## Database Backup and Restore

```sh
# Create a backup.
pg_dump -Fc galos_development > latest.dump

# Restore from backup.
pg_restore -Cd postgres < latest.dump
```
