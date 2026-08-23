# Galos
-----
[![CI](https://github.com/nixpulvis/galos/actions/workflows/ci.yml/badge.svg)](https://github.com/nixpulvis/galos/actions/workflows/ci.yml)

Somewhere between reality and the space/flight sim E:D.

Use `galos-sync` to populate the database and `galos` to perform basic queries
from the CLI.

The [`galos-map`](./galos_map) program is a 3D galaxy map, see its
documentation for more.

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
