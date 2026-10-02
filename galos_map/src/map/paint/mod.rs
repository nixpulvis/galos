//! How the galaxy reaches the screen: the star field, the glow behind it and
//! the volume its evenly filled cells are ray-marched as, the curve the two
//! are brought onto the display through, the size each mark is drawn at, and
//! the spheres a descended system is made of.

pub(crate) mod curve;
pub(crate) mod field;
pub(crate) mod glow;
pub(crate) mod sizing;
pub(crate) mod sphere;
pub(crate) mod volume;

use bevy::prelude::*;

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(sphere::plugin);
    app.add_plugins(field::plugin);
    app.add_plugins(glow::plugin);
    app.add_plugins(volume::plugin);
    app.add_plugins(curve::plugin);
    app.add_plugins(sizing::plugin);
}
