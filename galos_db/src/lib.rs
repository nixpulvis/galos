#![cfg(unix)]
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::env;

pub mod error;
pub use self::error::{Error, Result};

pub struct Database {
    pub(crate) pool: PgPool,
}

impl Database {
    pub async fn new() -> Result<Self> {
        dotenv::dotenv()?;
        let url = env::var("DATABASE_URL")?;

        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&url)
            .await?;

        Ok(Database { pool })
    }

    pub async fn from_url(url: &str) -> Result<Self> {
        let pool = PgPoolOptions::new().max_connections(5).connect(url).await?;

        Ok(Database { pool })
    }
}

pub struct Page {
    pub limit: i64,
    pub offset: i64,
}

impl Page {
    pub fn by(limit: i64) -> Self {
        Page { limit, offset: 0 }
    }

    pub fn turn(&self, n: i64) -> Self {
        Page {
            limit: self.limit,
            offset: self.offset + n,
        }
    }
}

pub mod articles;
pub mod bodies;
pub mod factions;
pub mod markets;
pub mod stations;
pub mod systems;
