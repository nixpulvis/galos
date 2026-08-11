//! Finding systems: by name, by who holds them, and by what is near one

use super::{quoted, Ask, Info, Query};
use crate::view::{Column, Row, Table, View};
use crate::{Error, Result};
use elite_journal::system::Coordinate;
use galos_db::{escaped, systems::System, Database};

/// Systems, however the user would rather name them
///
/// The three ways of naming compose. `--system` narrows by name, `--faction`
/// narrows by who is present, and `--radius` narrows by where; giving two of
/// them means both, which is the only reading of them that does not surprise
/// somebody.
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Search {
    /// Systems whose name holds this
    #[arg(short = 's', long = "system", value_name = "NAME")]
    pub system: Option<String>,

    /// Systems where a faction whose name holds this is present
    #[arg(short = 'f', long = "faction", value_name = "NAME")]
    pub faction: Option<String>,

    /// Only systems within this many light years of --system
    ///
    /// Measured from one system, so --system must be the whole of a name and
    /// not a fragment of several.
    #[arg(short = 'r', long, value_name = "LY")]
    pub radius: Option<f64>,

    /// How many to answer with
    #[arg(short = 'l', long, default_value_t = 25, value_name = "N")]
    pub limit: i64,

    /// Say how many there are rather than which they are
    #[arg(short = 'c', long)]
    pub count: bool,
}

impl Search {
    /// The systems this asks for, and what to measure them from
    ///
    /// The centre comes back with them because a distance column is only
    /// meaningful against something, and the something is the system a radius
    /// was drawn around.
    async fn find(
        &self,
        db: &Database,
    ) -> Result<(Vec<System>, Option<Coordinate>)> {
        match (&self.system, &self.faction, self.radius) {
            (None, None, _) => Err(Error::Nonsense(
                "search needs --system or --faction".into(),
            )),

            // A radius is drawn around a system, so the name has to be one.
            (None, Some(_), Some(_)) => Err(Error::Nonsense(
                "--radius needs a --system to be measured from".into(),
            )),

            (Some(name), faction, Some(radius)) => {
                let centre = super::locate(db, name).await?;
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
                // about from somewhere, and what is close to that somewhere is
                // what it was asked for.
                found.sort_by(|a, b| {
                    reach(position, a).total_cmp(&reach(position, b))
                });
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
                Ok((found, None))
            }

            (Some(name), None, None) => Ok((
                System::search_by_name(db, name, None, self.limit).await?,
                None,
            )),
        }
    }
}

impl Ask for Search {
    async fn ask(&self, db: &Database) -> Result<View> {
        let (found, centre) = self.find(db).await?;
        let total = found.len();

        if self.count {
            return Ok(View::new(self.title()).with(
                crate::view::Section::Note(format!(
                    "{} system{} found",
                    super::tally(total as u64),
                    if total == 1 { "" } else { "s" }
                )),
            ));
        }

        let table = table(found.iter().take(self.limit as usize), centre);
        let shown = table.rows.len();
        let view = View::new(self.title()).with(table);
        Ok(if shown < total {
            view.noting(format!(
                "{shown} of {} systems, by --limit",
                super::tally(total as u64)
            ))
        } else if shown as i64 == self.limit {
            // Cut exactly at the limit, the database was not asked whether
            // there was a next one, so this cannot promise there is not.
            view.noting(format!("{shown} systems, up to the --limit"))
        } else {
            view.noting(format!(
                "{shown} system{}",
                if shown == 1 { "" } else { "s" }
            ))
        })
    }

    fn command(&self) -> String {
        let mut line = String::from("galos search");
        if let Some(system) = &self.system {
            line += &format!(" -s {}", quoted(system));
        }
        if let Some(faction) = &self.faction {
            line += &format!(" -f {}", quoted(faction));
        }
        if let Some(radius) = self.radius {
            line += &format!(" -r {radius}");
        }
        if self.limit != 25 {
            line += &format!(" -l {}", self.limit);
        }
        if self.count {
            line += " -c";
        }
        line
    }
}

impl Search {
    /// A search for systems holding `name`
    ///
    /// What a faction row leads to, and the reason [`Factions`] answers with
    /// faction names rather than with the systems behind them: the systems
    /// are one enter away and are their own page when you get there.
    ///
    /// [`Factions`]: super::Factions
    pub fn by_faction(name: &str) -> Self {
        Search {
            system: None,
            faction: Some(name.to_string()),
            radius: None,
            limit: 25,
            count: false,
        }
    }

    fn title(&self) -> String {
        match (&self.system, &self.faction, self.radius) {
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
}

/// Systems as rows, each leading to what is known about it
///
/// Here rather than in [`Search::ask`] because a list of systems is not only
/// ever the answer to a search: asking for one by an ambiguous name answers
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
            system.position.map(super::place).unwrap_or_else(|| "-".into()),
            super::tally(system.population),
            super::or_dash(system.allegiance),
            super::or_dash(system.economies),
        ]);
        table
            .push(Row::new(cells).linking(Query::Info(Info::of(&system.name))));
    }
    table
}

/// The addresses of every system a faction named like `faction` is present in
async fn held_by(
    db: &Database,
    faction: &str,
) -> Result<std::collections::HashSet<i64>> {
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
