use super::Cluster;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::Cluster as JournalCluster;

impl Cluster {
    /// `discovered_at` is worked out by the caller rather than read off the
    /// scan. A scan reporting the cluster undiscovered is itself the
    /// discovery, so the reading is the enclosing entry's timestamp, and only
    /// something holding that entry can say which that is.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        cluster: &JournalCluster,
        system_address: i64,
        discovered_at: Option<DateTime<Utc>>,
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
                was_mapped,
                parent_ids,
                parent_types,
                discovered_at)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                -- A message delivered late is still taken, for whatever it
                -- fills in below. What it does not do is put the reading back
                -- in time, or call the row what it was called when it was
                -- sent.
                name = CASE WHEN $4 >= clusters.updated_at
                    THEN $3 ELSE clusters.name END,
                updated_at = GREATEST(clusters.updated_at, $4),
                updated_by = CASE WHEN $4 >= clusters.updated_at
                    THEN $5 ELSE clusters.updated_by END,

                distance_from_arrival =
                    COALESCE($6, clusters.distance_from_arrival),
                -- `was_mapped` only ever goes up. A scan that finds a cluster
                -- already mapped is knowledge, and one that finds it unmapped
                -- is not evidence that it has stayed that way.
                was_mapped = clusters.was_mapped OR $7,
                parent_ids = COALESCE($8, clusters.parent_ids),
                parent_types = COALESCE($9, clusters.parent_types),
                -- The earliest claim on record wins, and `LEAST` ignores a
                -- null, so a scan that says nothing about discovery leaves
                -- what is there alone.
                discovered_at = LEAST(clusters.discovered_at, $10)
            RETURNING *
            ",
            system_address,
            cluster.id,
            cluster.name,
            timestamp.naive_utc(),
            user,
            cluster.distance_from_arrival,
            cluster.discovery.mapped,
            (!parent_ids.is_empty()).then_some(&parent_ids[..]),
            (!parent_types.is_empty()).then_some(&parent_types[..]),
            discovered_at.map(|at| at.naive_utc()),
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
            mapped: row.was_mapped,
            discovered_at: row.discovered_at.map(|at| at.and_utc()),
            parent_ids: row.parent_ids.unwrap_or_default(),
            parent_types: row.parent_types.unwrap_or_default(),
        })
    }
}
