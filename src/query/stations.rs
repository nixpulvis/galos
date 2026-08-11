//! Somewhere to dock in a system

use super::{quoted, Ask, Home};
use crate::view::{Column, Row, Table, View};
use crate::Result;
use galos_db::{stations::Station, Database};

/// The stations of one system
///
/// Not a search — the question is what is in a place, not what something is
/// called, and `galos search station <NAME>` is the other one.
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
        let stations = Station::fetch_all(db, system.address).await?;
        let count = stations.len();
        Ok(View::new(format!("Stations of {}", system.name))
            .with(table(&stations, Home::Within(&system.name)))
            .noting(format!(
                "{count} station{}",
                if count == 1 { "" } else { "s" }
            )))
    }

    fn command(&self) -> String {
        format!("galos stations {}", quoted(&self.system))
    }
}

/// Stations as rows
///
/// Shared by the system's own page and by the search that finds stations
/// across systems, for the same reason the bodies are: what is worth saying
/// about a station is one decision.
pub(crate) fn table(stations: &[Station], home: Home) -> Table {
    let mut columns = vec![Column::text("Station")];
    if home.is_across() {
        columns.push(Column::text("System"));
    }
    columns.extend([
        Column::text("Type"),
        Column::number("Distance"),
        Column::text("Pads"),
        Column::text("Faction"),
        Column::text("Allegiance"),
    ]);

    let mut table = Table::new(columns).or_else("no stations on record");
    for station in home.ordered(stations, |station| {
        station.dist_from_star_ls.map(|ls| ls as f32)
    }) {
        // A station is named for itself rather than for its system, so the
        // name is left whole where a body's is trimmed.
        let mut cells = vec![station.name.clone()];
        if let Some(system) = home.named(station.system_address) {
            cells.push(system);
        }
        cells.extend([
            super::or_dash(station.ty.clone()),
            station
                .dist_from_star_ls
                .map(|ls| format!("{ls:.0} Ls"))
                .unwrap_or_else(|| "-".into()),
            station.landing_pads.map(pads).unwrap_or_else(|| "-".into()),
            station.faction.clone().unwrap_or_else(|| "-".into()),
            super::or_dash(station.allegiance),
        ]);
        table.push(Row::new(cells));
    }

    table
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
