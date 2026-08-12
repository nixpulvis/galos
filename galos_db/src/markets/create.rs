use super::Market;
use crate::systems::System;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::market::Market as JournalMarket;

impl Market {
    /// Write the market row that a station's trade data hangs off
    ///
    /// Commodities, outfitting, shipyard and the black market all name a
    /// market the same way -- an id, a station, and a system by name only --
    /// so they all place it the same way.
    ///
    /// Not knowing the system is not a reason to lose what was sent. The name
    /// is recorded either way, and `System::create` links the market up if the
    /// system turns up later.
    ///
    /// A market can move. Fleet carriers jump, and one of them shows up in
    /// this database under three systems in an hour. So where it is now
    /// replaces where it was, rather than filling in a blank: a carrier that
    /// jumps somewhere unheard of goes back to waiting, instead of keeping the
    /// last system it was seen in.
    ///
    /// Which is only true of the newest message about it. Uploaders batch and
    /// reconnect, and one naming the system a carrier has already left would
    /// otherwise put it back there. The stamp is the newest heard from any of
    /// the four kinds of trade message, so it says when the market was last
    /// placed and nothing about how fresh any one of its tables is.
    pub(crate) async fn touch(
        db: &Database,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        timestamp: DateTime<Utc>,
        market_id: i64,
        system_name: &str,
        station_name: &str,
    ) -> Result<Market, Error> {
        let address = match System::fetch_by_name(db, system_name).await {
            Ok(system) => Some(system.address),
            // There is no such system yet, so the market waits for one.
            Err(Error::Sqlx(sqlx::Error::RowNotFound)) => None,
            // Anything else is the database failing to answer, which is not
            // the same as an answer of no. Filing the market as waiting on
            // that would leave it waiting on a system that already exists,
            // for a write that may never come.
            Err(err) => return Err(err),
        };

        let row = sqlx::query!(
            r#"
            INSERT INTO markets (
                id,
                system_address,
                system_name,
                station_name,
                updated_at)
            VALUES ($1, $2, UPPER($3), $4, $5)
            ON CONFLICT (id)
            DO UPDATE SET
                system_address = CASE WHEN $5 >= markets.updated_at
                    THEN $2 ELSE markets.system_address END,
                system_name = CASE WHEN $5 >= markets.updated_at
                    THEN UPPER($3) ELSE markets.system_name END,
                station_name = CASE WHEN $5 >= markets.updated_at
                    THEN $4 ELSE markets.station_name END,
                updated_at = GREATEST(markets.updated_at, $5)
            RETURNING
                id,
                system_address,
                system_name,
                station_name,
                updated_at
            "#,
            market_id,
            address,
            system_name,
            station_name,
            timestamp.naive_utc(),
        )
        .fetch_one(&mut **tx)
        .await?;

        Ok(Market {
            id: row.id,
            system_address: row.system_address,
            system_name: row.system_name,
            station_name: row.station_name,
            updated_at: row.updated_at.and_utc(),
        })
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        market: &JournalMarket,
    ) -> Result<Market, Error> {
        // The market and its commodities go in together. Between clearing the
        // old prices and writing the new ones the market holds nothing it
        // trades, which is not a state any reader should be shown.
        let mut tx = db.pool.begin().await?;

        let placed = Market::touch(
            db,
            &mut tx,
            timestamp,
            market.market_id,
            &market.system_name,
            &market.station_name,
        )
        .await?;

        // Read as the whole of what is traded, so an older event replaces a
        // newer list rather than adding to it. Every price in it is one the
        // station has since moved on from. The market itself is placed above
        // and settled that on its own stamp, so that much of the message
        // stands.
        if newer_on_record(&mut tx, market.market_id, timestamp).await? {
            tx.commit().await?;
            return Ok(placed);
        }

        // A market event is read as the whole of what the station trades, so
        // what it leaves out is no longer stocked. Nothing else can retire a
        // commodity, and without this a station keeps quoting a price for
        // something it sold the last of months ago.
        //
        // A sender that trims commodities it does not recognise will take a
        // few rows down with it here. That repairs itself: the next event
        // naming them puts them back, and a row wrongly dropped for a while
        // is a smaller lie than one that never leaves.
        sqlx::query!(
            "DELETE FROM commodities WHERE market_id = $1",
            market.market_id,
        )
        .execute(&mut *tx)
        .await?;

        // TODO: This sends one statement per commodity, and a market can name
        // several hundred of them. The whole set could go in a single INSERT
        // by passing each column as an array and UNNESTing them.
        for commodity in &market.commodities {
            // The rows this writes were just cleared, so the conflict clause
            // answers only for one event naming a commodity twice, which two
            // spellings of the same name now do. The later reading wins.
            sqlx::query!(
                r#"
                INSERT INTO commodities (
                    market_id,
                    name,
                    mean_price,
                    buy_price,
                    sell_price,
                    demand,
                    demand_bracket,
                    stock,
                    stock_bracket,
                    listed_at)
                VALUES ($1, LOWER($2), $3, $4, $5, $6, $7, $8, $9, $10)
                ON CONFLICT (market_id, name)
                DO UPDATE SET
                    mean_price = $3,
                    buy_price = $4,
                    sell_price = $5,
                    demand = $6,
                    demand_bracket = $7,
                    stock = $8,
                    stock_bracket = $9,
                    listed_at = $10
                RETURNING *
                "#,
                market.market_id,
                commodity.name,
                commodity.mean_price,
                commodity.buy_price,
                commodity.sell_price,
                commodity.demand,
                commodity.demand_bracket,
                commodity.stock,
                commodity.stock_bracket,
                timestamp.naive_utc(),
            )
            .fetch_one(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        Ok(placed)
    }
}

/// Whether a market's prices were last written after `timestamp`
///
/// Asked of the rows themselves rather than of the market, whose stamp is the
/// newest of any of the four kinds of trade message and so says nothing about
/// how fresh the prices are.
async fn newer_on_record(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    market_id: i64,
    timestamp: DateTime<Utc>,
) -> Result<bool, Error> {
    let listed = sqlx::query_scalar!(
        "SELECT MAX(listed_at) FROM commodities WHERE market_id = $1",
        market_id,
    )
    .fetch_one(&mut **tx)
    .await?;

    Ok(listed.is_some_and(|listed| listed > timestamp.naive_utc()))
}
