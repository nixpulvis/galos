//! A station which can be docked at within a system
use chrono::{DateTime, Utc};
use elite_journal::station::{LandingPads, EconomyShare, Service, StationType};
use elite_journal::{Allegiance, Government};

#[derive(Debug, PartialEq)]
pub struct Station {
    pub system_address: i64,
    pub name: String,
    pub ty: Option<StationType>,
    pub dist_from_star_ls: Option<f64>,
    pub market_id: Option<i64>,
    pub landing_pads: Option<LandingPads>,
    pub faction: Option<String>,        // TODO: Faction type?
    pub government: Option<Government>, // TODO: Government type?
    pub allegiance: Option<Allegiance>,
    pub services: Option<Vec<Service>>,
    pub economies: Option<Vec<EconomyShare>>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,
}

impl Eq for Station {}

mod create;
mod fetch;
