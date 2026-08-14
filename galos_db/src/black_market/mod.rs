//! What a station's black market pays
//!
//! Kept apart from [`crate::markets::Commodity`] rather than folded into it.
//! The goods are mostly ones no legal market lists, so there is usually no row
//! there to fold into; and a commodity message is read as the whole of what a
//! station trades and clears what it does not mention, which would wipe these
//! on every legal market update.
use chrono::{DateTime, Utc};

/// One commodity as a station's black market takes it
#[derive(Debug, PartialEq, Eq)]
pub struct BlackMarket {
    pub market_id: i64,
    pub name: String,
    pub sell_price: i32,
    /// Whether the commodity is illegal at this station
    pub prohibited: bool,
    pub listed_at: DateTime<Utc>,
}

mod create;
mod fetch;
