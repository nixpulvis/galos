//! Finding factions by name

use super::{quoted, Ask, Query, Search};
use crate::view::{Column, Row, Table, View};
use crate::Result;
use galos_db::{factions::Faction, Database};

/// Factions, by any part of their name
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Factions {
    /// Factions whose name holds this
    #[arg(value_name = "NAME")]
    pub name: String,

    /// How many to answer with
    #[arg(short = 'l', long, default_value_t = 25, value_name = "N")]
    pub limit: i64,
}

impl Ask for Factions {
    async fn ask(&self, db: &Database) -> Result<View> {
        let found = Faction::search_by_name(db, &self.name, self.limit).await?;

        let mut table =
            Table::new([Column::text("Faction"), Column::number("Id")])
                .or_else("no factions found");
        for faction in &found {
            table.push(
                Row::new([faction.name.clone(), faction.id.to_string()])
                    // A faction on its own is a name and a number; what makes
                    // it worth looking up is where it is, so that is where
                    // following one goes.
                    .linking(Query::Search(Search::by_faction(&faction.name))),
            );
        }

        let count = found.len();
        Ok(View::new(format!("Factions matching {}", self.name))
            .with(table)
            .noting(format!(
                "{count} faction{}{}",
                if count == 1 { "" } else { "s" },
                if count as i64 == self.limit {
                    ", up to the --limit"
                } else {
                    ""
                }
            )))
    }

    fn command(&self) -> String {
        let mut line = format!("galos factions {}", quoted(&self.name));
        if self.limit != 25 {
            line += &format!(" -l {}", self.limit);
        }
        line
    }
}
