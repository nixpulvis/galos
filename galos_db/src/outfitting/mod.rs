//! The modules a station sells
use chrono::{DateTime, Utc};

/// One module as a station's outfitting bay sells it
#[derive(Debug, PartialEq, Eq)]
pub struct Outfitting {
    pub market_id: i64,
    /// Symbolic name, e.g. `Int_Engine_Size3_Class5_Fast`
    pub module_name: String,
    /// [`None`] where it came from the older of the two live schemas, which
    /// names a module without pricing it
    pub buy_price: Option<i64>,
    pub merc_coins_price: Option<i64>,
    pub listed_at: DateTime<Utc>,
}

mod create;
mod fetch;
