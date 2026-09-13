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

The tests that write get a migrated database of their own, made and dropped
by `galos_db::testing` behind the `testing` feature. `TEST_DATABASE_URL`
names the *server*; whichever database on it the url happens to name is only
somewhere to connect while the real one is made, so `postgres` will do.
`DATABASE_URL` is never read, so a database in use for anything else cannot
be reached from a test.

```sh
TEST_DATABASE_URL=postgresql://localhost/postgres cargo test -p galos_db
```

The migrations are run once into `galos_test_template` and each test's
database is a copy of it. That template persists between runs as a cache; a
test's own database is dropped as the test ends, and one left by a test that
panicked is dropped by the next run.

They stand down when there is no server to reach -- the variable unset, or
nothing listening where it points -- which is how CI passes without one
(building `SQLX_OFFLINE=true` against the cached query metadata).
