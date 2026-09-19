//! Planning the aggregate draw
//!
//! One walk of the resident index turns where the camera stands into what the
//! view needs: the cells whose systems draw as discrete marks, and the cells
//! that draw as a splat of the aggregate. The marks are also the fetch set; the
//! splats need nothing loaded, since a cell's aggregate stands for its whole
//! subtree.
//!
//! **Both halves drive the map.** [`super::bounded`] fetches the cells the
//! marks name and spawns one entity per system in their payloads, rather than
//! every system in a spyglass sphere; [`super::glow`] lays each splat down as
//! one additive Gaussian off the cell's aggregates, with nothing fetched.
//! Both read the one walk, so the two can never disagree about which cells are
//! which.
//!
//! A cell's splat carried a drawable description here for a while — where the
//! glow sits, how far it spreads, its flux-weighted tint — written every plan
//! and read by nobody, the renderer it was for never having been written. It
//! stays out of the plan now that the renderer exists: [`super::glow`] reads
//! the aggregates it needs off [`crate::ResidentIndex`] as it lays each quad,
//! so nothing is worked out here for a consumer to ignore.
//!
//! Read off the resident aggregates, so it costs no fetch and no server, and
//! only when the view moves.

use crate::ResidentIndex;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::scale::View;
use bevy::math::DVec3;
use bevy::prelude::*;
use galos_index::{
    CellId, Inhabited, Mode, Moments, Needed, View as Viewpoint,
};

pub fn plugin(app: &mut App) {
    app.insert_resource(Planned(Needed {
        mode: Mode::Shell,
        marks: Vec::new(),
        blobs: Vec::new(),
        splats: Vec::new(),
    }));
    app.init_resource::<Drawn>();
    // After the camera has settled where it stands this frame, and read for the
    // same reason everything in `Present` is: the plan follows the eye.
    app.add_systems(Update, plan.in_set(MapSet::Present));
}

/// What the walk asks the view for: the cells to draw as marks and as splats
///
/// `marks` is the discrete set — one system apiece from a cell's payload, and
/// so also what a loader fetches — and `splats` is the aggregate field drawn
/// from the index alone. Both halves are read: [`super::bounded`] fetches and
/// spawns the marks, [`super::glow`] lays the splats down.
#[derive(Resource)]
pub struct Planned(pub Needed);

/// What each marked cell's drawn systems already account for
///
/// The other half of the plan, and the thing that keeps the two halves from
/// drawing the same systems twice. A cell can be marked *and* splatted in the
/// same walk — the two tests are independent, and `galos_index::walk` means
/// them to be — so a cell whose systems are on the map would also have its
/// whole subtree laid into the field behind them. The field subtracts this and
/// draws the rest.
///
/// **It is what makes the fetch and the budget invisible.** What a cell has
/// not loaded, what the spawn budget has not reached, what the spyglass clamps
/// away per point and what the resolvable prefix leaves out are all simply
/// absent from here, so they stay in the residual and go on being drawn as
/// light. A cell mid-fetch shows its field; as its points arrive the light
/// moves from the splat to the marks with nothing added and nothing lost.
///
/// Written by [`super::bounded::reconcile`], which is the only thing that
/// knows the drawn set: the prefix is not a rank range, the filters having
/// promoted systems out of magnitude order (see `drawn_first`).
#[derive(Resource, Default)]
pub struct Drawn(pub rustc_hash::FxHashMap<CellId, Accounted>);

/// One cell's drawn systems, in the terms the two channels of the field are
/// laid in
///
/// Counts and moments rather than a list of addresses: what the field needs is
/// exactly what [`Moments::remove`] and [`Inhabited::remove`] take, and both
/// are the exact inverses of the merges that built the totals.
#[derive(Clone, Copy, Default)]
pub struct Accounted {
    /// How many of the cell's systems are drawn as themselves.
    pub count: u64,
    /// Their positions in count weight, for the backdrop's residual.
    pub mass: Moments,
    /// What they carry politically, for the colonies'.
    pub inhabited: Inhabited,
}

impl Accounted {
    /// Take one drawn system into the account.
    ///
    /// `political` is the system's reading where it is on the populated table
    /// and somebody lives in it, and [`None`] otherwise — the same
    /// `population > 0` the rest of the map tells inhabited by.
    pub fn took(&mut self, position: [f64; 3], political: Option<Inhabited>) {
        self.count += 1;
        self.mass = self.mass.merge(Moments::point(1.0, position));
        if let Some(one) = political {
            self.inhabited = self.inhabited.merge(one);
        }
    }
}

/// Walk the index for what the camera needs, when the camera has moved
///
/// Re-walked when the eye moves, when the viewport resizes, and when the drawn
/// [`View`] changes — the shell over a political field, or the photometric sky.
/// Not when the camera merely turns: every cut the index makes is a pure
/// function of eye position, [`galos_index::Index::walk_screen`] keeping "no
/// budget, no frustum" and the Real mode's two walks reading `view.eye` alone,
/// so a turn about one eye returns the same marks and the same splats. The
/// viewport earns its place in the key because the separation the marks are cut
/// at is measured in pixels. Put a frustum back in the walk and the rotation
/// has to come back into the key with it.
///
/// And whenever the aggregates themselves move. The walk plans off
/// [`ResidentIndex`], so a cell the index has only just published is a cell
/// this has never marked — and a camera at rest is the ordinary case, the eye
/// key alone holding the last move's answer for as long as nobody touches the
/// mouse. Without this a republished cell could reach the map only by being
/// evicted and asked for again, which is what zooming out until the walk stops
/// marking it and coming back does. See [`crate::refresh`].
pub(crate) fn plan(
    cameras: Query<(&OrbitCamera, &Camera)>,
    index: Res<ResidentIndex>,
    view_mode: Res<View>,
    spyglass: Res<crate::systems::Spyglass>,
    mut planned: ResMut<Planned>,
    mut last: Local<Option<(DVec3, Mode, UVec2, Option<(DVec3, f32)>)>>,
) {
    let Ok((orbit, camera)) = cameras.single() else { return };
    let Some(view) = view(orbit, camera) else { return };
    let mode = match *view_mode {
        View::Map => Mode::Shell,
        View::Realistic => Mode::Real,
    };
    let size = camera.logical_viewport_size().unwrap_or_default().as_uvec2();
    // The spyglass is a clamp on the walk and not a filter after it: a
    // subtree the bubble does not touch is never descended into, so the
    // sets the map works over are the sets it draws from. See
    // [`galos_index::Reach`], and [`super::bounded::reach`] for why the
    // clamp is the spyglass's `clear` rather than its radius alone.
    let bubble = spyglass
        .clear
        .then(|| (orbit.center(), spyglass.radius));
    let key = (orbit.eye(), mode, size, bubble);
    if last.as_ref() == Some(&key) && !index.is_changed() {
        return;
    }
    *last = Some(key);
    let within = bubble.map(|(center, radius)| galos_index::Reach {
        center: center.to_array(),
        radius: f64::from(radius),
    });
    planned.0 = index.0.needed(&view, mode, within);
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
    Some(viewpoint(orbit.eye(), orbit.rotation, cot_half_fov, viewport))
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
        app.insert_resource(crate::systems::Spyglass {
            radius: 0.,
            clear: false,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(Planned(Needed {
            mode: Mode::Shell,
            marks: Vec::new(),
            blobs: Vec::new(),
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
        // touches the camera. Enough of them to be worth reading: the walk
        // marks a cell once it is worth [`galos_index::MARK_LEAST`] marks,
        // so four systems are not a plan whatever the camera does.
        let inputs: Vec<galos_index::System> = (1..=64)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: [id as f64, 0., 0.],
                absolute_magnitude: id as f64,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::StarKind::G,
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

    /// Turning in place does not walk the index again
    ///
    /// Every cut the walk makes is a pure function of where the eye is — no
    /// frustum, no direction — so a camera turning about one eye asks for the
    /// same cells it already holds. Keying the plan on the rotation as well
    /// walked the whole tree for a bit-identical answer on every frame of a
    /// turn, which is the one motion the map makes continuously. Moving the eye
    /// still re-walks, which is the half that has to keep working.
    #[test]
    fn a_turn_in_place_does_not_walk_again() {
        use galos_index::{BuildParams, Snapshot};

        #[derive(Resource, Default)]
        struct Walks(usize);

        fn count(planned: Res<Planned>, mut walks: ResMut<Walks>) {
            if planned.is_changed() {
                walks.0 += 1;
            }
        }

        let inputs: Vec<galos_index::System> = (1..=64)
            .map(|id| galos_index::System {
                id64: id as u64,
                position: [id as f64, 0., 0.],
                absolute_magnitude: id as f64 / 8.,
                temperature: 5000.,
                age_bucket: 0,
                updated_at: 0,
                kind: galos_index::StarKind::G,
            })
            .collect();
        let built = Snapshot::build(&inputs, &BuildParams::default());

        let mut app = App::new();
        app.add_systems(Update, (plan, count.after(plan)));
        app.insert_resource(crate::systems::Spyglass {
            radius: 0.,
            clear: false,
            lock_camera: false,
            follow_camera: true,
        });
        app.insert_resource(Planned(Needed {
            mode: Mode::Shell,
            marks: Vec::new(),
            blobs: Vec::new(),
            splats: Vec::new(),
        }));
        app.insert_resource(View::Map);
        app.insert_resource(ResidentIndex(built.index.clone()));
        app.init_resource::<Walks>();
        let camera = app
            .world_mut()
            .spawn((OrbitCamera::default(), crate::systems::tests::seeing()))
            .id();

        app.update();
        let walked = app.world().resource::<Walks>().0;
        assert!(walked > 0, "the first frame planned nothing");
        let marks = app.world().resource::<Planned>().0.marks.clone();
        assert!(!marks.is_empty(), "an index of 64 systems marked no cells");

        // A quarter turn about Y, the eye left where it stands.
        app.world_mut()
            .entity_mut(camera)
            .get_mut::<OrbitCamera>()
            .unwrap()
            .rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        app.update();
        assert_eq!(
            app.world().resource::<Walks>().0,
            walked,
            "a turn walked the index again"
        );
        assert_eq!(
            app.world().resource::<Planned>().0.marks,
            marks,
            "the turn changed the plan it did not re-walk"
        );

        // The eye moves, which is the half that must still re-walk.
        app.world_mut()
            .entity_mut(camera)
            .get_mut::<OrbitCamera>()
            .unwrap()
            .stands_at(DVec3::new(32., 0., 0.));
        app.update();
        assert!(
            app.world().resource::<Walks>().0 > walked,
            "a move did not walk the index"
        );
    }
}
