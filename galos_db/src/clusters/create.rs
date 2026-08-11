use super::Cluster;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::Cluster as JournalCluster;

impl Cluster {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        cluster: &JournalCluster,
        system_address: i64,
    ) -> Result<Cluster, Error> {
        // A scan names each ancestor as a one entry map of kind to id, nearest
        // first, and is kept in that order for the reason a body's are: the
        // walk back to the star is what places the cluster.
        let (parent_types, parent_ids): (Vec<String>, Vec<i16>) = cluster
            .parents
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some((ty.clone(), *id))
            })
            .unzip();

        let row = sqlx::query!(
            "
            INSERT INTO clusters (
                system_address,
                id,
                name,
                updated_at,
                updated_by,

                distance_from_arrival,
                was_discovered,
                was_mapped,
                parent_ids,
                parent_types)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $3,
                updated_at = $4,
                updated_by = $5,

                distance_from_arrival = $6,
                was_discovered = $7,
                was_mapped = $8,
                parent_ids = $9,
                parent_types = $10
            RETURNING *
            ",
            system_address,
            cluster.id,
            cluster.name,
            timestamp.naive_utc(),
            user,
            cluster.distance_from_arrival,
            cluster.discovery.discovered,
            cluster.discovery.mapped,
            &parent_ids[..],
            &parent_types[..],
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Cluster {
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
    }
}
