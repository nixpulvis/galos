//! A station which can be docked at within a system
use chrono::{DateTime, Utc};
use elite_journal::entry::market::Commodity;

#[derive(Debug, PartialEq)]
pub struct Market {
    pub id: i64,
    pub system_address: i64,
    pub station_name: String,
    pub updated_at: DateTime<Utc>,
    pub commodities: Option<Vec<Commodity>>,
}

impl Eq for Market {}

mod create;
// mod fetch;
