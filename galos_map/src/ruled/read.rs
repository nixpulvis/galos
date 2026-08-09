//! What is drawn over a ruled plane rather than into it
//!
//! A plane's own lines and the numbers along them are painted by the shader.
//! What stands over it is here: the crosses that mark a place worth locating,
//! the lines dropped to it from whatever is off it, and the three numbers about
//! each of those places.
//!
//! Everything here fades with the plane under it. [`faded`] is the arithmetic
//! `ruled.wgsl` does per fragment, worked out for one point, so a number over
//! the plane goes as the plane goes.
use bevy::math::{DVec3, Vec2};

/// How sharply a plane goes as it is turned edge on
///
/// The cosine below which the ruling has gone entirely, which loses the plane
/// as the camera comes level with it. What [`super::Plane::edge_on`] is set to
/// unless a caller says otherwise, and what [`faded`] weighs a place against.
pub const EDGE_ON: f32 = 0.25;

/// How long each arm of a cross marking a place on the plane is, in pixels
///
/// The numbers at the middle are about one point on the plane, and a number
/// written over a plane with nothing under it is a number floating loose. So
/// the point is marked, along the plane's own axes, and they stand beside it.
///
/// The arms are laid in the plane rather than across the screen, so a cross
/// out towards the horizon is foreshortened the way the cells around it are.
/// It is a mark scratched on the plane and not a pointer laid over it.
pub const CROSS: f32 = 11.;

/// How tall a number standing over the plane draws, in logical pixels
///
/// The line box, which for one line of text is the size the face is set at.
/// The size the chrome's smallest lettering is set at: these are read at a
/// glance off a map rather than pored over, and there are up to a dozen of
/// them on screen at once.
pub const READS: f32 = 8.;

/// How far off the plane the middle's numbers are hung, in pixels
///
/// The two rulers lie in the plane and their numbers run along them. The third
/// is about the plane itself, so it is hung along the one direction on screen
/// that neither ruler runs in. Drawn where they cross it reads as one more
/// number in the row.
///
/// Under the plane rather than over it, which is the opposite side from the one
/// a pair on the plane is written on. The two are then on either side of the
/// lines they are both about, and the middle is read against a clear row rather
/// than into a number.
///
/// Far enough down to clear what is drawn around the place itself. The arms of
/// the cross reach [`CROSS`] from it and the ring around a thing picked out
/// reaches further still, so a row hung to clear the cross alone lands inside
/// the ring of a selection the camera is looking straight at.
pub const LIFT: f32 = 24.;

/// And how far to the side of a dropped line its own number stands, in pixels
///
/// Beside the line rather than over it, for the same reason a pair on the plane
/// stands beside its crossing: a number with a rule through it is a number to
/// be worked out rather than read.
pub const ASIDE: f32 = 6.;

/// How far a row of numbers reaches around the point it is about, in pixels
///
/// About the row the map writes there: three numbers each with its own power, a
/// unit and two commas comes to some forty characters of a [`READS`] tall
/// monospaced face, centred on the point, so it runs about ninety five either
/// side. Across it the [`LIFT`] that hangs it off the plane and half its own
/// height.
///
/// In pixels rather than in the plane's own units because the row holds one
/// size on screen and the plane does not. A unit of plane covers most of a
/// digit's width on screen with the camera overhead and a fraction of one with
/// the camera down near the plane, so a reach fixed in units is a reach that
/// means something different at every pitch. [`stand_clear`] converts.
pub const CROWDS: Vec2 = Vec2::new(96., 30.);

/// How much of the plane is left at a point on it, as the ruling fades
///
/// The plane's own fade, worked out for one point rather than for every pixel:
/// how far out it stands, softened towards nothing as the view squares up on
/// the plane, and how edge on the plane is there. `ruled.wgsl` does the same
/// arithmetic per fragment, and what is written over the plane by hand has to
/// carry it too or it goes on standing over a ruling that has gone.
///
/// Everything drawn on the plane takes it, lines and numbers alike and by the
/// same amount: what is left of the plane here is what anything on it is drawn
/// into. What sets a number apart from a line is the ink it starts in and
/// nothing else, [`super::INK`] against [`super::MINOR`] or
/// [`super::MAJOR`], so the numbers hold
/// on well after the lines have gone, which is the right way round, a ruler
/// being read off its numbers.
pub fn faded(from_eye: DVec3, reach: f64, edge_on: f32) -> f32 {
    let far = from_eye.length();
    if far <= 0. || reach <= 0. {
        return 1.;
    }
    let square = (from_eye.y.abs() / far) as f32;
    let near = (1. - far / reach).clamp(0., 1.) as f32;
    (near + (1. - near) * square) * (square / edge_on).min(1.)
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
