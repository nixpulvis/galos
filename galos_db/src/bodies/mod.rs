//! A body within a star system
use chrono::{DateTime, Utc};

/// ### Schema
///
/// ```
///                                Table "public.bodies"
///       Column        |            Type             | Collation | Nullable | Default
/// ---------------------+-----------------------------+-----------+----------+---------
///  system_address      | bigint                      |           | not null |
///  name                | character varying           |           | not null |
///  id                  | smallint                    |           | not null |
///  parent_id           | smallint                    |           |          |
///  updated_at          | timestamp without time zone |           | not null |
///  updated_by          | character varying           |           | not null |
///  planet_class        | character varying           |           | not null |
///  tidal_lock          | boolean                     |           | not null |
///  landable            | boolean                     |           | not null |
///  terraform_state     | character varying           |           |          |
///  atmosphere          | character varying           |           |          |
///  atmosphere_type     | character varying           |           | not null |
///  volcanism           | character varying           |           |          |
///  mass                | real                        |           | not null |
///  radius              | real                        |           | not null |
///  surface_gravity     | real                        |           | not null |
///  surface_temperature | real                        |           | not null |
///  surface_pressure    | real                        |           | not null |
///  semi_major_axis     | real                        |           | not null |
///  eccentricity        | real                        |           | not null |
///  orbital_inclination | real                        |           | not null |
///  periapsis           | real                        |           | not null |
///  orbital_period      | real                        |           | not null |
///  rotation_period     | real                        |           | not null |
///  axial_tilt          | real                        |           | not null |
///  ascending_node      | real                        |           | not null |
///  mean_anomaly        | real                        |           | not null |
///  was_mapped          | boolean                     |           | not null |
///  was_discovered      | boolean                     |           | not null |
/// Indexes:
///     "bodies_pkey" PRIMARY KEY, btree (system_address, id)
///     "bodies_system_address_name_key" UNIQUE CONSTRAINT, btree (system_address, name)
/// Foreign-key constraints:
///    "bodies_system_address_fkey" FOREIGN KEY (system_address) REFERENCES systems(address)
/// ```

#[derive(Debug, PartialEq)]
pub struct Body {
    pub system_address: i64,
    pub id: i16,
    pub parent_id: Option<i16>,
    pub name: String,
    pub updated_at: DateTime<Utc>,
    pub updated_by: String,

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

mod create;
mod fetch;
