use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::ui::{Gesture, PointerOverUi};
use bevy::camera::Hdr;
use bevy::input::mouse::{
    AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit,
};
use bevy::math::DVec3;
use bevy::picking::mesh_picking::MeshPickingCamera;
use bevy::post_process::bloom::Bloom;
use bevy::prelude::*;
use big_space::prelude::*;
use std::f32::consts::FRAC_PI_2;

pub fn plugin(app: &mut App) {
    app.add_message::<MoveCamera>();
    app.add_systems(Update, move_camera.in_set(MapSet::Camera));
    // Reads what `move_camera` and the spyglass asked for, and is the only
    // thing that writes the camera's cell and transform.
    app.add_systems(
        Update,
        orbit_camera.in_set(MapSet::Camera).after(move_camera),
    );
    // Answers the radius `orbit_camera` settled on, so it follows it.
    app.add_systems(
        Update,
        focus_lens.in_set(MapSet::Camera).after(orbit_camera),
    );
}

/// How far the camera may be pitched from the horizontal
///
/// Stopping just short of straight up keeps the up vector from flipping when
/// the camera passes over the point it is orbiting.
const PITCH_LIMIT: f32 = FRAC_PI_2 - 1e-3;

/// Radians of orbit per pixel of pointer travel, at unit sensitivity
const ORBIT_RATE: f32 = 5e-3;

/// Fraction of the orbit radius panned per pixel of pointer travel
///
/// Panning covers ground in proportion to how far out the camera is, so the
/// map moves under the pointer at about the same rate at every zoom.
const PAN_RATE: f32 = 2e-3;

/// E-folds of zoom per line of scroll, at unit sensitivity
///
/// Zoom is multiplicative because the map spans nine orders of magnitude. A
/// fixed step would cross the whole bubble near the surface of a star and
/// barely register out at the rim.
const ZOOM_RATE: f32 = 0.15;

/// Pixels of scroll that count as one line, for pointers that report them
const PIXELS_PER_LINE: f32 = 16.;

/// How near and how far the camera may be pulled from what it looks at, in
/// light years
///
/// The far end is past the width of the galaxy, so the whole map fits. The near
/// end is a metre, which is not a distance anything is drawn at so much as a
/// floor that never comes up: what stops the camera should be the thing it is
/// looking at, not a number.
///
/// The near end was `1e-6` while a system was drawn as one exaggerated sphere.
/// That is some thirty light seconds, which is a sensible place to stand to
/// look at a system whole and nowhere near close enough to look at anything in
/// it: an Earth seen from there is four hundredths of a degree across.
const MIN_RADIUS: f32 = (1. / crate::space::LIGHT_YEAR) as f32;
const MAX_RADIUS: f32 = 1e6;

/// How near the near plane sits, as a fraction of the orbit radius
///
/// Nothing can be drawn nearer to the camera than its near plane, and the map
/// spans seventeen orders of magnitude of zoom, so no fixed distance can serve
/// both ends of it. A fraction of how far back the camera is standing does:
/// whatever it is looking at is always well past the plane, and whatever is a
/// ten-thousandth of the way there is close enough to be behind the viewer.
///
/// This is why nothing inside a system could be drawn before. Bevy's default
/// near plane is `0.1`, and a world unit was a light year then, so everything
/// within a tenth of a light year of the camera was clipped away — which is
/// every star at its true size and every body without exception.
const NEAR_FRACTION: f32 = 1e-4;

/// How far the frustum reaches, in metres
///
/// Only culling depends on it. The projection is an infinite reversed one, so
/// the matrix never reads it, but the frustum built alongside is given it as a
/// far plane and quietly drops whatever lies beyond.
///
/// Past anything the map can put in front of the camera: the furthest the
/// camera may stand off what it looks at, plus the furthest the spyglass may
/// reach around it, and half again for room. Bevy's own default is `1000.`,
/// which is a thousand *metres*, and would leave the map showing nothing at
/// all.
const SIGHT: f32 = ((MAX_RADIUS + Spyglass::CEILING) as f64
    * 1.5
    * crate::space::LIGHT_YEAR) as f32;

/// How close to its target a value must be before it is pinned there
///
/// Relative to the target's magnitude, since a center ranges from a fraction
/// of a light year near Sol to a hundred thousand at the rim, and a radius
/// spans nine orders of magnitude.
const SNAP_TOLERANCE: f64 = 1e-9;

/// The shortest and longest a move may take, in seconds
///
/// [`travel_duration`] holds every move between the two.
const MIN_TRAVEL: f32 = 0.9;
const MAX_TRAVEL: f32 = 6.;

/// The steepest [`travelled`] gets, as a multiple of a move's length over
/// the square of its duration
///
/// A pure number, carrying no unit of its own: [`travelled`] maps a fraction
/// of the duration to a fraction of the distance, so neither of its axes is
/// measured in anything. The curve reaches this magnitude twice, once
/// speeding up and once slowing down.
///
/// A move of `d` light years over `t` seconds therefore changes speed by at
/// most `PEAK_ACCELERATION * d / t^2` light years per second squared, which
/// is the figure [`TRAVEL_BRAKING`] bounds.
const PEAK_ACCELERATION: f32 = 5.7735;

/// The fastest a move slows down, in light years per second squared
///
/// [`travel_duration`] gives each move the shortest duration whose steepest
/// moment stays within this, so any move long enough for it to bind reaches
/// exactly this figure and shorter ones stay under it. Raising it makes
/// moves quicker and sharper, lowering it makes them longer and gentler.
const TRAVEL_BRAKING: f32 = 10392.;

/// A move in progress
///
/// Set by [`move_camera`] and cleared on arrival. `from` and `to` are
/// absolute galactic positions in light years. `elapsed` counts up to
/// `duration`, and the ratio of the two drives [`travelled`].
pub struct Travel {
    from: DVec3,
    to: DVec3,
    elapsed: f32,
    duration: f32,
}

/// How long a move of `distance` light years takes, in seconds
///
/// Duration grows with the square root of the distance, so two hundred times
/// the distance takes about fourteen times as long and the rest is covered
/// by moving faster. Below about 1500 light years the result sits near
/// [`MIN_TRAVEL`], and it is capped at [`MAX_TRAVEL`].
fn travel_duration(distance: f64) -> f32 {
    let braking_takes = PEAK_ACCELERATION * distance as f32 / TRAVEL_BRAKING;
    // Squaring the floor before the root keeps the duration continuous, so
    // there is no distance at which it steps.
    (braking_takes + MIN_TRAVEL * MIN_TRAVEL).sqrt().min(MAX_TRAVEL)
}

/// The fraction of a move completed at `t`, itself a fraction of the duration
///
/// Zero at `t = 0` and one at `t = 1`, at rest at both ends, and symmetric
/// about the midpoint, so a move slows down exactly as it sped up. Speed
/// falls steadily across the whole second half.
fn travelled(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    // The integral of `travel_rate`, scaled to reach exactly one at `t = 1`.
    10. * t.powi(3) - 15. * t.powi(4) + 6. * t.powi(5)
}

/// The speed at `t`, as a multiple of the move's average speed
///
/// The derivative of [`travelled`]: zero at both ends, and 1.875 at the
/// midpoint. Used only by the tests, which check the shape of the curve.
#[cfg(test)]
fn travel_rate(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    30. * (t * (1. - t)).powi(2)
}

/// The fraction of the remaining distance to cover in `dt` seconds
///
/// `smoothness` is the fraction of the distance left after one second, and
/// the result is scaled by `dt`, so a given smoothness covers the same
/// ground per second at any frame rate. The seventh power puts the useful
/// range near zero: smaller converges quicker, and one never converges.
fn approach(smoothness: f32, dt: f32) -> f32 {
    1. - smoothness.clamp(0., 1.).powi(7).powf(dt)
}

/// Pin a value to its target once it is within [`SNAP_TOLERANCE`]
///
/// [`approach`] covers a fraction of what remains each frame, so it
/// converges on a target without reaching it. Pinning gives it an exact
/// end, which lets the star fetch settle: its regions are keyed on the
/// center, and a center that never stops moving keeps re-requesting them.
fn snap(value: f64, target: f64) -> f64 {
    if (target - value).abs() <= SNAP_TOLERANCE * target.abs().max(1.) {
        target
    } else {
        value
    }
}

/// Move `fraction` of the way from `value` to `target`, landing on it
fn eased(value: f32, target: f32, fraction: f32) -> f32 {
    snap(value.lerp(target, fraction) as f64, target as f64) as f32
}

/// As [`eased`], for a position that needs the precision of an `f64`
fn eased_position(value: DVec3, target: DVec3, fraction: f64) -> DVec3 {
    DVec3::new(
        snap(value.x.lerp(target.x, fraction), target.x),
        snap(value.y.lerp(target.y, fraction), target.y),
        snap(value.z.lerp(target.z, fraction), target.z),
    )
}

/// A message which triggers the movement of the camera
///
/// Send the camera to be centered on `position`, in absolute galactic light
/// years. Positions are `f64` because a `f32` cannot tell two points at the
/// galactic rim apart any closer than a few thousand light seconds, which is
/// most of the way across a star system.
#[derive(Message, Debug)]
pub struct MoveCamera {
    pub position: Option<DVec3>,
    /// How much to take in around it, in light years
    ///
    /// What is asked for is the thing to be seen, not how far back to stand
    /// to see it: how far back depends on the field of view and on the shape
    /// of the window, and the camera is what knows both.
    ///
    /// Nothing leaves the zoom where the user left it, which is what a move
    /// that only says where to look should do.
    pub framing: Option<f32>,
}

/// The half angle a camera sees across when nothing says otherwise
///
/// Half of Bevy's own default field of view, which is vertical.
const DEFAULT_HALF_FOV: f32 = std::f32::consts::PI / 8.;

/// How much room is left around something framed, as a fraction of its size
///
/// A route drawn corner to corner of the viewport reads as one that did not
/// quite fit.
///
/// Read by whatever sets the reach to match a framing, so that the map draws
/// out to the edge of what the camera was stood back to show. Something framed
/// exactly to its own size is a thing whose furthest points sit on the rim of
/// the reach, where an `f32` rounding decides whether they are drawn at all.
pub(crate) const FRAMING_MARGIN: f32 = 1.15;

/// The narrower of the two half angles the camera sees across
///
/// The narrower one is what clips. Bevy's field of view is the vertical one,
/// so a window wider than it is tall has room to spare at the sides and a
/// narrow one has none, and fitting whichever is tighter keeps the whole of
/// what is framed on screen either way.
fn half_angle(projection: Option<&Projection>) -> f32 {
    match projection {
        Some(Projection::Perspective(lens)) => {
            let vertical = lens.fov / 2.;
            let across = (vertical.tan() * lens.aspect_ratio).atan();
            vertical.min(across)
        }
        _ => DEFAULT_HALF_FOV,
    }
}

/// How far back to stand to take in `extent` light years about what is looked
/// at
pub(crate) fn stand_back(extent: f32, projection: Option<&Projection>) -> f32 {
    let half = half_angle(projection);
    (extent * FRAMING_MARGIN / half.sin()).clamp(MIN_RADIUS, MAX_RADIUS)
}

/// How far about what it looks at a camera `radius` back takes in, in light
/// years
///
/// Out to the edge of the view rather than to the edge of what was framed in
/// it, so this is [`stand_back`] undone without its margin. That margin is
/// room left around something being shown, and there is nothing being shown
/// here: the question is how much sky is on screen, and the answer runs to
/// where the screen ends.
///
/// So a camera stood back over an extent takes in `FRAMING_MARGIN` times it,
/// which is the reach [`crate::systems::route`] sets by hand when it frames a
/// route. A route the spyglass reached only to the ends of would have them
/// sitting on the rim, in or out by however an `f32` rounded.
///
/// What the spyglass reaches by when it follows the camera.
pub(crate) fn framed(radius: f32, projection: Option<&Projection>) -> f32 {
    let half = half_angle(projection);
    radius * half.sin()
}

/// A camera that orbits a point in the galaxy
///
/// Replaces `bevy_panorbit_camera`, which cannot be used here for two
/// reasons. Its center is a `Vec3`, so it cannot name a point out at the rim
/// any more precisely than a star system is wide. And it writes an absolute
/// translation every frame, which is exactly what a floating origin asks you
/// not to do: `big_space` would spend every frame recentering a camera that
/// had already put itself back.
///
/// The orbit is instead kept as a center, a radius and two angles, which is
/// what the controls actually manipulate. The cell and transform are
/// computed from those once per frame, so nothing is ever fought over.
#[derive(Component)]
pub struct OrbitCamera {
    /// Absolute galactic position the camera looks at, in light years
    pub center: DVec3,
    /// Where the center is heading, which it approaches smoothly
    pub target_center: DVec3,
    /// The move under way, if there is one
    ///
    /// While set, the center follows [`travelled`] between the move's two
    /// ends. Panning clears it.
    pub travel: Option<Travel>,
    /// Absolute galactic position of the camera itself, in light years
    ///
    /// Derived from the center and the orbit, and published here because
    /// distances to stars are wanted by half the map. Reading it avoids
    /// having to undo the cell split to ask where the camera is.
    pub eye: DVec3,
    /// Which way the camera faces, for anything that wants to line up with it
    pub rotation: Quat,
    pub radius: f32,
    pub target_radius: f32,
    pub yaw: f32,
    pub target_yaw: f32,
    pub pitch: f32,
    pub target_pitch: f32,
    /// Fraction of the distance to a target left after one second
    ///
    /// Zero arrives at once, one never converges. Each control carries its
    /// own so they can settle at different speeds.
    pub orbit_smoothness: f32,
    pub pan_smoothness: f32,
    pub zoom_smoothness: f32,
    pub orbit_sensitivity: f32,
    pub pan_sensitivity: f32,
    pub zoom_sensitivity: f32,
}

impl Default for OrbitCamera {
    fn default() -> Self {
        OrbitCamera {
            center: DVec3::ZERO,
            target_center: DVec3::ZERO,
            travel: None,
            eye: DVec3::ZERO,
            rotation: Quat::IDENTITY,
            radius: 1.,
            target_radius: 1.,
            yaw: 0.,
            target_yaw: 0.,
            pitch: 0.,
            target_pitch: 0.,
            orbit_smoothness: 0.1,
            pan_smoothness: 0.02,
            zoom_smoothness: 0.1,
            orbit_sensitivity: 1.,
            pan_sensitivity: 1.,
            zoom_sensitivity: 1.,
        }
    }
}

/// Everything the map's one camera is
///
/// Handed to [`crate::space`] to spawn, because a camera that is not a child
/// of the galaxy grid is not positioned by it.
pub fn camera(spyglass: &Spyglass) -> impl Bundle {
    (
        Camera3d::default(),
        Hdr,
        // Mesh picking requires markers, see `systems::spawn::plugin`.
        MeshPickingCamera,
        AmbientLight { color: Color::default(), brightness: 1e3, ..default() },
        // Every other entity is drawn relative to this one.
        FloatingOrigin,
        OrbitCamera {
            radius: spyglass.radius * 3.,
            target_radius: spyglass.radius * 3.,
            ..default()
        },
        Bloom::NATURAL,
    )
}

/// Starts a move on each [`MoveCamera`] message
///
/// Sets up a [`Travel`] from the camera's current center to the requested
/// position. A message arriving mid-move replaces it, starting a fresh
/// curve from wherever the camera has reached.
pub fn move_camera(
    mut query: Query<&mut OrbitCamera>,
    lens: Query<&Projection>,
    mut camera_events: MessageReader<MoveCamera>,
) {
    for event in camera_events.read() {
        let Ok(mut camera) = query.single_mut() else { continue };

        if let Some(position) = event.position {
            let from = camera.center;
            let distance = (position - from).length();
            let duration = travel_duration(distance);
            camera.target_center = position;
            camera.travel =
                Some(Travel { from, to: position, elapsed: 0., duration });
        }

        // The target rather than the radius itself, so pulling back happens
        // at the same rate a scroll does and the two cannot fight.
        //
        // Nothing comes of this while the camera is locked to the spyglass,
        // which writes the same field every frame from the spyglass's own
        // reach. That is what locking it means.
        if let Some(extent) = event.framing {
            camera.target_radius = stand_back(extent, lens.single().ok());
        }
    }
}

/// Drive the orbit from the pointer, and place the camera where it lands
///
/// The orbit is worked out in absolute light years and only split into a
/// cell and a remainder at the very end, so the arithmetic never has to know
/// about grids and the camera never lands between two cells.
pub fn orbit_camera(
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    over_ui: Res<PointerOverUi>,
    gesture: Gesture,
    time: Res<Time<Real>>,
    spyglass: Res<Spyglass>,
    grids: Query<&Grid, With<BigSpace>>,
    mut cameras: Query<(&mut OrbitCamera, &mut CellCoord, &mut Transform)>,
) {
    let Ok(grid) = grids.single() else { return };
    let Ok((mut orbit, mut cell, mut transform)) = cameras.single_mut() else {
        return;
    };

    // A drag that started on a slider is the user talking to the settings
    // window, not to the map behind it, and goes on being that wherever the
    // pointer is dragged to. So the whole drag answers to whose press began
    // it rather than to what the pointer is over from one frame to the next.
    if gesture.dragging_map() {
        if gesture.pressed(MouseButton::Left) {
            let rate = ORBIT_RATE * orbit.orbit_sensitivity;
            orbit.target_yaw -= motion.delta.x * rate;
            orbit.target_pitch = (orbit.target_pitch - motion.delta.y * rate)
                .clamp(-PITCH_LIMIT, PITCH_LIMIT);
        }

        if gesture.pressed(MouseButton::Right) {
            let rate = PAN_RATE * orbit.pan_sensitivity * orbit.radius;
            let across = orbit.rotation * Vec3::X * -motion.delta.x * rate;
            let up = orbit.rotation * Vec3::Y * motion.delta.y * rate;
            // Dragging cancels a move in progress and takes the target from
            // wherever it had reached, so the pointer has the center alone.
            if orbit.travel.take().is_some() {
                orbit.target_center = orbit.center;
            }
            orbit.target_center += (across + up).as_dvec3();
        }
    }

    // A scroll belongs to no press, so there is no owner to ask and what the
    // pointer is over now is the whole of the question. Which is what
    // [`PointerOverUi`] answers, a frame late and no worse for it: a wheel
    // turned over a pane that was not there last frame is a wheel turned at
    // something the user has only just opened.
    let lines = match scroll.unit {
        MouseScrollUnit::Line => scroll.delta.y,
        MouseScrollUnit::Pixel => scroll.delta.y / PIXELS_PER_LINE,
    };
    // Held to the spyglass, the camera has nowhere of its own to stand and a
    // scroll has nothing to say about where it goes. Taking one anyway moves
    // it for a frame, until `zoom_with_spyglass` writes the reach back over
    // it, which reads as a zoom that keeps snapping back. The reach is what to
    // move, and the radius on the settings pane is where it is moved.
    if lines != 0. && !over_ui.0 && !spyglass.locks_camera() {
        let zoom = -lines * ZOOM_RATE * orbit.zoom_sensitivity;
        orbit.target_radius =
            (orbit.target_radius * zoom.exp()).clamp(MIN_RADIUS, MAX_RADIUS);
    }

    // Approach whatever was asked for, rather than jumping to it. A search
    // can send the center clear across the galaxy, and arriving instantly
    // leaves no sense of where the new system is in relation to the old.
    let dt = time.delta_secs();
    let center = match orbit.travel.as_mut() {
        // A commanded move knows both ends of its journey from the start, so
        // it can be eased away from one and into the other.
        Some(travel) => {
            travel.elapsed += dt;
            let progress = if travel.duration > 0. {
                travel.elapsed / travel.duration
            } else {
                1.
            };
            let arrived = progress >= 1.;
            let center = if arrived {
                travel.to
            } else {
                travel.from.lerp(travel.to, travelled(progress) as f64)
            };
            if arrived {
                orbit.travel = None;
            }
            center
        }
        // A drag moves the target a little at a time, and is followed.
        None => eased_position(
            orbit.center,
            orbit.target_center,
            approach(orbit.pan_smoothness, dt) as f64,
        ),
    };

    let radius = eased(
        orbit.radius,
        orbit.target_radius,
        approach(orbit.zoom_smoothness, dt),
    );

    let turn = approach(orbit.orbit_smoothness, dt);
    let yaw = eased(orbit.yaw, orbit.target_yaw, turn);
    let pitch = eased(orbit.pitch, orbit.target_pitch, turn);

    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.);
    let eye = center + (rotation * Vec3::Z * radius).as_dvec3();

    orbit.center = center;
    orbit.radius = radius;
    orbit.yaw = yaw;
    orbit.pitch = pitch;
    orbit.rotation = rotation;
    orbit.eye = eye;

    // The whole orbit above is worked out in light years, which is what the
    // map measures in and what everything asking the camera where it is
    // expects. Only here does it become the metres the grid is laid out in.
    let (eye_cell, eye_translation) =
        grid.translation_to_grid(crate::space::metres(eye));
    cell.set_if_neq(eye_cell);
    transform.translation = eye_translation;
    transform.rotation = rotation;
}

/// Hold the near plane a fixed fraction of the way to what is being looked at
///
/// A camera sees between its near plane and its far one, and this map asks to
/// be looked at from a hundred thousand light years and from inside a planet
/// in the same session. No pair of fixed distances covers both, so the near
/// plane follows the zoom and the far one is simply set past everything.
///
/// Runs after [`orbit_camera`], which settles the radius this is worked out
/// from. Written only where it differs: a projection assigned every frame is a
/// frustum recomputed every frame.
///
/// [`PerspectiveProjection::near_clip_plane`] is deliberately left alone. Its
/// default reads as a second near plane stuck at `0.1`, but the matrix only
/// consults it when its normal is something other than straight back from the
/// camera, and by default it is not. Giving it one would turn on the oblique
/// clipping meant for portals and mirrors.
pub fn focus_lens(mut cameras: Query<(&OrbitCamera, &mut Projection)>) {
    let Ok((orbit, mut projection)) = cameras.single_mut() else { return };
    // The radius is a distance the map talks in and the planes are distances
    // it draws in, so this is one of the two places a light year is spoken to
    // metres. The other is where the camera's own cell is worked out.
    let near =
        (orbit.radius as f64 * crate::space::LIGHT_YEAR) as f32 * NEAR_FRACTION;

    // Asked before it is reached for, since reaching for it is what says it
    // has changed. Only the two planes are touched: bevy writes the aspect
    // ratio onto this same projection as the window is resized, and a fresh
    // one would put it back to a square.
    let Projection::Perspective(lens) = &*projection else { return };
    if lens.near == near && lens.far == SIGHT {
        return;
    }

    let Projection::Perspective(lens) = projection.as_mut() else { return };
    lens.near = near;
    lens.far = SIGHT;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::pointing::PRIMARY;
    use crate::ui::Grasp;
    use bevy::input::mouse::AccumulatedMouseScroll;

    /// A world holding a grid, a camera `back` light years out, and one
    /// scroll of the wheel waiting to be read
    ///
    /// Everything [`orbit_camera`] reads and nothing else. The pointer is out
    /// over the map rather than over the settings, since a scroll that lands
    /// on the pane is already ignored for a reason of its own.
    fn scrolled(back: f32, spyglass: Spyglass) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.insert_resource(spyglass);
        app.insert_resource(PointerOverUi(false));
        app.init_resource::<Grasp>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<AccumulatedMouseMotion>();
        app.insert_resource(AccumulatedMouseScroll {
            unit: MouseScrollUnit::Line,
            delta: Vec2::new(0., -1.),
        });
        app.world_mut().spawn((BigSpace::default(), Grid::new(1., 0.1)));
        app.world_mut().spawn((
            OrbitCamera { radius: back, target_radius: back, ..default() },
            CellCoord::default(),
            Transform::default(),
        ));
        app.add_systems(Update, orbit_camera);
        app
    }

    /// How far back the camera is asked to stand
    fn asked(app: &mut App) -> f32 {
        app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_radius
    }

    /// A spyglass reaching ten light years, set however the test wants
    fn spyglass(lock_camera: bool, follow_camera: bool) -> Spyglass {
        Spyglass {
            radius: Spyglass::OPENING,
            fetch: false,
            clear: true,
            lock_camera,
            follow_camera,
        }
    }

    /// A world where the left button is down and being dragged across the map
    ///
    /// `owner` is whose the press was, as the UI settled it at the end of the
    /// frame the button went down in.
    fn dragging(owner: bool) -> App {
        let mut app = scrolled(100., spyglass(false, false));
        app.insert_resource(AccumulatedMouseScroll::default());
        app.insert_resource(AccumulatedMouseMotion {
            delta: Vec2::new(20., 0.),
        });

        let world = app.world_mut();
        world.resource_mut::<ButtonInput<MouseButton>>().press(PRIMARY);
        let buttons = world.resource::<ButtonInput<MouseButton>>().clone();
        world.resource_mut::<Grasp>().settle(&buttons, owner);
        app
    }

    /// Which way the camera is turned to
    fn facing(app: &mut App) -> f32 {
        app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_yaw
    }

    /// A drag the map owns turns it
    #[test]
    fn dragging_the_map_orbits_it() {
        let mut app = dragging(false);

        app.update();

        assert!(facing(&mut app) != 0., "the map did not turn");
    }

    /// A drag that began on a control does not
    ///
    /// The whole drag belongs to whatever the press landed on, so a pointer
    /// pulled off a slider and across the sky goes on talking to the slider.
    #[test]
    fn dragging_off_a_control_does_not_orbit_the_map() {
        let mut app = dragging(true);

        app.update();

        assert_eq!(facing(&mut app), 0.);
    }

    /// Nor does one nobody has settled yet
    ///
    /// The first frame of a drag, the UI not having spoken. A frame of a map
    /// that has not started turning, against a frame of one that turns under
    /// a press meant for a slider.
    #[test]
    fn an_unsettled_drag_does_not_orbit_the_map() {
        let mut app = scrolled(100., spyglass(false, false));
        app.insert_resource(AccumulatedMouseScroll::default());
        app.insert_resource(AccumulatedMouseMotion {
            delta: Vec2::new(20., 0.),
        });
        app.world_mut()
            .resource_mut::<ButtonInput<MouseButton>>()
            .press(PRIMARY);

        app.update();

        assert_eq!(facing(&mut app), 0.);
    }

    /// Scrolling pulls the camera back
    #[test]
    fn the_wheel_zooms_the_camera() {
        let mut app = scrolled(100., spyglass(false, false));

        app.update();

        assert!(asked(&mut app) > 100., "stayed at {}", asked(&mut app));
    }

    /// Locked to the spyglass, the wheel does nothing at all
    ///
    /// Not even for the one frame it would take `zoom_with_spyglass` to write
    /// the reach back over it. A camera that lurches and returns on every
    /// notch of the wheel is worse than one that holds still, and holding
    /// still is what being locked to the reach means.
    #[test]
    fn a_locked_camera_does_not_zoom() {
        let mut app = scrolled(100., spyglass(true, false));

        app.update();

        assert_eq!(asked(&mut app), 100.);
    }

    /// Locked while the camera is what sets the reach, the wheel works
    ///
    /// Nothing writes the camera's distance in that case, so there is nothing
    /// for a zoom to be undone by, and a wheel that had stopped working would
    /// be a setting doing something it says it is not.
    #[test]
    fn the_wheel_zooms_a_camera_that_sets_the_reach() {
        let mut app = scrolled(100., spyglass(true, true));

        app.update();

        assert!(asked(&mut app) > 100., "stayed at {}", asked(&mut app));
    }

    /// Where [`approach`] lands after `steps` frames of `dt` seconds each
    fn travel(smoothness: f32, dt: f32, steps: usize) -> f64 {
        let target = 1.;
        let mut value = 0.;
        for _ in 0..steps {
            value = value.lerp(target, approach(smoothness, dt) as f64);
        }
        value
    }

    /// [`approach`] covers the same ground per second at any frame rate
    ///
    /// One second of approach reaches the same point whether it is taken in
    /// sixty steps or six.
    #[test]
    fn approach_is_independent_of_frame_rate() {
        let smoothness = 0.1;
        let at_60fps = travel(smoothness, 1. / 60., 60);
        let at_6fps = travel(smoothness, 1. / 6., 6);
        assert!(
            (at_60fps - at_6fps).abs() < 1e-6,
            "one second reached {at_60fps} at 60fps and {at_6fps} at 6fps"
        );
    }

    /// [`travelled`] starts and ends at rest
    ///
    /// Both ends advance by a small fraction of what the midpoint advances
    /// by over the same interval.
    #[test]
    fn travelled_starts_and_ends_at_rest() {
        let covered = |t: f32| travelled(t + 0.01) - travelled(t);
        let midpoint = covered(0.5);
        assert!(
            covered(0.) < midpoint / 4.,
            "covered {} at the start against {midpoint} at the midpoint",
            covered(0.)
        );
        assert!(
            covered(0.99) < midpoint / 4.,
            "covered {} at the end against {midpoint} at the midpoint",
            covered(0.99)
        );
    }

    /// [`travelled`] spans the whole move
    #[test]
    fn travelled_spans_zero_to_one() {
        assert_eq!(travelled(0.), 0.);
        assert!(
            (travelled(1.) - 1.).abs() < 1e-5,
            "ended at {}",
            travelled(1.)
        );
    }

    /// [`travel_rate`] is symmetric about the midpoint
    ///
    /// A move slows down exactly as it sped up.
    #[test]
    fn travel_rate_is_symmetric() {
        for step in 0..=100 {
            let t = step as f32 / 100.;
            let out = travel_rate(t);
            let back = travel_rate(1. - t);
            assert!(
                (out - back).abs() < 1e-5,
                "rate was {out} at {t} and {back} at {}",
                1. - t
            );
        }
    }

    /// [`travel_rate`] decreases throughout the second half
    ///
    /// Speed falls at every step of the approach, so deceleration is spread
    /// across all of it.
    #[test]
    fn travel_rate_falls_after_the_midpoint() {
        let mut previous = travel_rate(0.5);
        for step in 51..=100 {
            let t = step as f32 / 100.;
            let rate = travel_rate(t);
            assert!(rate < previous, "rate rose to {rate} at {t}");
            previous = rate;
        }
    }

    /// Peak speed grows with distance
    ///
    /// Duration grows only with the square root of the distance, so most of
    /// a longer move is covered by moving faster.
    #[test]
    fn peak_speed_grows_with_distance() {
        let peak = |distance: f64| {
            distance as f32 * travel_rate(0.5) / travel_duration(distance)
        };
        let short = peak(100.);
        let long = peak(22_000.);
        assert!(
            long > short * 15.,
            "22000ly peaked at {long} against {short} for 100ly, only {}x",
            long / short
        );
    }

    /// [`PEAK_ACCELERATION`] is the steepest [`travelled`] actually gets
    ///
    /// The two are only related by this constant, so a change to the curve
    /// that left it behind would put every duration out.
    #[test]
    fn peak_acceleration_matches_the_curve() {
        let step = 1e-3;
        let mut steepest: f32 = 0.;
        for point in 0..=1000 {
            let t = point as f32 / 1000.;
            let change = (travel_rate(t + step) - travel_rate(t)).abs() / step;
            steepest = steepest.max(change);
        }
        assert!(
            (steepest - PEAK_ACCELERATION).abs() < 0.02,
            "the curve peaks at {steepest}, not {PEAK_ACCELERATION}"
        );
    }

    /// No move exceeds [`TRAVEL_BRAKING`]
    ///
    /// Distances past the [`MAX_TRAVEL`] ceiling are left out, since there
    /// the ceiling sets the duration and the limit does not apply.
    #[test]
    fn braking_stays_within_the_limit() {
        for distance in [1., 100., 1_000., 22_000., 60_000.] {
            let duration = travel_duration(distance);
            let braking =
                PEAK_ACCELERATION * distance as f32 / (duration * duration);
            assert!(
                braking <= TRAVEL_BRAKING,
                "{distance}ly braked at {braking}, above the {TRAVEL_BRAKING} limit"
            );
        }
    }

    /// A move past the [`MIN_TRAVEL`] floor is bounded by braking
    ///
    /// Its duration comes within a tenth of the limit, so braking is what
    /// decides how long it takes.
    #[test]
    fn long_moves_brake_near_the_limit() {
        let duration = travel_duration(22_000.);
        let braking = PEAK_ACCELERATION * 22_000. / (duration * duration);
        assert!(
            braking > TRAVEL_BRAKING * 0.9,
            "22000ly braked at {braking}, well under the {TRAVEL_BRAKING} limit"
        );
    }

    /// A move under the [`MIN_TRAVEL`] floor brakes well inside the limit
    ///
    /// The floor sets its duration, leaving it gentler than braking requires.
    #[test]
    fn short_moves_brake_well_below_the_limit() {
        let duration = travel_duration(20.);
        let braking = PEAK_ACCELERATION * 20. / (duration * duration);
        assert!(
            braking < TRAVEL_BRAKING / 10.,
            "20ly braked at {braking}, close to the {TRAVEL_BRAKING} limit"
        );
    }

    /// [`snap`] pins a value that is within tolerance of its target
    #[test]
    fn snap_pins_a_value_within_tolerance() {
        let target = 1234.5678;
        assert_eq!(snap(target - 1e-9, target), target);
    }

    /// [`snap`] leaves a value with real distance still to cover
    #[test]
    fn snap_leaves_a_value_outside_tolerance() {
        let target = 1234.5678;
        assert_eq!(snap(target - 1., target), target - 1.);
    }

    /// [`SNAP_TOLERANCE`] scales with the size of the target
    ///
    /// The same absolute gap counts as arrival at a target of a hundred
    /// thousand and as real distance at a target of one.
    #[test]
    fn snap_tolerance_scales_with_the_target() {
        assert_eq!(snap(1e5 - 1e-5, 1e5), 1e5);
        assert_ne!(snap(1. - 1e-5, 1.), 1.);
    }

    /// A camera `wide` by `high`, seeing across Bevy's own default angle
    fn lens(wide: f32, high: f32) -> Projection {
        Projection::Perspective(PerspectiveProjection {
            aspect_ratio: wide / high,
            ..default()
        })
    }

    /// Whether `extent` about the middle lands inside what is seen
    ///
    /// The half angle a distance of `back` subtends at the camera, against
    /// the half angle the camera sees across. Worked out from the geometry
    /// rather than from the same expression under test.
    fn fits(extent: f32, back: f32, half_angle: f32) -> bool {
        (extent / back).asin() <= half_angle
    }

    /// Standing back holds what it was asked to hold
    ///
    /// A wide window, where the vertical angle is the tighter of the two and
    /// so the one that decides it.
    #[test]
    fn standing_back_holds_a_wide_window() {
        let back = stand_back(50., Some(&lens(1280., 720.)));

        assert!(fits(50., back, DEFAULT_HALF_FOV));
    }

    /// And holds it in a window taller than it is wide
    ///
    /// There the sides are the tighter, and fitting only the vertical would
    /// cut the ends off whatever was framed.
    #[test]
    fn standing_back_holds_a_tall_window() {
        let tall = lens(400., 1200.);
        let back = stand_back(50., Some(&tall));

        let Projection::Perspective(ref lens) = tall else { unreachable!() };
        let across = ((lens.fov / 2.).tan() * lens.aspect_ratio).atan();
        assert!(across < lens.fov / 2., "the sides are meant to be tighter");
        assert!(fits(50., back, across));
    }

    /// A tall window is stood back from further than a wide one
    #[test]
    fn a_tall_window_asks_for_more_room() {
        let wide = stand_back(50., Some(&lens(1280., 720.)));
        let tall = stand_back(50., Some(&lens(400., 1200.)));

        assert!(tall > wide);
    }

    /// With no camera to ask, the default angle answers
    #[test]
    fn standing_back_without_a_camera_holds_it_too() {
        let back = stand_back(50., None);

        assert!(fits(50., back, DEFAULT_HALF_FOV));
    }

    /// Twice as much to hold is twice as far to stand
    #[test]
    fn standing_back_follows_what_is_held() {
        let near = stand_back(10., None);
        let far = stand_back(20., None);

        assert!((far - near * 2.).abs() < 1e-3);
    }

    /// Nothing to hold is still somewhere the camera may stand
    #[test]
    fn holding_nothing_is_still_a_distance() {
        assert!(stand_back(0., None) >= MIN_RADIUS);
    }

    /// What a camera framing an extent takes in is that extent and the room
    /// left around it
    ///
    /// The spyglass reads the two in opposite directions, one to stand the
    /// camera back over a route and the other to take its reach from where
    /// the camera is standing. Standing back leaves [`FRAMING_MARGIN`] of
    /// room and the reach runs to the edge of the view, so the reach comes
    /// out at exactly the one `route::plotted` sets by hand, whatever the
    /// window is shaped like.
    #[test]
    fn what_is_framed_is_held_with_room_around_it() {
        for shape in [None, Some(lens(1280., 720.)), Some(lens(400., 1200.))] {
            for extent in [5., 50., 1e3, 1e4] {
                let back = stand_back(extent, shape.as_ref());
                let out = framed(back, shape.as_ref());
                let want = extent * FRAMING_MARGIN;

                assert!(
                    (out - want).abs() < want * 1e-4,
                    "framed {extent} from {back} back and took in {out}, \
                     wanted {want}"
                );
            }
        }
    }

    /// What is framed is inside what is taken in, with room to spare
    ///
    /// The reach is what decides whether the ends of a route are drawn, so
    /// the ends have to fall inside it rather than on it.
    #[test]
    fn what_is_framed_is_inside_what_is_taken_in() {
        let extent = 50.;
        let back = stand_back(extent, None);

        assert!(framed(back, None) > extent);
    }

    /// A tall window takes in less from the same place
    ///
    /// The reciprocal of [`a_tall_window_asks_for_more_room`]: the sides are
    /// what clips there, so from a distance that holds an extent in a wide
    /// window, a tall one holds less than that.
    #[test]
    fn a_tall_window_takes_in_less() {
        let wide = framed(1e3, Some(&lens(1280., 720.)));
        let tall = framed(1e3, Some(&lens(400., 1200.)));

        assert!(tall < wide, "took in {tall} against {wide}");
    }

    /// A world holding one camera standing `radius` back, and nothing else
    fn looking(radius: f32) -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.world_mut().spawn((
            OrbitCamera { radius, ..default() },
            Projection::Perspective(PerspectiveProjection::default()),
        ));
        app.add_systems(Update, focus_lens);
        app.update();
        app
    }

    /// The two planes the camera came out seeing between
    fn planes(app: &mut App) -> (f32, f32) {
        let mut lenses = app.world_mut().query::<&Projection>();
        let Projection::Perspective(lens) = lenses.single(app.world()).unwrap()
        else {
            panic!("the camera was given a perspective projection")
        };
        (lens.near, lens.far)
    }

    /// The near plane follows the zoom rather than standing still
    ///
    /// The whole point of it. A plane fixed anywhere is in the wrong place at
    /// one end or the other of seventeen orders of magnitude.
    #[test]
    fn the_near_plane_follows_the_zoom() {
        let (out, _) = planes(&mut looking(1e4));
        let (in_close, _) = planes(&mut looking(1e-8));

        assert!(
            out > in_close * 1e10,
            "the plane sat at {out} out and {in_close} in close"
        );
    }

    /// The near plane stays well short of what the camera is looking at
    ///
    /// Otherwise the thing being looked at is the thing being clipped, which
    /// is the failure this whole system exists to answer.
    #[test]
    fn the_near_plane_never_reaches_what_is_looked_at() {
        for radius in [MIN_RADIUS, 1e-8, 1., 1e4, MAX_RADIUS] {
            let (near, _) = planes(&mut looking(radius));
            // The radius is set in light years and the plane comes back in
            // the metres the map draws in.
            let back = (radius as f64 * crate::space::LIGHT_YEAR) as f32;
            assert!(
                near < back / 100.,
                "standing {back}m back, the plane sat at {near}"
            );
        }
    }

    /// Zoomed in as close as the map allows, a body is still in front of the
    /// plane rather than behind it
    ///
    /// Nothing about the near plane may put an Earth out of reach, and bevy's
    /// default of `0.1` put every one of them out of reach: a world unit was a
    /// light year then, so the plane stood a tenth of one from the camera and
    /// clipped away everything a system is made of.
    #[test]
    fn zooming_in_close_still_leaves_something_in_front_of_the_camera() {
        /// Metres, which is what the map is drawn in
        const EARTH_RADIUS: f32 = 6.371e6;

        let (near, _) = planes(&mut looking(MIN_RADIUS));
        assert!(near > 0., "the plane collapsed onto the camera");
        assert!(near.is_normal(), "the plane sat at {near}, a subnormal");
        assert!(
            near < EARTH_RADIUS,
            "the plane sat at {near}m, past a body {EARTH_RADIUS}m across"
        );
    }

    /// The far plane reaches past anything the map can draw
    ///
    /// It is a culling distance rather than a depth range, and the whole of
    /// the galaxy has to fall inside it from wherever the camera is standing.
    #[test]
    fn the_sight_reaches_past_everything_drawn() {
        let (_, far) = planes(&mut looking(MAX_RADIUS));

        assert!(
            far > MAX_RADIUS + Spyglass::CEILING,
            "sight reached {far}, short of the far side of the galaxy"
        );
    }

    /// How many frames have seen the projection written
    ///
    /// Counted from inside a system, since that is the only place change
    /// detection means anything: a query made by hand from outside one has no
    /// run of its own to measure the change against.
    #[derive(Resource, Default)]
    struct Wrote(usize);

    fn count_writes(
        mut wrote: ResMut<Wrote>,
        lenses: Query<(), Changed<Projection>>,
    ) {
        wrote.0 += lenses.iter().count();
    }

    /// A frame that moves nothing leaves the projection alone
    ///
    /// Assigning it regardless would have the frustum worked out afresh every
    /// frame for a camera standing perfectly still.
    #[test]
    fn a_resting_frame_leaves_the_lens_alone() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Wrote>();
        app.world_mut().spawn((
            OrbitCamera { radius: 1., ..default() },
            Projection::Perspective(PerspectiveProjection::default()),
        ));
        app.add_systems(Update, (focus_lens, count_writes).chain());

        // The camera arriving is itself a change, so the first frame is
        // counted whatever this system does. It is the second that says
        // whether a resting frame writes.
        app.update();
        let settled = app.world().resource::<Wrote>().0;

        app.update();
        assert_eq!(
            app.world().resource::<Wrote>().0,
            settled,
            "wrote a projection that had not moved"
        );
    }
}
