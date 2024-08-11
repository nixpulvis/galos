use crate::{Database, Error};
use chrono::{DateTime, Utc};
use elite_journal::body::Body as JournalBody;

#[derive(Debug, PartialEq)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    pub parent_id: Option<i16>,
    pub name: String,
    pub updated_at: DateTime<Utc>,

    pub planet_class: String,
    pub tidal_lock: bool,
    pub landable: bool,
    pub terraform_state: Option<String>,
    pub atmosphere: Option<String>,
    pub atmosphere_type: String,
    pub volcanism: Option<String>,

    pub mass: f32,
    pub radius: f32,
    pub surface_gravity: f32,
    pub surface_temperature: f32,
    pub surface_pressure: f32,
    pub semi_major_axis: f32,
    pub eccentricity: f32,
    pub orbital_inclination: f32,
    pub periapsis: f32,
    pub orbital_period: f32,
    pub rotation_period: f32,
    pub axial_tilt: f32,
    pub ascending_node: f32,
    pub mean_anomaly: f32,

    pub was_mapped: bool,
    pub was_discovered: bool,
}

impl Eq for Body {}

impl Body {
    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
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
                $18, $19, $20, $21, $22, $23, $24, $25, $26, $27, $28)
            ON CONFLICT (system_address, id)
            DO UPDATE SET
                name = $1,
                parent_id = $3,
                updated_at = $5,

                planet_class = $6,
                tidal_lock = $7,
                landable = $8,
                terraform_state = $9,
                atmosphere = $10,
                atmosphere_type = $11,
                volcanism = $12,

                mass = $13,
                radius = $14,
                surface_gravity = $15,
                surface_temperature = $16,
                surface_pressure = $17,
                semi_major_axis = $18,
                eccentricity = $19,
                orbital_inclination = $20,
                periapsis = $21,
                orbital_period = $22,
                rotation_period = $23,
                axial_tilt = $24,
                ascending_node = $25,
                mean_anomaly = $26,

                was_mapped = $27,
                was_discovered = $28
            RETURNING *
            ",
            body.name,
            body.id,
            parent,
            system_address,
            timestamp.naive_utc(),
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
        })
    }

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
            })
            .collect())
    }
}
