use chrono::{DateTime, Utc};
use elite_journal::body::Body as JournalBody;
use crate::{Error, Database};

#[derive(Debug, PartialEq, Eq)]
pub struct Body {
    pub address: i64,
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

impl Body {
    pub async fn create(db: &Database, name: &str, system_address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            INSERT INTO bodies (name, system_address, updated_at)
            VALUES ($1, $2, $3)
            RETURNING *
            ",
            name, system_address, Utc::now().naive_utc())
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        body: &JournalBody,
        system_address: i64)
        -> Result<Body, Error>
    {
        let parent = if let Some(map) = body.parents.get(0) {
            map.values().next()
        } else {
            None
        };

        let row = sqlx::query!(
            "
            INSERT INTO bodies (name, id, parent_id, system_address, updated_at)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING *
            ",
            body.name, body.id, parent, system_address, timestamp.naive_utc())
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch(db: &Database, address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE address = $1
            ", address)
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch_by_name_and_system_address(db: &Database, name: &str, system_address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE lower(name) = $1
            AND   system_address = $2
            ", name.to_lowercase(), system_address)
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch_like_name(db: &Database, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM bodies
            WHERE name ILIKE $1
            ORDER BY name
            "#, name)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.into_iter().map(|row| {
            Body {
                address: row.address,
                system_address: row.system_address,
                id: row.id,
                name: row.name,
                updated_at: DateTime::from_utc(row.updated_at, Utc)
            }
        }).collect())
    }
}
