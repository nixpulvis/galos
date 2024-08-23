//! A station which can be docked at within a system
use chrono::{DateTime, Utc};

#[derive(Debug, PartialEq, Eq)]
pub struct Market {
    pub id: i64,
    pub system_address: i64,
    pub station_name: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Listing {
    pub market_id: i64,
    pub name: String,
    pub mean_price: i64,
    pub buy_price: i64,
    pub sell_price: i64,
    pub demand: i64,
    pub demand_bracket: i64,
    pub stock: i64,
    pub stock_bracket: i64,
    pub updated_at: i64,
}

mod create;
// mod fetch;
