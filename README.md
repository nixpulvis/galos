# Galos
![Galos Starmap Demo](galos-map/demo.gif)
-----
[![CI](https://github.com/nixpulvis/galos/actions/workflows/ci.yml/badge.svg)](https://github.com/nixpulvis/galos/actions/workflows/ci.yml)

Somewhere between reality and the space/flight sim E:D.

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

# Restore from backup.
pg_restore -Cd postgres < latest.dump
```
