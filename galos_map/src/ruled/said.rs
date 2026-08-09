//! What a number on a ruler is called
//!
//! A figure and a power, `1.2e6` rather than `1200000`, said to exactly the
//! places the step it is written along carries. The same rule for the numbers
//! painted at a plane's crossings and for the three about a place standing on
//! it: they are read against each other, so they are written the same way.
//!
//! Strings rather than digits handed to the card. What a number is worth, which
//! power it is said in and how many places it runs to are questions about
//! arithmetic, and the shader is left with characters to lay out.
use bevy::math::DVec3;

/// What a plane's numbers are said in
///
/// A length and what to call it. Everything else here counts in whatever the
/// caller is counting in and never asks what that is; this is only for the
/// numbers that stand away from a ruler, with nothing beside them to say what
/// they count.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Unit {
    /// How many of whatever the world is drawn in one of these comes to
    pub metres: f64,
    /// And what a number said in it is marked with
    pub mark: &'static str,
}

/// How many parts of a step a position is said to
///
/// A tenth of one. Written to exactly the places its own step carries, a
/// position resolves somewhere between the whole step and a tenth of it,
/// depending where on the one, two, five ladder the step landed — and at the
/// worst of that it reads `0` and then `1e4` with nothing between, which is a
/// number that jumps rather than moves.
///
/// A tenth is a fifth of a screen of dragging at the coarse end of that and a
/// twenty fifth at the fine end, so the last figure turns over as the view
/// goes. It costs a place, sometimes two: `1.0e4` where the ruler says `1e4`.
/// Which is resolution rather than digits — the figures a position runs to are
/// still the ruler's own, one power and a couple of places.
pub const RESOLVES: f64 = 10.;

/// The power of ten a number is said in
///
/// Its own, so that whatever is written before the point is the figure that
/// matters and everything after it is a place. The same rule for the numbers
/// painted on the plane and for the reading at the middle of the view, which
/// are read against each other and so have to be read the same way.
///
/// Nothing at all for numbers a person reads without help. `5.6e1` is a worse
/// way of writing `56`, and most of the map is looked at from scales that have
/// words.
///
/// Asked of the larger of the number and the step it is written to. A ruler
/// counting by tenths never says anything finer than a tenth, so a coordinate a
/// hair below the origin is a nought on it rather than a billionth.
pub fn power(value: f64) -> i32 {
    if value == 0. || !value.is_finite() {
        return 0;
    }
    let power = value.abs().log10().floor() as i32;
    if power.abs() >= 3 { power } else { 0 }
}

/// How a number is written along a ruler stepping by `step`
///
/// As a figure and a power past a thousand: `1e3` rather than `1000`, and
/// `1.2e6` rather than `1200000`. The noughts carry no more than the power does
/// and take four times the room, and a plane of them is a plane of noughts.
///
/// As many places as the step has once that is taken off it, and no more: a
/// ruler counting by hundreds that writes three decimals is three columns of
/// noughts.
///
/// Zero is written as zero however it was arrived at. Rounding a coordinate a
/// hair below the origin otherwise gives `-0`, which reads as somewhere else.
pub fn ticked(value: f64, step: f64) -> String {
    let power = power(value.abs().max(step));
    let under = 10f64.powi(power);
    let places = -(step / under).log10().floor();
    if !places.is_finite() {
        return format!("{value}");
    }
    let places = places.clamp(0., 6.) as usize;
    let said = format!("{:.places$}", value / under);
    // Zero is written as zero however it was arrived at. Rounding a coordinate
    // a hair below the origin otherwise gives `-0`, which reads as somewhere
    // else.
    if said.trim_start_matches('-').trim_matches(['0', '.']).is_empty() {
        format!("{:.places$}", 0.)
    } else if power == 0 {
        said
    } else {
        format!("{said}e{power}")
    }
}

/// How the three numbers about one place are said
///
/// Each in its own power, written onto the number itself. Shared, the smaller
/// of them are written at the largest's scale and come out as a row of noughts
/// — a view sixty light years out reads `0.0690, 2.0910, -2.2090 e9`, where the
/// first says nothing and says it at a scale that is not its own.
///
/// By the same [`ticked`] as the plane's own numbers, to [`RESOLVES`] of the
/// same step. A position and the labels on the ruler beside it are read against
/// each other, so they are written the same way, and a power carries the
/// magnitude, which is what a ruler used a column of figures for. The tenth is
/// what keeps it moving as the view goes rather than stepping from one of the
/// plane's numbers to the next.
pub fn told(at: DVec3, step: f64) -> String {
    let fine = step / RESOLVES;
    format!(
        "{}, {}, {}",
        ticked(at.x, fine),
        ticked(at.y, fine),
        ticked(at.z, fine)
    )
}

/// How far off the plane something standing off it is, said out loud
///
/// The third number, and the one a ruler lying in the plane cannot carry. Which
/// way it went is said with a sign rather than left to the line to show: a line
/// dropped from above the plane and one dropped from below are drawn the same
/// way round on a screen, and which of the two it is, is half the answer.
///
/// Nothing at all for something standing on the plane, or near enough that the
/// number would read as nought. The line is already as short as it can be
/// there, and a `+0.0` beside it says less than the line does.
pub fn off_plane(high: f64, step: f64, unit: Unit) -> Option<String> {
    let fine = step / RESOLVES;
    let said = ticked(high, fine);
    if said == ticked(0., fine) {
        return None;
    }
    // Marked with its unit, unlike the numbers on the plane. Those are read
    // against the rulers they are painted on; this one stands wherever the
    // thing it is about stands, with nothing beside it to say what it counts.
    //
    // The offset alone. Where the thing stands is said in full under the mark
    // at the line's foot, and a number said twice on one screen is a number to
    // be checked against itself.
    let sign = if high > 0. { "+" } else { "" };
    Some(format!("{sign}{said} {}", unit.mark))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::ladder::{numbering, tests::zooms};
    use super::*;

    const LIGHT_YEARS: Unit = Unit { metres: 9.4607304725808e15, mark: "Ly" };
    const LIGHT_SECONDS: Unit = Unit { metres: 2.99792458e8, mark: "Ls" };

    /// Somewhere a few views out from where the map is measured from, for a
    /// view `across` wide
    ///
    /// With the axes decades apart, which is the reading that goes wrong: a
    /// position at the rim is tens of thousands of light years along one axis
    /// and tens above the plane.
    fn near(across: f64) -> [DVec3; 2] {
        [DVec3::ZERO, DVec3::new(across * 7., across * 0.31, -across * 3.)]
    }

    /// And somewhere far enough out to run the reading out of figures
    fn far(across: f64) -> [DVec3; 1] {
        [DVec3::new(across * 1234., across * 0.02, -across * 87.)]
    }

    fn wheres(across: f64) -> impl Iterator<Item = DVec3> {
        near(across).into_iter().chain(far(across))
    }

    /// The zooms whose step a number can be said to the whole of
    ///
    /// [`ticked`] stops at six places, so a number a million times its own step
    /// away from the origin is said as finely as it can be rather than as
    /// finely as the step asks. The ladder does not reach there unless the bar
    /// pins a unit the space is not measured in.
    fn steps() -> impl Iterator<Item = f64> {
        zooms().filter(|across| numbering(*across) >= 1e-6)
    }

    /// The middle turns over in its last figure as the view moves
    ///
    /// The whole of what the reading at the middle is for. Said to the places
    /// its own step carries, as the plane's numbers are, it would stand still
    /// until the view had crossed the gap between two of them, which at the
    /// rim is a hundred light years of dragging for one figure.
    ///
    /// Swept over the zoom, since a count of figures holds at one scale and
    /// fails at the next, which is what [`RESOLVES`] being a share of the step
    /// is about.
    #[test]
    fn the_middle_moves_when_the_view_does() {
        for across in steps() {
            let step = numbering(across);
            // A tenth of the way from one of the plane's numbers to the next,
            // which is what a position resolves and a label does not.
            let nudge = step / RESOLVES;
            for at in near(across) {
                assert_ne!(
                    told(at, step),
                    told(at + DVec3::X * nudge, step),
                    "{across} across reads the same at {at} and {nudge} along"
                );
            }
        }
    }

    /// And no more than a place or two finer than the ruler beside it
    ///
    /// A position and a label are read against each other. One written to
    /// several more places than the other reads as a different kind of number,
    /// and the places past those say nothing a power has not already said.
    ///
    /// Held on the last place written rather than on what a nudge does to it: a
    /// number sitting on a rounding boundary turns over for a hair whatever it
    /// is written to.
    #[test]
    fn the_middle_is_said_no_finer_than_the_ruler() {
        for across in steps() {
            let step = numbering(across);
            for at in wheres(across) {
                for said in told(at, step).split(", ") {
                    let (figures, power) =
                        said.split_once('e').unwrap_or((said, "0"));
                    let places = figures
                        .split_once('.')
                        .map_or(0, |(_, places)| places.len());
                    let last = 10f64
                        .powi(power.parse::<i32>().unwrap() - places as i32);
                    // A tenth of a step is written to its own last place and
                    // a fiftieth to a fifth of that, the places being whole.
                    // Or to no places at all, which is where [`ticked`] stops
                    // however coarse the step is.
                    assert!(
                        places == 0 || last >= step / (RESOLVES * 10.),
                        "{across} across says {said} at {at}, to {last} \
                         against a step of {step}"
                    );
                }
            }
        }
    }

    /// And says it in a handful of figures
    ///
    /// The other half of it. A reading fine enough to move is a reading that
    /// can run to noughts, and three of them stand in one row at the middle of
    /// the view with a unit after them.
    #[test]
    fn the_middle_stays_short() {
        for across in steps() {
            let step = numbering(across);
            for at in wheres(across) {
                let said = told(at, step);
                assert!(
                    said.len() <= 32,
                    "{across} across says {said} at {at}, {} characters",
                    said.len()
                );
            }
        }
    }

    /// And every number in its own scale, written onto the number itself
    ///
    /// Shared, the smaller of them are written at the largest's scale and come
    /// out as a row of noughts, which is a number that says nothing and says it
    /// at a scale that is not its own.
    #[test]
    fn every_number_carries_its_own_scale() {
        // Sixty light years out, ruled in light seconds, where one axis stands
        // decades under the others.
        let at = DVec3::new(6.9e7, 2.091e9, -2.209e9);
        assert_eq!(told(at, 5e7), "6.9e7, 2.091e9, -2.209e9");

        // And no scale at all on numbers a person reads unaided.
        assert_eq!(told(DVec3::new(2.19, 6.62, -7.), 0.2), "2.19, 6.62, -7.00");
    }

    /// A number is written to as many places as its step has
    #[test]
    fn numbers_are_written_to_the_step() {
        assert_eq!(ticked(-20.5, 0.5), "-20.5");
        assert_eq!(ticked(0.25, 0.05), "0.25");
        assert_eq!(ticked(213., 100.), "213");
    }

    /// And past a thousand it is written in thousands
    ///
    /// A ruler counting in thousands writes `1K`. The noughts carry no more
    /// than the letter does and take four times the room, and a plane of them
    /// is a plane of noughts.
    #[test]
    fn a_thousand_is_written_as_one_and_a_power() {
        assert_eq!(ticked(1000., 1000.), "1e3");
        assert_eq!(ticked(1234., 100.), "1.2e3");
        assert_eq!(ticked(1234., 10.), "1.23e3");
        assert_eq!(ticked(-20_000., 10_000.), "-2e4");
        assert_eq!(ticked(1_200_000., 100_000.), "1.2e6");
        // And under a thousand it is said as it is.
        assert_eq!(ticked(999., 1.), "999");
    }

    /// Zero is written as zero, however it was arrived at
    ///
    /// Rounding a coordinate a hair below the origin gives `-0`, which reads
    /// as a place on the other side of the middle rather than as the middle.
    #[test]
    fn zero_is_never_written_as_minus_zero() {
        assert_eq!(ticked(-0.0, 1.), "0");
        assert_eq!(ticked(-1e-9, 0.1), "0.0");
    }

    /// A line dropped to the plane says which way it went
    ///
    /// A line dropped from above the plane and one dropped from below are drawn
    /// the same way round on a screen, so the sign carries the half of the
    /// answer the line cannot.
    #[test]
    fn a_dropped_line_says_which_way_it_went() {
        assert_eq!(off_plane(7., 2., LIGHT_YEARS).as_deref(), Some("+7.0 Ly"));
        assert_eq!(off_plane(-7., 2., LIGHT_YEARS).as_deref(), Some("-7.0 Ly"));
        // And in whatever the numbers are being said in.
        assert_eq!(
            off_plane(-1500., 500., LIGHT_SECONDS).as_deref(),
            Some("-1.50e3 Ls")
        );
    }

    /// And says nothing at all where it has no length to speak of
    ///
    /// The line is already as short as it can be there, and a `+0.0` beside it
    /// says less than the line does.
    #[test]
    fn a_line_dropped_nowhere_says_nothing() {
        assert_eq!(off_plane(0., 2., LIGHT_YEARS), None);
        // Under half of the last place it is written to, which reads as nought.
        assert_eq!(off_plane(0.04, 2., LIGHT_YEARS), None);
        assert!(off_plane(0.06, 2., LIGHT_YEARS).is_some());
    }
}
