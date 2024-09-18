use super::Body;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::Body as JournalBody;

impl Body {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        body: &JournalBody,
        system_address: i64,
    ) -> Result<Body, Error> {
        let parent = if let Some(map) = body.parents.get(0) {
            map.values().next()
        } else {
            None
        };

        let row = sqlx::query!(
            "
            INSERT INTO bodies (
                name,
                id,
                parent_id,
                system_address,
                updated_at,
                updated_by,

                planet_class,
                tidal_lock,
                landable,
                terraform_state,
                atmosphere,
                atmosphere_type,
                volcanism,

                mass,
                radius,
                surface_gravity,
                surface_temperature,
                surface_pressure,
                semi_major_axis,
                eccentricity,
                orbital_inclination,
                periapsis,
                orbital_period,
                rotation_period,
                axial_tilt,
                ascending_node,
                mean_anomaly,

                was_mapped,
                was_discovered)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28, $29)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $1,
                parent_id = $3,
                updated_at = $5,
                updated_by = $6,

                planet_class = $7,
                tidal_lock = $8,
                landable = $9,
                terraform_state = $10,
                atmosphere = $11,
                atmosphere_type = $12,
                volcanism = $13,

                mass = $14,
                radius = $15,
                surface_gravity = $16,
                surface_temperature = $17,
                surface_pressure = $18,
                semi_major_axis = $19,
                eccentricity = $20,
                orbital_inclination = $21,
                periapsis = $22,
                orbital_period = $23,
                rotation_period = $24,
                axial_tilt = $25,
                ascending_node = $26,
                mean_anomaly = $27,

                was_mapped = $28,
                was_discovered = $29
            RETURNING *
            ",
            body.name,
            body.id,
            parent,
            system_address,
            timestamp.naive_utc(),
            user,
            body.planet_class,
            body.tidal_lock,
            body.landable,
            body.terraform_state,
            body.atmosphere,
            body.atmosphere_type,
            body.volcanism,
            body.mass,
            body.radius,
            body.surface_gravity,
            body.surface_temperature,
            body.surface_pressure,
            body.semi_major_axis,
            body.eccentricity,
            body.orbital_inclination,
            body.periapsis,
            body.orbital_period,
            body.rotation_period,
            body.axial_tilt,
            body.ascending_node,
            body.mean_anomaly,
            body.was_mapped,
            body.was_discovered
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Body {
            system_address: row.system_address,
            id: row.id,
            parent_id: row.parent_id,
            name: row.name,
            planet_class: row.planet_class,
            tidal_lock: row.tidal_lock,
            landable: row.landable,
            terraform_state: row.terraform_state,
            atmosphere: row.atmosphere,
            atmosphere_type: row.atmosphere_type,
            volcanism: row.volcanism,
            mass: row.mass,
            radius: row.radius,
            surface_gravity: row.surface_gravity,
            surface_temperature: row.surface_temperature,
            surface_pressure: row.surface_pressure,
            semi_major_axis: row.semi_major_axis,
            eccentricity: row.eccentricity,
            orbital_inclination: row.orbital_inclination,
            periapsis: row.periapsis,
            orbital_period: row.orbital_period,
            rotation_period: row.rotation_period,
            axial_tilt: row.axial_tilt,
            ascending_node: row.ascending_node,
            mean_anomaly: row.mean_anomaly,
            was_mapped: row.was_mapped,
            was_discovered: row.was_discovered,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        })
    }
}
