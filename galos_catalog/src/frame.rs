//! The axes a catalog is written in, and how to leave them.
//!
//! A catalog measured from Earth is written in equatorial coordinates: `x`
//! toward the vernal equinox, `z` toward the north celestial pole, `y`
//! completing a right-handed set. That frame is an accident of Earth's spin
//! axis and means nothing to a galaxy, so anything that wants to place a
//! catalog beside another dataset converts out of it first.
//!
//! Galactic coordinates are the frame that does mean something: `x` toward the
//! galactic centre, `z` toward the north galactic pole. This is where the one
//! rotation between them lives, as a constant with a test, which is the whole
//! of what an "astrometry" layer would have amounted to.
//!
//! The Elite frame is deliberately absent. Its rotation is derivable — but the
//! honest way to derive it is by matching a catalog against Elite's own
//! seeded stars, and **angular separation between two stars is the same in
//! every frame**, so that comparison needs no rotation to run. The matrix is
//! what it produces, not what it needs.

/// Equatorial to galactic, the J2000 rotation, as three row vectors.
///
/// The standard matrix, built from the galactic pole at
/// `(12h51m26.28s, +27°07'42.0")` and the ascending node at position angle
/// `122.932°`. Applied to a unit direction it answers a unit direction; applied
/// to a position it answers the same position seen down different axes, since a
/// rotation is length-preserving and the origin — Sol, in both — does not move.
pub const EQUATORIAL_TO_GALACTIC: [[f64; 3]; 3] = [
    [-0.054_875_560_4, -0.873_437_090_2, -0.483_835_015_4],
    [0.494_109_427_9, -0.444_829_629_8, 0.746_982_244_5],
    [-0.867_666_149_0, -0.198_076_373_4, 0.455_983_776_2],
];

/// Turn a vector by a rotation given as three row vectors.
pub fn rotate(matrix: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    let row = |r: &[f64; 3]| r[0] * v[0] + r[1] * v[1] + r[2] * v[2];
    [row(&matrix[0]), row(&matrix[1]), row(&matrix[2])]
}

/// A position or direction in equatorial coordinates, seen in galactic ones.
pub fn equatorial_to_galactic(v: [f64; 3]) -> [f64; 3] {
    rotate(&EQUATORIAL_TO_GALACTIC, v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(v: [f64; 3]) -> f64 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    /// A rotation preserves length, which is the one property everything
    /// downstream leans on: a star does not move nearer by being described
    /// differently.
    #[test]
    fn the_rotation_preserves_distance() {
        for v in [[1.0, 0.0, 0.0], [3.0, -4.0, 12.0], [0.1, 0.2, 0.3]] {
            let turned = equatorial_to_galactic(v);
            assert!((norm(turned) - norm(v)).abs() < 1e-9);
        }
    }

    /// The galactic centre — Sagittarius A*, at RA 17h45m40s, Dec -29°00'28"
    /// — lands on the galactic `+x` axis, which is the definition of the
    /// frame and the one check that the matrix is the right way round.
    #[test]
    fn the_galactic_centre_lands_on_the_x_axis() {
        let ra = (17.0 + 45.0 / 60.0 + 40.04 / 3600.0) * 15f64.to_radians();
        let dec = -(29.0 + 0.0 / 60.0 + 28.1 / 3600.0f64).to_radians();
        let equatorial =
            [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()];
        let g = equatorial_to_galactic(equatorial);
        assert!(g[0] > 0.9999, "x should be ~1, got {g:?}");
        assert!(g[1].abs() < 0.001 && g[2].abs() < 0.001, "{g:?}");
    }

    /// The north galactic pole lands on `+z`, the frame's other axis.
    #[test]
    fn the_north_galactic_pole_lands_on_the_z_axis() {
        let ra = (12.0 + 51.0 / 60.0 + 26.28 / 3600.0) * 15f64.to_radians();
        let dec = (27.0 + 7.0 / 60.0 + 42.0 / 3600.0f64).to_radians();
        let equatorial =
            [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()];
        let g = equatorial_to_galactic(equatorial);
        assert!(g[2] > 0.9999, "z should be ~1, got {g:?}");
    }
}
