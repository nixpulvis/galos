use super::Star;
use crate::{Database, Error};

/// Turn a row of `stars` into one
///
/// The four queries below select the same columns and differ only in what
/// they select by, so the mapping between a row and a [`Star`] is written
/// once here. `sqlx::query!` gives each query an anonymous row type of its
/// own, so this is a macro rather than a function: there is no one type to
/// name in a signature.
macro_rules! star {
    ($row:expr) => {{
        let row = $row;
        Star {
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
            star_class: row.star_class,
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
        }
    }};
}

impl Star {
    /// The one star with this id in this system
    pub async fn fetch(
        db: &Database,
        system_address: i64,
        id: i16,
    ) -> Result<Self, Error> {
        let row = sqlx::query!(
            "
            SELECT *
            FROM stars
            WHERE system_address = $1 AND id = $2
            ",
            system_address,
            id
        )
        .fetch_one(&db.pool)
        .await?;

        Ok(star!(row))
    }

    /// Every star in a system
    ///
    /// What the map asks for on its way in to a system, alongside the bodies:
    /// a star is what a body goes round, and there may be several.
    pub async fn fetch_all(
        db: &Database,
        system_address: i64,
    ) -> Result<Vec<Self>, Error> {
        let rows = sqlx::query!(
            "
            SELECT *
            FROM stars
            WHERE system_address = $1
            ",
            system_address
        )
        .fetch_all(&db.pool)
        .await?;

        Ok(rows.into_iter().map(|row| star!(row)).collect())
    }
}
