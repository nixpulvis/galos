//! Carrying something from one market to another at a profit
//!
//! A trade is one commodity, one market that will sell it to you, and one
//! that will buy it off you for more. What makes finding them hard is that
//! there are far too many pairs to look at: gold alone is carried by ten
//! thousand markets, and there are four hundred commodities. The whole cross
//! product is billions of pairs and nobody wants it.
//!
//! Three questions are asked instead, and which one is asked depends on how
//! much has already been decided. Both ends known is arithmetic rather than a
//! search. One end known is a search over what that market sells, which is a
//! hundred commodities rather than a galaxy. Neither end known is the only
//! one that is really a search, and it leans on the galaxy's own lopsidedness
//! to stay cheap.
//!
//! ## Why the last one is affordable
//!
//! Of two and a half million commodity rows, only about 278,000 are stocked
//! and priced, which is to say buyable. The rest are things a station will
//! take off you and never sell. So the buy side is a tenth the size of the
//! sell side, and it is the buy side a search walks.
//!
//! Then the profit of carrying something is `sell - buy`, and for one
//! commodity the largest `sell` anywhere is a single number. Four hundred of
//! those, one pass, and every buyable row has an upper bound on what it could
//! ever be worth. That bound is what the galaxy-wide search ranks by, and no
//! pair is ever enumerated to get it.
//!
//! ## What the bound does not do
//!
//! It ignores distance, and the biggest margin in the galaxy is generally two
//! stations twenty thousand light years apart. Anchored at a market
//! [`Trade::from_market`] bounds the search by real distance and the answer is
//! the best trade you can actually fly. Unanchored, [`Trade::anywhere`] is a
//! board of what is worth money somewhere, and the distance column is there
//! to show you how little that means on its own.
use crate::market::{Database, Error};
use chrono::{DateTime, NaiveDateTime, Utc};
use sqlx::Row;
use sqlx::postgres::PgRow;

/// The market ids a fleet carrier is given
///
/// Carriers set their own prices, hold a handful of tons at them, and are
/// somewhere else tomorrow. A search that leaves them in answers with them
/// and nothing else: the biggest margins on record are Thargoid tissue
/// samples at fifty million a ton, one ton of it, priced by whoever owns the
/// carrier.
///
/// The station table cannot be asked which markets those are. A market
/// message names a station without saying what kind it is, so 12,936 of the
/// stations on record have no type at all and most carriers are among them.
/// The id answers where the type does not, because Frontier hands carriers
/// their own range of them. On this database it separates the two exactly:
/// all 1,269 markets known to be a carrier fall inside it, every market
/// known to be anything else falls outside, and it catches 1,510 in all.
///
/// A range and not a name pattern, though carrier callsigns do look alike.
/// The pattern misses 175 of these, among them the ones sitting at the top
/// of an unfiltered board.
const CARRIERS: std::ops::Range<i64> = 3_700_000_000..3_716_000_000;

/// One end of a trade
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Endpoint {
    pub market_id: i64,
    pub system_name: String,
    pub station_name: String,
}

/// What a search will and will not count as a trade
///
/// Every one of these is also what makes the search affordable. Unfiltered,
/// the best margins in the galaxy are Thargoid tissue samples on fleet
/// carriers at fifty million a ton, one ton in stock, priced by their owner
/// and gone by tomorrow.
#[derive(Clone, Debug, PartialEq)]
pub struct Filters {
    /// How many tons the seller must have before it counts as stocked
    pub min_stock: i32,
    /// How many the buyer must want
    pub min_demand: i32,
    /// How old a reading may be, in days
    ///
    /// A third of the commodity rows on record were last read more than a day
    /// ago, and a price nobody has confirmed in a month is a rumour.
    pub max_age: i32,
    /// Whether to count fleet carriers
    pub carriers: bool,
    /// How far the destination may be from the source, in light years
    ///
    /// None searches the galaxy. Only meaningful where the search has an end
    /// to measure from.
    pub within: Option<f64>,
    /// How many tons the ship can carry
    ///
    /// Not asked of the database. It decides how much of a margin can
    /// actually be realised, since a trade with four tons in stock is four
    /// tons of profit however good the price is.
    pub hold: i32,
    /// How many rows to bring back
    pub limit: i32,
}

impl Default for Filters {
    fn default() -> Self {
        Filters {
            min_stock: 100,
            min_demand: 100,
            max_age: 3,
            carriers: false,
            within: Some(100.),
            hold: 720,
            limit: 200,
        }
    }
}

/// One commodity worth carrying from one market to another
#[derive(Clone, Debug, PartialEq)]
pub struct Trade {
    pub name: String,
    pub source: Endpoint,
    pub destination: Endpoint,
    /// What the source charges for a ton
    pub buy_price: i32,
    /// How many tons it has
    pub stock: i32,
    /// What the destination pays for a ton
    pub sell_price: i32,
    /// How many tons it wants
    pub demand: i32,
    /// Light years between the two systems
    ///
    /// None where either end is a market still waiting on the system it named,
    /// which 751 of them are, or a system on record without a position.
    pub distance: Option<f64>,
    /// The older of the two readings this was worked out from
    ///
    /// A trade is only as current as its stalest half.
    pub read_at: DateTime<Utc>,
}

impl Trade {
    /// What one ton earns
    pub fn margin(&self) -> i32 {
        self.sell_price - self.buy_price
    }

    /// How many tons can really change hands
    ///
    /// The least of what is there, what is wanted, and what fits.
    pub fn tons(&self, hold: i32) -> i32 {
        self.stock.min(self.demand).min(hold)
    }

    /// What the whole run earns
    ///
    /// The number that matters, and the one that sinks a spectacular margin
    /// on a commodity nobody has more than four tons of.
    pub fn haul(&self, hold: i32) -> i64 {
        self.margin() as i64 * self.tons(hold) as i64
    }

    /// Everywhere worth carrying something this market sells
    ///
    /// One row per commodity, naming the best it can be sold for within
    /// reach, best margin first. The search is over what this market stocks,
    /// which is a hundred commodities at most, and [`Filters::within`] bounds
    /// how far the answers may be.
    pub async fn from_market(
        db: &Database,
        market_id: i64,
        filters: &Filters,
    ) -> Result<Vec<Self>, Error> {
        let (first, last) = (CARRIERS.start, CARRIERS.end - 1);
        let sql = format!(
            r#"
            WITH origin AS (
                SELECT s.position
                  FROM markets m
                  JOIN systems s ON s.address = m.system_address
                 WHERE m.id = $1
            ),
            src AS (
                SELECT name, buy_price, stock, listed_at
                  FROM commodities
                 WHERE market_id = $1
                   AND stock >= $2
                   AND buy_price > 0
                   AND listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $4)
            ),
            -- The best destination for each thing the source sells. A
            -- commodity is one row however many markets would take it, since
            -- a hold can only be filled with it once.
            best AS (
                SELECT DISTINCT ON (src.name)
                       src.name,
                       src.buy_price,
                       src.stock,
                       src.listed_at AS bought_at,
                       dst.sell_price,
                       dst.demand,
                       dst.listed_at AS sold_at,
                       dm.id AS market_id,
                       dm.system_name,
                       dm.station_name,
                       ST_3DDistance(ds.position, origin.position) AS distance
                  FROM src
                  CROSS JOIN origin
                  JOIN commodities dst ON dst.name = src.name
                  JOIN markets dm ON dm.id = dst.market_id
                  JOIN systems ds ON ds.address = dm.system_address
                 WHERE dst.demand >= $3
                   AND dst.sell_price > src.buy_price
                   AND dst.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $4)
                   AND dm.id <> $1
                   AND ($5 OR dm.id NOT BETWEEN {first} AND {last})
                   AND ($6::float8 IS NULL
                        OR ST_3DDWithin(ds.position, origin.position, $6))
                 ORDER BY src.name, dst.sell_price - src.buy_price DESC
            )
            SELECT * FROM best
             ORDER BY sell_price - buy_price DESC
             LIMIT $7
            "#
        );

        let rows = sqlx::query(&sql)
            .bind(market_id)
            .bind(filters.min_stock)
            .bind(filters.min_demand)
            .bind(filters.max_age)
            .bind(filters.carriers)
            .bind(filters.within)
            .bind(filters.limit as i64)
            .fetch_all(&db.pool)
            .await?;

        let source = endpoint(db, market_id).await?;
        Ok(rows
            .iter()
            .map(|row| Trade {
                name: row.get("name"),
                source: source.clone(),
                destination: Endpoint {
                    market_id: row.get("market_id"),
                    system_name: row.get("system_name"),
                    station_name: row.get("station_name"),
                },
                buy_price: row.get("buy_price"),
                stock: row.get("stock"),
                sell_price: row.get("sell_price"),
                demand: row.get("demand"),
                distance: row.get("distance"),
                read_at: older(row.get("bought_at"), row.get("sold_at")),
            })
            .collect())
    }

    /// The best run to be had around a named system
    ///
    /// Both ends inside [`Filters::within`] of the system, so the whole run
    /// is somewhere you already are. One row per commodity: the cheapest
    /// place in reach to buy it and the dearest to sell it.
    ///
    /// Note that the radius is measured from the system, not along the run,
    /// so two markets on opposite edges of the same bubble are up to twice it
    /// apart. What each row cost to fly is its own distance, which is why
    /// that column is there.
    ///
    /// ## Why it is not the pair search it looks like
    ///
    /// Fifty light years of Sol holds 1,103 markets. Joining what they sell
    /// against what they buy, commodity by commodity, is a few hundred
    /// thousand pairs and takes sixteen seconds.
    ///
    /// None of those pairs are needed. The best run for one commodity is the
    /// cheapest anyone in the bubble sells it for against the dearest anyone
    /// pays, and each of those is one pass over the same rows. Two passes and
    /// a join of four hundred rows against four hundred: 380ms.
    ///
    /// What that gives up is the second best run for a commodity, which is
    /// not asked for, and any commodity whose cheapest seller is also its
    /// dearest buyer, which is a market that would be trading with itself.
    pub async fn near(
        db: &Database,
        origin: &str,
        filters: &Filters,
    ) -> Result<Vec<Self>, Error> {
        // Asked first and on its own, so that somewhere nobody has heard of
        // is told apart from somewhere with nothing worth carrying.
        let known = sqlx::query(
            "SELECT 1 FROM systems WHERE upper(name) = upper($1) LIMIT 1",
        )
        .bind(origin)
        .fetch_optional(&db.pool)
        .await?;
        if known.is_none() {
            return Err(Error::NoSuchSystem(origin.to_owned()));
        }

        let (first, last) = (CARRIERS.start, CARRIERS.end - 1);
        let sql = format!(
            r#"
            WITH origin AS (
                SELECT position FROM systems
                 WHERE upper(name) = upper($1) LIMIT 1
            ),
            -- Every market in reach, off the spatial index. Ids only: a
            -- geometry is wide, and carrying one through everything below
            -- costs more than looking two of them up at the end.
            near AS (
                SELECT m.id
                  FROM markets m
                  JOIN systems s ON s.address = m.system_address, origin
                 WHERE ST_3DDWithin(s.position, origin.position, $2)
                   AND ($3 OR m.id NOT BETWEEN {first} AND {last})
            ),
            -- The two halves of a run, each read separately. One pass over
            -- what the bubble sells and one over what it buys, rather than
            -- one pass over everything that is then filtered twice: the buy
            -- side is a tenth of the rows and there is no sense sorting the
            -- other nine tenths to find it.
            cheapest AS (
                SELECT DISTINCT ON (c.name)
                       c.name, c.buy_price, c.stock, c.listed_at, c.market_id
                  FROM commodities c
                  JOIN near n ON n.id = c.market_id
                 WHERE c.stock >= $4
                   AND c.buy_price > 0
                   AND c.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $6)
                 -- Cheapest, and where two are level, whoever has more of it.
                 ORDER BY c.name, c.buy_price ASC, c.stock DESC
            ),
            dearest AS (
                SELECT DISTINCT ON (c.name)
                       c.name, c.sell_price, c.demand, c.listed_at, c.market_id
                  FROM commodities c
                  JOIN near n ON n.id = c.market_id
                 WHERE c.demand >= $5
                   AND c.sell_price > 0
                   AND c.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $6)
                 ORDER BY c.name, c.sell_price DESC, c.demand DESC
            )
            SELECT cheapest.name,
                   cheapest.buy_price,
                   cheapest.stock,
                   cheapest.listed_at AS bought_at,
                   dearest.sell_price,
                   dearest.demand,
                   dearest.listed_at AS sold_at,
                   bm.id AS source_id,
                   bm.system_name AS source_system,
                   bm.station_name AS source_station,
                   sm.id AS market_id,
                   sm.system_name,
                   sm.station_name,
                   ST_3DDistance(bs.position, ss.position) AS distance
              FROM cheapest
              JOIN dearest USING (name)
              JOIN markets bm ON bm.id = cheapest.market_id
              JOIN markets sm ON sm.id = dearest.market_id
              LEFT JOIN systems bs ON bs.address = bm.system_address
              LEFT JOIN systems ss ON ss.address = sm.system_address
             WHERE dearest.sell_price > cheapest.buy_price
               AND dearest.market_id <> cheapest.market_id
             ORDER BY dearest.sell_price - cheapest.buy_price DESC
             LIMIT $7
            "#
        );

        let rows = sqlx::query(&sql)
            .bind(origin)
            .bind(filters.within.unwrap_or(100.))
            .bind(filters.carriers)
            .bind(filters.min_stock)
            .bind(filters.min_demand)
            .bind(filters.max_age)
            .bind(filters.limit as i64)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.iter().map(both_ends).collect())
    }

    /// The biggest margins anywhere, with no regard for how far apart they are
    ///
    /// A board of what is worth money somewhere, not a plan. The best margin
    /// in the galaxy usually has its two ends on opposite sides of it, which
    /// the distance column is there to show. Bounding this properly by
    /// distance is [`Trade::from_market`]: pick which end you are standing at
    /// and the question becomes answerable.
    ///
    /// The upper bound is what keeps it cheap. Each commodity's best selling
    /// market is found once, and every buyable row is then ranked against
    /// that one number rather than against every market that would take it.
    pub async fn anywhere(
        db: &Database,
        filters: &Filters,
    ) -> Result<Vec<Self>, Error> {
        let (first, last) = (CARRIERS.start, CARRIERS.end - 1);
        let sql = format!(
            r#"
            -- The upper bound: what a commodity fetches at the best market
            -- that will take it. A few hundred rows out of a million.
            --
            -- An aggregate rather than DISTINCT ON, though the two say the
            -- same thing here. DISTINCT ON has to sort a million rows by
            -- (name, price) to find the first of each name, which spills 20MB
            -- to disk and takes fourteen seconds. A max needs no order at
            -- all: it hashes by name and keeps one number per group.
            WITH best AS (
                SELECT c.name, max(c.sell_price) AS sell_price
                  FROM commodities c
                  JOIN markets m ON m.id = c.market_id
                 WHERE c.demand >= $2
                   AND c.sell_price > 0
                   AND c.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $3)
                   AND ($4 OR m.id NOT BETWEEN {first} AND {last})
                 GROUP BY c.name
            ),
            -- Which market that was. Asking for the price and the place at
            -- once is what cost the sort, so the place is looked up
            -- afterwards, by the price. Ties go to whoever wants most of it.
            --
            -- Every condition above is repeated, or a market excluded from
            -- the bound could be the one found here.
            dest AS (
                SELECT DISTINCT ON (c.name)
                       c.name,
                       c.sell_price,
                       c.demand,
                       c.listed_at,
                       m.id,
                       m.system_address,
                       m.system_name,
                       m.station_name
                  FROM best
                  JOIN commodities c
                    ON c.name = best.name AND c.sell_price = best.sell_price
                  JOIN markets m ON m.id = c.market_id
                 WHERE c.demand >= $2
                   AND c.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $3)
                   AND ($4 OR m.id NOT BETWEEN {first} AND {last})
                 ORDER BY c.name, c.demand DESC
            ),
            -- Which rows win, settled before anything is measured. Ranking
            -- is a subtraction over a quarter million rows; the distance
            -- between two systems is geometry, and doing that per candidate
            -- rather than per answer costs forty times the whole query.
            top AS (
                SELECT src.name,
                       src.buy_price,
                       src.stock,
                       src.listed_at AS bought_at,
                       dest.sell_price,
                       dest.demand,
                       dest.listed_at AS sold_at,
                       sm.id AS source_id,
                       sm.system_address AS source_address,
                       sm.system_name AS source_system,
                       sm.station_name AS source_station,
                       dest.id AS market_id,
                       dest.system_address,
                       dest.system_name,
                       dest.station_name
                  FROM commodities src
                  JOIN markets sm ON sm.id = src.market_id
                  JOIN dest ON dest.name = src.name
                 WHERE src.stock >= $1
                   AND src.buy_price > 0
                   AND src.listed_at >
                       (now() AT TIME ZONE 'utc') - make_interval(days => $3)
                   AND ($4 OR sm.id NOT BETWEEN {first} AND {last})
                   AND dest.sell_price > src.buy_price
                   AND dest.id <> sm.id
                 ORDER BY dest.sell_price - src.buy_price DESC
                 LIMIT $5
            )
            SELECT top.*,
                   ST_3DDistance(ss.position, bs.position) AS distance
              FROM top
              -- Outer, so a market still waiting on the system it named is
              -- ranked with the rest and simply has no distance to give.
              LEFT JOIN systems ss ON ss.address = top.source_address
              LEFT JOIN systems bs ON bs.address = top.system_address
             ORDER BY top.sell_price - top.buy_price DESC
            "#
        );

        let rows = sqlx::query(&sql)
            .bind(filters.min_stock)
            .bind(filters.min_demand)
            .bind(filters.max_age)
            .bind(filters.carriers)
            .bind(filters.limit as i64)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.iter().map(both_ends).collect())
    }
}

/// Read a trade off a row that names both of its ends
///
/// Shared by the two searches that choose the source as well as the
/// destination. The anchored one has its source in hand already and names
/// only the far end.
fn both_ends(row: &PgRow) -> Trade {
    Trade {
        name: row.get("name"),
        source: Endpoint {
            market_id: row.get("source_id"),
            system_name: row.get("source_system"),
            station_name: row.get("source_station"),
        },
        destination: Endpoint {
            market_id: row.get("market_id"),
            system_name: row.get("system_name"),
            station_name: row.get("station_name"),
        },
        buy_price: row.get("buy_price"),
        stock: row.get("stock"),
        sell_price: row.get("sell_price"),
        demand: row.get("demand"),
        distance: row.get("distance"),
        read_at: older(row.get("bought_at"), row.get("sold_at")),
    }
}

/// One commodity as two markets both trade it
///
/// No search in this at all. Two markets are named and the arithmetic is what
/// each would pay the other, which is the whole of what "profit between two
/// markets" means once there is nothing left to look for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comparison {
    pub name: String,
    pub here: Side,
    pub there: Side,
}

/// What one market makes of one commodity
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Side {
    pub buy_price: i32,
    pub stock: i32,
    pub sell_price: i32,
    pub demand: i32,
    pub listed_at: DateTime<Utc>,
}

impl Side {
    pub fn is_stocked(&self) -> bool {
        self.stock > 0 && self.buy_price > 0
    }

    pub fn is_wanted(&self) -> bool {
        self.demand > 0 && self.sell_price > 0
    }
}

impl Comparison {
    /// What a ton earns carried from here to there, where it can be
    ///
    /// None where the trade cannot be made at all: nothing to buy at this
    /// end, or nothing wanted at the other. A negative number is a real
    /// answer and worth showing, since it says the run is worth making the
    /// other way.
    pub fn out(&self) -> Option<i32> {
        (self.here.is_stocked() && self.there.is_wanted())
            .then(|| self.there.sell_price - self.here.buy_price)
    }

    /// And carried back
    pub fn back(&self) -> Option<i32> {
        (self.there.is_stocked() && self.here.is_wanted())
            .then(|| self.here.sell_price - self.there.buy_price)
    }

    /// Everything both markets trade, and what each would pay for it
    ///
    /// Only what both carry. A commodity one of them has never heard of
    /// cannot be sold there, so it is not a row of this table.
    pub async fn fetch_all(
        db: &Database,
        here: i64,
        there: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query(
            r#"
            SELECT a.name,
                   a.buy_price  AS here_buy,
                   a.stock      AS here_stock,
                   a.sell_price AS here_sell,
                   a.demand     AS here_demand,
                   a.listed_at  AS here_at,
                   b.buy_price  AS there_buy,
                   b.stock      AS there_stock,
                   b.sell_price AS there_sell,
                   b.demand     AS there_demand,
                   b.listed_at  AS there_at
              FROM commodities a
              JOIN commodities b ON b.name = a.name AND b.market_id = $2
             WHERE a.market_id = $1
             ORDER BY a.name
            "#,
        )
        .bind(here)
        .bind(there)
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .iter()
            .map(|row| Comparison {
                name: row.get("name"),
                here: side(row, "here"),
                there: side(row, "there"),
            })
            .collect())
    }
}

/// Read one market's side of a comparison row
fn side(row: &PgRow, which: &str) -> Side {
    let listed_at: NaiveDateTime = row.get(format!("{}_at", which).as_str());
    Side {
        buy_price: row.get(format!("{}_buy", which).as_str()),
        stock: row.get(format!("{}_stock", which).as_str()),
        sell_price: row.get(format!("{}_sell", which).as_str()),
        demand: row.get(format!("{}_demand", which).as_str()),
        listed_at: listed_at.and_utc(),
    }
}

/// The older of two readings
fn older(a: NaiveDateTime, b: NaiveDateTime) -> DateTime<Utc> {
    a.min(b).and_utc()
}

/// Which market this is, for naming an end of a trade
pub async fn endpoint(
    db: &Database,
    market_id: i64,
) -> Result<Endpoint, Error> {
    let row = sqlx::query(
        "SELECT id, system_name, station_name FROM markets WHERE id = $1",
    )
    .bind(market_id)
    .fetch_one(&db.pool)
    .await?;

    Ok(Endpoint {
        market_id: row.get("id"),
        system_name: row.get("system_name"),
        station_name: row.get("station_name"),
    })
}

/// How far apart two markets are, in light years
///
/// None where either is a market waiting on the system it named, or the
/// system on record has no position.
pub async fn between(
    db: &Database,
    here: i64,
    there: i64,
) -> Result<Option<f64>, Error> {
    let row = sqlx::query(
        r#"
        SELECT ST_3DDistance(a.position, b.position) AS distance
          FROM markets ma
          JOIN systems a ON a.address = ma.system_address,
               markets mb
          JOIN systems b ON b.address = mb.system_address
         WHERE ma.id = $1 AND mb.id = $2
        "#,
    )
    .bind(here)
    .bind(there)
    .fetch_optional(&db.pool)
    .await?;

    Ok(row.and_then(|row| row.get("distance")))
}
