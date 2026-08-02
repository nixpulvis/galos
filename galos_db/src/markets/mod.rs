//! A station which can be docked at within a system
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq, Eq)]
pub struct Market {
    pub id: i64,
    /// The system this market's station sits in, once it is known
    ///
    /// A market message names its system but cannot give an address, so a
    /// market can be recorded before the system it belongs to exists.
    pub system_address: Option<i64>,
    pub system_name: String,
    pub station_name: String,
    pub updated_at: DateTime<Utc>,
}

/// One commodity as a market trades it
#[derive(Debug, PartialEq, Eq)]
pub struct Commodity {
    pub market_id: i64,
    pub name: String,
    pub mean_price: i32,
    pub buy_price: i32,
    pub sell_price: i32,
    pub demand: i32,
    pub demand_bracket: i32,
    pub stock: i32,
    pub stock_bracket: i32,
    pub listed_at: DateTime<Utc>,
}

mod create;
// mod fetch;
