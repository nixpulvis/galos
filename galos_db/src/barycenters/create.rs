use super::Barycenter;
use crate::orbit;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::entry::incremental::exploration::ScanBaryCentre;

impl Barycenter {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        scan: &ScanBaryCentre,
    ) -> Result<Barycenter, Error> {
        let scanned = scan.orbit.as_ref();

        let row = sqlx::query!(
            "
            INSERT INTO barycenters (
                system_address,
                id,
                updated_at,
                updated_by,

                semi_major_axis,
                eccentricity,
                orbital_inclination,
                periapsis,
                orbital_period,
                ascending_node,
                mean_anomaly)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                updated_at = $3,
                updated_by = $4,

                semi_major_axis = $5,
                eccentricity = $6,
                orbital_inclination = $7,
                periapsis = $8,
                orbital_period = $9,
                ascending_node = $10,
                mean_anomaly = $11
            RETURNING *
            ",
            scan.system_address,
            scan.body_id,
            timestamp.naive_utc(),
            user,
            scanned.map(|orbit| orbit.semi_major_axis),
            scanned.map(|orbit| orbit.eccentricity),
            scanned.map(|orbit| orbit.orbital_inclination),
            scanned.map(|orbit| orbit.periapsis),
            scanned.map(|orbit| orbit.orbital_period),
            scanned.map(|orbit| orbit.ascending_node),
            scanned.map(|orbit| orbit.mean_anomaly),
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Barycenter {
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
    }
}
