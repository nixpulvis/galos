# galos_db

The database layer for Galos: the schema migrations, the query code, and the
types written to and read from PostgreSQL.

The connection is read from `DATABASE_URL`, taken from the environment or a
`.env` file (see the top-level [README](../README.md)), e.g.
`postgresql://postgres@localhost/galos_development`.

## Migrations

Migrations live in `migrations/` and are applied with `sqlx-cli`. Run these
from the workspace root:

```sh
cargo install sqlx-cli --locked --version "$(cargo pkgid sqlx | sed 's/.*@//')"

# Run any pending migrations.
cargo sqlx migrate run --source galos_db/migrations/

# Drop, create, and migrate the whole thing.
cargo sqlx database reset --source galos_db/migrations/
```

The migrations install the `postgis`, `postgis_topology`, and `pg_trgm`
extensions, so the connecting role must be allowed to `CREATE EXTENSION`.

## Testing

The write-path tests need a database of their own, named by
`TEST_DATABASE_URL`, so a database in use for anything else cannot be reached
from them. They stand down when it is unset, which is how CI passes without a
database (building `SQLX_OFFLINE=true` against the cached query metadata).

```sh
createdb galos_test
DATABASE_URL=postgresql://…/galos_test \
    cargo sqlx migrate run --source galos_db/migrations/
TEST_DATABASE_URL=postgresql://…/galos_test \
    cargo test -p galos_db --test write_path
```
