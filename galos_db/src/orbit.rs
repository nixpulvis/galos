//! Reading an orbit back out of the columns that hold it
//!
//! `stars` and `barycenters` both keep the numbers of
//! [`elite_journal::body::Orbit`] as nullable columns, because either may go
//! round nothing: the primary star of a system, and the barycenter at the root
//! of a multi-star one.
use elite_journal::body::Orbit;

/// An orbit, where there is one
///
/// The five that describe the path are written and cleared together, so one of
/// them being absent means the thing goes round nothing. The last two say where
/// it stood along that path and are absent wherever the uploader left them out,
/// which says nothing about the path itself.
pub(crate) fn read(
    semi_major_axis: Option<f32>,
    eccentricity: Option<f32>,
    orbital_inclination: Option<f32>,
    periapsis: Option<f32>,
    orbital_period: Option<f32>,
    ascending_node: Option<f32>,
    mean_anomaly: Option<f32>,
) -> Option<Orbit> {
    Some(Orbit {
        semi_major_axis: semi_major_axis?,
        eccentricity: eccentricity?,
        orbital_inclination: orbital_inclination?,
        periapsis: periapsis?,
        orbital_period: orbital_period?,
        ascending_node,
        mean_anomaly,
    })
}
