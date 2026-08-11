use super::{composition, Body, Parent, Surface};
use crate::{Database, Error};
use chrono::NaiveDateTime;
use elite_journal::body::{Discovery, Material, Orbit, Spin};

/// A body as the table holds it, with what it is made of gathered alongside
///
/// The four queries below differ only in what they select on, so the reading
/// of a row is written once here rather than at each of them.
struct Row {
    system_address: i64,
    id: i16,
    name: String,
    parent_ids: Option<Vec<i16>>,
    parent_types: Option<Vec<String>>,
    body_type: Option<String>,
    distance_from_arrival: Option<f32>,
    updated_at: NaiveDateTime,
    updated_by: String,

    planet_class: String,
    tidal_lock: bool,
    landable: bool,
    terraform_state: Option<String>,
    atmosphere: Option<String>,
    atmosphere_type: Option<String>,
    volcanism: Option<String>,

    mass: f32,
    radius: f32,
    gravity: f32,
    temperature: Option<f32>,
    surface_pressure: Option<f32>,
    composition_ice: Option<f32>,
    composition_rock: Option<f32>,
    composition_metal: Option<f32>,
    semi_major_axis: f32,
    eccentricity: f32,
    orbital_inclination: f32,
    periapsis: f32,
    orbital_period: f32,
    rotation_period: f32,
    axial_tilt: f32,
    ascending_node: f32,
    mean_anomaly: f32,

    was_mapped: bool,
    was_discovered: bool,

    material_names: Vec<String>,
    material_percents: Vec<f64>,
}

impl From<Row> for Body {
    fn from(row: Row) -> Self {
        // The ids are what a chain is walked by. The kinds went unrecorded
        // until they were stored alongside, so a row may have the one without
        // the other, and the ids are what decides how long the chain is.
        let types = row.parent_types.unwrap_or_default();
        let parents = row
            .parent_ids
            .unwrap_or_default()
            .into_iter()
            .enumerate()
            .map(|(depth, id)| Parent { ty: types.get(depth).cloned(), id })
            .collect();

        let materials = row
            .material_names
            .into_iter()
            .zip(row.material_percents)
            .map(|(name, percent)| Material { name, percent })
            .collect();

        Body {
            system_address: row.system_address,
            id: row.id,
            parents,
            name: row.name,
            body_type: row.body_type.map(|ty| ty.as_str().into()),
            distance_from_arrival: row.distance_from_arrival,
            planet_class: row.planet_class,
            tidal_lock: row.tidal_lock,
            surface: Surface::read(
                row.atmosphere_type,
                row.surface_pressure,
                composition(
                    row.composition_ice,
                    row.composition_rock,
                    row.composition_metal,
                ),
                row.landable,
                row.atmosphere,
                row.volcanism,
                row.terraform_state,
                materials,
            ),
            mass: row.mass,
            radius: row.radius,
            gravity: row.gravity,
            temperature: row.temperature,

            orbit: Orbit {
                semi_major_axis: row.semi_major_axis,
                eccentricity: row.eccentricity,
                orbital_inclination: row.orbital_inclination,
                periapsis: row.periapsis,
                orbital_period: row.orbital_period,
                ascending_node: row.ascending_node,
                mean_anomaly: row.mean_anomaly,
            },
            spin: Spin { period: row.rotation_period, tilt: row.axial_tilt },
            discovery: Discovery {
                discovered: row.was_discovered,
                mapped: row.was_mapped,
            },
            updated_at: row.updated_at.and_utc(),
            updated_by: row.updated_by,
        }
    }
}

impl Body {
    pub async fn fetch(
        db: &Database,
        system_address: i64,
        id: i16,
    ) -> Result<Self, Error> {
        let row = sqlx::query_as!(
            Row,
            r#"
            SELECT
                b.system_address,
                b.id,
                b.name,
                b.parent_ids,
                b.parent_types,
                b.body_type,
                b.distance_from_arrival,
                b.updated_at,
                b.updated_by,
                b.planet_class,
                b.tidal_lock,
                b.landable,
                b.terraform_state,
                b.atmosphere,
                b.atmosphere_type,
                b.volcanism,
                b.mass,
                b.radius,
                b.gravity,
                b.temperature,
                b.surface_pressure,
                b.composition_ice,
                b.composition_rock,
                b.composition_metal,
                b.semi_major_axis,
                b.eccentricity,
                b.orbital_inclination,
                b.periapsis,
                b.orbital_period,
                b.rotation_period,
                b.axial_tilt,
                b.ascending_node,
                b.mean_anomaly,
                b.was_mapped,
                b.was_discovered,
                COALESCE(ARRAY_AGG(m.name ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_names!: Vec<String>",
                COALESCE(ARRAY_AGG(m.percent ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_percents!: Vec<f64>"
            FROM bodies b
            LEFT JOIN body_materials m
                ON m.system_address = b.system_address AND m.body_id = b.id
            WHERE b.system_address = $1 AND b.id = $2
            GROUP BY b.system_address, b.id
            "#,
            system_address,
            id
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(row.into())
    }

    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query_as!(
            Row,
            r#"
            SELECT
                b.system_address,
                b.id,
                b.name,
                b.parent_ids,
                b.parent_types,
                b.body_type,
                b.distance_from_arrival,
                b.updated_at,
                b.updated_by,
                b.planet_class,
                b.tidal_lock,
                b.landable,
                b.terraform_state,
                b.atmosphere,
                b.atmosphere_type,
                b.volcanism,
                b.mass,
                b.radius,
                b.gravity,
                b.temperature,
                b.surface_pressure,
                b.composition_ice,
                b.composition_rock,
                b.composition_metal,
                b.semi_major_axis,
                b.eccentricity,
                b.orbital_inclination,
                b.periapsis,
                b.orbital_period,
                b.rotation_period,
                b.axial_tilt,
                b.ascending_node,
                b.mean_anomaly,
                b.was_mapped,
                b.was_discovered,
                COALESCE(ARRAY_AGG(m.name ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_names!: Vec<String>",
                COALESCE(ARRAY_AGG(m.percent ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_percents!: Vec<f64>"
            FROM bodies b
            LEFT JOIN body_materials m
                ON m.system_address = b.system_address AND m.body_id = b.id
            WHERE b.system_address = $1
            GROUP BY b.system_address, b.id
            "#,
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }

    pub async fn fetch_like_name_and_system_address(
        db: &Database,
        system_address: i64,
        name: &str,
    ) -> Result<Self, Error> {
        let row = sqlx::query_as!(
            Row,
            r#"
            SELECT
                b.system_address,
                b.id,
                b.name,
                b.parent_ids,
                b.parent_types,
                b.body_type,
                b.distance_from_arrival,
                b.updated_at,
                b.updated_by,
                b.planet_class,
                b.tidal_lock,
                b.landable,
                b.terraform_state,
                b.atmosphere,
                b.atmosphere_type,
                b.volcanism,
                b.mass,
                b.radius,
                b.gravity,
                b.temperature,
                b.surface_pressure,
                b.composition_ice,
                b.composition_rock,
                b.composition_metal,
                b.semi_major_axis,
                b.eccentricity,
                b.orbital_inclination,
                b.periapsis,
                b.orbital_period,
                b.rotation_period,
                b.axial_tilt,
                b.ascending_node,
                b.mean_anomaly,
                b.was_mapped,
                b.was_discovered,
                COALESCE(ARRAY_AGG(m.name ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_names!: Vec<String>",
                COALESCE(ARRAY_AGG(m.percent ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_percents!: Vec<f64>"
            FROM bodies b
            LEFT JOIN body_materials m
                ON m.system_address = b.system_address AND m.body_id = b.id
            WHERE b.system_address = $1 AND lower(b.name) ILIKE $2
            GROUP BY b.system_address, b.id
            "#,
            system_address,
            name.to_lowercase()
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(row.into())
    }

    pub async fn fetch_like_name(
        db: &Database,
        name: &str,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query_as!(
            Row,
            r#"
            SELECT
                b.system_address,
                b.id,
                b.name,
                b.parent_ids,
                b.parent_types,
                b.body_type,
                b.distance_from_arrival,
                b.updated_at,
                b.updated_by,
                b.planet_class,
                b.tidal_lock,
                b.landable,
                b.terraform_state,
                b.atmosphere,
                b.atmosphere_type,
                b.volcanism,
                b.mass,
                b.radius,
                b.gravity,
                b.temperature,
                b.surface_pressure,
                b.composition_ice,
                b.composition_rock,
                b.composition_metal,
                b.semi_major_axis,
                b.eccentricity,
                b.orbital_inclination,
                b.periapsis,
                b.orbital_period,
                b.rotation_period,
                b.axial_tilt,
                b.ascending_node,
                b.mean_anomaly,
                b.was_mapped,
                b.was_discovered,
                COALESCE(ARRAY_AGG(m.name ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_names!: Vec<String>",
                COALESCE(ARRAY_AGG(m.percent ORDER BY m.name)
                    FILTER (WHERE m.name IS NOT NULL), '{}')
                    AS "material_percents!: Vec<f64>"
            FROM bodies b
            LEFT JOIN body_materials m
                ON m.system_address = b.system_address AND m.body_id = b.id
            WHERE b.name ILIKE $1
            GROUP BY b.system_address, b.id
            ORDER BY b.name
            "#,
            name
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(Into::into).collect())
    }
}
