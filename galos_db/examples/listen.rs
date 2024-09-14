use std::env;
use serde::Deserialize;
use sqlx::types::chrono::NaiveDate;
use sqlx::postgres::PgListener;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::Pool;
use sqlx::Postgres;
use galos_db::{Database, Update, Result, Error};
use galos_db::systems::System;

#[async_std::main]
async fn main() -> Result<()> {
    let db = Database::new().await?;
    db.listen("systems_update", |event: Result<Update<System>>| {
        match event {
            Ok(su) => {
                dbg!(su);
            }
            Err(e) => {
                dbg!(e);
            }
        }
    }).await?;
    Ok(())
}
