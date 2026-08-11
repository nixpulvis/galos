use super::Ring;
use crate::{Database, Error};
use elite_journal::body::Orbit;

impl Ring {
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM rings
            WHERE system_address = $1
            ",
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Ring {
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
            .collect())
    }
}
