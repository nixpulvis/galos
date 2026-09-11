//! The rules both derivations of the index have to answer alike.
//!
//! A directory is produced two ways. `galos_db::index` derives one from
//! Postgres, over every system anybody has ever reported; `galos_journal`
//! derives one from a single pilot's own journal files, with no database
//! anywhere. They write the same format, for the same client, and a running
//! `galos-sync` maintains one directory from live events while catching it up
//! from the database, so the two answers land in the same files. Where they
//! read the same fact they have to read it the same way: a system binned into
//! Recency bucket three out of the database and bucket four out of a journal
//! is one galaxy disagreeing with itself about when it was last heard from,
//! and the disagreement shows as a cell whose counts do not add up to the
//! stars drawn inside it.
//!
//! Each rule below stood written out in both crates, word for word in places,
//! with a comment in each admitting the other existed. They live here
//! instead. This crate is the format both sides agree on and both already
//! link it, so a rule kept once cannot drift; a rule kept twice only has not
//! drifted yet.

use crate::meta::SystemBodies;
use chrono::NaiveDateTime;
use galos_photometry::{ClassLight, Flux, Magnitude};

/// The edges between the eight Recency buckets, in days since a system was
/// last written. Updated today lands in bucket 0, untouched for a decade in
/// bucket 7.
///
/// Replaces the identical arrays in `galos_db::index` and
/// `galos_journal::galaxy`. The count of buckets is part of the published
/// format — a cell aggregate is a count per bucket, and the client's Recency
/// control indexes straight into them — so the edges are not a builder's
/// choice to make.
pub const AGE_EDGES: [i64; 7] = [1, 7, 30, 90, 365, 1095, 3650];

/// Which Recency bucket an age in days falls in, `0..8`.
///
/// The half-open reading of [`AGE_EDGES`]: a day old is already bucket 1, and
/// an age past the last edge saturates at 7 rather than running off the end.
/// A negative age — a report stamped in the future, which a client can write
/// — falls in bucket 0 with everything else fresh.
pub fn age_bucket(days: i64) -> usize {
    AGE_EDGES.iter().filter(|&&edge| days >= edge).count()
}

/// How lately a system was updated, in the two forms the index wants it: the
/// Recency bucket the cell aggregates count by and the Unix second the
/// payload carries.
///
/// One reading of `at`, so the two cannot disagree about a system. The bucket
/// alone was what the index carried and the buckets are days wide, which
/// answers a Recency span of thirty days and none of the five spans shorter
/// than a day — the end of the control the map is actually used at. So the
/// second goes on the payload point beside it.
///
/// `u32`, which is Unix seconds to 2106 and four bytes rather than eight on a
/// record of thirty-five. Clamped rather than wrapped: `at` comes off a
/// journal entry and a client can write whatever it likes there, and a year
/// outside the `u32` range should read as the far end of the axis rather than
/// fold back into the middle of it.
///
/// Replaces `galos_db::index::updated` and the same pair worked out inline in
/// `galos_journal::galaxy`, where the clamp was written a second time.
pub fn updated(at: NaiveDateTime, now: NaiveDateTime) -> (usize, u32) {
    (
        age_bucket((now - at).num_days()),
        at.and_utc().timestamp().clamp(0, u32::MAX as i64) as u32,
    )
}

/// One system's photometry, by the fallback chain: its scanned stars if it
/// has any, else the class it is named for, else the default M dwarf.
///
/// `stars` is the `(visual absolute magnitude, temperature)` of every scanned
/// star — visual, the caller having already carried Elite's bolometric
/// scanned figure through [`Magnitude::visual`], which is where a white dwarf
/// keeps its faint brightness and a black hole falls to nothing. Their light
/// adds, so the magnitudes combine to the one figure the sky is ordered by,
/// and the tint is the brightest star's, that being the one that dominates
/// what the pair looks like. With no stars `fallback_class` stands in, and
/// with no class [`ClassLight::of`] hands back the M dwarf the galaxy is
/// mostly made of.
///
/// Replaces the fold in `galos_db::index::system_input` and the identical one
/// in `galos_journal::galaxy::Galaxy::system`. The two disagreeing would show
/// as a system changing brightness when the same directory was next written
/// by the other side.
///
/// Folded in one pass rather than collecting, since the database side runs
/// this over a hundred and twenty-nine million systems: the flux sums as it
/// goes and the brightest star is carried along, which is
/// [`Magnitude::combine`] and a `min_by` without the vector in between. Ties
/// keep the first star, as `min_by` did.
pub fn lit(
    stars: impl Iterator<Item = (f64, f64)>,
    fallback_class: &str,
) -> (f64, f64) {
    let mut total = Flux(0.0);
    let mut brightest: Option<(f64, f64)> = None;
    for (magnitude, temperature) in stars {
        total.0 += Magnitude(magnitude).flux().0;
        let brighter =
            |(so_far, _): (f64, f64)| magnitude.total_cmp(&so_far).is_lt();
        if brightest.is_none_or(brighter) {
            brightest = Some((magnitude, temperature));
        }
    }
    match brightest {
        // No light is not a magnitude, as `Magnitude::combine` has it, so a
        // set of stars that sums to nothing falls to the class as an empty
        // one does.
        Some((_, temperature)) if total.0 > 0.0 => {
            (total.magnitude().0, temperature)
        }
        _ => {
            let light = ClassLight::of(fallback_class);
            (light.absolute_magnitude.0, light.temperature.0)
        }
    }
}

/// The class of the star a ship drops in at, as far as the scan record says.
///
/// Nearest the arrival point, ties broken by body id. The tie-break is what
/// makes it an answer rather than a coin toss: a close pair is recorded at
/// the same distance from arrival to the resolution the journal prints, and
/// whichever of them the caller happened to read first would otherwise decide
/// whether the system supercharges a drive.
///
/// [`None`] for a system with no star on record, which leaves the caller to
/// its own fallback — the database side coalesces to the `primary_star_class`
/// column and the journal side to the class a plotted route named, those
/// being statements about the same star from the only sources each has.
///
/// Replaces `galos_journal::galaxy::Galaxy::arrival_class` and the
/// `DISTINCT ON (system_address) ... ORDER BY system_address,
/// distance_from_arrival_ls, id` in `galos_db::index::metadata`, which is the
/// same rule said in SQL. The boost table is written by both and patched by
/// one, so a directory whose full build and whose watch passes ordered
/// differently would disagree with itself about the same galaxy.
pub fn arrival_class(bodies: &SystemBodies) -> Option<&str> {
    bodies
        .stars
        .iter()
        .min_by(|one, other| {
            one.distance_from_arrival_ls
                .total_cmp(&other.distance_from_arrival_ls)
                .then(one.id.cmp(&other.id))
        })
        .map(|star| star.star_class.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::Star;

    fn at(seconds: i64) -> NaiveDateTime {
        chrono::DateTime::from_timestamp(seconds, 0).unwrap().naive_utc()
    }

    /// A star of `class`, `away` light seconds out, with `id` to break a tie
    fn star(id: i16, away: f32, class: &str) -> Star {
        Star {
            system_address: 1,
            id,
            name: String::new(),
            parents: Vec::new(),
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            absolute_magnitude: 0.,
            age_my: 0,
            distance_from_arrival_ls: away,
            luminosity: String::new(),
            star_class: class.to_owned(),
            stellar_mass: 0.,
            subclass: 0,
            orbit: None,
            spin: elite_journal::body::Spin { period: 0., tilt: 0. },
            radius: 0.,
            temperature: 0.,
            mapped: false,
            discovered_at: None,
        }
    }

    /// Every edge bins the day it names into the bucket above, and an age
    /// past the last edge saturates rather than running off the end.
    ///
    /// The eight buckets are the published format: a cell aggregate is a
    /// count per bucket and the client indexes straight into them, so this is
    /// the table both derivations answer by.
    #[test]
    fn the_edges_bin_where_the_format_says() {
        assert_eq!(age_bucket(-7), 0, "a report stamped in the future");
        assert_eq!(age_bucket(0), 0);
        assert_eq!(age_bucket(1), 1, "a day old is already the next bucket");
        assert_eq!(age_bucket(6), 1);
        assert_eq!(age_bucket(7), 2);
        assert_eq!(age_bucket(29), 2);
        assert_eq!(age_bucket(30), 3);
        assert_eq!(age_bucket(89), 3);
        assert_eq!(age_bucket(90), 4);
        assert_eq!(age_bucket(364), 4);
        assert_eq!(age_bucket(365), 5);
        assert_eq!(age_bucket(1095), 6);
        assert_eq!(age_bucket(3649), 6);
        assert_eq!(age_bucket(3650), 7);
        assert_eq!(age_bucket(i64::MAX), AGE_EDGES.len(), "saturates at 7");
    }

    /// The second the payload carries saturates at either end of the `u32`
    /// rather than wrapping into the middle of the axis.
    ///
    /// `updated_at` comes off a journal entry, which a client writes, so a
    /// date in 1969 or in 2200 is a thing the builder will be handed. Wrapped,
    /// either would land beside genuinely fresh systems and colour a cell by
    /// a recency nothing in it has.
    #[test]
    fn the_clamp_saturates_rather_than_wrapping() {
        let now = at(1_700_000_000);

        let (_, second) = updated(at(1_700_000_000), now);
        assert_eq!(second, 1_700_000_000);

        let (bucket, second) = updated(at(-86_400), now);
        assert_eq!(second, 0, "before the epoch is the near end of the axis");
        assert_eq!(bucket, 7, "and it is ancient");

        let far = at(i64::from(u32::MAX) + 86_400);
        let (bucket, second) = updated(far, now);
        assert_eq!(second, u32::MAX, "past 2106 is the far end");
        assert_eq!(bucket, 0, "and it is in the future, so it is fresh");
    }

    /// With no stars the photometry is the class's, and with no class the
    /// default the galaxy is mostly made of.
    ///
    /// The unscanned case is nearly every system there is, so the fallback is
    /// the common path rather than the edge one.
    #[test]
    fn lit_falls_back_with_nothing_scanned() {
        let class = ClassLight::of("G");
        assert_eq!(
            lit(std::iter::empty(), "G"),
            (class.absolute_magnitude.0, class.temperature.0)
        );

        let unnamed = ClassLight::of("");
        assert_eq!(
            lit(std::iter::empty(), ""),
            (unnamed.absolute_magnitude.0, unnamed.temperature.0)
        );
    }

    /// Scanned stars combine to one figure brighter than any of them, and the
    /// tint is the brightest star's.
    ///
    /// A magnitude is inverted and logarithmic, so the two mistakes available
    /// here are taking the dimmest star's tint and averaging light that
    /// should add. Both are checked: the pair must come out brighter than its
    /// brightest member, and wear that member's temperature.
    #[test]
    fn lit_adds_the_light_and_takes_the_brightest_tint() {
        let (magnitude, temperature) =
            lit([(4.0, 3000.0), (2.0, 9000.0)].into_iter(), "G");
        assert_eq!(temperature, 9000.0, "the brighter star's tint");
        assert!(
            magnitude < 2.0,
            "the pair outshines its brightest: {magnitude}"
        );

        // Two equal stars are three quarters of a magnitude brighter than
        // either alone, and the first of them keeps the tint, as `min_by` did.
        let (magnitude, temperature) =
            lit([(5.0, 4000.0), (5.0, 7000.0)].into_iter(), "G");
        assert!((magnitude - (5.0 - 0.7526)).abs() < 1e-3, "{magnitude}");
        assert_eq!(temperature, 4000.0, "a tie keeps the first star");
    }

    /// The nearest star to arrival is the one a ship drops at, and a tie
    /// between two at the same distance goes to the lower body id.
    ///
    /// A close pair is recorded at one distance from arrival to the
    /// resolution the journal prints, so without the tie-break the answer
    /// would be whichever star the caller read first — and the database side
    /// reads them in whatever order a query returns.
    #[test]
    fn arrival_class_breaks_a_tie_by_id() {
        let mut bodies = SystemBodies::default();
        bodies.stars = vec![star(2, 0.0, "N"), star(1, 0.0, "G")];
        assert_eq!(arrival_class(&bodies), Some("G"));

        bodies.stars = vec![star(1, 900.0, "G"), star(9, 12.0, "M")];
        assert_eq!(arrival_class(&bodies), Some("M"), "nearest wins outright");
    }

    /// A system with nothing scanned has no arrival class of its own, and
    /// says so rather than picking one, the caller having a fallback this
    /// cannot see.
    #[test]
    fn arrival_class_is_none_with_no_stars() {
        assert_eq!(arrival_class(&SystemBodies::default()), None);
    }
}
