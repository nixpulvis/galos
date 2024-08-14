use crate::{Database, Error, Page};
use sqlx::types::chrono::NaiveDate;
use super::Article;

impl Article {
    pub async fn fetch(db: &Database, id: i32) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM articles
            WHERE id = $1
            ",
            id
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

    pub async fn fetch_all(db: &Database, page: Page) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM articles
            LIMIT $1 OFFSET $2
            ",
            page.limit,
            page.offset
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Article {
                id: row.id,
                title: row.title,
                date: row.date,
                body: row.body,
            })
            .collect())
    }

    // TODO: https://github.com/chronotope/chrono/issues/152
    // TODO: Add pages to this as well.
    pub async fn fetch_dates(
        db: &Database,
        from: NaiveDate,
        to: NaiveDate,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM articles
            WHERE date BETWEEN $1 AND $2
            ",
            from,
            to
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Article {
                id: row.id,
                title: row.title,
                date: row.date,
                body: row.body,
            })
            .collect())
    }

    pub async fn delete(&self, db: &Database) -> Result<(), Error> {
        sqlx::query!(
            "
            DELETE FROM articles
            WHERE id = $1
            ",
            self.id
        )
        .execute(&db.pool)
        .await?;

        Ok(())
    }
}
