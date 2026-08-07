use super::Barycenter;
use crate::orbit;
use crate::{Database, Error};

impl Barycenter {
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM barycenters
            WHERE system_address = $1
            ",
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Barycenter {
                system_address: row.system_address,
                id: row.id,
                updated_at: row.updated_at.and_utc(),
                updated_by: row.updated_by,
                orbit: orbit::read(
                    row.semi_major_axis,
                    row.eccentricity,
                    row.orbital_inclination,
                    row.periapsis,
                    row.orbital_period,
                    row.ascending_node,
                    row.mean_anomaly,
                ),
            })
            .collect())
    }
}
