//! A migrated database of each test's own
//!
//! `TEST_DATABASE_URL` names a *server*. Whichever database on it the url
//! happens to name is only what this module connects to in order to work --
//! `postgresql://localhost/postgres` will do -- because what a test writes to
//! is a database made here. Nothing has to be created or migrated by hand.
//!
//! ```no_run
//! # async fn f() {
//! use galos_db::testing::Scratch;
//! let Some(db) = Scratch::new().await else { return };
//! // ... db derefs to a `Database`
//! db.done().await;
//! # }
//! ```
//!
//! The migrations run into a database per test would be the whole set run
//! once per test, so they are run into [`TEMPLATE`] instead and every test's
//! database is a copy of that. The template persists between runs: it is a
//! cache, and the only thing on the server a run leaves behind.
//!
//! A test's own database is dropped by [`Scratch::done`], which a test that
//! panics never reaches. What such a run leaves is dropped by the next one:
//! [`Scratch::new`] reaps, at its first use in a process, every database
//! named here whose process is gone. Only a name this module could have
//! made is ever dropped -- `galos_test_<pid>_<n>`, two numbers and nothing
//! else.
use crate::Database;
use sqlx::postgres::{PgConnectOptions, PgPool, PgPoolOptions};
use sqlx::{Connection, Executor, PgConnection};
use std::ops::Deref;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

/// The server these tests run against, or nothing and they stand down
///
/// `TEST_DATABASE_URL` and never `DATABASE_URL`, so that the server being
/// filled from EDDN is one `cargo test` cannot be pointed at by accident.
const SERVER: &str = "TEST_DATABASE_URL";

/// What every database made here is called, and nothing else on the server is
const PREFIX: &str = "galos_test";

/// The migrated database each test's own is copied from
///
/// Made on demand and never dropped, this being the whole of what makes a
/// database per test affordable.
pub const TEMPLATE: &str = "galos_test_template";

/// Held while the template is seen to, so that two runs do not both make it
///
/// `galos` in ASCII. Advisory locks share one space per server, so the number
/// only has to be one nothing else in it uses.
const LOCK: i64 = 0x67_61_6c_6f_73;

/// How long to keep asking for a copy of the template
///
/// A server that wants an idle source to copy from -- which is any of them
/// before the write-ahead-log strategy became the default, and any of them
/// told to copy files -- refuses a copy outright while anything is connected
/// to the template. What connects to it is a run seeing to its migrations:
/// one connection, held for as long as reading a table takes.
const TRIES: u32 = 40;
const WAIT: Duration = Duration::from_millis(250);

/// Postgres for "that is in use by someone else"
const IN_USE: &str = "55006";

/// What makes each name in a process unique
static NEXT: AtomicU32 = AtomicU32::new(0);

/// A database of one test's own
///
/// Derefs to the [`Database`] it wraps, so it is passed and asked of exactly
/// as one. It is dropped from the server by [`Self::done`], which every test
/// that reaches its end must call.
pub struct Scratch {
    db: Database,
    name: String,
    server: Server,
}

impl Scratch {
    /// A fresh migrated database, or nothing and the caller stands down
    ///
    /// Standing down is for having no server to reach: `TEST_DATABASE_URL`
    /// unset, or set to something nothing is listening on. A server that
    /// answers and then refuses what this asks of it is a server meant to be
    /// run against, so it panics.
    pub async fn new() -> Option<Scratch> {
        let server = server().await?;
        let name = format!(
            "{}_{}_{}",
            PREFIX,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed),
        );

        let mut conn =
            server.pool.acquire().await.expect("a connection to the server");
        keep_asking(
            &mut conn,
            &format!("CREATE DATABASE {} TEMPLATE {}", name, TEMPLATE),
        )
        .await;
        drop(conn);

        // Five connections, as `Database::from_url` has it: a test asserting
        // that a write does not wait on the pool needs the pool it would.
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect_with(server.options(&name))
            .await
            .unwrap_or_else(|e| panic!("{} should connect: {}", name, e));

        Some(Scratch { db: Database { pool }, name, server })
    }

    /// Drop this test's database
    ///
    /// Said at the end of a test rather than in a `Drop`: dropping the
    /// database is a query, `Drop` cannot await one, and blocking on one from
    /// inside the runtime a test is running on is a thread of that runtime
    /// waiting on work that same runtime has to do. A test that panics on the
    /// way here leaves its database for the next run to reap.
    pub async fn done(self) {
        let Scratch { db, name, server } = self;
        // The pool, and with it whatever this test still holds of it. A
        // connection a test never dropped outlives the pool, which is what
        // `FORCE` below is for.
        drop(db);

        let mut conn =
            server.pool.acquire().await.expect("a connection to the server");
        discard(&mut conn, &name).await;
    }
}

impl Deref for Scratch {
    type Target = Database;

    fn deref(&self) -> &Database {
        &self.db
    }
}

/// The server every test in a process shares
#[derive(Clone)]
struct Server {
    /// For the work that is done on a server rather than in a database:
    /// making a copy of the template and dropping it again.
    pool: PgPool,
    url: String,
}

impl Server {
    /// The same server, a named database on it
    fn options(&self, database: &str) -> PgConnectOptions {
        self.url
            .parse::<PgConnectOptions>()
            .unwrap_or_else(|e| panic!("{} should be a url: {}", SERVER, e))
            .database(database)
    }
}

/// What a process has found out about the server
enum Reached {
    /// Nothing has looked yet
    Unasked,
    /// There is nothing there, and every test stands down
    Nowhere,
    At(Server),
}

/// The server, reached once per process, or nothing and tests stand down
///
/// The first caller through reaps what earlier runs left and sees to the
/// template; every later one is handed what it made, or told what it found.
async fn server() -> Option<Server> {
    static REACHED: LazyLock<async_std::sync::Mutex<Reached>> =
        LazyLock::new(|| async_std::sync::Mutex::new(Reached::Unasked));
    let mut held = REACHED.lock().await;
    match &*held {
        Reached::At(server) => return Some(server.clone()),
        Reached::Nowhere => return None,
        Reached::Unasked => {}
    }

    let url = match url() {
        Some(url) => url,
        None => {
            eprintln!("no {}: standing down", SERVER);
            *held = Reached::Nowhere;
            return None;
        }
    };
    // A connection of its own rather than one of a pool's: a pool keeps
    // asking until its acquire timeout, and what this wants is the first
    // answer. Nothing listening, or nowhere to listen, is no server here to
    // have meant; anything else -- a password refused, a database that is
    // not there -- is a server that was meant and will not do what this
    // needs, and standing down over that would be standing down over a typo.
    let mut conn = match PgConnection::connect(&url).await {
        Ok(conn) => conn,
        Err(sqlx::Error::Io(e)) => {
            eprintln!("{} answers nothing ({}): standing down", SERVER, e);
            *held = Reached::Nowhere;
            return None;
        }
        Err(e) => panic!("{} should connect: {}", SERVER, e),
    };

    let pool = PgPoolOptions::new()
        .max_connections(4)
        .connect_lazy(&url)
        .unwrap_or_else(|e| panic!("{} should be a url: {}", SERVER, e));
    let server = Server { pool, url };

    // One run at a time past here. Reaping and making the template are both
    // statements about the whole server, and two test binaries start at once
    // often enough -- `cargo test` runs one per target -- that "unlikely" is
    // not a plan. The lock is on a connection rather than in a transaction
    // because `CREATE DATABASE` cannot run in one, which is also why the
    // connection is this call's own and not the pool's: a panic in here
    // closes it, and closing it is what lets the lock go. Handed back to a
    // pool it would hold the lock against every other run for as long as
    // this process lived.
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK)
        .execute(&mut conn)
        .await
        .expect("the lock should be takeable");
    reap(&mut conn).await;
    see_to_the_template(&mut conn, &server).await;
    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(LOCK)
        .execute(&mut conn)
        .await
        .expect("the lock should be releasable");
    let _ = conn.close().await;

    *held = Reached::At(server.clone());
    Some(server)
}

/// Where the tests run, out of the environment or out of `.env`
fn url() -> Option<String> {
    dotenv::dotenv().ok();
    std::env::var(SERVER).ok()
}

/// Make the template if it is not there, and bring it up to date if it is
///
/// Run under the lock, so the connection this holds to the template is the
/// only one any run holds to it, and no two runs make it at once. A copy
/// taken while it is held is a copy a strict server refuses rather than
/// waits out, which is what the asking in [`keep_asking`] is for: two test
/// binaries do not share a process, so nothing in one can wait on the
/// other's connection.
///
/// The migrations are read off disk rather than embedded, so that adding one
/// takes effect without rebuilding anything, and run against the template
/// every time a process starts. That is one query when there is nothing to
/// do, and the alternative is a template a schema has moved on from.
async fn see_to_the_template(conn: &mut PgConnection, server: &Server) {
    let there: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)",
    )
    .bind(TEMPLATE)
    .fetch_one(&mut *conn)
    .await
    .expect("the server should say what it holds");

    if !there {
        let create = format!("CREATE DATABASE {}", TEMPLATE);
        match (&mut *conn).execute(&*create).await {
            Ok(_) => {}
            // Two servers' worth of runs can hold this lock at once: it is
            // taken in whichever database the url names, and two urls may
            // name two of them on one server.
            Err(e) if code(&e).as_deref() == Some("42P04") => {}
            Err(e) => panic!("{} should be creatable: {}", TEMPLATE, e),
        }
    }

    let pool = PgPoolOptions::new()
        .max_connections(1)
        .connect_with(server.options(TEMPLATE))
        .await
        .unwrap_or_else(|e| panic!("{} should connect: {}", TEMPLATE, e));
    let source = std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/migrations"
    ));
    sqlx::migrate::Migrator::new(source)
        .await
        .expect("the migrations should read")
        .run(&pool)
        .await
        .expect("the migrations should run");
    // Awaited, and not merely dropped: a copy of the template cannot be taken
    // while anything is connected to it.
    pool.close().await;
}

/// Drop every database here whose process is gone
///
/// A test that panics never drops its own, and a run that is killed drops
/// none of them, so this is where those go. Nothing live is touched: a
/// database whose process is still running belongs to a test still using it,
/// and this process's own are younger than this call.
async fn reap(conn: &mut PgConnection) {
    // Every name that could be one of ours, which `owner` then decides on.
    // `_` is a wildcard here, so this asks for slightly more than it means
    // and throws the rest away.
    let names: Vec<String> = sqlx::query_scalar(
        "SELECT datname FROM pg_database WHERE datname LIKE $1",
    )
    .bind(format!("{}_%", PREFIX))
    .fetch_all(&mut *conn)
    .await
    .expect("the server should say what it holds");

    for name in names {
        match owner(&name) {
            Some(pid) if pid != std::process::id() && !alive(pid) => {
                discard(&mut *conn, &name).await;
            }
            _ => {}
        }
    }
}

/// Whose database this is, or nothing and it is not one of ours
///
/// Strict, because what it answers is dropped: `galos_test_<pid>_<n>` and
/// both of those all digits. The template's name is not one of these.
fn owner(name: &str) -> Option<u32> {
    let mut parts = name.strip_prefix(PREFIX)?.strip_prefix('_')?.split('_');
    let pid = parts.next()?.parse().ok()?;
    let _: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(pid)
}

/// Whether a process is still running
///
/// Signal nothing to it and see whether it is there to be signalled. A pid
/// the system has since handed to something else reads as alive, which costs
/// one database left for the run after this one.
fn alive(pid: u32) -> bool {
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    // There, and someone else's.
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

/// Drop a database, whoever is still connected to it
///
/// `FORCE` throws the rest off first, which is Postgres 13 and up. Without it
/// a test that left a connection checked out of its pool would keep its own
/// database alive.
async fn discard(conn: &mut PgConnection, name: &str) {
    let drop = format!("DROP DATABASE IF EXISTS {} WITH (FORCE)", name);
    keep_asking(conn, &drop).await;
}

/// Run a statement, waiting out whatever else is using what it names
///
/// Making a database and dropping one both answer "in use" rather than wait
/// where something else is connected to what they name. Between test
/// binaries there is nothing to wait on, so waiting is done here.
async fn keep_asking(conn: &mut PgConnection, statement: &str) {
    for attempt in 0..TRIES {
        match (&mut *conn).execute(statement).await {
            Ok(_) => return,
            Err(e) if code(&e).as_deref() == Some(IN_USE) => {
                if attempt + 1 == TRIES {
                    panic!("`{}` never stopped waiting: {}", statement, e);
                }
                async_std::task::sleep(WAIT).await;
            }
            Err(e) => panic!("`{}` should run: {}", statement, e),
        }
    }
}

/// What Postgres called what went wrong, if it was Postgres that said so
fn code(error: &sqlx::Error) -> Option<String> {
    match error {
        sqlx::Error::Database(e) => e.code().map(|it| it.into_owned()),
        _ => None,
    }
}
