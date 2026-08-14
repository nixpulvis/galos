# Galos
-----
[![CI](https://github.com/nixpulvis/galos/actions/workflows/ci.yml/badge.svg)](https://github.com/nixpulvis/galos/actions/workflows/ci.yml)

Somewhere between reality and the space/flight sim E:D.

Use `galos-sync` to populate the databas and `galos` to perform basic queries
from the CLI.

The [`galos-map`](./galos_map) program is a 3D galaxy map, see it's
documentation for more.

### Database Setup

```sh
cargo install sqlx-cli

# Create the database and run the migrations.
cargo sqlx database setup --source galos_db/migrations/

# Run any pending migrations.
cargo sqlx migrate run --source galos_db/migrations/

# Drop, create, and migrate the whole thing.
cargo sqlx database reset --source galos_db/migrations/
```

### Database Backup and Restore

```sh
# Create a backup.
pg_dump -Fc elite_development > latest.dump

# Restore from backup. Note that `-C` takes the database away while it runs,
# and EDDN drops every message it is handed meanwhile.
pg_restore -Cd postgres < latest.dump
```

[`DATABASE.md`](./DATABASE.md) has the restores that leave EDDN running, why
the one above does not, and the monthly Spansh galaxy dump import.
[`ROUTING.md`](./ROUTING.md) is a design note for long-range route plotting,
and is not built.
