//! Looking for bodies by name, wherever they orbit

use super::{Kind, Look};
use crate::query::{bodies, quoted, Home};
use crate::view::Table;
use crate::Result;
use galos_db::{bodies::Body, Database};

/// Planets, moons and stars, by any part of their name
#[derive(clap::Args, Clone, Debug, PartialEq)]
pub struct Bodies {
    /// Bodies whose name holds this
    #[arg(value_name = "NAME")]
    pub name: String,
}

impl Bodies {
    /// Bodies whose name holds `name`
    pub fn named(name: &str) -> Self {
        Bodies { name: name.to_string() }
    }
}

impl Look for Bodies {
    fn kind(&self) -> &'static str {
        "Body"
    }

    fn title(&self) -> String {
        format!("Bodies matching {}", self.name)
    }

    async fn look(&self, db: &Database, limit: i64) -> Result<Table> {
        let found = Body::search_by_name(db, &self.name, limit).await?;
        // A body is named after the system holding it, so most of a name
        // typed here is a system name and the systems are worth naming.
        let addresses: Vec<i64> =
            found.iter().map(|body| body.system_address).collect();
        let systems = crate::query::homes(db, &addresses).await?;
        Ok(bodies::table(&found, Home::Across(&systems)))
    }

    fn arguments(&self) -> String {
        format!(" body {}", quoted(&self.name))
    }
}

impl From<Bodies> for Kind {
    fn from(look: Bodies) -> Kind {
        Kind::Body(look)
    }
}
