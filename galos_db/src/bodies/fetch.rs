use super::Body;
use crate::{Database, Error};

impl Body {
    pub async fn fetch(db: &Database, system_address: i64, id: i16) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE system_address = $1 AND id = $2
            ",
            system_address,
            id
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

    pub async fn fetch_all(db: &Database, system_address: i64) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM bodies
            WHERE system_address = $1
            "#,
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Body {
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
            .collect())
    }

    pub async fn fetch_like_name_and_system_address(
        db: &Database,
        system_address: i64,
        name: &str,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE system_address = $1 AND lower(name) ILIKE $2
            ",
            system_address,
            name.to_lowercase()
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

    pub async fn fetch_like_name(db: &Database, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM bodies
            WHERE name ILIKE $1
            ORDER BY name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows
            .into_iter()
            .map(|row| Body {
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
            .collect())
    }
}
