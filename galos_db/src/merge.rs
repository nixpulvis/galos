//! One database folded into another, by the rule the write path already
//! holds.
//!
//! The use for this is stated by how a failure actually goes. Something is
//! wrong with the database EDDN is landing in; collection is pointed at a
//! fresh one so the feed keeps arriving somewhere; the old one is repaired
//! beside it. Two databases now hold overlapping halves of the same galaxy
//! and neither is the answer. This is the third step, and without it the
//! only way back to one database is a reimport, which for a seeded galaxy
//! is most of a day during which EDDN goes on not waiting.
//!
//! # The rule, and why it is not restated here
//!
//! Every write path in this crate is a guarded upsert keyed by a natural
//! key and weighed by a stamp: `CASE WHEN $n >= t.updated_at THEN
//! COALESCE($new, stored) ELSE COALESCE(stored, $new) END`, with
//! `updated_at = GREATEST(t.updated_at, $n)` beside it. The newer of two
//! readings wins where they disagree; an older one still fills in what the
//! row has never held, a blank being no reading at all; a tie goes to the
//! arriving record, which is what `>=` says.
//!
//! Folding a whole database in is that statement with the values coming
//! from a table instead of a message. What is written below is not a second
//! copy of it: the clause is **generated from the schema** — the column list
//! read out of `information_schema.columns` in ordinal order, the conflict
//! target read off `pg_index` — so a migration that adds a column gets the
//! rule applied to it without anybody remembering to come here. That is the
//! closest thing to reuse a cross-database merge can have, and the oracle
//! test at the bottom of this file is what pins it: two databases fed
//! different halves of one stream and merged must equal a third that saw
//! both halves interleaved.
//!
//! Only the exceptions are named, and each is named because a `create.rs`
//! says so rather than because it seemed wise: `discovered_at` takes the
//! earliest claim (`LEAST`, which ignores a null), `was_mapped` only ever
//! goes up (`OR`), and `received_at` is set to this server's clock.
//!
//! # Why `received_at` is re-stamped and not carried
//!
//! `updated_at` says when a thing happened out in the galaxy and travels
//! with the row. `received_at` says when the report reached *this*
//! database, and everything that follows the feed — `galos db report`, the
//! index's changed-since cursor — keeps its place by it. A merged row
//! carrying the other database's `received_at` would arrive already behind
//! a cursor that has passed that point, and would never be read again. So
//! it is `clock_timestamp() AT TIME ZONE 'utc'` on both paths, the insert's
//! as well as the update's: the row is new here whatever its age out there.
//!
//! # Postgres' own wire format, and text rather than binary
//!
//! The rows are not moved through Rust. `sqlx` cannot rebind an arbitrary
//! `PgValue` — there is no way to read a column of unknown type and hand it
//! back as a parameter — so a row-at-a-time merge would need a typed
//! decoder per column, which is the second copy of the schema this module
//! exists to avoid. Instead each table is `COPY (SELECT …) TO STDOUT` on
//! the source, streamed straight into `COPY … FROM STDIN` on the target's
//! temporary table, and every statement after that is SQL against two
//! tables in one database.
//!
//! Text format, not binary. Binary `COPY` is a little faster and is not
//! portable across a major-version difference between the two servers,
//! which is exactly the case a restore-onto-a-newer-machine puts you in.
//! The stream is passed through in the chunks the server sends it in and
//! never collected.
//!
//! # The four things that make it harder than `INSERT … SELECT`
//!
//! **Faction ids are per database.** `factions.id` is a `serial`; the
//! natural key is the unique index on `lower(name)`. Four tables reference
//! it, so carrying the source's ids across writes the wrong faction into
//! the target. `factions` is therefore merged first and by name, and the
//! `source id -> target id` map it builds is joined against by every later
//! statement. That map is the one piece of this that cannot be a per-table
//! statement.
//!
//! **Foreign keys fix the order.** Derived rather than written down: the
//! `contype = 'f'` rows of `pg_constraint` are a graph and [`ordered`]
//! topologically sorts it, parents first. A cycle is [`Error::Cyclic`] and
//! a table with no rule and no usable key is [`Error::Unruled`] or
//! [`Error::Keyless`] — reported, never quietly dropped. Running with the
//! constraints disabled instead is how a galaxy ends up holding bodies in
//! systems it does not have.
//!
//! **The list-valued tables merge whole, not per row.** `commodities`,
//! `outfitting` and `shipyard` are replaced as a set by their writer, and
//! so are `body_materials` and `system_faction_states`. Merging those row
//! by row unions two market snapshots and invents a station stocking both.
//! The newer list wins entire; see [`Rule::List`].
//!
//! **Nothing is withdrawn.** An absence on the incoming side says "I have
//! not heard", never "it is gone". This is where the merge deliberately
//! parts from the write path, and it is worth being exact about: several
//! writers replace their row wholesale, because a *message* that leaves a
//! field out has been asked about it and said nothing. A *database* that
//! holds a null was never asked. `markets.system_address` is the clearest
//! case — `Market::touch` lets a newer message move a carrier to nowhere,
//! and a merge will not, because the other database's null is its ignorance
//! of the system and not the carrier's absence from one.
//!
//! The cost of that choice is the one place the oracle can be made to
//! fail: a column that one side's newest reading left out and an older
//! reading filled in comes across, where a database that had seen both
//! streams in order would hold the newer blank. Every reading of a row in
//! practice carries the same set of columns, which is why this is a
//! footnote rather than a defect, but it is a real one and it is not
//! papered over.
//!
//! # What is not merged
//!
//! `articles` is skipped and said so. Its key is a bare `serial` with
//! nothing natural under it, so two databases cannot agree which row is
//! which; inventing a key would be inventing data. `_sqlx_migrations` and
//! anything owned by an extension (PostGIS' three) are never touched, and
//! a run across two different migration versions is refused outright by
//! [`Error::Divergent`] — merging across a schema difference is how a
//! column silently stops being written.
//!
//! `system_faction_influences` is carried across but cannot be made to
//! agree with an interleaved database, and that is a fact about the table
//! rather than about this code: it is a journal of the influence
//! transitions *this database witnessed*, written by an `AFTER UPDATE`
//! trigger on `system_factions`, and two databases that witnessed
//! different subsets of a run of readings did not witness the same
//! transitions. What the merge holds to is that no witnessed transition is
//! lost. The trigger also fires on the `system_factions` merge itself,
//! which is why the journal is merged after it — the foreign key order
//! already puts it there — and why the incoming rows are matched against
//! what is on record rather than assumed absent.
//!
//! # One transaction, so a dry run is a rollback
//!
//! Everything happens inside one transaction on the target, and `dry_run`
//! is that transaction rolled back. It runs the real statements rather
//! than counting what they would have done, because a dry run whose
//! arithmetic differs from the real one is a dry run that can lie. The one
//! thing it leaves behind is a gap in `factions_id_seq`, sequences not
//! being transactional, which costs nothing.
//!
//! The temporary tables are `ON COMMIT DROP`, so the transaction's end is
//! also their end whichever way it goes.
//!
//! # The queries are unchecked
//!
//! Every statement here is built at run time out of the catalog, so none
//! of it could be a `sqlx::query!` even in principle, and `galos_db`
//! compiles `SQLX_OFFLINE=true` against a cached query set that a new
//! macro would invalidate. Unchecked `sqlx::query`, columns read back by
//! name, as `index` and `report` already do.

use crate::{migrate, Database, Error};
use async_std::stream::StreamExt;
use chrono::NaiveDateTime;
use sqlx::{PgConnection, Row};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt;
use std::time::{Duration, Instant};
use tracing::debug;

/// What one table's fold did.
///
/// `read` is what came across the wire, before anything was decided about
/// it. `inserted` and `updated` are counted off the statement that wrote
/// them, and `refused` is the remainder: rows the target already held a
/// newer reading of, plus the few dropped for colliding with a stored row
/// under a second unique key. Nothing is lost silently — the three always
/// sum to `read`.
pub struct Table {
    pub name: String,
    pub read: u64,
    pub inserted: u64,
    pub updated: u64,
    pub refused: u64,
}

/// What a whole run did.
pub struct Merged {
    /// One per table folded, in the order they were folded, which is the
    /// foreign key order.
    pub tables: Vec<Table>,

    /// How many of the source's factions were resolved to an id in the
    /// target — the size of the remap. Those the target had never heard of
    /// were minted here and are the `factions` row's `inserted`.
    pub factions: usize,

    /// Tables this merge will not carry, by name. Empty is the ordinary
    /// case for a schema nothing has been added to; see [`Rule::Skipped`]
    /// for why any name is here.
    pub skipped: Vec<String>,

    pub took: Duration,
}

impl fmt::Display for Table {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "{}: {} read, {} inserted, {} updated, {} refused",
            self.name, self.read, self.inserted, self.updated, self.refused
        )
    }
}

/// The run's totals and what it would not carry.
///
/// Not a second listing of the tables: each one is handed to the caller as
/// it lands, so repeating them here would print the whole thing twice. The
/// skipped are named rather than counted, because a count is something an
/// operator has to go and look up and a name is something they can grep.
impl fmt::Display for Merged {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let read: u64 = self.tables.iter().map(|t| t.read).sum();
        let inserted: u64 = self.tables.iter().map(|t| t.inserted).sum();
        let updated: u64 = self.tables.iter().map(|t| t.updated).sum();
        let refused: u64 = self.tables.iter().map(|t| t.refused).sum();

        writeln!(
            f,
            "{} tables, {} rows read, {} inserted, {} updated, \
             {} refused, {} factions remapped, in {:.1}s",
            self.tables.len(),
            read,
            inserted,
            updated,
            refused,
            self.factions,
            self.took.as_secs_f64(),
        )?;

        for name in &self.skipped {
            let why = match rule(name) {
                Some(Rule::Skipped(why)) => why,
                _ => "no rule here carries it",
            };
            writeln!(f, "not merged -- {name}: {why}")?;
        }

        Ok(())
    }
}

/// How a table is folded, where "the guarded upsert" is not the answer.
///
/// [`rule`] answers [`None`] for everything else, and everything else is
/// the ordinary case: a stamped row under a unique key, merged by the
/// clause the write path uses.
enum Rule {
    /// Left where it is, for the reason given.
    Skipped(&'static str),

    /// `factions`: resolved by name, because the id is whichever database
    /// minted it first.
    Remap,

    /// An append-only journal with no key of its own. Rows are carried
    /// across whole, and only the ones not already on record.
    ///
    /// `stamp` is the column `since` bounds it by. It is not a stamp in
    /// the sense the rest of this module means — nothing is weighed
    /// against it — it is only which end of the journal to read.
    Journal { stamp: &'static str },

    /// Replaced as a set, per parent key, because its writer does.
    ///
    /// `by` names the columns that identify one list. `stamp` says which
    /// clock decides whether the incoming list is the newer one.
    List { by: &'static [&'static str], stamp: Listed },
}

/// Which clock a list is weighed by.
enum Listed {
    /// A column of its own, written across the whole list at once:
    /// `listed_at` on the three trade tables.
    Own(&'static str),

    /// The parent row's, for a list that carries no clock. `on` maps the
    /// child's key columns to the parent's, child first.
    Parent {
        table: &'static str,
        on: &'static [(&'static str, &'static str)],
    },
}

/// What is not an ordinary guarded upsert, and why.
///
/// A table that reaches [`None`] is folded generically, which is sound so
/// long as it has a stamp and a unique key; one that has neither and is
/// not named here stops the run rather than being guessed at. That is the
/// pressure this function is for: a migration adding a stamped table needs
/// nothing from anybody, and one adding an unstamped table has to come
/// here and say what it means.
fn rule(table: &str) -> Option<Rule> {
    match table {
        "articles" => Some(Rule::Skipped(
            "a serial id with no natural key under it, so two databases \
             cannot agree which row is which",
        )),

        "factions" => Some(Rule::Remap),

        "system_faction_influences" => {
            Some(Rule::Journal { stamp: "new_timestamp" })
        }

        // A market event states the whole of what a station trades, so what
        // it leaves out is no longer stocked. Two snapshots unioned is a
        // station that sells the union of what it sold on two days.
        "commodities" | "outfitting" | "shipyard" => Some(Rule::List {
            by: &["market_id"],
            stamp: Listed::Own("listed_at"),
        }),

        // A surface scan states the whole composition, and carries no clock
        // of its own: `bodies/create.rs` clears and refills these under the
        // scan that found them, so the body's stamp is the list's stamp.
        "body_materials" => Some(Rule::List {
            by: &["system_address", "body_id"],
            stamp: Listed::Parent {
                table: "bodies",
                on: &[("system_address", "system_address"), ("body_id", "id")],
            },
        }),

        // The same shape one level up: `SystemFaction::from_journal` calls
        // `State::clear` and writes the states the report named, so the
        // set belongs to the `system_factions` row and takes its stamp.
        // Every column is in the primary key, so there is nothing a row
        // could be updated to in any case.
        "system_faction_states" => Some(Rule::List {
            by: &["system_address", "faction_id"],
            stamp: Listed::Parent {
                table: "system_factions",
                on: &[
                    ("system_address", "system_address"),
                    ("faction_id", "faction_id"),
                ],
            },
        }),

        _ => None,
    }
}

/// The columns whose rule is not "the newest non-null reading wins".
///
/// Each is here because a `create.rs` says so. `discovered_at` takes the
/// earliest claim on record, a later scan finding a body already
/// discovered being no evidence about when; `was_mapped` only ever goes
/// up, a scan finding a body unmapped being no evidence that it has stayed
/// that way; `received_at` is this server's clock, for the reason in the
/// module doc.
const EARLIEST: &str = "discovered_at";
const ONLY_UP: &str = "was_mapped";
const ARRIVED: &str = "received_at";

/// What a stamp column is called, newest first in the search.
///
/// Read by name rather than declared per table, so a new stamped table
/// needs nothing from this module. No table in the schema carries both.
const STAMPS: [&str; 2] = ["updated_at", "listed_at"];

/// The columns that hold a faction id, and so have to be remapped.
///
/// `conflicts` names two of them. By name rather than by reading
/// `pg_constraint` for references to `factions`, because the remap has to
/// happen in the temporary table before the foreign key is ever consulted,
/// and the name is the thing the statement needs either way.
const FACTIONS: [&str; 3] = ["faction_id", "faction_1_id", "faction_2_id"];

/// What the target's catalog says one table is.
struct Shape {
    name: String,
    /// Every column, in ordinal order. Dropped columns are not here, which
    /// is `information_schema`'s doing and the right answer: `LIKE` does
    /// not recreate them either.
    columns: Vec<String>,
    /// The primary key's columns, which is the conflict target for the
    /// guarded fold. Empty where the table has no primary key.
    key: Vec<String>,
    /// Every other unique index, in the terms `pg_get_indexdef` puts them.
    unique: Vec<Unique>,
    /// The stamp, where the table has one; see [`STAMPS`].
    stamp: Option<String>,
}

/// A unique index, as a conflict target is written.
enum Unique {
    /// Plain columns, so it can be joined on as well as conflicted on.
    Columns(Vec<String>),
    /// An expression, which `ON CONFLICT` takes and a join cannot.
    /// `factions_name` is the only one in this schema.
    Expression(String),
}

impl Shape {
    /// This table's faction id columns, in ordinal order.
    fn factions(&self) -> Vec<&str> {
        self.columns
            .iter()
            .map(|c| c.as_str())
            .filter(|c| FACTIONS.contains(c))
            .collect()
    }

    /// The natural key, for a table whose primary key is not one.
    ///
    /// The first unique index that is not the primary key. `factions` is
    /// the case this exists for: its primary key is the serial and its
    /// natural key is `lower(name::text)`, read out of the catalog rather
    /// than written here, so the merge conflicts on exactly the index
    /// `factions/create.rs` conflicts on.
    fn natural(&self) -> Option<&Unique> {
        self.unique.first()
    }
}

/// A name as Postgres will read it back.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// Where a table's rows are put down on the target while they are decided
/// about.
fn staged(table: &str) -> String {
    quoted(&format!("_merge_{table}"))
}

/// Where the parent keys whose incoming list wins are put down.
fn winners(table: &str) -> String {
    quoted(&format!("_merge_won_{table}"))
}

/// A timestamp as a statement carries it.
///
/// Inlined rather than bound, because `COPY` takes a statement and not
/// parameters, and the two halves of a bounded copy have to read the same
/// literal. Safe to inline for being a `NaiveDateTime` and not a string:
/// there is nothing in it to escape.
fn literal(at: NaiveDateTime) -> String {
    format!("TIMESTAMP '{}'", at.format("%Y-%m-%d %H:%M:%S%.6f"))
}

/// A column list, optionally qualified.
fn listed(columns: &[String], by: Option<&str>) -> String {
    columns
        .iter()
        .map(|c| match by {
            Some(alias) => format!("{alias}.{}", quoted(c)),
            None => quoted(c),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The same list as a statement selects it for writing.
///
/// Identical to [`listed`] but for `received_at`, which is this server's
/// clock on the way in as well as on the way over — a merged row is new
/// here whatever its age out in the galaxy.
fn selected(columns: &[String], by: &str) -> String {
    columns
        .iter()
        .map(|c| match c.as_str() {
            ARRIVED => "clock_timestamp() AT TIME ZONE 'utc'".to_owned(),
            _ => format!("{by}.{}", quoted(c)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Fold one database into another.
///
/// `into` is the database this process is connected to and `from` is the
/// one being folded in; nothing is written to `from`. `since` bounds each
/// table by its stamp, for the ordinary case where only what was collected
/// after a known moment is wanted — a table with no stamp is carried whole,
/// there being nothing to bound it by. `said` is handed each [`Table`] as
/// it lands, so a caller can print a long run as it goes.
///
/// `dry_run` runs the whole thing and rolls it back. See the module doc for
/// why that rather than an estimate.
///
/// Returns [`Error::Divergent`] where the two databases are at different
/// migration versions, which is refused before anything is read.
pub async fn merge(
    into: &Database,
    from: &Database,
    since: Option<NaiveDateTime>,
    dry_run: bool,
    said: &mut dyn FnMut(&Table),
) -> Result<Merged, Error> {
    let start = Instant::now();

    // Before anything else, and before a single row is read. Two databases
    // at different migration versions agree about most of their columns,
    // which is what makes merging them so easy to do by accident.
    let ours = migrate::applied(into).await?;
    let theirs = migrate::applied(from).await?;
    if ours.as_ref().map(|(v, _)| *v) != theirs.as_ref().map(|(v, _)| *v) {
        return Err(Error::Divergent(
            ours.map(|(v, _)| v),
            theirs.map(|(v, _)| v),
        ));
    }

    let shapes = shapes(into).await?;
    let order = ordered(into, &shapes).await?;

    let mut source = from.acquire().await?;
    let mut tx = into.begin().await?;

    let mut tables = Vec::with_capacity(order.len());
    let mut skipped = Vec::new();
    let mut factions = 0;

    for name in &order {
        let shape = &shapes[name];
        let did = match rule(name) {
            Some(Rule::Skipped(_)) => {
                skipped.push(name.clone());
                continue;
            }
            Some(Rule::Remap) => {
                let (did, mapped) =
                    remap(&mut source, &mut tx, shape).await?;
                factions = mapped;
                did
            }
            Some(Rule::Journal { stamp }) => {
                journal(&mut source, &mut tx, shape, since, stamp).await?
            }
            Some(Rule::List { by, stamp }) => {
                list(&mut source, &mut tx, shape, &shapes, since, by, &stamp)
                    .await?
            }
            None => guarded(&mut source, &mut tx, shape, since).await?,
        };

        said(&did);
        tables.push(did);
    }

    match dry_run {
        true => tx.rollback().await?,
        false => tx.commit().await?,
    }

    Ok(Merged { tables, factions, skipped, took: start.elapsed() })
}

/// Every table the target holds that is this program's to merge.
///
/// `_sqlx_migrations` is `sqlx`'s own bookkeeping and the three PostGIS
/// relations belong to an extension, which `pg_depend` says outright: both
/// are read by asking rather than by naming them, so an extension added
/// later is excluded for the same reason and without an edit here.
const TABLES: &str = "\
SELECT c.relname::text \
  FROM pg_class c \
  JOIN pg_namespace n ON n.oid = c.relnamespace \
 WHERE n.nspname = 'public' \
   AND c.relkind = 'r' \
   AND c.relname <> '_sqlx_migrations' \
   AND NOT EXISTS (SELECT 1 FROM pg_depend d \
                    WHERE d.objid = c.oid AND d.deptype = 'e') \
 ORDER BY c.relname";

const COLUMNS: &str = "\
SELECT table_name::text AS \"table\", column_name::text AS \"column\" \
  FROM information_schema.columns \
 WHERE table_schema = 'public' \
 ORDER BY table_name, ordinal_position";

/// Every unique index, primary keys first within a table.
///
/// `pg_get_indexdef` is asked for one key column at a time, which answers a
/// plain column with its name and an expression with the expression —
/// exactly what `ON CONFLICT` takes either way. `indexprs IS NULL` is how
/// the two are told apart, and is the difference between a key that can be
/// joined on and one that can only be conflicted on.
const KEYS: &str = "\
SELECT t.relname::text AS \"table\", \
       i.indisprimary AS \"primary\", \
       (i.indexprs IS NULL) AS \"plain\", \
       ARRAY(SELECT pg_get_indexdef(i.indexrelid, k, true) \
               FROM generate_series(1, i.indnkeyatts) AS k) AS \"keys\" \
  FROM pg_index i \
  JOIN pg_class t ON t.oid = i.indrelid \
  JOIN pg_namespace n ON n.oid = t.relnamespace \
 WHERE n.nspname = 'public' AND i.indisunique AND i.indisvalid \
 ORDER BY t.relname, i.indisprimary DESC, i.indexrelid";

const REFERENCES: &str = "\
SELECT t.relname::text AS \"child\", p.relname::text AS \"parent\" \
  FROM pg_constraint c \
  JOIN pg_class t ON t.oid = c.conrelid \
  JOIN pg_class p ON p.oid = c.confrelid \
  JOIN pg_namespace n ON n.oid = t.relnamespace \
 WHERE c.contype = 'f' AND n.nspname = 'public'";

/// Read what the target's tables are made of.
///
/// Off the target and not the source, deliberately: what is being written
/// is the target's, and the two are already known to be at the same
/// migration version. A column the source has and the target does not
/// would be a schema difference, which is refused before this is called.
async fn shapes(db: &Database) -> Result<HashMap<String, Shape>, Error> {
    let names: Vec<String> =
        sqlx::query_scalar(TABLES).fetch_all(&db.pool).await?;
    let wanted: BTreeSet<&str> =
        names.iter().map(|n| n.as_str()).collect();

    let mut columns: HashMap<String, Vec<String>> = HashMap::new();
    for row in sqlx::query(COLUMNS).fetch_all(&db.pool).await? {
        let table: String = row.try_get("table")?;
        if !wanted.contains(table.as_str()) {
            continue;
        }
        columns.entry(table).or_default().push(row.try_get("column")?);
    }

    let mut keys: HashMap<String, Vec<String>> = HashMap::new();
    let mut unique: HashMap<String, Vec<Unique>> = HashMap::new();
    for row in sqlx::query(KEYS).fetch_all(&db.pool).await? {
        let table: String = row.try_get("table")?;
        if !wanted.contains(table.as_str()) {
            continue;
        }
        let parts: Vec<String> = row.try_get("keys")?;
        match (row.try_get::<bool, _>("primary")?, row.try_get("plain")?) {
            (true, _) => {
                keys.insert(table, parts);
            }
            (false, true) => unique
                .entry(table)
                .or_default()
                .push(Unique::Columns(parts)),
            (false, false) => unique
                .entry(table)
                .or_default()
                .push(Unique::Expression(parts.join(", "))),
        }
    }

    Ok(names
        .into_iter()
        .map(|name| {
            let columns = columns.remove(&name).unwrap_or_default();
            let stamp = STAMPS
                .iter()
                .find(|s| columns.iter().any(|c| c == *s))
                .map(|s| (*s).to_owned());
            let shape = Shape {
                key: keys.remove(&name).unwrap_or_default(),
                unique: unique.remove(&name).unwrap_or_default(),
                columns,
                stamp,
                name: name.clone(),
            };
            (name, shape)
        })
        .collect())
}

/// The order the tables have to be written in, parents first.
///
/// Derived from the foreign keys rather than written down, so a migration
/// that adds a reference moves the table without anybody noticing it had
/// to. `factions` is lifted to the front afterwards: it has no reference
/// out of it, so every order the sort admits allows it there, and every
/// table carrying a faction id needs the remap to exist before it runs.
///
/// Ties are broken alphabetically, which makes a run's report the same
/// shape twice running and is worth more than it costs.
async fn ordered(
    db: &Database,
    shapes: &HashMap<String, Shape>,
) -> Result<Vec<String>, Error> {
    let mut needs: BTreeMap<&str, BTreeSet<&str>> =
        shapes.keys().map(|t| (t.as_str(), BTreeSet::new())).collect();

    for row in sqlx::query(REFERENCES).fetch_all(&db.pool).await? {
        let child: String = row.try_get("child")?;
        let parent: String = row.try_get("parent")?;
        // A table referencing itself orders nothing, and neither does a
        // reference onto something excluded above.
        if child == parent {
            continue;
        }
        let (Some(child), Some(parent)) =
            (shapes.get_key_value(&child), shapes.get_key_value(&parent))
        else {
            continue;
        };
        needs
            .get_mut(child.0.as_str())
            .expect("every table is in the map")
            .insert(parent.0.as_str());
    }

    let mut order: Vec<String> = Vec::with_capacity(needs.len());
    while !needs.is_empty() {
        let ready: Vec<&str> = needs
            .iter()
            .filter(|(_, parents)| parents.is_empty())
            .map(|(table, _)| *table)
            .collect();

        if ready.is_empty() {
            let mut caught: Vec<String> =
                needs.keys().map(|t| (*t).to_owned()).collect();
            caught.sort();
            return Err(Error::Cyclic(caught));
        }

        for table in &ready {
            needs.remove(table);
            order.push((*table).to_owned());
        }
        for parents in needs.values_mut() {
            for table in &ready {
                parents.remove(table);
            }
        }
    }

    if let Some(at) = order.iter().position(|t| t == "factions") {
        let factions = order.remove(at);
        order.insert(0, factions);
    }

    Ok(order)
}

/// Stream one table's rows into a temporary table beside the real one.
///
/// The copy runs on both connections at once and the chunks are passed
/// through as the source sends them: nothing is collected, so a table too
/// large to hold is no different from a small one.
async fn stage(
    from: &mut PgConnection,
    into: &mut PgConnection,
    shape: &Shape,
    select: &str,
) -> Result<u64, Error> {
    let temp = staged(&shape.name);
    let make = format!(
        "CREATE TEMP TABLE {temp} (LIKE {} INCLUDING DEFAULTS) \
         ON COMMIT DROP",
        quoted(&shape.name),
    );
    debug!(statement = %make, "staging");
    sqlx::query(&make).execute(&mut *into).await?;

    let out = format!("COPY ({select}) TO STDOUT");
    let inn =
        format!("COPY {temp} ({}) FROM STDIN", listed(&shape.columns, None));
    debug!(statement = %out, "copying out");

    let mut stream = from.copy_out_raw(&out).await?;
    let mut sink = into.copy_in_raw(&inn).await?;

    let mut stopped = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(bytes) => {
                sink.send(bytes).await?;
            }
            Err(e) => {
                stopped = Some(e);
                break;
            }
        }
    }
    drop(stream);

    match stopped {
        Some(e) => {
            sink.abort("the source stopped sending").await?;
            Err(e.into())
        }
        None => Ok(sink.finish().await?),
    }
}

/// `SELECT` one table off the source, bounded by its stamp.
fn reading(
    shape: &Shape,
    since: Option<NaiveDateTime>,
    stamp: Option<&str>,
) -> String {
    let mut select = format!(
        "SELECT {} FROM {}",
        listed(&shape.columns, None),
        quoted(&shape.name),
    );
    if let (Some(since), Some(stamp)) = (since, stamp) {
        select.push_str(&format!(
            " WHERE {} >= {}",
            quoted(stamp),
            literal(since)
        ));
    }
    select
}

/// Translate the faction ids in a staged table to the target's.
///
/// One statement per column, `conflicts` being the table with two. Nothing
/// is dropped for want of a mapping: every faction id in the source points
/// at a row of the source's `factions`, which was copied whole, so the map
/// covers all of them. A missing one would be a broken foreign key on the
/// source, which the target's own constraint will refuse rather than
/// quietly write.
async fn translate(
    into: &mut PgConnection,
    shape: &Shape,
) -> Result<(), Error> {
    let temp = staged(&shape.name);
    for column in shape.factions() {
        let statement = format!(
            "UPDATE {temp} AS m SET {0} = k.\"to_id\" \
               FROM \"_merge_faction_map\" k \
              WHERE k.\"from_id\" = m.{0}",
            quoted(column),
        );
        debug!(statement = %statement, "remapping");
        sqlx::query(&statement).execute(&mut *into).await?;
    }
    Ok(())
}

/// `factions`, which is merged by name and never by id.
///
/// Carried whole however `since` is set: a faction is a name, the name is
/// the whole of what the row holds, and a map that covers only the
/// factions heard from recently is a map that cannot translate the rest.
///
/// Returns the row for the report and the size of the map, which is every
/// source faction resolved to an id here.
async fn remap(
    from: &mut PgConnection,
    into: &mut PgConnection,
    shape: &Shape,
) -> Result<(Table, usize), Error> {
    let read = stage(from, into, shape, &reading(shape, None, None)).await?;

    let target = match shape.natural() {
        Some(Unique::Expression(keys)) => keys.clone(),
        Some(Unique::Columns(keys)) => listed(keys, None),
        None => return Err(Error::Keyless(shape.name.clone())),
    };

    let temp = staged(&shape.name);
    let minted = sqlx::query(&format!(
        "INSERT INTO {0} (\"name\") SELECT m.\"name\" FROM {temp} AS m \
         ON CONFLICT ({target}) DO NOTHING",
        quoted(&shape.name),
    ))
    .execute(&mut *into)
    .await?
    .rows_affected();

    // Every source id against the id the target answers to that name by,
    // which for the ones just minted is the id the insert above made. The
    // index is what the statements that join it are for -- one per table
    // carrying a faction id, each over the whole of that table's rows.
    sqlx::query(
        "CREATE TEMP TABLE \"_merge_faction_map\" ON COMMIT DROP AS \
         SELECT m.\"id\" AS \"from_id\", f.\"id\" AS \"to_id\" \
           FROM \"_merge_factions\" AS m \
           JOIN \"factions\" AS f \
             ON lower(f.\"name\") = lower(m.\"name\")",
    )
    .execute(&mut *into)
    .await?;
    sqlx::query(
        "CREATE UNIQUE INDEX ON \"_merge_faction_map\" (\"from_id\")",
    )
    .execute(&mut *into)
    .await?;

    let mapped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM \"_merge_faction_map\"",
    )
    .fetch_one(&mut *into)
    .await?;

    // Nothing is ever refused here. A faction is a name, and a name that
    // the target already holds is the same faction rather than a reading
    // that lost: `updated` is what resolved to a row already on record.
    let did = Table {
        name: shape.name.clone(),
        read,
        inserted: minted,
        updated: read.saturating_sub(minted),
        refused: 0,
    };

    Ok((did, mapped as usize))
}

/// The ordinary case: a stamped row under a unique key.
///
/// The `DO UPDATE` clause is generated column by column out of the shape,
/// which is the module's whole argument — see the module doc.
async fn guarded(
    from: &mut PgConnection,
    into: &mut PgConnection,
    shape: &Shape,
    since: Option<NaiveDateTime>,
) -> Result<Table, Error> {
    let Some(stamp) = shape.stamp.clone() else {
        return Err(Error::Unruled(shape.name.clone()));
    };
    if shape.key.is_empty() {
        return Err(Error::Keyless(shape.name.clone()));
    }

    let select = reading(shape, since, Some(&stamp));
    let read = stage(from, into, shape, &select).await?;
    if read == 0 {
        return Ok(nothing(&shape.name));
    }

    translate(into, shape).await?;

    let temp = staged(&shape.name);
    let table = quoted(&shape.name);

    // A row that collides with a stored one under a *second* unique key is
    // the two databases disagreeing about which row a thing is -- the same
    // body under two ids, which `stars/create.rs` meets from the other
    // direction when a name is rescanned onto a new id. `ON CONFLICT` can
    // name one index, so the rest are settled before the statement runs:
    // the incoming row is dropped and counted refused. Losing it is the
    // conservative half of the disagreement; writing it would be an error
    // that takes the whole merge down.
    for other in &shape.unique {
        let Unique::Columns(keys) = other else { continue };
        let matched = keys
            .iter()
            .map(|c| format!("t.{0} = m.{0}", quoted(c)))
            .collect::<Vec<_>>()
            .join(" AND ");
        let statement = format!(
            "DELETE FROM {temp} AS m USING {table} AS t \
              WHERE {matched} \
                AND ({0}) IS DISTINCT FROM ({1})",
            listed(&shape.key, Some("t")),
            listed(&shape.key, Some("m")),
        );
        debug!(statement = %statement, "settling a second unique key");
        sqlx::query(&statement).execute(&mut *into).await?;
    }

    let clause = clause(shape, &stamp);
    let returning = shape
        .key
        .iter()
        .enumerate()
        .map(|(n, c)| format!("t.{} AS \"k{n}\"", quoted(c)))
        .collect::<Vec<_>>()
        .join(", ");
    let matched = shape
        .key
        .iter()
        .enumerate()
        .map(|(n, c)| format!("m.{} = l.\"k{n}\"", quoted(c)))
        .collect::<Vec<_>>()
        .join(" AND ");

    // `(xmax = 0)` says the row was inserted: the conflict path locks the
    // row it is about to update and the new version carries that lock, so
    // an update answers a live xmax and an insert answers zero. Whether
    // the incoming reading won is the same test every `CASE` arm above
    // makes, asked of the result: `stamp` is `GREATEST(stored, incoming)`,
    // which equals the incoming one exactly when the incoming one won.
    let statement = format!(
        "WITH landed AS ( \
           INSERT INTO {table} AS t ({0}) \
           SELECT {1} FROM {temp} AS m \
           ON CONFLICT ({2}) DO UPDATE SET {clause} \
           RETURNING {returning}, (t.xmax = 0) AS \"made\", \
                     t.{3} AS \"stamp\" \
         ) \
         SELECT count(*) FILTER (WHERE l.\"made\") AS \"made\", \
                count(*) FILTER (WHERE NOT l.\"made\" \
                                   AND l.\"stamp\" = m.{3}) AS \"took\", \
                count(*) FILTER (WHERE NOT l.\"made\" \
                                   AND l.\"stamp\" <> m.{3}) AS \"lost\" \
           FROM landed AS l JOIN {temp} AS m ON {matched}",
        listed(&shape.columns, None),
        selected(&shape.columns, "m"),
        listed(&shape.key, None),
        quoted(&stamp),
    );
    debug!(statement = %statement, "folding");

    let counted = sqlx::query(&statement).fetch_one(&mut *into).await?;
    let inserted: i64 = counted.try_get("made")?;
    let updated: i64 = counted.try_get("took")?;

    Ok(counted_up(&shape.name, read, inserted as u64, updated as u64))
}

/// The `DO UPDATE SET` clause, one column at a time out of the schema.
///
/// The key columns are left out: they are what the row was found by and
/// cannot move. Everything else is the rule the write path holds, bar the
/// three columns whose rule is something else; see [`EARLIEST`],
/// [`ONLY_UP`] and [`ARRIVED`].
fn clause(shape: &Shape, stamp: &str) -> String {
    shape
        .columns
        .iter()
        .filter(|c| !shape.key.contains(c))
        .map(|column| {
            let c = quoted(column);
            match column.as_str() {
                _ if column == stamp => {
                    format!("{c} = GREATEST(t.{c}, EXCLUDED.{c})")
                }
                ARRIVED => {
                    format!("{c} = clock_timestamp() AT TIME ZONE 'utc'")
                }
                EARLIEST => format!("{c} = LEAST(t.{c}, EXCLUDED.{c})"),
                ONLY_UP => format!("{c} = t.{c} OR EXCLUDED.{c}"),
                _ => format!(
                    "{c} = CASE WHEN EXCLUDED.{0} >= t.{0} \
                       THEN COALESCE(EXCLUDED.{c}, t.{c}) \
                       ELSE COALESCE(t.{c}, EXCLUDED.{c}) END",
                    quoted(stamp),
                ),
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A list-valued table, replaced per parent key rather than per row.
///
/// The newer list wins whole. Which list is newer is [`Listed`]: a clock
/// of the list's own where it has one, and the parent row's where it does
/// not.
///
/// The parent case turns on something worth saying out loud. The parent
/// has already been merged by the time this runs -- the foreign key order
/// guarantees it -- so the target's stamp for it is now
/// `GREATEST(what it was, what the source said)`. That is enough: the
/// incoming list wins exactly when the source's reading of the parent won,
/// and the source's reading won exactly when the merged stamp equals the
/// source's. So no snapshot of what the parent's stamp used to be is
/// needed, and none is taken.
#[allow(clippy::too_many_arguments)]
async fn list(
    from: &mut PgConnection,
    into: &mut PgConnection,
    shape: &Shape,
    shapes: &HashMap<String, Shape>,
    since: Option<NaiveDateTime>,
    by: &[&str],
    stamp: &Listed,
) -> Result<Table, Error> {
    let table = quoted(&shape.name);
    let temp = staged(&shape.name);
    let won = winners(&shape.name);

    let read = match stamp {
        Listed::Own(clock) => {
            let select = reading(shape, since, Some(clock));
            stage(from, into, shape, &select).await?
        }
        // Bounded by the parent's stamp, which is the only clock it has,
        // and which also keeps the copy and the decision below agreeing
        // about which lists arrived: a child whose parent was left behind
        // by `since` would otherwise arrive with nothing to weigh it
        // against and be refused for it.
        Listed::Parent { table: parent, on } => {
            let joined = on
                .iter()
                .map(|(child, up)| {
                    format!("p.{} = m.{}", quoted(up), quoted(child))
                })
                .collect::<Vec<_>>()
                .join(" AND ");
            let mut select = format!(
                "SELECT {} FROM {table} AS m JOIN {} AS p ON {joined}",
                listed(&shape.columns, Some("m")),
                quoted(parent),
            );
            if let Some(since) = since {
                let clock = shapes[*parent]
                    .stamp
                    .as_deref()
                    .ok_or_else(|| Error::Unruled((*parent).to_owned()))?;
                select.push_str(&format!(
                    " WHERE p.{} >= {}",
                    quoted(clock),
                    literal(since)
                ));
            }
            stage(from, into, shape, &select).await?
        }
    };
    if read == 0 {
        return Ok(nothing(&shape.name));
    }

    translate(into, shape).await?;

    let keys: Vec<String> = by.iter().map(|c| (*c).to_owned()).collect();
    let held = keys
        .iter()
        .map(|c| format!("t.{0} = w.{0}", quoted(c)))
        .collect::<Vec<_>>()
        .join(" AND ");

    // Which parents' incoming list is the one to keep, and whether the
    // target already held a list for them -- the second is what tells an
    // inserted list from a replaced one in the report.
    let picked = match stamp {
        Listed::Own(clock) => format!(
            "SELECT {0} FROM \
               (SELECT {1}, MAX({2}) AS \"stamp\" FROM {temp} \
                 GROUP BY {1}) AS m \
             LEFT JOIN \
               (SELECT {1}, MAX({2}) AS \"stamp\" FROM {table} \
                 GROUP BY {1}) AS t ON {3} \
             WHERE t.\"stamp\" IS NULL OR t.\"stamp\" <= m.\"stamp\"",
            listed(&keys, Some("m")),
            listed(&keys, None),
            quoted(clock),
            keys.iter()
                .map(|c| format!("t.{0} = m.{0}", quoted(c)))
                .collect::<Vec<_>>()
                .join(" AND "),
        ),
        Listed::Parent { table: parent, on } => {
            let clock = shapes[*parent]
                .stamp
                .as_deref()
                .ok_or_else(|| Error::Unruled((*parent).to_owned()))?;
            let onto = |alias: &str| {
                on.iter()
                    .map(|(child, up)| {
                        format!("{alias}.{} = m.{}", quoted(up), quoted(child))
                    })
                    .collect::<Vec<_>>()
                    .join(" AND ")
            };
            format!(
                "SELECT DISTINCT {0} FROM {temp} AS m \
                   JOIN {1} AS s ON {2} \
                   JOIN {3} AS p ON {4} \
                  WHERE p.{5} = s.{5}",
                listed(&keys, Some("m")),
                staged(parent),
                onto("s"),
                quoted(parent),
                onto("p"),
                quoted(clock),
            )
        }
    };

    let statement = format!(
        "CREATE TEMP TABLE {won} ON COMMIT DROP AS \
         SELECT w.*, EXISTS (SELECT 1 FROM {table} AS t WHERE {0}) \
                     AS \"replaced\" \
           FROM ({picked}) AS w",
        keys.iter()
            .map(|c| format!("t.{0} = w.{0}", quoted(c)))
            .collect::<Vec<_>>()
            .join(" AND "),
    );
    debug!(statement = %statement, "picking the newer lists");
    sqlx::query(&statement).execute(&mut *into).await?;

    // The old list goes before the new one lands, which is what makes this
    // a replacement rather than a union. Inside one transaction, so
    // nothing ever reads a station that stocks nothing.
    let statement = format!(
        "DELETE FROM {table} AS t USING {won} AS w WHERE {held}"
    );
    debug!(statement = %statement, "clearing the lists being replaced");
    sqlx::query(&statement).execute(&mut *into).await?;

    let joined = keys
        .iter()
        .map(|c| format!("w.{0} = m.{0}", quoted(c)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "WITH put AS ( \
           INSERT INTO {table} ({0}) \
           SELECT {1} FROM {temp} AS m JOIN {won} AS w ON {joined} \
           RETURNING 1 \
         ) \
         SELECT count(*) FILTER (WHERE NOT w.\"replaced\") AS \"made\", \
                count(*) FILTER (WHERE w.\"replaced\") AS \"took\" \
           FROM {temp} AS m JOIN {won} AS w ON {joined}",
        listed(&shape.columns, None),
        selected(&shape.columns, "m"),
    );
    debug!(statement = %statement, "laying down the newer lists");

    let counted = sqlx::query(&statement).fetch_one(&mut *into).await?;
    let inserted: i64 = counted.try_get("made")?;
    let updated: i64 = counted.try_get("took")?;

    Ok(counted_up(&shape.name, read, inserted as u64, updated as u64))
}

/// An append-only journal: whatever it holds that the target does not.
///
/// Matched on every column, because there is no key to match on -- the
/// table is what `system_factions` did, not what it is. The
/// `system_factions` merge fires the same trigger that writes these, and
/// has already run by the time this does, so the rows this compares
/// against include the ones the merge itself just caused. Assuming them
/// absent would write each of those twice.
async fn journal(
    from: &mut PgConnection,
    into: &mut PgConnection,
    shape: &Shape,
    since: Option<NaiveDateTime>,
    stamp: &str,
) -> Result<Table, Error> {
    let select = reading(shape, since, Some(stamp));
    let read = stage(from, into, shape, &select).await?;
    if read == 0 {
        return Ok(nothing(&shape.name));
    }

    translate(into, shape).await?;

    let same = shape
        .columns
        .iter()
        .map(|c| format!("t.{0} IS NOT DISTINCT FROM m.{0}", quoted(c)))
        .collect::<Vec<_>>()
        .join(" AND ");
    let statement = format!(
        "INSERT INTO {0} ({1}) SELECT {2} FROM {3} AS m \
          WHERE NOT EXISTS (SELECT 1 FROM {0} AS t WHERE {same})",
        quoted(&shape.name),
        listed(&shape.columns, None),
        selected(&shape.columns, "m"),
        staged(&shape.name),
    );
    debug!(statement = %statement, "carrying a journal across");

    let wrote = sqlx::query(&statement)
        .execute(&mut *into)
        .await?
        .rows_affected();

    Ok(counted_up(&shape.name, read, wrote, 0))
}

/// A table the source had nothing to say about.
fn nothing(name: &str) -> Table {
    Table {
        name: name.to_owned(),
        read: 0,
        inserted: 0,
        updated: 0,
        refused: 0,
    }
}

/// What a fold did, with the remainder accounted for.
///
/// `refused` is never computed by the statements: it is what came across
/// and did not land, which is the only way to be sure a row that went
/// missing for some reason nobody thought of is still counted somewhere.
fn counted_up(name: &str, read: u64, inserted: u64, updated: u64) -> Table {
    Table {
        name: name.to_owned(),
        read,
        inserted,
        updated,
        refused: read.saturating_sub(inserted).saturating_sub(updated),
    }
}
