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
//!
//! # The hole this leaves, which is not small
//!
//! Dropping the sentinel rows is right — there is nowhere to put a star with no
//! distance — but it is not free, and the cost falls exactly where it is most
//! visible. The stars whose parallax Hipparcos could not measure are the
//! *distant luminous* ones: supergiants bright enough to see and far enough
//! that their parallax vanished into the noise or came back negative.
//!
//! Over the whole catalog that is 105 stars brighter than sixth magnitude, 59
//! of them carrying a Bayer or Flamsteed designation, and the brightest of them
//! at magnitude 3.32 — as bright as Megrez in the Dipper. Mu Cephei, Alpha
//! Camelopardalis and Kappa Cassiopeiae are among them.
//!
//! So a sky drawn from this catalog is missing about one naked-eye star in
//! fifty, and they are not a random fiftieth. [`Skipped`] carries
//! [`naked_eye`](Skipped::naked_eye) and [`brightest`](Skipped::brightest) so
//! that the hole is a number a caller can read rather than one it would have to
//! go looking for.
//!
//! Nothing here invents a distance to close it. But these rows are not empty:
//! what they lack is a *distance*, and a direction and a brightness were both
//! measured. So they come back as [`Unplaced`] rather than being discarded, and
//! a caller can draw where the holes in its picture are without anything having
//! been made up to fill them.
//!
//! An [`Unplaced`] is not a [`Star`] and is not convertible to one. Its
//! direction is the direction *from Sol*, because that is where the measurement
//! was taken, and there is no viewpoint-independent fact to be had from it —
//! from anywhere else the star could be anywhere along that line. Two types
//! rather than an optional field, so that nothing can accidentally treat a
//! bearing as a place.

use crate::{Catalog, Star, Unplaced};
use galos_photometry::LY_PER_PARSEC;
use std::io;

/// The distance HYG writes where it has no parallax to work from.
const NO_PARALLAX: f64 = 100_000.0;

/// Read a HYG CSV into stars, in light years, in the catalog's equatorial
/// frame.
///
/// Columns are read by header name, so a catalog revision that adds or
/// reorders columns still reads. A row missing one of the five that matter —
/// `id`, `dist`, `mag`, `absmag` and the three coordinates — is counted in
/// [`Skipped::unreadable`] and passed over; a row missing a name, a colour
/// index or a spectral type is kept, since those are the holes a real catalog
/// has and a star with none of them still has a place and a brightness.
pub fn read<R: io::Read>(reader: R) -> csv::Result<Catalog> {
    let mut csv = csv::Reader::from_reader(reader);
    let headers = csv.headers()?.clone();
    let column = |name: &str| headers.iter().position(|h| h == name);

    let (Some(c_id), Some(c_dist), Some(c_mag), Some(c_absmag)) =
        (column("id"), column("dist"), column("mag"), column("absmag"))
    else {
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
    let c_rarad = column("rarad");
    let c_decrad = column("decrad");
    let c_bf = column("bf");
    let c_hip = column("hip");
    let c_proper = column("proper");
    let c_ci = column("ci");
    let c_spect = column("spect");
    let c_con = column("con");

    let mut catalog = Catalog::new(crate::HYG);

    for record in csv.records() {
        let record = record?;
        let number =
            |i: usize| record.get(i).and_then(|f| f.parse::<f64>().ok());
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
            catalog.skipped.unreadable += 1;
            continue;
        };

        if distance_pc >= NO_PARALLAX {
            catalog.skipped.no_parallax += 1;
            catalog.skipped.note(apparent_magnitude);
            // Not discarded: the bearing is a measurement even where the
            // distance is not. Only a row whose direction cannot be recovered
            // either is truly lost.
            if let Some(direction) =
                direction(&record, c_rarad, c_decrad, [x, y, z])
            {
                catalog.unplaced.push(Unplaced {
                    id: id as u64,
                    // A proper name if it has one, else its Bayer or Flamsteed
                    // designation, since most of these are catalogued but unnamed.
                    name: text(c_proper).or_else(|| text(c_bf)),
                    direction,
                    apparent_magnitude,
                    spectral_type: text(c_spect),
                });
            }
            continue;
        }
        if distance_pc <= 0.0 {
            catalog.skipped.at_the_origin += 1;
            continue;
        }

        catalog.stars.push(Star {
            source: crate::HYG,
            id: id as u64,
            hip: text(c_hip).and_then(|h| h.parse().ok()),
            name: text(c_proper),
            position: [x * LY_PER_PARSEC, y * LY_PER_PARSEC, z * LY_PER_PARSEC],
            distance: distance_pc * LY_PER_PARSEC,
            apparent_magnitude,
            absolute_magnitude,
            color_index: text(c_ci).and_then(|c| c.parse().ok()),
            spectral_type: text(c_spect),
            constellation: text(c_con)
                .as_deref()
                .and_then(crate::constellation::from_abbreviation),
        });
    }

    Ok(catalog)
}

/// The unit direction a row points in.
///
/// From `rarad` and `decrad` where the catalog gives them, since those are the
/// measurement. Failing that, from normalizing the cartesian columns — which
/// works even for a no-parallax row, because those coordinates are the true
/// direction scaled by the sentinel, and a scale is exactly what normalizing
/// removes.
fn direction(
    record: &csv::StringRecord,
    c_rarad: Option<usize>,
    c_decrad: Option<usize>,
    xyz: [f64; 3],
) -> Option<[f64; 3]> {
    let angle = |i: Option<usize>| {
        i.and_then(|i| record.get(i)).and_then(|f| f.parse::<f64>().ok())
    };
    if let (Some(ra), Some(dec)) = (angle(c_rarad), angle(c_decrad)) {
        return Some([dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()]);
    }
    let length = (xyz.iter().map(|c| c * c).sum::<f64>()).sqrt();
    (length > 0.0).then(|| [xyz[0] / length, xyz[1] / length, xyz[2] / length])
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_photometry::apparent_magnitude_ly;

    /// The eighty brightest named stars, cut from the real catalog so the
    /// tests here are about measured sky rather than about invented rows.
    const BRIGHT: &str = include_str!("../data/bright.csv");

    fn bright() -> (Vec<Star>, crate::Skipped) {
        let catalog = read(BRIGHT.as_bytes()).expect("a HYG catalog");
        (catalog.stars, catalog.skipped)
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
        let from_position =
            (sirius.position.iter().map(|c| c * c).sum::<f64>()).sqrt();
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
            let predicted =
                apparent_magnitude_ly(star.absolute_magnitude, star.distance);
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

    /// **The hole is reported, not merely left.** A dropped row that nobody
    /// could see is one thing; a dropped row at fourth magnitude is another,
    /// and `total` alone cannot tell them apart.
    #[test]
    fn a_dropped_naked_eye_star_is_counted_as_one() {
        let header = BRIGHT.lines().next().expect("a header");
        // Two rows carrying the no-parallax sentinel: one a bright star, one
        // far below anything an eye reaches.
        let mut csv = String::from(header);
        csv.push_str("\n900001,,,,,,Bright,0,0,100000,0,0,0,3.5,-8,B1Ia,-0.1,0,0,0,0,0,0,0,0,0,0,,,,1,0,,1,,,");
        csv.push_str("\n900002,,,,,,Faint,0,0,100000,0,0,0,14.2,10,M5V,1.6,0,0,0,0,0,0,0,0,0,0,,,,1,0,,1,,,");

        let catalog = read(csv.as_bytes()).expect("a HYG catalog");
        let skipped = catalog.skipped;
        assert_eq!(skipped.no_parallax, 2);
        assert_eq!(skipped.naked_eye, 1, "only the bright one is visible");
        assert_eq!(skipped.brightest, Some(3.5));
    }

    /// Nothing dropped is no complaint, rather than a magnitude of infinity.
    #[test]
    fn dropping_nothing_reports_nothing() {
        let header = BRIGHT.lines().next().expect("a header");
        let skipped = read(header.as_bytes()).expect("a HYG catalog").skipped;
        assert_eq!(skipped.brightest, None);
        assert_eq!(skipped.naked_eye, 0);
    }

    /// **The hole is enumerable, not merely counted.** A row with no parallax
    /// comes back as an `Unplaced` carrying the bearing that was measured, so
    /// a caller can draw where its picture is missing something without
    /// anything having been invented to fill it.
    #[test]
    fn a_star_with_no_distance_still_has_a_direction() {
        let header = BRIGHT.lines().next().expect("a header");
        let mut csv = String::from(header);
        csv.push_str("\n900001,,,,,15Kap Cas,,0.94,62.93,100000,0,0,0,4.17,-8,B1Ia,0.14,0,0,0,0,0,0,0.246,1.098,0,0,,,Cas,1,0,,1,,,");

        let catalog = read(csv.as_bytes()).expect("a HYG catalog");
        assert!(catalog.stars.is_empty());
        assert_eq!(catalog.unplaced.len(), 1);

        let star = &catalog.unplaced[0];
        assert_eq!(star.name.as_deref(), Some("15Kap Cas"));
        assert_eq!(star.apparent_magnitude, 4.17);
        let length = (star.direction.iter().map(|c| c * c).sum::<f64>()).sqrt();
        assert!((length - 1.0).abs() < 1e-9, "a bearing is a unit vector");
    }

    /// An `Unplaced` is a different kind of thing from a `Star` and the two do
    /// not mix: nothing dropped for want of a distance appears among the
    /// placed, whatever else is true of it.
    #[test]
    fn the_unplaced_are_never_counted_among_the_stars() {
        let catalog = read(BRIGHT.as_bytes()).expect("a HYG catalog");
        assert_eq!(catalog.unplaced.len(), catalog.skipped.no_parallax);
        for star in &catalog.stars {
            assert!(star.distance > 0.0 && star.distance.is_finite());
        }
    }

    /// A star names the constellation it falls in, read from the `con` column
    /// and resolved to the IAU table: Betelgeuse and Rigel are both in Orion,
    /// Sirius in Canis Major. The join is what lets a consumer light up a whole
    /// constellation from membership that rides on the star.
    #[test]
    fn a_star_names_its_constellation() {
        let (stars, _) = bright();
        let of = |name| {
            named(&stars, name).constellation.map(|c| c.name).unwrap_or("?")
        };
        assert_eq!(of("Betelgeuse"), "Orion");
        assert_eq!(of("Rigel"), "Orion");
        assert_eq!(of("Sirius"), "Canis Major");
    }
}
