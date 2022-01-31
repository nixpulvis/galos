use chrono::{DateTime, Utc};
use elite_journal::body::Body as JournalBody;
use crate::{Error, Database};

#[derive(Debug, PartialEq, Eq)]
pub struct Body {
    pub address: i64,
    pub system_address: i64,
    pub id: i16,
    pub name: String,
    pub updated_at: DateTime<Utc>,
}

impl Body {
    pub async fn create(db: &Database, name: &str, system_address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            INSERT INTO bodies (name, system_address, updated_at)
            VALUES ($1, $2, $3)
            RETURNING *
            ",
            name, system_address, Utc::now().naive_utc())
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn from_journal(
        db: &Database,
        timestamp: DateTime<Utc>,
        body: &JournalBody,
        system_address: i64)
        -> Result<Body, Error>
    {
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
            body.name, body.id, parent, system_address, timestamp.naive_utc(), body.planet_class,
            body.tidal_lock, body.landable, body.terraform_state, body.atmosphere,
            body.atmosphere_type, body.volcanism, body.mass, body.radius, body.surface_gravity,
            body.surface_temperature, body.surface_pressure, body.semi_major_axis,
            body.eccentricity, body.orbital_inclination, body.periapsis, body.orbital_period,
            body.rotation_period, body.axial_tilt, body.ascending_node, body.mean_anomaly,
            body.was_mapped, body.was_discovered)
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch(db: &Database, address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE address = $1
            ", address)
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch_by_name_and_system_address(db: &Database, name: &str, system_address: i64) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM bodies
            WHERE lower(name) = $1
            AND   system_address = $2
            ", name.to_lowercase(), system_address)
            .fetch_one(&db.pool)
            .await?;

        Ok(Body {
            address: row.address,
            system_address: row.system_address,
            id: row.id,
            name: row.name,
            updated_at: DateTime::from_utc(row.updated_at, Utc)
        })
    }

    pub async fn fetch_like_name(db: &Database, name: &str) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            r#"
            SELECT *
            FROM bodies
            WHERE name ILIKE $1
            ORDER BY name
            "#, name)
            .fetch_all(&db.pool)
            .await?;

        Ok(rows.into_iter().map(|row| {
            Body {
                address: row.address,
                system_address: row.system_address,
                id: row.id,
                name: row.name,
                updated_at: DateTime::from_utc(row.updated_at, Utc)
            }
        }).collect())
    }
}
