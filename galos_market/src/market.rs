//! What the database holds about trade, and how to ask it
//!
//! The same `commodities` and `markets` tables the EDDN listener writes
//! through `galos_db`, read here directly. That crate keeps its pool to
//! itself, so a crate outside it cannot borrow the connection, and a
//! prototype that is not part of the workspace cannot add a method to it
//! either. These are reads only, and nothing here writes a row.
//!
//! The queries are checked when they run. `sqlx::query!` would want its
//! offline data written into the repository's tracked `.sqlx`, which is the
//! one thing this crate is arranged not to touch.
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::{env, fmt};

/// A connection to the galos database
///
/// Cloning it shares the pool rather than opening another, which is what lets
/// each question be asked on a thread of its own.
#[derive(Clone)]
pub struct Database {
    /// Shared with [`crate::trade`], which asks its own questions of it
    pub(crate) pool: PgPool,
}

impl Database {
    /// Connect to whatever `DATABASE_URL` names
    ///
    /// A `.env` beside the repository is read first where there is one. Not
    /// having one is not a failure: the variable may just as well be set in
    /// the environment.
    pub async fn new() -> Result<Self, Error> {
        let _ = dotenv::dotenv();
        let url = env::var("DATABASE_URL")?;
        Ok(Database {
            pool: PgPoolOptions::new().max_connections(5).connect(&url).await?,
        })
    }
}

/// Anything that can come back instead of an answer
#[derive(Debug)]
pub enum Error {
    Sqlx(sqlx::Error),
    /// Nothing said where the database is
    NoUrl(env::VarError),
    /// A search was anchored somewhere nobody has heard of
    ///
    /// Its own answer rather than an empty result, since "no system by that
    /// name" and "nothing worth carrying there" are different things to be
    /// told.
    NoSuchSystem(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Sqlx(err) => write!(f, "{}", err),
            Error::NoUrl(err) => write!(f, "DATABASE_URL: {}", err),
            Error::NoSuchSystem(name) => {
                write!(f, "no system called {}", name)
            }
        }
    }
}

impl std::error::Error for Error {}

impl From<sqlx::Error> for Error {
    fn from(err: sqlx::Error) -> Self {
        Error::Sqlx(err)
    }
}

impl From<env::VarError> for Error {
    fn from(err: env::VarError) -> Self {
        Error::NoUrl(err)
    }
}

/// One commodity as a market trades it
///
/// The two prices are the station's side of the trade, which is the game's
/// way round: `buy_price` is what it charges for one, and it needs `stock` to
/// have any to charge for. `sell_price` is what it pays for one, and it needs
/// `demand` to want any. A market quotes both for most of what it carries
/// while trading in only one direction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commodity {
    pub market_id: i64,
    pub name: String,
    pub mean_price: i32,
    pub buy_price: i32,
    pub sell_price: i32,
    pub demand: i32,
    pub stock: i32,
    pub listed_at: DateTime<Utc>,
}

impl Commodity {
    /// Whether a ship can fill its hold here
    pub fn is_stocked(&self) -> bool {
        self.stock > 0 && self.buy_price > 0
    }

    /// Whether a ship can empty its hold here
    pub fn is_wanted(&self) -> bool {
        self.demand > 0 && self.sell_price > 0
    }

    /// Everything one market trades, in the order a board lists it
    pub async fn fetch_all(
        db: &Database,
        market_id: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                market_id,
                name,
                mean_price,
                buy_price,
                sell_price,
                demand,
                stock,
                listed_at
            FROM commodities
            WHERE market_id = $1
            ORDER BY name
            "#,
        )
        .bind(market_id)
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.iter().map(commodity).collect())
    }
}

/// Read one commodity off a row that holds its columns
fn commodity(row: &sqlx::postgres::PgRow) -> Commodity {
    let listed_at: NaiveDateTime = row.get("listed_at");
    Commodity {
        market_id: row.get("market_id"),
        name: row.get("name"),
        mean_price: row.get("mean_price"),
        buy_price: row.get("buy_price"),
        sell_price: row.get("sell_price"),
        demand: row.get("demand"),
        stock: row.get("stock"),
        listed_at: listed_at.and_utc(),
    }
}

/// One commodity as one market trades it, and where that market is
///
/// The market is named rather than pointed at, so everywhere a commodity
/// trades can be listed from these alone. Which market each row came from is
/// in the commodity's own `market_id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Quote {
    /// Held as the market gave it, which is not always a system on record
    ///
    /// A market can arrive before anything that would create the system it
    /// names, and it keeps trading under that name until one does.
    pub system_name: String,
    pub station_name: String,
    pub commodity: Commodity,
}

impl Quote {
    /// Everywhere one commodity is traded
    ///
    /// Every market carrying it, including the ones that neither stock it nor
    /// want it just now. A station quoting a price for nothing it holds is
    /// still what that station thinks the thing is worth, and dropping those
    /// rows here would leave the caller unable to tell a market that has run
    /// out from one that never carried it.
    ///
    /// Names are stored lowercase, so this asks in lowercase whatever case it
    /// was given.
    pub async fn fetch_all(
        db: &Database,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                markets.system_name,
                markets.station_name,
                commodities.market_id,
                commodities.name,
                commodities.mean_price,
                commodities.buy_price,
                commodities.sell_price,
                commodities.demand,
                commodities.stock,
                commodities.listed_at
            FROM commodities
            JOIN markets ON markets.id = commodities.market_id
            WHERE commodities.name = LOWER($1)
            "#,
        )
        .bind(name)
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|row| Quote {
                system_name: row.get("system_name"),
                station_name: row.get("station_name"),
                commodity: commodity(row),
            })
            .collect())
    }
}

/// What every market together makes of one commodity
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub name: String,
    /// How many markets carry it at all, stocked or not
    pub markets: i64,
    /// What the galaxy holds it to be worth, averaged over those markets
    ///
    /// Each market carries its own idea of the mean price and they differ, so
    /// this is a mean of means rather than the single number the game quotes.
    pub mean_price: i32,
    /// The least it can be bought for anywhere it is stocked
    ///
    /// None where no market stocks it, which is most of what the galaxy only
    /// ever buys: rare goods, salvage, and the things a station will take off
    /// you and never sell.
    pub lowest_buy: Option<i32>,
    /// The most it can be sold for anywhere it is wanted
    pub highest_sell: Option<i32>,
}

impl Summary {
    /// Every commodity traded anywhere, and what the galaxy makes of it
    ///
    /// One pass over every commodity row there is, which is the whole of what
    /// makes it expensive. Ask for it once and keep it, rather than per
    /// commodity: reading the whole table 400 times to answer 400 questions
    /// costs more than answering all of them at once.
    pub async fn fetch_all(db: &Database) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT
                name,
                count(*) AS markets,
                avg(mean_price)::int AS mean_price,
                min(buy_price) FILTER (
                    WHERE stock > 0 AND buy_price > 0) AS lowest_buy,
                max(sell_price) FILTER (
                    WHERE demand > 0 AND sell_price > 0) AS highest_sell
            FROM commodities
            GROUP BY name
            ORDER BY name
            "#,
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|row| Summary {
                name: row.get("name"),
                markets: row.get("markets"),
                mean_price: row.get("mean_price"),
                lowest_buy: row.get("lowest_buy"),
                highest_sell: row.get("highest_sell"),
            })
            .collect())
    }
}
