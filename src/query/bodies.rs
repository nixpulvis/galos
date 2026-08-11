//! What is in orbit around a system

use super::{quoted, Ask, Home};
use crate::view::{Column, Row, Table, View};
use crate::Result;
use galos_db::{bodies::Body, Database};

/// The bodies of one system
///
/// Not a search — the question is what is in a place, not what something is
/// called, and `galos search body <NAME>` is the other one.
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
        let bodies = Body::fetch_all(db, system.address).await?;
        let count = bodies.len();
        Ok(View::new(format!("Bodies of {}", system.name))
            .with(table(&bodies, Home::Within(&system.name)))
            .noting(format!(
                "{count} bod{}",
                if count == 1 { "y" } else { "ies" }
            )))
    }

    fn command(&self) -> String {
        format!("galos bodies {}", quoted(&self.system))
    }
}

/// Bodies as rows
///
/// Shared by the system's own page, which shows the first few of these, and
/// by the search that finds bodies across systems. Three lists of the same
/// things with different columns is the thing the whole arrangement exists to
/// avoid, and bodies are where it would happen first.
///
/// The rows lead nowhere. A body has a page's worth to say about it —
/// atmosphere, materials, the orbit it keeps — and nothing asks for one yet,
/// so they are left unlinked rather than pointed somewhere approximate: what
/// the cursor stops on is what can be followed.
pub(crate) fn table(bodies: &[Body], home: Home) -> Table {
    let mut columns = vec![Column::text("Body")];
    if home.is_across() {
        columns.push(Column::text("System"));
    }
    columns.extend([
        Column::text("Type"),
        Column::text("Class"),
        Column::number("Distance"),
        Column::number("Gravity"),
        Column::number("Temp"),
    ]);

    let mut table = Table::new(columns).or_else("no bodies scanned");
    for body in home.ordered(bodies, |body| body.distance_from_arrival) {
        let mut cells = vec![home.short(&body.name)];
        if let Some(system) = home.named(body.system_address) {
            cells.push(system);
        }
        cells.extend([
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
        ]);
        table.push(Row::new(cells));
    }

    table
}
