//! Looking for minor factions by name

use super::{Kind, Look, Search};
use crate::query::{quoted, Query};
use crate::view::{Column, Row, Table};
use crate::Result;
use galos_db::{factions::Faction, Database};

/// Minor factions, by any part of their name
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Factions {
    /// Factions whose name holds this
    #[arg(value_name = "NAME")]
    pub name: String,
}

impl Factions {
    /// Factions whose name holds `name`
    pub fn named(name: &str) -> Self {
        Factions { name: name.to_string() }
    }
}

impl Look for Factions {
    fn kind(&self) -> &'static str {
        "Faction"
    }

    fn title(&self) -> String {
        format!("Factions matching {}", self.name)
    }

    async fn look(&self, db: &Database, limit: i64) -> Result<Table> {
        Ok(table(&Faction::search_by_name(db, &self.name, limit).await?))
    }

    fn arguments(&self) -> String {
        format!(" faction {}", quoted(&self.name))
    }
}

impl From<Factions> for Kind {
    fn from(look: Factions) -> Kind {
        Kind::Faction(look)
    }
}

/// Factions as rows, each leading to where it is
pub(crate) fn table(factions: &[Faction]) -> Table {
    let mut table = Table::new([Column::text("Faction"), Column::number("Id")])
        .or_else("no factions found");
    for faction in factions {
        table.push(
            Row::new([faction.name.clone(), faction.id.to_string()])
                // A faction on its own is a name and a number; what makes it
                // worth looking up is where it is, so that is where following
                // one goes.
                .linking(Query::Search(Search::systems_of(&faction.name))),
        );
    }
    table
}
