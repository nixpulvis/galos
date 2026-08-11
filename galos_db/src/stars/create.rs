use super::Star;
use crate::bodies::Parent;
use crate::orbit;
use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::{Discovery, Spin, Star as JournalStar};

impl Star {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        user: &str,
        star: &JournalStar,
        system_address: i64,
    ) -> Result<Star, Error> {
        let parents = Parent::chain(&star.parents);
        let (parent_ids, parent_types) = Parent::columns(&parents);
        let parent_id = parent_ids.first().copied();
        let orbit = star.orbit.as_ref();

        let row = sqlx::query!(
            "
            INSERT INTO stars (
                system_address,
                id,
                name,
                parent_id,
                parent_ids,
                parent_types,
                updated_at,
                updated_by,

                absolute_magnitude,
                age_my,
                distance_from_arrival_ls,
                luminosity,
                star_class,
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
                temperature,

                was_mapped,
                was_discovered)
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17,
                $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $3,
                parent_id = $4,
                parent_ids = $5,
                parent_types = $6,
                updated_at = $7,
                updated_by = $8,

                absolute_magnitude = $9,
                age_my = $10,
                distance_from_arrival_ls = $11,
                luminosity = $12,
                star_class = $13,
                stellar_mass = $14,
                subclass = $15,

                ascending_node = COALESCE($16, stars.ascending_node),
                axial_tilt = $17,
                eccentricity = COALESCE($18, stars.eccentricity),
                mean_anomaly = COALESCE($19, stars.mean_anomaly),
                orbital_inclination = COALESCE($20, stars.orbital_inclination),
                orbital_period = COALESCE($21, stars.orbital_period),
                periapsis = COALESCE($22, stars.periapsis),
                radius = $23,
                rotation_period = $24,
                semi_major_axis = COALESCE($25, stars.semi_major_axis),
                temperature = $26,

                was_mapped = stars.was_mapped OR $27,
                was_discovered = stars.was_discovered OR $28
            RETURNING *
            ",
            system_address,
            star.id,
            star.name,
            parent_id,
            &parent_ids,
            &parent_types,
            timestamp.naive_utc(),
            user,
            star.absolute_magnitude,
            star.age_my,
            star.distance_from_arrival_ls,
            star.luminosity,
            star.star_class,
            star.stellar_mass,
            star.subclass,
            orbit.and_then(|orbit| orbit.ascending_node),
            star.spin.tilt,
            orbit.map(|orbit| orbit.eccentricity),
            orbit.and_then(|orbit| orbit.mean_anomaly),
            orbit.map(|orbit| orbit.orbital_inclination),
            orbit.map(|orbit| orbit.orbital_period),
            orbit.map(|orbit| orbit.periapsis),
            star.radius,
            star.spin.period,
            orbit.map(|orbit| orbit.semi_major_axis),
            star.temperature,
            star.discovery.mapped,
            star.discovery.discovered,
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(Star {
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            parents,
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,

            absolute_magnitude: row.absolute_magnitude,
            age_my: row.age_my,
            distance_from_arrival_ls: row.distance_from_arrival_ls,
            luminosity: row.luminosity,
            star_class: row.star_class,
            stellar_mass: row.stellar_mass,
            subclass: row.subclass,

            orbit: orbit::read(
                row.semi_major_axis,
                row.eccentricity,
                row.orbital_inclination,
                row.periapsis,
                row.orbital_period,
                row.ascending_node,
                row.mean_anomaly,
            ),
            spin: Spin { period: row.rotation_period, tilt: row.axial_tilt },
            radius: row.radius,
            temperature: row.temperature,
            discovery: Discovery {
                discovered: row.was_discovered,
                mapped: row.was_mapped,
            },
        })
    }
}
