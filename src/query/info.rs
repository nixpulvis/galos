//! Everything on record about one system

use super::{quoted, Ask, Query};
use crate::view::{Column, Fields, Row, Section, Table, View};
use crate::{Error, Result};
use galos_db::{factions::SystemFaction, systems::System, Database};

/// How much of a system's bodies and stations a system's own page shows
///
/// A well surveyed system has a hundred bodies and its page is not a list of
/// them; it is the handful of facts about the system, with enough of the rest
/// to see what kind of place it is. Past this the page says how many there
/// are and which command shows them all, and in the terminal UI that command
/// is one keypress away on the row under the cursor.
const PREVIEW: usize = 8;

/// One system, named
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Info {
    /// The system, whole or in part
    #[arg(value_name = "SYSTEM")]
    pub system: String,
}

impl Info {
    /// What is known about the system called `name`
    pub fn of(name: &str) -> Self {
        Info { system: name.to_string() }
    }
}

impl Ask for Info {
    async fn ask(&self, db: &Database) -> Result<View> {
        let system = match super::locate(db, &self.system).await {
            Ok(system) => system,
            // Several systems hold the fragment, so the honest answer to
            // "which one is this" is the list of the ones it could be. Their
            // rows lead here, so the ambiguity is resolved by pressing enter
            // on the one that was meant.
            Err(Error::Nonsense(_)) => return self.candidates(db).await,
            Err(err) => return Err(err),
        };

        let mut view = View::new(system.name.clone()).with(
            Fields::new()
                .and("address", system.address)
                .maybe("position", system.position.map(super::place))
                .and("population", super::tally(system.population))
                .maybe("security", system.security)
                .maybe("government", system.government)
                .maybe("allegiance", system.allegiance)
                .maybe("economy", system.economies)
                .and("updated", system.updated_at.format("%Y-%m-%d %H:%M"))
                .and("source", &system.updated_by),
        );

        if let Some(factions) = self.factions(db, system.address).await? {
            view = view.with(factions);
        }
        if let Some(stations) = self.stations(db, &system).await? {
            view = view.with(stations);
        }
        if let Some(bodies) = self.bodies(db, &system).await? {
            view = view.with(bodies);
        }

        Ok(view)
    }

    fn command(&self) -> String {
        format!("galos info {}", quoted(&self.system))
    }
}

impl Info {
    /// The systems the name could have meant
    async fn candidates(&self, db: &Database) -> Result<View> {
        let holding =
            System::search_by_name(db, &self.system, None, 25).await?;
        Ok(View::new(format!("Systems matching {}", self.system))
            .with(super::search::table(holding.iter(), None))
            .noting("several systems hold that name; pick one"))
    }

    /// Who is present, and how much of it they hold
    ///
    /// Ordered by influence by the database, which is the order they are worth
    /// reading in: the faction running the place is the one being asked about.
    async fn factions(
        &self,
        db: &Database,
        address: i64,
    ) -> Result<Option<Section>> {
        let present = SystemFaction::fetch_all(db, Some(address)).await?;
        if present.is_empty() {
            return Ok(None);
        }

        let mut table = Table::new([
            Column::text("Faction"),
            Column::number("Influence"),
            Column::text("State"),
            Column::text("Happiness"),
        ])
        .captioned("Factions");
        for (name, faction) in &present {
            table.push(
                Row::new([
                    name.clone(),
                    format!("{:.1}%", faction.influence * 100.),
                    faction
                        .state
                        .map(|s| format!("{s:?}"))
                        .unwrap_or_else(|| "-".into()),
                    faction
                        .happiness
                        .map(|h| format!("{h:?}"))
                        .unwrap_or_else(|| "-".into()),
                ])
                // Following a faction asks where else it is, which is the
                // question a name in this list raises.
                .linking(Query::Search(super::Search::by_faction(name))),
            );
        }
        Ok(Some(table.into()))
    }

    /// Somewhere to dock, if anything docks here
    async fn stations(
        &self,
        db: &Database,
        system: &System,
    ) -> Result<Option<Section>> {
        let stations = super::stations::table(db, system.address).await?;
        Ok(clipped(
            stations,
            "Stations",
            Query::Stations(super::Stations::of(&system.name)),
        ))
    }

    /// What is in orbit, if anything has been scanned
    async fn bodies(
        &self,
        db: &Database,
        system: &System,
    ) -> Result<Option<Section>> {
        let bodies = super::bodies::table(db, system).await?;
        Ok(clipped(
            bodies,
            "Bodies",
            Query::Bodies(super::Bodies::of(&system.name)),
        ))
    }
}

/// A table cut to [`PREVIEW`] rows, saying where the rest of them are
///
/// Nothing at all where there are no rows: a system with no stations on
/// record says so by not having a stations heading, which is shorter than
/// saying it and reads the same.
fn clipped(mut table: Table, caption: &str, whole: Query) -> Option<Section> {
    let total = table.rows.len();
    if total == 0 {
        return None;
    }

    let caption = if total > PREVIEW {
        table.rows.truncate(PREVIEW);
        format!("{caption} ({PREVIEW} of {total})")
    } else {
        caption.to_string()
    };

    // The row that leads to the rest of them, drawn as one of the list so
    // that the terminal UI's cursor reaches it the way it reaches the others.
    if total > PREVIEW {
        let width = table.columns.len();
        let mut more = vec![format!("… {} more", total - PREVIEW)];
        more.resize(width, String::new());
        table.push(Row::new(more).linking(whole));
    }

    Some(table.captioned(caption).into())
}
