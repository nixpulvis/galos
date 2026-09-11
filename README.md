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
EDDN. A commander's own is written to a directory of journal files on their own
machine, and [`galos_journal`](./galos_journal) reads that directory into the
same index vocabulary — no database — so the map can draw both at once and turn
either off.

[ARCHITECTURE.md](./ARCHITECTURE.md) is the map of it: what each of the nine
crates is for, which way the data runs, what crosses the seam between the
database and the index, and which module header to open for a given decision.

Use `galos-sync` to populate the database and `galos` to perform basic queries
from the CLI. The map has its own [README](./galos_map/README.md) for the mouse
and keys.

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

Every binary and `cargo sqlx` read the connection from `DATABASE_URL`, taken
from the environment or a `.env` file in the working directory or one above it.

```sh
# .env
DATABASE_URL=postgresql://postgres@localhost/galos_development
```

## Database Setup

```sh
cargo install sqlx-cli --locked --version "$(cargo pkgid sqlx | sed 's/.*@//')"

# Create the database and run the migrations.
cargo sqlx database setup --source galos_db/migrations/
```

Managing migrations and resetting the database live with the database crate,
[`galos_db`](./galos_db).

To build or test without a database, use the cached query metadata in `.sqlx/`:

```sh
SQLX_OFFLINE=true cargo build
```

## Running

`galos-sync` moves the galaxy from its publishers into somewhere it can be
read, in one process. Sources are named with `--from`, which repeats:
`eddn`, `journal=PATH`, `edsm=PATH`, `edsm-api=NAME` and `eddb=PATH`. Sinks
are named with `--db` and `--index DIR`: Postgres, and a `galos_index`
directory the map draws from with no server at all. Naming neither is
refused; naming both reads each publisher once into the pair.

```sh
# Populate the database. `galos-sync --help` lists the flags.
cargo run --release --bin galos-sync -- --from eddn --db
cargo run --release --bin galos-sync -- --from edsm=systems.json --db
cargo run --release --bin galos-sync -- --from journal="$JOURNAL" --db

# Keep reading the journal while the game writes it.
cargo run --release --bin galos-sync -- --from journal="$JOURNAL" --db --watch

# Build the index out of the database, or follow it and republish as it moves.
cargo run --release --bin galos-sync -- --db --index .galos_index
cargo run --release --bin galos-sync -- --db --index .galos_index --watch 5
cargo run --release --bin galos-sync -- --db --index .galos_index --only reaches

# The same sources into an index directory instead, with no database anywhere.
cargo run --release --bin galos-sync -- \
    --from journal="$JOURNAL" --index .galos_journal_index --watch
cargo run --release --bin galos-sync -- --from eddn --index .galos_index

# Or both at once: one read of the feed, written to the database and to a
# directory, with the directory brought level with the database first.
cargo run --release --bin galos-sync -- --from eddn --db --index .galos_index
```

`$JOURNAL` is where the game writes its logs, typically
`~/Saved Games/Frontier Developments/Elite Dangerous`.

The index resumes from `<dir>.checkpoint` beside the directory it writes,
unless `--checkpoint` names one. The resume point holds the whole editable
tree of that directory, and says which derivation wrote it: a directory built
from the database and one written from a feed are not the same artefact, and
neither is resumed onto the other's work.

With `--db --index` the directory is brought level with the database before
it takes live events, and everything read in the meantime is buffered and
applied after — the overlap is duplicate work, and applying an event twice
lands exactly where applying it once did. Without `--db` the
directory is whatever the feed has said since somebody started it. One
process per directory: the run takes `<dir>.lock` and a second is refused.

Ctrl-C asks the run to stop rather than killing it, so the last publish, the
whole directory and its resume point are written before it exits. A second
Ctrl-C stops it where it stands.

```sh
# Query from the CLI.
cargo run --bin galos -- --help

# Open the 3D map. See galos_map/README.md. `GALOS_INDEX_DIR` names the
# directory it draws from, `.galos_index` under the working directory
# unless it is set.
cargo run --release -p galos_map
GALOS_INDEX_DIR=/srv/galos_index cargo run --release -p galos_map

# And with the commander's own journal drawn over the published index. `J`
# takes that layer off and puts it back while the map runs.
GALOS_JOURNAL_DIR="$JOURNAL" cargo run --release -p galos_map

# What a journal directory holds, without writing anything anywhere.
cargo run -p galos_journal -- info "$JOURNAL"
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
