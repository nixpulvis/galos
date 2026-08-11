//! Looking for star systems: by name, by who holds them, by what is near one

use super::{Kind, Look};
use crate::query::{quoted, Info, Query};
use crate::view::{Column, Row, Table};
use crate::{Error, Result};
use elite_journal::system::Coordinate;
use galos_db::{escaped, systems::System, Database};
use std::collections::HashSet;

/// Star systems, however the user would rather name them
///
/// The three ways of naming compose. A name narrows by name, `--faction`
/// narrows by who is present, and `--radius` narrows by where; giving two of
/// them means both, which is the only reading that surprises nobody.
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Systems {
    /// Systems whose name holds this
    #[arg(value_name = "NAME")]
    pub name: Option<String>,

    /// Only systems where a faction whose name holds this is present
    #[arg(short = 'f', long = "faction", value_name = "NAME")]
    pub faction: Option<String>,

    /// Only systems within this many light years of the named one
    ///
    /// Measured from one system, so the name has to be a whole one rather
    /// than a fragment several answer to.
    #[arg(short = 'r', long, value_name = "LY")]
    pub radius: Option<f64>,
}

impl Systems {
    /// Systems whose name holds `name`
    pub fn named(name: &str) -> Self {
        Systems { name: Some(name.to_string()), faction: None, radius: None }
    }

    /// Systems a faction named like `faction` is present in
    pub fn held_by(faction: &str) -> Self {
        Systems { name: None, faction: Some(faction.to_string()), radius: None }
    }

    /// The systems this asks for, and what to measure them from
    ///
    /// The centre comes back with them because a distance column means
    /// nothing on its own, and what it means is the system a radius was drawn
    /// around.
    async fn find(
        &self,
        db: &Database,
        limit: i64,
    ) -> Result<(Vec<System>, Option<Coordinate>)> {
        match (&self.name, &self.faction, self.radius) {
            (None, None, _) => Err(Error::Nonsense(
                "search system needs a name or --faction".into(),
            )),

            // A radius is drawn around a system, so the name has to be one.
            (None, Some(_), Some(_)) => Err(Error::Nonsense(
                "--radius needs a system for it to be measured from".into(),
            )),

            (Some(name), faction, Some(radius)) => {
                let centre = crate::query::locate(db, name).await?;
                let position = centre
                    .position
                    .ok_or_else(|| Error::Unplaced { name: centre.name })?;
                let mut found = System::fetch_in_range_of_point(
                    db,
                    radius,
                    [position.x, position.y, position.z],
                    None,
                    None,
                )
                .await?;
                if let Some(faction) = faction {
                    let held = held_by(db, faction).await?;
                    found.retain(|system| held.contains(&system.address));
                }
                // Nearest first, the centre itself leading: a radius is asked
                // about from somewhere, and what is close to that somewhere
                // is what it was asked for.
                found.sort_by(|a, b| {
                    reach(position, a).total_cmp(&reach(position, b))
                });
                found.truncate(limit as usize);
                Ok((found, Some(position)))
            }

            (name, Some(faction), None) => {
                let mut found =
                    System::fetch_faction(db, &like(faction)).await?;
                if let Some(name) = name {
                    let name = name.to_lowercase();
                    found.retain(|system| {
                        system.name.to_lowercase().contains(&name)
                    });
                }
                found.sort_by(|a, b| a.name.cmp(&b.name));
                found.truncate(limit as usize);
                Ok((found, None))
            }

            (Some(name), None, None) => {
                Ok((System::search_by_name(db, name, None, limit).await?, None))
            }
        }
    }
}

impl Look for Systems {
    fn kind(&self) -> &'static str {
        "System"
    }

    fn title(&self) -> String {
        match (&self.name, &self.faction, self.radius) {
            (Some(name), _, Some(radius)) => {
                format!("Systems within {radius} Ly of {name}")
            }
            (Some(name), Some(faction), None) => {
                format!("Systems matching {name}, held by {faction}")
            }
            (None, Some(faction), _) => format!("Systems held by {faction}"),
            (Some(name), None, None) => format!("Systems matching {name}"),
            (None, None, _) => "Systems".into(),
        }
    }

    async fn look(&self, db: &Database, limit: i64) -> Result<Table> {
        let (found, centre) = self.find(db, limit).await?;
        Ok(table(found.iter(), centre))
    }

    fn arguments(&self) -> String {
        let mut line = String::from(" system");
        if let Some(name) = &self.name {
            line += &format!(" {}", quoted(name));
        }
        if let Some(faction) = &self.faction {
            line += &format!(" -f {}", quoted(faction));
        }
        if let Some(radius) = self.radius {
            line += &format!(" -r {radius}");
        }
        line
    }
}

impl From<Systems> for Kind {
    fn from(look: Systems) -> Kind {
        Kind::System(look)
    }
}

/// Systems as rows, each leading to what is known about it
///
/// Here rather than in [`Look::look`] because a list of systems is not only
/// ever the answer to a search: asking about one by an ambiguous name answers
/// with the several it could have been, and that list wants the same columns
/// under the same headings. The moment it is written twice, one of them gains
/// a column.
///
/// `centre` adds the distance column, which exists only where there is
/// somewhere for a distance to be from.
pub(crate) fn table<'a>(
    systems: impl Iterator<Item = &'a System>,
    centre: Option<Coordinate>,
) -> Table {
    let mut columns = vec![Column::text("System")];
    if centre.is_some() {
        columns.push(Column::number("Distance"));
    }
    columns.extend([
        Column::text("Position"),
        Column::number("Population"),
        Column::text("Allegiance"),
        Column::text("Economy"),
    ]);

    let mut table = Table::new(columns).or_else("no systems found");
    for system in systems {
        let mut cells = vec![system.name.clone()];
        if let Some(centre) = centre {
            cells.push(match system.position {
                Some(p) => format!("{:.2} Ly", distance(centre, p)),
                None => "-".into(),
            });
        }
        cells.extend([
            system
                .position
                .map(crate::query::place)
                .unwrap_or_else(|| "-".into()),
            crate::query::tally(system.population),
            crate::query::or_dash(system.allegiance),
            crate::query::or_dash(system.economies),
        ]);
        table
            .push(Row::new(cells).linking(Query::Info(Info::of(&system.name))));
    }
    table
}

/// The addresses of every system a faction named like `faction` is present in
async fn held_by(db: &Database, faction: &str) -> Result<HashSet<i64>> {
    Ok(System::fetch_faction(db, &like(faction))
        .await?
        .into_iter()
        .map(|system| system.address)
        .collect())
}

/// A fragment of a name, as the pattern matching every name holding it
///
/// `fetch_faction` matches with `ILIKE` and adds no wildcards of its own, so
/// a bare name would only ever find the faction called exactly that. Escaped
/// first, or a `%` somebody typed as part of a name would widen the search
/// they meant to narrow.
fn like(faction: &str) -> String {
    format!("%{}%", escaped(faction))
}

/// How far apart two points are, in light years
fn distance(a: Coordinate, b: Coordinate) -> f64 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2) + (b.z - a.z).powi(2)).sqrt()
}

/// How far a system is from a point, with the unplaceable ones sorting last
fn reach(from: Coordinate, system: &System) -> f64 {
    system.position.map(|p| distance(from, p)).unwrap_or(f64::INFINITY)
}
