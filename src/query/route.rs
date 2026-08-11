//! Plotting a way between two systems

use super::{quoted, Ask, Info, Query};
use crate::view::{Column, Row, Table, View};
use crate::{Error, Result};
use galos_db::{systems::System, Database};
use itertools::Itertools;

/// A way from one system to another, in jumps
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Route {
    /// Where the route starts
    #[arg(value_name = "FROM")]
    pub start: String,

    /// Where the route ends
    #[arg(value_name = "TO")]
    pub end: String,

    /// How far the ship jumps, in light years
    #[arg(default_value_t = 7.5, short = 'r', long, value_name = "LY")]
    pub range: f64,
}

impl Ask for Route {
    async fn ask(&self, db: &Database) -> Result<View> {
        let start = placed(db, &self.start).await?;
        let end = placed(db, &self.end).await?;

        // The search walks the graph a jump at a time and asks the database
        // for the neighbours of every system it reaches, blocking as it goes.
        // Held off the thread that is drawing, or the terminal UI stops
        // repainting for as long as a long route takes to find.
        let (found, range) = (db.clone(), self.range);
        let (from, to) = (start.clone(), end.clone());
        let plotted = async_std::task::spawn_blocking(move || {
            from.route_to(&found, &to, range)
        })
        .await;

        let Some((route, jumps)) = plotted else {
            return Err(Error::NoRoute {
                start: start.name,
                end: end.name,
                range: self.range,
            });
        };

        let mut table = Table::new([
            Column::text("Origin"),
            Column::text("Destination"),
            Column::number("Distance"),
        ])
        .or_else("nowhere to go");

        let mut travelled = 0.;
        for (a, b) in route.iter().tuple_windows() {
            let leg = a.distance(b);
            travelled += leg;
            table.push(
                Row::new([
                    a.name.clone(),
                    b.name.clone(),
                    format!("{leg:.2} Ly"),
                ])
                // Following a leg asks about where it lands, which is the
                // thing worth knowing about a stop on a route.
                .linking(Query::Info(Info::of(&b.name))),
            );
        }

        Ok(View::new(format!("{} to {}", start.name, end.name))
            .with(table)
            .noting(format!(
                "{:.0} jumps, {:.2} Ly flown, {:.2} Ly apart",
                jumps,
                travelled,
                start.distance(&end)
            )))
    }

    fn command(&self) -> String {
        let mut line = format!(
            "galos route {} {}",
            quoted(&self.start),
            quoted(&self.end)
        );
        if self.range != 7.5 {
            line += &format!(" -r {}", self.range);
        }
        line
    }
}

/// The one system that name means, if it is anywhere
///
/// Both ends go through this before anything is plotted, so that a route
/// which cannot be found says which end it could not have. Three quarters of
/// the systems on record have no coordinates — Sol among them — and one of
/// those as an endpoint is not a failed search, it is a system nothing can be
/// measured to.
async fn placed(db: &Database, name: &str) -> Result<System> {
    let system = super::locate(db, name).await?;
    match system.position {
        Some(_) => Ok(system),
        None => Err(Error::Unplaced { name: system.name }),
    }
}
