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
own machine, and `galos-index ingest` reads both into one place: naming the
feed and the journal together merges the world and the commander as they are
written, so the map draws the pair out of one directory with no database in
the path.

[doc/ARCHITECTURE.md](./doc/ARCHITECTURE.md) is the map of it: what each of the nine
crates is for, which way the data runs, what crosses the seam between the
database and the index, and which module header to open for a given decision.

Use `galos-db` to populate the database, `galos-index` to fill the directory
the map draws, and `galos` to perform basic queries from the CLI. The map has
its own [README](./galos_map/README.md) for the mouse and keys.

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

`galos`, `galos-db` and `cargo sqlx` read the connection from
`DATABASE_URL`, taken from the environment or a `.env` file in the working
directory or one above it. `galos-index` reads it only for the two things
that are about the other store — `build --from database` and `ingest
--catch-up` — and a copy built without the `db` feature has no such flag
and no client to open one with.

```sh
# .env
DATABASE_URL=postgresql://postgres@localhost/galos_development
```

## Database Setup

`galos-db migrate` carries the migrations inside it and runs whichever the
database has not, so a server needs the binary and nothing else:

```sh
createdb galos_development

# `SQLX_OFFLINE` for this one build: the checked query macros verify
# themselves against `DATABASE_URL` as they compile, and the database this
# is about to migrate has no schema for them to check against yet. `.sqlx/`
# is the cached metadata they use instead.
SQLX_OFFLINE=true cargo run --bin galos-db -- migrate

cargo run --bin galos-db -- status   # the version it left, and what is in there
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

# Or build the index tool with no database client in it at all, which is
# what a machine that only serves the map wants.
cargo build --bin galos-index --no-default-features
```

## Running

Two tools, one for each store. `galos-db` reads the publishers into Postgres;
`galos-index` fills and repairs the directory the map draws. Both name their
sources with `--from`, which repeats: `eddn`, `spool=DIR`, `journal=PATH`,
`edsm=PATH`, `edsm-api=NAME`, `eddb=PATH` and `spansh=PATH`.

Two programs rather than two sink flags of one, because the sinks are not
interchangeable and a machine that only serves the map has no Postgres on it:
`galos-index` built with `--no-default-features` carries no database client
at all. Filling both stores is the two runs side by side, each reading a
publisher for itself.

`galos-db` has `status ingest migrate verify catalog stats`; `galos-index`
has `status ingest build migrate verify sweep pack diff sectors`. Into a
directory, `ingest` and `build` are both "fill this", and which one to reach
for is a question about memory: `ingest` holds a live tree, because something
may be reading the directory while it is written, and `build` holds one
region at a time and publishes nothing until it is done, which is the only
way a two hundred million system dump fits.

```sh
# Populate the database. `galos-db ingest --help` lists the flags.
cargo run --release --bin galos-db -- ingest --from eddn
cargo run --release --bin galos-db -- ingest --from edsm=systems.json
cargo run --release --bin galos-db -- ingest --from journal="$JOURNAL"

# Keep reading the journal while the game writes it.
cargo run --release --bin galos-db -- ingest --from journal="$JOURNAL" --watch

# Derive the index from the database, or follow the rows and republish as
# they move. `--dir` is `.galos_index` wherever it is left off.
cargo run --release --bin galos-index -- build --from database
cargo run --release --bin galos-index -- build --from database --watch 5
cargo run --release --bin galos-index -- build --from database --only reaches

# The same publishers into an index directory instead, with no database
# anywhere.
cargo run --release --bin galos-index -- ingest --from eddn
cargo run --release --bin galos-index -- \
    ingest --from journal="$JOURNAL" --watch

# Or both publishers into one directory: everybody else's galaxy and this
# commander's, merged as they are written, which is what the map draws.
cargo run --release --bin galos-index -- \
    ingest --from eddn --from journal="$JOURNAL" --watch

# Or both stores at once, which is the two runs beside each other, one read
# of the feed apiece. `--catch-up` starts the directory level with the
# database rather than with whatever the feed has mentioned since.
cargo run --release --bin galos-db -- ingest --from eddn &
cargo run --release --bin galos-index -- ingest --from eddn --catch-up &

# A galaxy-sized dump into the index is `build` and not `ingest`: one region
# held at a time, rather than a tree of every system read so far.
cargo run --release --bin galos-index -- \
    build --from spansh=galaxy.json --dir .galos_index

# The same dump into the database, which is an import that can be re-run from
# its source: --bulk leaves commits unflushed, and --shard I/N takes one
# process's share of a file so N of them cover it exactly once between them.
cargo run --release --bin galos-db -- \
    ingest --from spansh=galaxy.json --bulk
for i in 0 1 2 3; do
    cargo run --release --bin galos-db -- \
        ingest --from spansh=galaxy.json --bulk --shard "$i/4" &
done; wait
```

`$JOURNAL` is where the game writes its logs, typically
`~/Saved Games/Frontier Developments/Elite Dangerous`.

The index resumes from `<dir>.checkpoint` beside the directory it writes,
unless `--checkpoint` names one. The resume point holds the whole editable
tree of that directory, and says which derivation wrote it: a directory built
from the database and one written from a feed are not the same artefact, and
neither is resumed onto the other's work.

With `galos-index ingest --catch-up` the directory is brought level with the
database before it takes live events, and everything read in the meantime is
buffered and applied after — the overlap is duplicate work, and applying an
event twice lands exactly where applying it once did. Without it the
directory is whatever the feed has said since somebody started it. One
process per directory: an index run takes `<dir>.lock` and a second is
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
cargo run --bin galos-index -- status .galos_index
```

`RUST_LOG` selects what the tools log (e.g. `RUST_LOG=debug`), info and above
by default.

## Database Backup and Restore

```sh
# Create a backup.
pg_dump -Fc galos_development > latest.dump

# Restore from backup.
pg_restore -Cd postgres < latest.dump
```
