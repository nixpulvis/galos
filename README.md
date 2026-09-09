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

```sh
# Populate the database. `galos-sync --help` lists the sources.
cargo run --release --bin galos-sync -- eddn      # live feed from EDDN
cargo run --release --bin galos-sync -- edsm      # EDSM nightly dumps
cargo run --release --bin galos-sync -- journal   # local journal files

# Query from the CLI.
cargo run --bin galos -- --help

# Open the 3D map. See galos_map/README.md.
cargo run --release -p galos_map

# And with the commander's own journal drawn over the published index. `J`
# takes that layer off and puts it back while the map runs.
GALOS_JOURNAL_DIR="$HOME/Saved Games/Frontier Developments/Elite Dangerous" \
    cargo run --release -p galos_map

# What a journal directory holds on its own, and the same written out as an
# index directory the map can be pointed at with nothing else running.
cargo run -p galos_journal -- info "$HOME/Saved Games/.../Elite Dangerous"
cargo run -p galos_journal -- watch "$HOME/Saved Games/.../Elite Dangerous"
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
