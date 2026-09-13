use super::Star;
use crate::bodies::ancestry;
use crate::orbit;
use crate::{Database, Error};
use elite_journal::body::Spin;

impl Star {
    /// The one star with this id in this system
    pub async fn fetch(
        db: &Database,
        system_address: i64,
        id: i16,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM stars
            WHERE system_address = $1 AND id = $2
            ",
            system_address,
            id
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(star!(row))
    }

    /// Every star in a system
    ///
    /// What the map asks for on its way in to a system, alongside the bodies:
    /// a star is what a body goes round, and there may be several.
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM stars
            WHERE system_address = $1
            ",
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(|row| star!(row)).collect())
    }
}
