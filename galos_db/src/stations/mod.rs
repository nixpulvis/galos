//! A station which can be docked at within a system
use chrono::{DateTime, Utc};
use elite_journal::station::{EconomyShare, LandingPads, Service, StationType};
use elite_journal::{Allegiance, Government};

#[derive(Debug, PartialEq)]
pub struct Station {
    pub system_address: i64,
    pub name: String,
    pub ty: Option<StationType>,
    pub dist_from_star_ls: Option<f64>,
    pub market_id: Option<i64>,
    pub landing_pads: Option<LandingPads>,
    pub faction: Option<String>, // TODO: Faction type?
    pub government: Option<Government>, // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    pub economies: Option<Vec<EconomyShare>>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

    /// Which body it sits on, for a settlement
    ///
    /// All four are [`None`] for anything in orbit, which is most of them.
    /// Only `ApproachSettlement` reports them, and it reports nothing else.
    pub body_id: Option<i16>,
    pub body_name: Option<String>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Eq for Station {}

mod create;
mod fetch;
