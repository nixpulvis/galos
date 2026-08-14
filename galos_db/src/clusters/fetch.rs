use super::Cluster;
use crate::{Database, Error};

impl Cluster {
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM clusters
            WHERE system_address = $1
            ",
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Cluster {
                system_address: row.system_address,
                id: row.id,
                name: row.name,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
                distance_from_arrival: row.distance_from_arrival,
                discovered: row.was_discovered,
                mapped: row.was_mapped,
                parent_ids: row.parent_ids.unwrap_or_default(),
                parent_types: row.parent_types.unwrap_or_default(),
            })
            .collect())
    }
}
