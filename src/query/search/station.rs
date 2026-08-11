//! Looking for stations by name, wherever they are docked

use super::{Kind, Look};
use crate::query::{quoted, stations, Home};
use crate::view::Table;
use crate::Result;
use galos_db::{stations::Station, Database};

/// Ports, outposts and carriers, by any part of their name
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Stations {
    /// Stations whose name holds this
    #[arg(value_name = "NAME")]
    pub name: String,
}

impl Stations {
    /// Stations whose name holds `name`
    pub fn named(name: &str) -> Self {
        Stations { name: name.to_string() }
    }
}

impl Look for Stations {
    fn kind(&self) -> &'static str {
        "Station"
    }

    fn title(&self) -> String {
        format!("Stations matching {}", self.name)
    }

    async fn look(&self, db: &Database, limit: i64) -> Result<Table> {
        let found = Station::search_by_name(db, &self.name, limit).await?;
        // Which system each is in, before the rows are written rather than
        // per row: a page of stations would otherwise be a page of queries.
        let addresses: Vec<i64> =
            found.iter().map(|station| station.system_address).collect();
        let systems = crate::query::homes(db, &addresses).await?;
        Ok(stations::table(&found, Home::Across(&systems)))
    }

    fn arguments(&self) -> String {
        format!(" station {}", quoted(&self.name))
    }
}

impl From<Stations> for Kind {
    fn from(look: Stations) -> Kind {
        Kind::Station(look)
    }
}
