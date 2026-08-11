use super::Ring;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::Orbit;
use elite_journal::entry::incremental::exploration::Ring as JournalRing;

impl Ring {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        ring: &JournalRing,
        system_address: i64,
    ) -> Result<Ring, Error> {
        // A scan names each ancestor as a one entry map of kind to id, nearest
        // first, and is kept in that order for the reason a body's are: the
        // walk back to the star is what places the ring.
        let (parent_types, parent_ids): (Vec<String>, Vec<i16>) = ring
            .parents
            .iter()
            .filter_map(|parent| {
                let (ty, id) = parent.iter().next()?;
                Some((ty.clone(), *id))
            })
            .unzip();

        let row = sqlx::query!(
            "
            INSERT INTO rings (
                system_address,
                id,
                name,
                updated_at,
                updated_by,

                distance_from_arrival,
                was_discovered,
                was_mapped,
                parent_ids,
                parent_types,

                semi_major_axis,
                eccentricity,
                orbital_inclination,
                periapsis,
                orbital_period,
                ascending_node,
                mean_anomaly)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13,
                $14, $15, $16, $17)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $3,
                updated_at = $4,
                updated_by = $5,

                distance_from_arrival =
                    COALESCE($6, rings.distance_from_arrival),
                was_discovered = rings.was_discovered OR $7,
                was_mapped = rings.was_mapped OR $8,
                parent_ids = $9,
                parent_types = $10,

                semi_major_axis = $11,
                eccentricity = $12,
                orbital_inclination = $13,
                periapsis = $14,
                orbital_period = $15,
                ascending_node = $16,
                mean_anomaly = $17
            RETURNING *
            ",
            system_address,
            ring.id,
            ring.name,
            timestamp.naive_utc(),
            user,
            ring.distance_from_arrival,
            ring.discovery.discovered,
            ring.discovery.mapped,
            &parent_ids[..],
            &parent_types[..],
            ring.orbit.semi_major_axis,
            ring.orbit.eccentricity,
            ring.orbit.orbital_inclination,
            ring.orbit.periapsis,
            ring.orbit.orbital_period,
            ring.orbit.ascending_node,
            ring.orbit.mean_anomaly,
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Ring {
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
            orbit: Orbit {
                semi_major_axis: row.semi_major_axis,
                eccentricity: row.eccentricity,
                orbital_inclination: row.orbital_inclination,
                periapsis: row.periapsis,
                orbital_period: row.orbital_period,
                ascending_node: row.ascending_node,
                mean_anomaly: row.mean_anomaly,
            },
        })
    }
}
