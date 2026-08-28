//! The HYG catalog: Hipparcos, Yale Bright Star and Gliese, merged.
//!
//! About 119,000 stars in one CSV, and the pragmatic first catalog to read
//! because it has already done the work a raw survey leaves to the caller: it
//! carries cartesian positions rather than right ascension and declination, so
//! there is no astrometry to do, and it carries an absolute magnitude, a
//! colour index and a spectral type in the same row, which is three
//! independent columns to check a claim against.
//!
//! Its axes are equatorial and its distances are parsecs. Both are converted
//! at the read — see [`crate::frame`] for the first and
//! [`galos_photometry::LY_PER_PARSEC`] for the second — so a [`Star`] that
//! leaves here is in light years and nothing downstream carries the unit
//! around.
//!
//! # Sol, and the stars that are not really there
//!
//! Two rows in every hundred are junk in a specific, documented way, and both
//! are dropped rather than passed on:
//!
//! - **Sol is row zero**, at distance zero, at apparent magnitude −26.7. It is
//!   the observer, not an observation. Left in, it is a star at the eye's own
//!   position and its apparent magnitude from anywhere is a division by zero.
//! - **Ten thousand rows carry `dist = 100000`**, which is not a distance but
//!   the catalog's sentinel for a star whose parallax was never measured or
//!   came back negative. Their positions are meaningless and their absolute
//!   magnitudes are computed from the sentinel, so they are not dim stars far
//!   away, they are stars of unknown distance.
//!
//! [`read`] reports how many it dropped for each reason rather than swallowing
//! it, because "the sky came out too empty" and "ten thousand rows were
//! discarded" are the same fact and a caller should be able to see it.

use crate::Star;
use galos_photometry::LY_PER_PARSEC;
use std::io;

/// The distance HYG writes where it has no parallax to work from.
const NO_PARALLAX: f64 = 100_000.0;

/// What a read passed over, and why.
///
/// Returned beside the stars rather than logged, so a caller that cares can
/// assert on it and one that does not can ignore it.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Skipped {
    /// Rows at distance zero: Sol, and anything else sitting on the observer.
    pub at_the_origin: usize,
    /// Rows carrying the no-parallax sentinel.
    pub no_parallax: usize,
    /// Rows whose required columns would not parse.
    pub unreadable: usize,
}

impl Skipped {
    /// How many rows were dropped in total.
    pub fn total(&self) -> usize {
        self.at_the_origin + self.no_parallax + self.unreadable
    }
}

/// Read a HYG CSV into stars, in light years, in the catalog's equatorial
/// frame.
///
/// Columns are read by header name, so a catalog revision that adds or
/// reorders columns still reads. A row missing one of the five that matter —
/// `id`, `dist`, `mag`, `absmag` and the three coordinates — is counted in
/// [`Skipped::unreadable`] and passed over; a row missing a name, a colour
/// index or a spectral type is kept, since those are the holes a real catalog
/// has and a star with none of them still has a place and a brightness.
pub fn read<R: io::Read>(reader: R) -> csv::Result<(Vec<Star>, Skipped)> {
    let mut csv = csv::Reader::from_reader(reader);
    let headers = csv.headers()?.clone();
    let column = |name: &str| headers.iter().position(|h| h == name);

    let (Some(c_id), Some(c_dist), Some(c_mag), Some(c_absmag)) = (
        column("id"),
        column("dist"),
        column("mag"),
        column("absmag"),
    ) else {
        return Err(csv::Error::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a HYG catalog: missing id, dist, mag or absmag",
        )));
    };
    let (Some(c_x), Some(c_y), Some(c_z)) =
        (column("x"), column("y"), column("z"))
    else {
        return Err(csv::Error::from(io::Error::new(
            io::ErrorKind::InvalidData,
            "not a HYG catalog: missing x, y or z",
        )));
    };
    let c_hip = column("hip");
    let c_proper = column("proper");
    let c_ci = column("ci");
    let c_spect = column("spect");

    let mut stars = Vec::new();
    let mut skipped = Skipped::default();

    for record in csv.records() {
        let record = record?;
        let number = |i: usize| record.get(i).and_then(|f| f.parse::<f64>().ok());
        let text = |i: Option<usize>| {
            i.and_then(|i| record.get(i))
                .map(str::trim)
                .filter(|f| !f.is_empty())
                .map(str::to_owned)
        };

        let (
            Some(id),
            Some(distance_pc),
            Some(apparent_magnitude),
            Some(absolute_magnitude),
            Some(x),
            Some(y),
            Some(z),
        ) = (
            number(c_id),
            number(c_dist),
            number(c_mag),
            number(c_absmag),
            number(c_x),
            number(c_y),
            number(c_z),
        )
        else {
            skipped.unreadable += 1;
            continue;
        };

        if distance_pc >= NO_PARALLAX {
            skipped.no_parallax += 1;
            continue;
        }
        if distance_pc <= 0.0 {
            skipped.at_the_origin += 1;
            continue;
        }

        stars.push(Star {
            id: id as u64,
            hip: text(c_hip).and_then(|h| h.parse().ok()),
            name: text(c_proper),
            position: [
                x * LY_PER_PARSEC,
                y * LY_PER_PARSEC,
                z * LY_PER_PARSEC,
            ],
            distance: distance_pc * LY_PER_PARSEC,
            apparent_magnitude,
            absolute_magnitude,
            color_index: text(c_ci).and_then(|c| c.parse().ok()),
            spectral_type: text(c_spect),
        });
    }

    Ok((stars, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_photometry::apparent_magnitude_ly;

    /// The eighty brightest named stars, cut from the real catalog so the
    /// tests here are about measured sky rather than about invented rows.
    const BRIGHT: &str = include_str!("../data/bright.csv");

    fn bright() -> (Vec<Star>, Skipped) {
        read(BRIGHT.as_bytes()).expect("the fixture is a HYG catalog")
    }

    fn named<'a>(stars: &'a [Star], name: &str) -> &'a Star {
        stars
            .iter()
            .find(|s| s.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("{name} should be in the fixture"))
    }

    /// Sol is dropped, being the observer rather than an observation, and the
    /// drop is reported rather than silent.
    #[test]
    fn sol_is_not_a_star_in_the_sky() {
        let (stars, skipped) = bright();
        assert!(stars.iter().all(|s| s.name.as_deref() != Some("Sol")));
        assert_eq!(skipped.at_the_origin, 1);
    }

    /// Sirius is the brightest star in the sky. If this fails, either the
    /// magnitude scale is upside down or the catalog did not load.
    #[test]
    fn sirius_is_the_brightest_star_in_the_sky() {
        let (stars, _) = bright();
        let brightest = stars
            .iter()
            .min_by(|a, b| {
                a.apparent_magnitude.total_cmp(&b.apparent_magnitude)
            })
            .expect("the fixture is not empty");
        assert_eq!(brightest.name.as_deref(), Some("Sirius"));
        assert!((brightest.apparent_magnitude - -1.44).abs() < 0.01);
    }

    /// Sirius is eight and a half light years away, which is the one distance
    /// in astronomy everybody knows, and it comes out in light years rather
    /// than the parsecs the file is written in.
    #[test]
    fn distances_arrive_in_light_years() {
        let (stars, _) = bright();
        let sirius = named(&stars, "Sirius");
        assert!((sirius.distance - 8.6).abs() < 0.1, "{}", sirius.distance);
        let from_position = (sirius.position.iter().map(|c| c * c).sum::<f64>())
            .sqrt();
        assert!((from_position - sirius.distance).abs() < 0.01);
    }

    /// **The measurement the workspace cannot make.** For every star in the
    /// fixture, the absolute magnitude and the distance run through the
    /// distance modulus have to reproduce the apparent magnitude the catalog
    /// measured. This is the first test in the repository that can tell
    /// `galos_photometry` it is wrong rather than merely inconsistent.
    #[test]
    fn the_distance_modulus_reproduces_measured_magnitudes() {
        let (stars, _) = bright();
        assert!(stars.len() > 50);
        for star in &stars {
            let predicted = apparent_magnitude_ly(
                star.absolute_magnitude,
                star.distance,
            );
            assert!(
                (predicted - star.apparent_magnitude).abs() < 0.01,
                "{}: predicted {predicted:.3}, measured {:.3}",
                star.name.as_deref().unwrap_or("?"),
                star.apparent_magnitude,
            );
        }
    }

    /// A colour index is read and turned into a temperature that matches the
    /// star's kind: Betelgeuse is cool and red, Rigel is hot and blue.
    #[test]
    fn temperature_follows_the_colour_index() {
        let (stars, _) = bright();
        let betelgeuse = named(&stars, "Betelgeuse").temperature();
        let rigel = named(&stars, "Rigel").temperature();
        assert!(betelgeuse < 4200.0, "{betelgeuse}");
        assert!(rigel > 9000.0, "{rigel}");
    }

    /// Names, Hipparcos numbers and spectral types survive the read, since
    /// they are the columns a catalog is worth reading for.
    #[test]
    fn the_rich_columns_survive() {
        let (stars, _) = bright();
        let sirius = named(&stars, "Sirius");
        assert_eq!(sirius.hip, Some(32349));
        assert!(sirius.spectral_type.as_deref().unwrap().starts_with('A'));
        assert!(sirius.color_index.is_some());
    }

    /// A file that is not a HYG catalog is an error rather than an empty sky.
    #[test]
    fn a_foreign_file_is_an_error() {
        assert!(read("alpha,beta\n1,2\n".as_bytes()).is_err());
    }
}
