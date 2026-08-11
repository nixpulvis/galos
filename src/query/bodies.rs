//! What is in orbit around a system

use super::{quoted, Ask};
use crate::view::{Column, Row, Table, View};
use crate::Result;
use galos_db::{bodies::Body, systems::System, Database};

/// The bodies of one system
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Bodies {
    /// The system, whole or in part
    #[arg(value_name = "SYSTEM")]
    pub system: String,
}

impl Bodies {
    /// The bodies of the system called `name`
    pub fn of(name: &str) -> Self {
        Bodies { system: name.to_string() }
    }
}

impl Ask for Bodies {
    async fn ask(&self, db: &Database) -> Result<View> {
        let system = super::locate(db, &self.system).await?;
        let bodies = table(db, &system).await?;
        let count = bodies.rows.len();
        Ok(View::new(format!("Bodies of {}", system.name)).with(bodies).noting(
            format!("{count} bod{}", if count == 1 { "y" } else { "ies" }),
        ))
    }

    fn command(&self) -> String {
        format!("galos bodies {}", quoted(&self.system))
    }
}

/// The bodies of a system as rows, arrival star first
///
/// Shared with the system's own page, which shows the first few of these
/// under the same headings. Two lists of the same things with different
/// columns is the thing the whole arrangement exists to avoid, and a system
/// page is where it would happen first.
///
/// The rows lead nowhere. A body has a page's worth to say about it —
/// atmosphere, materials, the orbit it keeps — and nothing asks for one yet,
/// so they are left unlinked rather than pointed somewhere approximate: what
/// the cursor stops on is what can be followed.
pub(crate) async fn table(db: &Database, system: &System) -> Result<Table> {
    let mut bodies = Body::fetch_all(db, system.address).await?;
    // Outward from the arrival star, which is the order they are flown past
    // in and the order a system is learned in. Unscanned distances sort last
    // rather than at the star.
    bodies.sort_by(|a, b| {
        a.distance_from_arrival
            .unwrap_or(f32::INFINITY)
            .total_cmp(&b.distance_from_arrival.unwrap_or(f32::INFINITY))
    });

    let mut table = Table::new([
        Column::text("Body"),
        Column::text("Type"),
        Column::text("Class"),
        Column::number("Distance"),
        Column::number("Gravity"),
        Column::number("Temp"),
    ])
    .or_else("no bodies scanned");

    for body in &bodies {
        table.push(Row::new([
            short(&body.name, &system.name),
            super::or_dash(body.body_type.clone()),
            if body.planet_class.is_empty() {
                "-".into()
            } else {
                body.planet_class.clone()
            },
            body.distance_from_arrival
                .map(|ls| format!("{ls:.0} Ls"))
                .unwrap_or_else(|| "-".into()),
            format!("{:.2}", body.gravity),
            body.temperature
                .map(|k| format!("{k:.0} K"))
                .unwrap_or_else(|| "-".into()),
        ]));
    }

    Ok(table)
}

/// A body's name with its system's taken off the front
///
/// Bodies are named after the system they are in, so a column of them in a
/// page already headed by that system spends its width saying `Col 285 Sector
/// KR-V b4-2` over and over and its last two characters saying which body.
/// The arrival star, which is named exactly for its system, keeps its name.
fn short(body: &str, system: &str) -> String {
    match body.strip_prefix(system).map(str::trim) {
        Some(rest) if !rest.is_empty() => rest.to_string(),
        _ => body.to_string(),
    }
}
