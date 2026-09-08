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
//! a wide view stops spawning an entity per system, and the glow rendering
//! will draw [`Planned`]'s splats as the field behind them. Both read the one
//! walk so the two can never disagree about which cells are which.
//!
//! A cell's splat carried a drawable description here for a while — where the
//! glow sits, how far it spreads, its flux-weighted tint — written every plan
//! and read by nobody, the renderer it was for never having been written. It
//! is in the history rather than in the build, to be worked out again against
//! the renderer that will read it.
//!
//! Read off the resident aggregates, so it costs no fetch and no server, and
//! only when the view moves.

use crate::ResidentIndex;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::scale::View;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::{Mode, Needed, View as Viewpoint};

pub fn plugin(app: &mut App) {
    app.insert_resource(Planned(Needed {
        mode: Mode::Shell,
        marks: Vec::new(),
        splats: Vec::new(),
    }));
    // After the camera has settled where it stands this frame, and read for the
    // same reason everything in `Present` is: the plan follows the eye.
    app.add_systems(Update, plan.in_set(MapSet::Present));
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
///
/// And whenever the aggregates themselves move. The walk plans off
/// [`ResidentIndex`], so a cell the index has only just published is a cell
/// this has never marked — and a camera at rest is the ordinary case, the eye
/// key alone holding the last move's answer for as long as nobody touches the
/// mouse. Without this a republished cell could reach the map only by being
/// evicted and asked for again, which is what zooming out until the walk stops
/// marking it and coming back does. See [`crate::refresh`].
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
    if last.as_ref() == Some(&key) && !index.is_changed() {
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

    /// Republished aggregates are re-walked without the camera moving
    ///
    /// The reported trouble: the walk is skipped while the eye stands still,
    /// and a camera at rest is the ordinary case — so a cell the aggregates
    /// did not hold when the map started could not be marked, could not be
    /// fetched, and never appeared. The only way to see one was to move the
    /// camera. See [`crate::refresh`].
    #[test]
    fn a_republished_index_is_walked_again_where_it_stands() {
        use galos_index::{BuildParams, Snapshot};

        let mut app = App::new();
        app.add_systems(Update, plan);
        app.insert_resource(Planned(Needed {
            mode: Mode::Shell,
            marks: Vec::new(),
            splats: Vec::new(),
        }));
        app.insert_resource(View::Map);
        app.insert_resource(ResidentIndex(galos_index::Index::default()));
        app.world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()));

        app.update();
        let empty = app.world().resource::<Planned>().0.marks.len();
        assert_eq!(empty, 0, "an empty index planned marks");

        // The builder publishes systems the map had never heard of. Nothing
        // touches the camera.
        let inputs: Vec<galos_index::System> = (1..=4)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: [id as f64, 0., 0.],
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());
        app.insert_resource(ResidentIndex(built.index.clone()));

        app.update();
        assert!(
            app.world().resource::<Planned>().0.marks.len() > empty,
            "the walk kept the plan it made before the cells existed"
        );
    }
}
