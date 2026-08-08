//! The ships a station sells
use chrono::{DateTime, Utc};

/// One ship as a station's shipyard sells it
///
/// Names only. A ship costs the same everywhere, so the schema carries no
/// price and there is nothing else to keep.
#[derive(Debug, PartialEq, Eq)]
pub struct Shipyard {
    pub market_id: i64,
    /// Symbolic name, e.g. `Federation_Corvette`
    pub ship_name: String,
    pub listed_at: DateTime<Utc>,
}

mod create;
mod fetch;
