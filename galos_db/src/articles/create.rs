use super::Article;
use crate::{Database, Error};
use sqlx::types::chrono::NaiveDate;

impl Article {
    pub async fn create(
        db: &Database,
        title: Option<String>,
        date: NaiveDate,
        body: String,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            INSERT INTO articles (title, date, body)
            VALUES ($1, $2, $3)
            RETURNING *
            ",
            title,
            date,
            body
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Article {
            id: row.id,
            title: row.title,
            date: row.date,
            body: row.body,
        })
    }
}
