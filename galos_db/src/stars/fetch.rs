use super::Star;
use crate::bodies::Parent;
use crate::orbit;
use crate::{Database, Error};
use elite_journal::body::{Discovery, Spin};

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
            parents: Parent::rows(row.parent_ids, row.parent_types),
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
