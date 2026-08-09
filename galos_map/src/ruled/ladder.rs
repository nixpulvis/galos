//! How wide a cell is, and how far apart the numbers along it go
//!
//! The one figure any of it follows is how much of the world is on screen. From
//! that comes the decade a plane is ruled in, the crossfade that carries one
//! decade into the next as the camera comes in, and the round step to put
//! between two numbers.
//!
//! In whatever unit the caller is counting in. Nothing here names a length.
use super::{FAMILIES, Family, MAJOR, MINOR, SPAN};
use bevy::math::DVec3;

/// How many cells the finer of the two planes lays across the view
///
/// The ladder is decades, so what is actually on screen runs from this up to
/// ten times it before the next decade takes over. Eight at the sparse end is
/// eighty at the dense end, which is about as fine as ruling gets before the
/// lines stop reading as lines and start reading as shading.
pub const CELLS_ACROSS: f64 = 8.;

/// How many numbers to aim for across the view
///
/// Far fewer than there are cells, because the numbers are laid over the whole
/// plane rather than along one line through the middle of it: they land on a
/// lattice, and a lattice this wide is what keeps two of them from being
/// written on top of each other where the plane is nearest.
///
/// The cells are there to be counted between the numbers.
pub const TICKS_ACROSS: f64 = 2.;

/// How many digits tall the view is
///
/// The lettering is painted on the plane, so its size has to be asked for in
/// cells — but a cell is a decade and the view runs from eight of them across
/// to eighty before the next decade takes over. Sized in cells a digit would
/// be ten times too large at one end of every decade and ten times too small
/// at the other. Sized as a share of the view it is the same on screen at any
/// zoom, which is what a number wants to be.
///
/// Thirty four is about eleven pixels of digit on a tall window, which is
/// where a real face still reads as letters rather than as grey. A cut bitmap
/// will take half that and stay crisp; a drawn one will not.
pub const FIGURES_ACROSS: f64 = 34.;

/// The decade a view `across` wide is ruled in, and how far past it it has got
///
/// The exponent of the finer plane's cell, and a fraction from nothing to one
/// saying how far the view has zoomed out towards the decade above it. The
/// coarser plane is that decade.
pub fn rung(across: f64) -> (f64, f32) {
    let wanted = (across / CELLS_ACROSS).max(f64::MIN_POSITIVE);
    let ladder = wanted.log10();
    let decade = ladder.floor();
    (decade, (ladder - decade) as f32)
}

/// Which cells a space is ruled in over one decade, and how strongly each row
/// is drawn
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Decade {
    /// The finer plane's cell, in whatever unit it was asked in
    pub fine: f64,
    pub fine_strength: f32,
    /// The coarser plane's cell, a decade above [`Decade::fine`]
    pub coarse: f64,
    pub coarse_strength: f32,
    /// How much of this ruling is drawn at all
    ///
    /// Not the two strengths above put together. Those are a crossfade, and
    /// through the middle of a decade both sit at half while what is on screen
    /// is one whole ruling handing over to another. This is whether there is a
    /// ruling there to hand over, which is a different question and the only
    /// one anything outside the plane should be asking: chrome that dimmed
    /// every time a cell subdivided would be pulsing about nothing.
    pub drawn: f32,
}

impl Decade {
    /// The rows of lines this comes to, in cells of [`Decade::fine`]
    ///
    /// Three rows rather than four: the finer cell's tenth lines and the
    /// coarser cell's own lines fall in the same places, so they are the one
    /// row, drawn at both strengths laid over each other. Which is what the
    /// two planes did by being blended over each other, and is now arithmetic.
    ///
    /// Widest first, [`ruled`] drawing each row into what the wider ones have
    /// left so that a line two rows fall on is drawn once.
    pub fn rows(&self, handed: f32) -> [Family; FAMILIES] {
        let over = |a: f32, b: f32| a + b - a * b;
        let fine = self.fine_strength * handed;
        let coarse = self.coarse_strength * handed;
        [
            Family { apart: 100., strength: MAJOR * coarse },
            Family { apart: 10., strength: over(MAJOR * fine, MINOR * coarse) },
            Family { apart: 1., strength: MINOR * fine },
            Family::default(),
        ]
    }
}

/// How to rule a space with `across` of it on screen
///
/// The two planes are a decade apart, so that the finer one's tenth lines fall
/// exactly on the coarser one's own lines, and they are crossfaded across the
/// decade. A cell therefore subdivides into ten as the camera comes in, rather
/// than the whole ruling stepping from one size to the next.
///
/// The handoff at the end of a decade is exact. As the crossfade completes the
/// coarse plane is drawn alone, ruling its cell and its tenth lines; the
/// decade then turns over and those same two rows of lines are the fine
/// plane's, at the same two strengths. Nothing on screen changes at the moment
/// it happens.
///
/// `finest` is the smallest cell the space can be ruled in. The ladder stops
/// there rather than going on, a plane whose lines cannot be placed within a
/// cell being a plane whose lines swim. Held at the floor the fine plane is
/// drawn alone, which costs nothing: its own tenth lines are what the coarse
/// plane would have been drawing.
///
/// Where the view has come in nearer than a single cell, both go. A ruling
/// with no lines left in it is chrome standing in front of the map for
/// nothing.
pub fn ruling(across: f64, finest: f64) -> Decade {
    let (decade, through) = rung(across);
    let floor = finest.log10().round();
    let held = decade < floor;
    let fine = 10f64.powf(decade.max(floor));

    // A cell wider than the view leaves at most one line on screen. Faded over
    // the last of it rather than switched off, so that a camera coming down
    // onto a body loses the plane rather than having it vanish.
    let showing = ((across / fine) - 1.).clamp(0., 1.) as f32;

    Decade {
        drawn: showing,
        fine,
        // Held at the floor there is no decade left to cross to, so the fine
        // plane is simply what is drawn.
        fine_strength: if held { showing } else { (1. - through) * showing },
        coarse: fine * 10.,
        coarse_strength: if held { 0. } else { through * showing },
    }
}

/// The next step up the one, two, five ladder
pub fn wider(step: f64) -> f64 {
    let decade = 10f64.powf(step.log10().floor());
    let rung = step / decade;
    if rung < 1.5 {
        decade * 2.
    } else if rung < 3.5 {
        decade * 5.
    } else {
        decade * 10.
    }
}

/// How far apart to number the crossings, for a view `across` wide
///
/// [`tick_step`] if it will do, and the next step up the ladder for as long as
/// it will not. The numbers are painted along their own lines at a size fixed
/// against the view, so a step chosen only for how many fit runs them into
/// each other wherever the ladder lands short — and a pair that runs into the
/// next is two pairs neither of which can be read.
///
/// Stepped rather than squeezed. The size cannot give: the lettering is
/// already as small as a drawn face reads at. So what gives is how many are
/// written, which is a thing the eye follows and the ladder keeps round.
pub fn numbering(across: f64) -> f64 {
    // The widest a pair can be, in the same terms as the step.
    let widest = SPAN as f64 * across / (5. * FIGURES_ACROSS);
    let mut step = tick_step(across);
    // The ladder climbs by at least two a rung, so this is a handful of turns
    // at the very most. Bounded all the same, a step that has come out as
    // nothing being a step that never reaches anything.
    for _ in 0..8 {
        if step >= widest {
            break;
        }
        step = wider(step);
    }
    step
}

/// The roundest step to put between two numbers, for a view `across` wide
///
/// One, two or five times a power of ten, which is the ladder a scale is read
/// in wherever scales are read. Always a whole multiple of the fine plane's
/// cell, so that every number written falls on a line rather than between two.
pub fn tick_step(across: f64) -> f64 {
    let wanted = (across / TICKS_ACROSS).max(f64::MIN_POSITIVE);
    let decade = 10f64.powf(wanted.log10().floor());
    let rung = wanted / decade;
    decade
        * if rung >= 5. {
            5.
        } else if rung >= 2. {
            2.
        } else {
            1.
        }
}

/// Round `value` onto the nearest multiple of `step`
///
/// In `f64`, which holds a position out at the rim to far better than the
/// smallest cell the map rules. Landing on a multiple is the whole point of
/// moving the plane, so this is the one piece of the arithmetic that cannot be
/// done in the float the shader works in.
pub fn snapped(value: f64, step: f64) -> f64 {
    if step > 0. && step.is_finite() {
        (value / step).round() * step
    } else {
        value
    }
}

/// [`snapped`] on all three axes at once
pub fn snapped_to(place: DVec3, step: f64) -> DVec3 {
    DVec3::new(
        snapped(place.x, step),
        snapped(place.y, step),
        snapped(place.z, step),
    )
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The whole zoom the map allows, in light years
    ///
    /// From a metre, which is as near as the camera may be pulled to what it
    /// looks at, out past the far rim of the galaxy. Every property below is
    /// swept across all of it: the ladder has to hold at both ends and at
    /// every decade between, and it is the seams between decades that go
    /// wrong rather than the middles.
    pub(crate) fn zooms() -> impl Iterator<Item = f64> {
        (-17..=6).flat_map(|decade| {
            [1., 1.7, 2.5, 4.2, 7.9]
                .into_iter()
                .map(move |through| through * 10f64.powi(decade))
        })
    }

    /// The fine plane's cell is always a decade, and the coarse one the decade
    /// above it
    #[test]
    fn the_two_planes_are_a_decade_apart() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            let decades = (ruled.coarse / ruled.fine).log10();
            assert!(
                (decades - 1.).abs() < 1e-9,
                "{across} ruled {} and {}, {decades} decades apart",
                ruled.fine,
                ruled.coarse
            );
            let exponent = ruled.fine.log10();
            assert!(
                (exponent - exponent.round()).abs() < 1e-9,
                "{across} ruled a cell of {}, not a decade",
                ruled.fine
            );
        }
    }

    /// However far the camera zooms, what is on screen is a countable number
    /// of cells
    ///
    /// The whole point of the ladder. A ruling that came out at two cells
    /// across at one zoom and two thousand at another is not a scale, and the
    /// seams between decades are where that would happen.
    #[test]
    fn the_view_always_holds_a_countable_number_of_cells() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            // Whichever plane is the more strongly drawn is the one being
            // counted, the other having faded towards nothing.
            let cell = if ruled.fine_strength >= ruled.coarse_strength {
                ruled.fine
            } else {
                ruled.coarse
            };
            let cells = across / cell;
            assert!(
                (2. ..=90.).contains(&cells),
                "{across} across came out {cells} cells wide"
            );
        }
    }

    /// The crossfade between the two planes never leaves the sky unruled
    ///
    /// One of them is always drawn at most of its strength. Both fading at
    /// once is a zoom that passes through a moment with no ruling on screen.
    #[test]
    fn something_is_always_drawn() {
        for across in zooms() {
            let ruled = ruling(across, 0.);
            let loudest = ruled.fine_strength.max(ruled.coarse_strength);
            assert!(
                loudest > 0.49,
                "{across} across left the strongest cell at {loudest}"
            );
        }
    }

    /// A decade turning over changes nothing on screen
    ///
    /// The seam the crossfade exists to hide. Just below the turn the coarse
    /// plane is drawn alone; just above it the fine plane is, ruled in the
    /// same cell and at the same strength. Anything else is a visible step in
    /// the middle of a smooth zoom.
    #[test]
    fn a_decade_turns_over_without_a_step() {
        for decade in -6..=4 {
            let turn = CELLS_ACROSS * 10f64.powi(decade);
            let under = ruling(turn * (1. - 1e-9), 0.);
            let over = ruling(turn * (1. + 1e-9), 0.);

            assert!(
                (under.coarse - over.fine).abs() < over.fine * 1e-6,
                "at {turn} the coarse plane ruled {} and the fine one {}",
                under.coarse,
                over.fine
            );
            assert!(
                (under.coarse_strength - over.fine_strength).abs() < 1e-3,
                "at {turn} the ruling stepped from {} to {}",
                under.coarse_strength,
                over.fine_strength
            );
        }
    }

    /// A decade turning over moves no line and changes no strength
    ///
    /// [`a_decade_turns_over_without_a_step`] one level down, on the rows of
    /// lines actually drawn rather than on the two cells they are worked out
    /// from. Just under the turn the wider cell draws its own lines and its
    /// tenths; just over, the same two rows are the finer cell's, at the same
    /// two strengths and the same two spacings.
    #[test]
    fn a_decade_turns_over_without_a_line_moving() {
        // What is drawn, as how far apart the lines really are and how
        // strongly, faintest rows dropped as being nothing on screen.
        let drawn = |ruled: &Decade| {
            let mut rows: Vec<(f64, f32)> = ruled
                .rows(1.)
                .into_iter()
                .filter(|row| row.apart > 0. && row.strength > 1e-4)
                .map(|row| (ruled.fine * row.apart as f64, row.strength))
                .collect();
            rows.sort_by(|a, b| a.0.partial_cmp(&b.0).expect("a spacing"));
            rows
        };

        for decade in -4..=4 {
            let turn = CELLS_ACROSS * 10f64.powi(decade);
            let under = drawn(&ruling(turn * (1. - 1e-9), 0.));
            let over = drawn(&ruling(turn * (1. + 1e-9), 0.));

            assert_eq!(
                under.len(),
                over.len(),
                "at {turn} the ruling went from {under:?} to {over:?}"
            );
            for (before, after) in under.iter().zip(&over) {
                assert!(
                    (before.0 - after.0).abs() < before.0 * 1e-6,
                    "at {turn} a row of lines moved from {} apart to {}",
                    before.0,
                    after.0
                );
                assert!(
                    (before.1 - after.1).abs() < 1e-3,
                    "at {turn} a row of lines went from {} to {}",
                    before.1,
                    after.1
                );
            }
        }
    }

    /// Every number written falls on a line of the fine plane
    ///
    /// The numbers and the ruling are chosen separately — one for how many
    /// will fit, the other for how dense the lines may be — so nothing but
    /// this says they agree. A number standing between two lines is a number
    /// about a place the ruling does not mark.
    #[test]
    fn every_number_falls_on_a_line() {
        for across in zooms() {
            let step = tick_step(across);
            let cell = ruling(across, 0.).fine;
            let cells = step / cell;
            assert!(
                (cells - cells.round()).abs() < 1e-6,
                "{across} across steps {step} over cells of {cell}"
            );
        }
    }

    /// No pair of numbers can run into the next
    ///
    /// The lettering is sized against the view and painted along its own
    /// lines, so what keeps two of them apart is the spacing and nothing else.
    /// It has to clear the widest a pair can ever be, at every rung of the
    /// ladder and every zoom — not on average, and not usually.
    #[test]
    fn no_pair_can_run_into_the_next() {
        for across in zooms() {
            let step = numbering(across);
            let widest = SPAN as f64 * across / (5. * FIGURES_ACROSS);
            assert!(
                step >= widest,
                "{across} across numbered every {step}, which a pair {widest} \
                 wide runs straight through"
            );
        }
    }

    /// And it is still a round number, however far it had to climb
    #[test]
    fn a_stepped_up_number_is_still_round() {
        for across in zooms() {
            let step = numbering(across);
            let decade = 10f64.powf(step.log10().floor());
            let rung = step / decade;
            assert!(
                [1., 2., 5.].iter().any(|it| (rung - it).abs() < 1e-6),
                "{across} across steps {step}, which is {rung} of a decade"
            );
        }
    }

    /// The numbers are stepped one, two or five times a power of ten
    ///
    /// The ladder a scale is read in everywhere scales are read. Three, seven
    /// or eleven is arithmetic the reader has to do.
    #[test]
    fn numbers_step_by_one_two_or_five() {
        for across in zooms() {
            let step = tick_step(across);
            let decade = 10f64.powf(step.log10().floor());
            let rung = step / decade;
            assert!(
                [1., 2., 5.].iter().any(|it| (rung - it).abs() < 1e-6),
                "{across} across steps {step}, which is {rung} of a decade"
            );
        }
    }

    /// And there are enough of them to read a scale off, without a wall of
    /// them
    ///
    /// A lattice rather than a row, so this is counted across the view in one
    /// direction and squared for what actually lands on screen. Two either way
    /// is a handful; five either way is twenty five numbers over the sky.
    #[test]
    fn the_plane_holds_a_handful_of_numbers() {
        for across in zooms() {
            let ticks = across / tick_step(across);
            assert!(
                (1. ..=5.).contains(&ticks),
                "{across} across wanted {ticks} numbers either way"
            );
        }
    }

    /// Snapping lands on a multiple, which is what keeps the ruling still
    ///
    /// The plane is moved under the camera every frame and its lines must not
    /// move with it. They do not, so long as every place it is moved to is a
    /// whole number of cells from every other.
    #[test]
    fn snapping_lands_on_a_multiple() {
        // Out at the rim, in light years, which is where a float would have
        // given up long ago and the `f64` this is done in must not.
        let step = 1e-3;
        for out in [0., 1., 1234.5678, 20_000.371, 68_272.94] {
            let landed = snapped(out, step);
            let cells = landed / step;
            assert!(
                (cells - cells.round()).abs() < 1e-6,
                "{out} snapped to {landed}, which is {cells} cells"
            );
            assert!(
                (landed - out).abs() <= step / 2. + f64::EPSILON * out.abs(),
                "{out} snapped to {landed}, more than half a cell away"
            );
        }
    }

    /// Zero is a multiple of every step, so a crossing snapped anywhere near
    /// the middle lands exactly on it
    ///
    /// Which is what puts a `0` at the middle of a ruler rather than a number
    /// that happens to be small. The rulers are read against the crossing, so
    /// where the crossing is off by a hair every number along them is.
    #[test]
    fn a_crossing_near_the_middle_lands_on_it() {
        let step = 100.;
        for along in [-49., -0.4, 0., 12., 49.9] {
            assert_eq!(snapped(along, step), 0.);
        }
    }
}
