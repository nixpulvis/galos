use super::Ring;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::Orbit;
use elite_journal::entry::incremental::exploration::Ring as JournalRing;

impl Ring {
    /// `discovered_at` is worked out by the caller rather than read off the
    /// scan. A scan reporting the ring undiscovered is itself the discovery,
    /// so the reading is the enclosing entry's timestamp, and only something
    /// holding that entry can say which that is.
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        ring: &JournalRing,
        system_address: i64,
        discovered_at: Option<DateTime<Utc>>,
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
                was_mapped,
                parent_ids,
                parent_types,
                discovered_at,

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
                -- A message delivered late is still taken, for whatever it
                -- fills in below. What it does not do is put the reading back
                -- in time, or call the row what it was called when it was
                -- sent.
                name = CASE WHEN $4 >= rings.updated_at
                    THEN $3 ELSE rings.name END,
                updated_at = GREATEST(rings.updated_at, $4),
                updated_by = CASE WHEN $4 >= rings.updated_at
                    THEN $5 ELSE rings.updated_by END,

                distance_from_arrival =
                    COALESCE($6, rings.distance_from_arrival),
                -- `was_mapped` only ever goes up. A scan that finds a ring
                -- already mapped is knowledge, and one that finds it unmapped
                -- is not evidence that it has stayed that way.
                was_mapped = rings.was_mapped OR $7,
                parent_ids = COALESCE($8, rings.parent_ids),
                parent_types = COALESCE($9, rings.parent_types),
                -- The earliest claim on record wins, and `LEAST` ignores a
                -- null, so a scan that says nothing about discovery leaves
                -- what is there alone.
                discovered_at = LEAST(rings.discovered_at, $10),

                semi_major_axis = $11,
                eccentricity = $12,
                orbital_inclination = $13,
                periapsis = $14,
                orbital_period = $15,
                ascending_node = COALESCE($16, rings.ascending_node),
                mean_anomaly = COALESCE($17, rings.mean_anomaly)
            RETURNING *
            ",
            system_address,
            ring.id,
            ring.name,
            timestamp.naive_utc(),
            user,
            ring.distance_from_arrival,
            ring.discovery.mapped,
            (!parent_ids.is_empty()).then_some(&parent_ids[..]),
            (!parent_types.is_empty()).then_some(&parent_types[..]),
            discovered_at.map(|at| at.naive_utc()),
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
            mapped: row.was_mapped,
            discovered_at: row.discovered_at.map(|at| at.and_utc()),
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
