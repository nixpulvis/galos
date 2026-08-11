//! Somewhere to dock in a system

use super::{quoted, Ask};
use crate::view::{Column, Row, Table, View};
use crate::Result;
use galos_db::{stations::Station, Database};

/// The stations of one system
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Stations {
    /// The system, whole or in part
    #[arg(value_name = "SYSTEM")]
    pub system: String,
}

impl Stations {
    /// The stations of the system called `name`
    pub fn of(name: &str) -> Self {
        Stations { system: name.to_string() }
    }
}

impl Ask for Stations {
    async fn ask(&self, db: &Database) -> Result<View> {
        let system = super::locate(db, &self.system).await?;
        let stations = table(db, system.address).await?;
        let count = stations.rows.len();
        Ok(View::new(format!("Stations of {}", system.name))
            .with(stations)
            .noting(format!(
                "{count} station{}",
                if count == 1 { "" } else { "s" }
            )))
    }

    fn command(&self) -> String {
        format!("galos stations {}", quoted(&self.system))
    }
}

/// The stations of a system as rows, nearest to the star first
///
/// Shared with the system's own page, for the same reason its bodies are:
/// what a station is worth saying about it is one decision, and a system page
/// showing a different four columns than the stations command is two.
pub(crate) async fn table(db: &Database, address: i64) -> Result<Table> {
    let mut stations = Station::fetch_all(db, address).await?;
    stations.sort_by(|a, b| {
        a.dist_from_star_ls
            .unwrap_or(f64::INFINITY)
            .total_cmp(&b.dist_from_star_ls.unwrap_or(f64::INFINITY))
    });

    let mut table = Table::new([
        Column::text("Station"),
        Column::text("Type"),
        Column::number("Distance"),
        Column::text("Pads"),
        Column::text("Faction"),
        Column::text("Allegiance"),
    ])
    .or_else("no stations on record");

    for station in &stations {
        table.push(Row::new([
            station.name.clone(),
            super::or_dash(station.ty.clone()),
            station
                .dist_from_star_ls
                .map(|ls| format!("{ls:.0} Ls"))
                .unwrap_or_else(|| "-".into()),
            station.landing_pads.map(pads).unwrap_or_else(|| "-".into()),
            station.faction.clone().unwrap_or_else(|| "-".into()),
            super::or_dash(station.allegiance),
        ]));
    }

    Ok(table)
}

/// What can land, in the width of a column
///
/// [`LandingPads`] writes itself out as `Large: 1, Medium: 4, Small: 2`,
/// which is a sentence. Down a column the words are the same on every row and
/// only the numbers differ, so only the numbers are kept.
///
/// [`LandingPads`]: elite_journal::station::LandingPads
fn pads(pads: elite_journal::station::LandingPads) -> String {
    format!("{}L {}M {}S", pads.large, pads.medium, pads.small)
}
