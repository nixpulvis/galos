use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::system::Star as JournalStar;
use super::Star;

impl Star {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        star: &JournalStar,
        system_address: i64,
    ) -> Result<Star, Error> {
        let parent = if let Some(map) = star.parents.get(0) {
            map.values().next()
        } else {
            None
        };

        let row = sqlx::query!(
            "
            INSERT INTO stars (
                system_address,
                id,
                name,
                parent_id,
                updated_at,
                updated_by,

                absolute_magnitude,
                age_my,
                distance_from_arrival_ls,
                luminosity,
                star_type,
                stellar_mass,
                subclass,

                ascending_node,
                axial_tilt,
                eccentricity,
                mean_anomaly,
                orbital_inclination,
                orbital_period,
                periapsis,
                radius,
                rotation_period,
                semi_major_axis,
                surface_temperature,

                was_mapped,
                was_discovered)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                $18, $19, $20, $21, $22, $23, $24, $25, $26)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $3,
                parent_id = $4,
                updated_at = $5,
                updated_by = $6,

                absolute_magnitude = $7,
                age_my = $8,
                distance_from_arrival_ls = $9,
                luminosity = $10,
                star_type = $11,
                stellar_mass = $12,
                subclass = $13,

                ascending_node = $14,
                axial_tilt = $15,
                eccentricity = $16,
                mean_anomaly = $17,
                orbital_inclination = $18,
                orbital_period = $19,
                periapsis = $20,
                radius = $21,
                rotation_period = $22,
                semi_major_axis = $23,
                surface_temperature = $24,

                was_mapped = $25,
                was_discovered = $26
            RETURNING *
            ",
            system_address,
            star.id,
            star.name,
            parent,
            timestamp.naive_utc(),
            user,
            star.absolute_magnitude,
            star.age_my,
            // TODO: rename _ls (lowercase 's').
            star.distance_from_arrival_lS,
            star.luminosity,
            star.star_type,
            star.stellar_mass,
            star.subclass,
            star.ascending_node,
            star.axial_tilt,
            star.eccentricity,
            star.mean_anomaly,
            star.orbital_inclination,
            star.orbital_period,
            star.periapsis,
            star.radius,
            star.rotation_period,
            star.semi_major_axis,
            star.surface_temperature,
            star.was_discovered,
            star.was_mapped,
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Star {
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            parent_id: row.parent_id,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,

            absolute_magnitude: row.absolute_magnitude,
            age_my: row.age_my,
            distance_from_arrival_ls: row.distance_from_arrival_ls,
            luminosity: row.luminosity,
            star_type: row.star_type,
            stellar_mass: row.stellar_mass,
            subclass: row.subclass,

            ascending_node: row.ascending_node,
            axial_tilt: row.axial_tilt,
            eccentricity: row.eccentricity,
            mean_anomaly: row.mean_anomaly,
            orbital_inclination: row.orbital_inclination,
            orbital_period: row.orbital_period,
            periapsis: row.periapsis,
            radius: row.radius,
            rotation_period: row.rotation_period,
            semi_major_axis: row.semi_major_axis,
            surface_temperature: row.surface_temperature,

            was_mapped: row.was_mapped,
            was_discovered: row.was_discovered,
        })
    }
}
