use std::env;
use serde::Deserialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::postgres::PgListener;

pub mod error;
pub use self::error::{Error, Result};

#[derive(Clone)]
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

    pub async fn listen<T: for<'a> Deserialize<'a>>(
        &self,
        stream: &str,
        func: impl Fn(Result<T>))
        -> Result<()>
    {
        let mut listener = PgListener::connect_with(&self.pool).await?;
        listener.listen(stream).await?;

        loop {
            while let Some(notification) = listener.try_recv().await? {
                let payload = notification.payload().to_owned();
                let result = serde_json::from_str::<T>(&payload).map_err(|e| e.into());
                func(result);
            }
        }

        Ok(())
    }
}

#[derive(Deserialize, Debug)]
pub struct Update<T> {
    table: String,
    action: String,
    row: T,
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
pub mod systems;
pub mod stars;
pub mod bodies;
pub mod factions;
pub mod markets;
pub mod stations;
