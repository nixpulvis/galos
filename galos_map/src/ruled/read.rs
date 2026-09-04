//! What can be read off a ruled plane
//!
//! A plane's own lines and the numbers along them are painted by the shader.
//! What the ruling comes to for whatever is drawn over it is here: how strongly
//! it is drawn, how far it has faded towards the plane's horizon, and where the
//! camera is looking.
//!
//! The numbers themselves — the three about the place looked at and about each
//! thing picked out — along with the crosses that mark those places and the
//! lines dropped to the plane, are painted flat in screen space by
//! [`crate::grid::draw_readouts`]. Projected on the processor in `f64` from the
//! camera and each thing's true position, they hold steady where a text mesh out
//! at a system's galaxy coordinate jitters; see there and `docs/night-sky.md`.
use super::DistanceUnit;
use bevy::math::DVec3;
use bevy::prelude::*;

/// How sharply a plane goes as it is turned edge on
///
/// The cosine below which the ruling has gone entirely, which loses the plane
/// as the camera comes level with it. What [`super::Plane::edge_on`] is set to
/// unless a caller says otherwise, and the shader's own term. Nothing standing
/// over the plane takes it; see [`faded`].
pub const EDGE_ON: f32 = 0.25;

/// How much is left at a point on the plane, as the ruling fades out
///
/// The plane is unbounded, so everything on it fades away towards its horizon.
/// `reach` is how far that runs. Whatever stands over the plane carries the
/// same fade or it goes on standing over a ruling that has gone. `from_eye` and
/// `reach` need only be in one unit between them, being a length over a length.
///
/// The pitch does not enter. The lines themselves are lost as the plane is
/// turned edge on, because a ruling at a grazing angle is moire rather than
/// lines, and [`EDGE_ON`] is where the shader gives up on them. A number is
/// drawn facing the camera and a dropped line stands across the plane rather
/// than along it, so neither has that trouble; fading them with the lines would
/// take the numbers away exactly when the view is too flat to read anything
/// else off the plane, which is when they are the only thing left worth reading.
pub fn faded(from_eye: DVec3, reach: f64) -> f32 {
    let far = from_eye.length();
    if far <= 0. || reach <= 0. {
        return 1.;
    }
    (1. - far / reach).clamp(0., 1.) as f32
}

/// How strongly something the ruling draws comes out, once the caller has had
/// its say
///
/// One knob over the whole of it. The lines and the numbers along them are one
/// thing seen at once, and a ruler whose lines dimmed while its numbers did not
/// would read as two.
///
/// Never past whole, an alpha having nowhere above one to go.
pub fn drawn_at(strength: f32, bright: f32) -> f32 {
    (strength * bright).clamp(0., 1.)
}

/// Where a plane's rulers stand, and what they are said in
///
/// Written by whoever rules the plane, every frame, before anything here is
/// read. How wide a cell is and how far apart the numbers go are [`super::ladder`]'s
/// to answer and the caller's to ask; this is where the answers land.
///
/// Nothing is drawn over a plane whose [`Reading::strength`] has run out. A
/// number standing over a ruling that has gone is a number about nothing.
#[derive(Component, Clone, Copy, Debug)]
pub struct Reading {
    /// Where the camera is looking, in [`Reading::unit`] along the plane's own
    /// axes from its origin
    ///
    /// The middle of the view, which is where the plane hangs and where the
    /// three numbers of the place being looked at are said. Not snapped, so it
    /// sits still while the plane slides under it. Its `y` is therefore the
    /// altitude the plane itself is hung at.
    pub at: DVec3,
    /// How far apart two numbers are, in [`Reading::unit`]
    pub step: f64,
    pub unit: DistanceUnit,
    /// How much of the ruling is drawn, which everything over it follows
    pub strength: f32,
    /// How loudly the whole of it is asked for, over that
    pub bright: f32,
    /// Whether the place the camera is looking at is said out loud
    pub middle: bool,
}

impl Default for Reading {
    fn default() -> Self {
        Reading {
            at: DVec3::ZERO,
            step: 0.,
            unit: DistanceUnit { metres: 1., mark: "m" },
            strength: 0.,
            bright: 1.,
            middle: false,
        }
    }
}

/// Something a plane should locate
///
/// A line is dropped to the plane from wherever it stands, with the three
/// numbers the plane can say about it under the foot and how far off the plane
/// it went beside the line. What is worth pointing out is the caller's to
/// decide; put this on it and [`crate::grid::draw_readouts`] says where it is.
#[derive(Component)]
pub struct Located;

#[cfg(test)]
mod tests {
    use super::*;

    /// What is drawn over the plane fades out towards the plane's horizon
    ///
    /// Whole where the camera stands, gone at the reach, and evenly between.
    #[test]
    fn what_is_written_fades_out_towards_the_horizon() {
        let reach = 60.;
        assert_eq!(faded(DVec3::ZERO, reach), 1.);
        assert_eq!(faded(DVec3::new(0., -reach, 0.), reach), 0.);
        assert_eq!(faded(DVec3::new(0., -reach / 2., 0.), reach), 0.5);
        // And nothing is left past it.
        assert_eq!(faded(DVec3::new(0., -reach * 2., 0.), reach), 0.);
    }

    /// And not by how the plane is pitched
    ///
    /// The lines go as the plane is turned edge on, a ruling at a grazing
    /// angle being moire rather than lines. Nothing standing over it has that
    /// trouble, and fading it with them would take the numbers away exactly
    /// when the view is too flat to read anything else off the plane.
    #[test]
    fn what_is_written_does_not_fade_by_pitch() {
        let reach = 60.;
        let far = 10.;
        // Straight down onto the plane, and along it as near as makes no
        // difference, at the one distance.
        let square = faded(DVec3::new(0., -far, 0.), reach);
        let grazing = faded(DVec3::new(far, 0., 0.), reach);
        assert_eq!(square, grazing);
        assert!(square > 0., "nothing was left to tell apart");
    }
}
