//! Planning the aggregate draw
//!
//! One walk of the resident index turns where the camera stands into what the
//! view needs: the cells whose systems draw as discrete marks, and the cells
//! that draw as a splat of the aggregate. The marks are also the fetch set; the
//! splats need nothing loaded, since a cell's aggregate stands for its whole
//! subtree.
//!
//! This is the plan alone. Nothing draws from it yet: [`super::spawn`] will
//! come to fetch by [`Planned`]'s marks rather than by the spyglass region, so
//! a wide view stops spawning an entity per system, and the splat rendering
//! will draw [`Planned`]'s splats as the glow behind them. Both read the one
//! walk so the two can never disagree about which cells are which.
//!
//! Read off the resident aggregates, so it costs no fetch and no server, and
//! only when the view moves.

use crate::ResidentIndex;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::scale::View;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::aggregate::TEMP_BUCKETS;
use galos_index::{Aggregate, Cell, Mode, Needed, View as Viewpoint};
use galos_photometry::blackbody_color;

pub fn plugin(app: &mut App) {
    app.insert_resource(Planned(Needed {
        mode: Mode::Shell,
        marks: Vec::new(),
        splats: Vec::new(),
    }));
    // After the camera has settled where it stands this frame, and read for the
    // same reason everything in `Present` is: the plan follows the eye.
    app.add_systems(Update, plan.in_set(MapSet::Present));
    app.init_resource::<Splats>();
    // After the plan it reads: the splats it describes are the cells the walk
    // would draw as a glow field, kept resident for that field's renderer.
    // Nothing draws them yet — the billboard first pass was removed because
    // its discrete blobs popped and would not blend; see galaxy.md.
    app.add_systems(Update, describe.in_set(MapSet::Present).after(plan));
}

/// What the walk asks the view for: the cells to draw as marks and as splats
///
/// `marks` is the discrete set — one system apiece from a cell's payload, and
/// so also what a loader fetches — and `splats` is the aggregate field drawn
/// from the index alone. The map does not read it yet; it is the seam the
/// bounded fetch and the glow will both plan on.
#[derive(Resource)]
pub struct Planned(pub Needed);

/// Walk the index for what the camera needs, when the camera has moved
///
/// Re-walked whenever the eye or the direction it looks changes — a move, a
/// zoom, or a turn — because the walk is culled to the view frustum, so a turn
/// changes what is framed. A far orbit that keeps looking at its centre frames
/// the same region at ≈the same distance whatever the angle, so the marks come
/// back the same and nothing spawns or evicts; a near eye among the stars
/// sweeps in new sky on a turn, which is what should load. The mode follows
/// the drawn [`View`]: the shell over a political field, or the photometric
/// sky.
fn plan(
    cameras: Query<(&OrbitCamera, &Camera)>,
    index: Res<ResidentIndex>,
    view_mode: Res<View>,
    mut planned: ResMut<Planned>,
    mut last: Local<Option<(DVec3, Quat, Mode, UVec2)>>,
) {
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = view(orbit, camera) else { return };
    let mode = match *view_mode {
        View::Map => Mode::Shell,
        View::Realistic => Mode::Real,
    };
    let size = camera.logical_viewport_size().unwrap_or_default().as_uvec2();
    let key = (orbit.eye, orbit.rotation, mode, size);
    if last.as_ref() == Some(&key) {
        return;
    }
    *last = Some(key);
    planned.0 = index.0.needed(&view, mode);
}

/// The index's view of where the camera stands, if it has a viewport to see
/// through
///
/// The map draws in light years about the galactic centre, which is what the
/// index cells are keyed in, so the eye is handed over as it stands. The lens
/// is read off the camera's own clip matrix rather than its projection, since
/// that is where the field of view has already been worked out.
pub fn view(orbit: &OrbitCamera, camera: &Camera) -> Option<Viewpoint> {
    let viewport = camera.logical_viewport_size()?;
    // `y_axis.y` of the clip matrix is the cotangent of half the vertical field
    // of view.
    let cot_half_fov = camera.clip_from_view().y_axis.y;
    Some(viewpoint(orbit.eye, orbit.rotation, cot_half_fov, viewport))
}

/// Where the eye is, which way it faces, and the lens, as the index wants them
///
/// Split out from [`view`] so the arithmetic can be checked without a camera to
/// hand: a real [`Camera`] answers nothing for its viewport until the render
/// target it draws to is up.
fn viewpoint(
    eye: DVec3,
    rotation: Quat,
    cot_half_fov: f32,
    viewport: Vec2,
) -> Viewpoint {
    let forward = (rotation * Vec3::NEG_Z).as_dvec3();
    let up = (rotation * Vec3::Y).as_dvec3();
    Viewpoint {
        eye: [eye.x, eye.y, eye.z],
        forward: [forward.x, forward.y, forward.z],
        up: [up.x, up.y, up.z],
        fov_y: (2.0 * (1.0 / cot_half_fov as f64).atan()) as f32,
        viewport_height: viewport.y,
        aspect: viewport.x / viewport.y,
    }
}

/// A cell's aggregate as one drawable glow: where it sits, how far it spreads,
/// its colour, and how much light it carries
///
/// The colour is the flux-weighted blackbody tint of the cell's temperature
/// buckets, so a warm bulge and blue arms come out without a temperature per
/// star; the flux is the intensity the glow is drawn at, which bloom turns
/// into apparent size. Positions and the spread are in light years.
pub struct Splat {
    /// The flux-weighted centre of the cell, light years.
    pub at: DVec3,
    /// The flux-weighted RMS radius, the Gaussian footprint, light years.
    pub spread: f64,
    /// The flux-weighted blackbody tint, chroma with a channel near one; the
    /// flux carries brightness, not this.
    pub color: LinearRgba,
    /// The total linear flux over the cell's subtree.
    pub flux: f64,
    /// The share of the cell's weight this draw lays down, `0.0..=1.0`: the
    /// walk's cross-level fade, so a splat mid-split contributes less and its
    /// children make up the rest without the field brightening.
    pub blend: f64,
}

/// The splats the walk asked for, described from the resident aggregates
///
/// Nothing draws them yet: this is the drawable form the glow rendering will
/// read, kept apart so the description can be tested without a renderer.
#[derive(Resource, Default)]
pub struct Splats(pub Vec<Splat>);

/// The representative temperature of a flux bucket, kelvin
///
/// The buckets are even in log temperature between the coolest star worth
/// colouring and the hottest whose blue has stopped moving (see
/// [`galos_index::aggregate::temp_bucket`]); this is the geometric centre of
/// one, which the whole of its flux is coloured as.
fn bucket_temperature(bucket: usize) -> f64 {
    const LO: f64 = 2000.0;
    const HI: f64 = 50000.0;
    let f = (bucket as f64 + 0.5) / TEMP_BUCKETS as f64;
    (LO.ln() + f * (HI.ln() - LO.ln())).exp()
}

/// One cell as a splat, or [`None`] where it carries no light to draw.
pub fn splat(cell: &Cell) -> Option<Splat> {
    splat_of(&cell.aggregate, cell.id.bounds().center())
}

/// A splat from an aggregate, sitting at `centre` where it holds no
/// light-weighted one of its own
///
/// Split from [`splat`] so the colour and the weighting can be checked without
/// a cell to build one from.
fn splat_of(aggregate: &Aggregate, centre: [f64; 3]) -> Option<Splat> {
    let flux = aggregate.total_flux();
    if flux <= 0.0 {
        return None;
    }
    let at = aggregate.luminosity_centroid().unwrap_or(centre);
    // The tint is the flux-weighted mean of each bucket's blackbody colour, so
    // a cell of mostly cool stars comes out red and a hot few carry their blue
    // in proportion to the light they add.
    let mut rgb = [0.0f64; 3];
    for (bucket, &f) in aggregate.flux().iter().enumerate() {
        if f <= 0.0 {
            continue;
        }
        let tint = blackbody_color(bucket_temperature(bucket));
        for (channel, &weight) in rgb.iter_mut().zip(tint.iter()) {
            *channel += f * weight as f64;
        }
    }
    Some(Splat {
        at: DVec3::new(at[0], at[1], at[2]),
        spread: aggregate.luminosity_spread(),
        color: LinearRgba::rgb(
            (rgb[0] / flux) as f32,
            (rgb[1] / flux) as f32,
            (rgb[2] / flux) as f32,
        ),
        flux,
        blend: 1.0,
    })
}

/// Describe the cells the walk marked for splatting, when the plan changes
///
/// One splat per cell, off the resident aggregate, so it costs no fetch, and
/// only on a new plan: a still view splats the same cells the same way.
fn describe(
    index: Res<ResidentIndex>,
    planned: Res<Planned>,
    mut splats: ResMut<Splats>,
) {
    if !planned.is_changed() {
        return;
    }
    splats.0.clear();
    for sr in &planned.0.splats {
        if let Some(cell) = index.0.get(sr.id) {
            if let Some(mut described) = splat(cell) {
                described.blend = sr.blend;
                splats.0.push(described);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The viewpoint faces where the camera looks and carries its lens
    ///
    /// At rest and unrotated the camera looks down its own negative Z with Y
    /// up, and a clip cotangent of one is a ninety degree vertical field.
    #[test]
    fn the_viewpoint_reads_the_camera() {
        let view = viewpoint(
            DVec3::new(1., 2., 3.),
            Quat::IDENTITY,
            1.,
            Vec2::new(1600., 900.),
        );

        assert_eq!(view.eye, [1., 2., 3.]);
        assert!((view.forward[2] + 1.).abs() < 1e-6, "not facing -Z");
        assert!((view.up[1] - 1.).abs() < 1e-6, "not Y up");
        assert!(
            (view.fov_y - std::f32::consts::FRAC_PI_2).abs() < 1e-5,
            "cot 1 is a 90 degree field"
        );
        assert!((view.aspect - 1600. / 900.).abs() < 1e-6);
        assert_eq!(view.viewport_height, 900.);
    }

    /// A turned camera turns the forward and up with it
    #[test]
    fn a_turn_carries_forward_and_up() {
        let quarter = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let view = viewpoint(DVec3::ZERO, quarter, 1., Vec2::new(100., 100.));

        // A quarter turn about Y sends -Z to -X, and leaves Y up.
        assert!((view.forward[0] + 1.).abs() < 1e-6, "not facing -X");
        assert!((view.up[1] - 1.).abs() < 1e-6, "up did not stay Y");
    }

    /// A cell of one sun-like star splats where it sits, in a warm tint
    #[test]
    fn a_star_splats_where_it_sits() {
        let sun = Aggregate::of_system([10., 0., -5.], 4.83, 5772.0, 0);
        let splat = splat_of(&sun, [0., 0., 0.]).expect("a star has light");

        assert_eq!(splat.at, DVec3::new(10., 0., -5.), "not at the star");
        assert!(splat.flux > 0.0);
        assert_eq!(splat.spread, 0.0, "one point has no spread");
        assert!(splat.color.red >= splat.color.blue, "the sun came out blue");
    }

    /// An empty aggregate has nothing to splat
    #[test]
    fn no_light_no_splat() {
        assert!(splat_of(&Aggregate::ZERO, [1., 2., 3.]).is_none());
    }

    /// The glow sits at the light, not the middle: a bright star pulls the
    /// splat's centre toward it
    #[test]
    fn the_splat_follows_the_light() {
        let bright = Aggregate::of_system([100., 0., 0.], 0.0, 6000.0, 0);
        let dim = Aggregate::of_system([-100., 0., 0.], 10.0, 4000.0, 0);
        let splat = splat_of(&bright.merge(dim), [0., 0., 0.]).unwrap();

        assert!(splat.at.x > 50.0, "the centre ignored the bright star");
    }
}
