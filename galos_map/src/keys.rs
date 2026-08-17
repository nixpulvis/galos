//! What the keyboard asks of the map
//!
//! Every binding is a key struck on its own, but for the one that opens the
//! search. The map is read with one hand on the pointer, so the other hand is
//! what these are for: it rests on the letters and never has to reach for a
//! modifier to move the camera or to take an annotation off the sky.
//!
//! Which is why a chord is not a binding here. A key held with control,
//! command or alt is on its way somewhere else. Shift is weighed apart from
//! those three, exactly one binding wanting it.
//!
//! Nothing answers while a field is being typed into, but for the escape that
//! puts the field away. A system named SOL is spelled with the same S that pans
//! the map back, and [`crate::ui::Keyboard`] is what tells the two apart. The
//! one binding on a space asks that resource the wider of its two questions,
//! egui holding a space to be a click on whatever has the focus.
//!
//! Nothing here ends the session. The map is quit by closing its window, which
//! every platform already has its own gesture for, and a key that quit would
//! sit one keystroke from the ones that pan and search with nothing to undo it.

use crate::camera::{
    MAX_RADIUS, MIN_RADIUS, MoveCamera, OrbitCamera, PITCH_LIMIT, move_camera,
    orbit_camera,
};
use crate::grid::ShowGrid;
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::bodies::spawn::ShowOrbits;
use crate::systems::labels::ShowBodyNames;
use crate::systems::selection::Selection;
use crate::systems::spawn::ShowNames;
use crate::ui::{BarFields, Keyboard};
use bevy::math::DVec3;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    // Flying is asked for here rather than with the rest of the camera, since
    // what it asks for is a [`MoveCamera`], which `Camera` is where the map
    // answers.
    app.add_systems(
        Update,
        (toggle, open_search, shut_search, fly).in_set(MapSet::Search),
    );
    // Between the two systems that already write the camera. After the one
    // that starts a commanded move, since a key cancels one, and before the
    // one that follows the center, so a key lands in the frame it was pressed
    // in rather than the one after it.
    app.add_systems(
        Update,
        (pan, swing, zoom)
            .in_set(MapSet::Camera)
            .after(move_camera)
            .before(orbit_camera),
    );
}

/// Fraction of the orbit radius panned per second a key is held
///
/// Panning covers ground in proportion to how far out the camera is, as a drag
/// does, so a key moves the map at about the same rate at every zoom. Half a
/// radius a second crosses about two thirds of the view.
const PAN_PER_SECOND: f32 = 0.5;

/// Radians of orbit per second a key is held
///
/// A whole turn about what the camera is looking at in a little over four
/// seconds. Long enough to stop where it was meant to and short enough to get
/// round the back of something without letting go of the key.
const ORBIT_PER_SECOND: f32 = 1.5;

/// E-folds of zoom per second a key is held
///
/// Zoom is multiplicative for the reason the wheel's is: the map spans nine
/// orders of magnitude, and a fixed step would cross the whole bubble near the
/// surface of a star and barely register out at the rim. This covers a decade
/// and a third of it a second, so the whole range is a few seconds away and a
/// single tap is still worth a few percent.
const ZOOM_PER_SECOND: f32 = 3.;

/// How near the camera has to be pointed for something to count as centered,
/// as a fraction of the orbit radius
///
/// A fraction rather than a distance, since what being looked at means is a
/// matter of the view: a light year off is the far side of the map from inside
/// a system and is dead center from out among the stars.
const CENTERED: f64 = 1e-2;

/// The keys that turn a press into a chord rather than a binding
///
/// Shift is not among them. It has one binding of its own here, so it is asked
/// about separately by [`bare`] and [`shifted`].
const CHORDING: [KeyCode; 6] = [
    KeyCode::ControlLeft,
    KeyCode::ControlRight,
    KeyCode::SuperLeft,
    KeyCode::SuperRight,
    KeyCode::AltLeft,
    KeyCode::AltRight,
];

const SHIFT: [KeyCode; 2] = [KeyCode::ShiftLeft, KeyCode::ShiftRight];

/// Whether a press is on its way somewhere other than the map
fn chorded(keys: &ButtonInput<KeyCode>) -> bool {
    keys.any_pressed(CHORDING)
}

/// Whether a key is being pressed on its own
fn bare(keys: &ButtonInput<KeyCode>) -> bool {
    !chorded(keys) && !keys.any_pressed(SHIFT)
}

/// Whether a key is being pressed with shift and nothing else
fn shifted(keys: &ButtonInput<KeyCode>) -> bool {
    !chorded(keys) && keys.any_pressed(SHIFT)
}

/// Pan the map along the ruled plane, and up and down through it
///
/// `WASD` runs in the plane rather than across the screen. Forward is the way
/// the camera faces laid flat onto the plane, so holding W crosses the galaxy
/// the way the map is ruled however far the camera is pitched over, and the
/// altitude the user settled on is theirs to keep. `QE` is the one direction
/// the plane cannot carry, straight up through it and straight down.
///
/// Held rather than pressed, since panning is something the user does for as
/// long as they mean to be moving.
fn pan(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    time: Res<Time<Real>>,
    mut cameras: Query<&mut OrbitCamera>,
) {
    if keyboard.typing || !bare(&keys) {
        return;
    }
    let Ok(mut orbit) = cameras.single_mut() else { return };

    // The camera's own right, which lies in the plane whatever the pitch. The
    // orbit is a yaw about the plane's normal and then a pitch about the line
    // that yaw left, so the pitch turns the camera over this line rather than
    // taking it off the plane, and there is no roll to tip it out.
    let across = orbit.rotation * Vec3::X;
    // And the way the camera faces, laid flat: the perpendicular of that line
    // in the plane, `Vec3::Y` being the plane's normal.
    let along = Vec3::Y.cross(across);

    let mut asked = Vec3::ZERO;
    for (key, way) in [
        (KeyCode::KeyD, across),
        (KeyCode::KeyA, -across),
        (KeyCode::KeyW, along),
        (KeyCode::KeyS, -along),
        (KeyCode::KeyE, Vec3::Y),
        (KeyCode::KeyQ, Vec3::NEG_Y),
    ] {
        if keys.pressed(key) {
            asked += way;
        }
    }
    // Normalised, so two keys held at once cover the same ground per second as
    // one. Nothing asked for, or two keys cancelling, leaves nothing to point.
    let Ok(way) = Dir3::new(asked) else { return };

    // A key cancels a move in progress and takes the target from wherever it
    // had reached, the same as a drag does. The user is steering now.
    if orbit.travel.take().is_some() {
        orbit.target_center = orbit.center;
    }
    let rate = PAN_PER_SECOND * orbit.pan_sensitivity * orbit.radius;
    orbit.target_center += (*way * rate * time.delta_secs()).as_dvec3();
}

/// Swing the camera round what it is looking at
///
/// `Z` and `X` carry it round, `C` and `V` lower and raise it. The two angles
/// a left drag works, so a key and the pointer reach one control rather than
/// two, and what they write is the target rather than the angle itself, so
/// neither can fight the other or the easing between them.
///
/// It stops short of straight overhead and straight under. Passing either
/// would flip the up vector as the camera crossed the point it is orbiting,
/// and the whole map would turn over.
fn swing(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    time: Res<Time<Real>>,
    mut cameras: Query<&mut OrbitCamera>,
) {
    if keyboard.typing || !bare(&keys) {
        return;
    }
    let Ok(mut orbit) = cameras.single_mut() else { return };

    let mut round = 0.;
    let mut over = 0.;
    if keys.pressed(KeyCode::KeyZ) {
        round += 1.;
    }
    if keys.pressed(KeyCode::KeyX) {
        round -= 1.;
    }
    // The camera stands over what it is looking at at a negative pitch, the
    // map being read from above, so the key that raises it is the one that
    // takes the pitch down.
    if keys.pressed(KeyCode::KeyC) {
        over += 1.;
    }
    if keys.pressed(KeyCode::KeyV) {
        over -= 1.;
    }
    if round == 0. && over == 0. {
        return;
    }

    let rate = ORBIT_PER_SECOND * orbit.orbit_sensitivity * time.delta_secs();
    orbit.target_yaw += round * rate;
    orbit.target_pitch =
        (orbit.target_pitch + over * rate).clamp(-PITCH_LIMIT, PITCH_LIMIT);
}

/// Pull the camera in and back out again
///
/// `F` in and `R` back. Multiplicative, as the wheel is, so a key covers the
/// same share of the range wherever it is pressed rather than crossing a
/// system in a frame down close and doing nothing at all out at the rim.
///
/// Held to the spyglass the camera has nowhere of its own to stand, and this
/// stands down for the reason the wheel does: the reach writes the distance
/// back on the next frame, so a zoom taken anyway is a camera that lurches and
/// returns. The reach is what to move there, and the settings pane is where.
fn zoom(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    spyglass: Res<Spyglass>,
    time: Res<Time<Real>>,
    mut cameras: Query<&mut OrbitCamera>,
) {
    if keyboard.typing || !bare(&keys) || spyglass.locks_camera() {
        return;
    }
    let Ok(mut orbit) = cameras.single_mut() else { return };

    let mut asked = 0.;
    if keys.pressed(KeyCode::KeyF) {
        asked -= 1.;
    }
    if keys.pressed(KeyCode::KeyR) {
        asked += 1.;
    }
    if asked == 0. {
        return;
    }

    // The target rather than the radius itself, so that a key and the wheel
    // move the same thing at the same rate and the two cannot fight.
    let folds =
        asked * ZOOM_PER_SECOND * orbit.zoom_sensitivity * time.delta_secs();
    orbit.target_radius =
        (orbit.target_radius * folds.exp()).clamp(MIN_RADIUS, MAX_RADIUS);
}

/// Which of the things picked out the camera is already looking at
///
/// The nearest one within [`CENTERED`] of where the camera is pointed, and
/// nothing where none of them is that near. Nearest rather than first, so that
/// a pair standing close together, seen from far enough out that both are
/// within tolerance, still counts from the one actually being looked at.
fn looked_at(selection: &Selection, at: DVec3, radius: f32) -> Option<usize> {
    let near = radius as f64 * CENTERED;
    (0..selection.len())
        .filter_map(|index| Some((index, selection.position(index)?)))
        .map(|(index, position)| (index, (position - at).length()))
        .filter(|(_, off)| *off <= near)
        .min_by(|one, other| one.1.total_cmp(&other.1))
        .map(|(index, _)| index)
}

/// Where to send the camera next, given where it is pointed now
///
/// The first thing picked out, or the one after whatever is already being
/// looked at, round to the first again off the end. So a key walks a gathered
/// set one at a time and keeps walking it, and pressing it from anywhere else
/// on the map starts again at the top.
fn head_for(selection: &Selection, at: DVec3, radius: f32) -> Option<DVec3> {
    if selection.is_empty() {
        return None;
    }
    let next = match looked_at(selection, at, radius) {
        Some(index) => (index + 1) % selection.len(),
        None => 0,
    };
    selection.position(next)
}

/// Fly to what is picked out, one at a time
///
/// Space, which is the one key on the board that says go on. A search leaves
/// the camera where it is and a click picks something out without moving, so
/// after either of those there is a set held with nothing done about it, and
/// this is what does something about it.
///
/// The zoom is left where the user set it, as it is for a double click and for
/// a line in the bar: a move that only says where to look has nothing to say
/// about how much to take in.
///
/// Read from where the camera has been asked to stand rather than from where
/// it has reached, both the point and the distance, so that a second press
/// mid-flight counts from the place the first press was heading for and the
/// set can be walked at any speed.
///
/// The only binding that asks [`Keyboard::focused`] rather than
/// [`Keyboard::typing`]. Egui reads a space as a click on whatever holds the
/// focus, so a control tabbed onto and left holding it would be clicked again
/// by every press of this key, and one of the controls on the settings pane
/// despawns every system on the map.
fn fly(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    selection: Res<Selection>,
    cameras: Query<&OrbitCamera>,
    mut camera: MessageWriter<MoveCamera>,
) {
    if keyboard.focused || !bare(&keys) || !keys.just_pressed(KeyCode::Space) {
        return;
    }
    let Ok(orbit) = cameras.single() else { return };
    let Some(position) =
        head_for(&selection, orbit.target_center, orbit.target_radius)
    else {
        return;
    };

    camera.write(MoveCamera { position: Some(position), framing: None });
}

/// Take the map's annotations off the sky and put them back
///
/// `L` the names, `O` the orbit lines, `G` the ruled plane. The three things
/// drawn over the galaxy rather than in it, which is what a key is worth
/// having for: they are what stands between the user and a clear look at what
/// they are pointed at.
fn toggle(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    mut show_names: ResMut<ShowNames>,
    mut show_body_names: ResMut<ShowBodyNames>,
    mut show_orbits: ResMut<ShowOrbits>,
    mut show_grid: ResMut<ShowGrid>,
) {
    if keyboard.typing || !bare(&keys) {
        return;
    }

    if keys.just_pressed(KeyCode::KeyL) {
        // One key over the two settings the pane keeps apart, which it can
        // leave disagreeing. A press with either of them on turns both off, so
        // the first press always clears the names off the map whatever state
        // they were left in, and the next puts them back.
        let showing = show_names.0 || show_body_names.0;
        show_names.0 = !showing;
        show_body_names.0 = !showing;
    }

    if keys.just_pressed(KeyCode::KeyO) {
        show_orbits.0 = !show_orbits.0;
    }

    if keys.just_pressed(KeyCode::KeyG) {
        show_grid.0 = !show_grid.0;
    }
}

/// Put the caret in the search box
///
/// Two ways in. `/` is where a reader who came from a browser or an editor
/// will look for it, and shift-S is under a hand already resting on the pan
/// keys. Neither is the bare S, which pans the map back.
fn open_search(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    mut bar: ResMut<BarFields>,
) {
    if keyboard.typing {
        return;
    }

    let slashed = keys.just_pressed(KeyCode::Slash) && bare(&keys);
    let spelled = keys.just_pressed(KeyCode::KeyS) && shifted(&keys);
    if slashed || spelled {
        bar.open();
    }
}

/// Put the form away and take the caret out of it
///
/// Escape, which is where a reader looks for the way out of something they
/// have opened. What was typed is left standing for whenever the form is
/// opened again, the form being shut rather than the question thrown out.
///
/// The one binding that answers while a field is being typed into, and it has
/// to: a caret in a field is the state this exists to undo. Nothing is lost by
/// its doing so, an escape being no part of any name.
fn shut_search(keys: Res<ButtonInput<KeyCode>>, mut bar: ResMut<BarFields>) {
    if keys.just_pressed(KeyCode::Escape) && bare(&keys) {
        bar.shut();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::math::DVec3;
    use std::time::Duration;

    /// How long a frame lasts here
    ///
    /// Said rather than measured. What a key is worth is a rate, and a clock
    /// running at the speed of the machine would answer differently every time
    /// it was asked.
    const STEP: f32 = 1. / 60.;

    /// The keyboard is answered here and nowhere else
    ///
    /// Which is what keeps every binding in one place to be read against the
    /// others, and is how the map comes to have no key that ends the session:
    /// quitting is the window's, by whatever gesture the platform closes one
    /// with. A binding that quit would sit one keystroke from the ones that pan
    /// and search, with nothing to undo it.
    #[test]
    fn the_keyboard_is_answered_in_one_place() {
        let main = include_str!("main.rs");

        assert!(
            !main.contains("KeyCode"),
            "main answers the keyboard, where a binding cannot be read \
             against the rest of them"
        );
    }

    /// A world with the keyboard and the clock the bindings read, and nothing
    /// running in it
    fn world() -> App {
        let mut app = App::new();
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<Time<Real>>();
        app.init_resource::<Keyboard>();
        app
    }

    /// Run one frame with `keys` down and every other key up
    ///
    /// The input is cleared first, as bevy's own does at the top of a frame:
    /// a toggle reads `just_pressed`, which stands until something clears it,
    /// and a key left marked would be read as pressed again every frame.
    fn frame(app: &mut App, keys: &[KeyCode]) {
        app.world_mut()
            .resource_mut::<Time<Real>>()
            .advance_by(Duration::from_secs_f32(STEP));

        let mut input = app.world_mut().resource_mut::<ButtonInput<KeyCode>>();
        input.clear();
        let down: Vec<KeyCode> = input.get_pressed().copied().collect();
        for key in down {
            if !keys.contains(&key) {
                input.release(key);
            }
        }
        for key in keys {
            input.press(*key);
        }

        app.update();
    }

    /// Press `keys` and let them up again
    ///
    /// Two frames, so that a test can press the same key twice.
    fn pressed(app: &mut App, keys: &[KeyCode]) {
        frame(app, keys);
        frame(app, &[]);
    }

    /// Put a name in a field and the caret in it
    ///
    /// A field being typed into holds the focus as well, so both are said. The
    /// two only ever part company the other way round.
    fn type_a_name(app: &mut App) {
        *app.world_mut().resource_mut::<Keyboard>() =
            Keyboard { typing: true, focused: true };
    }

    /// Tab the focus onto a control that is not a field
    ///
    /// A checkbox on the settings pane, say. Egui takes a space as a click on
    /// it and does nothing at all with a letter, which is why the two questions
    /// are asked apart.
    fn tab_onto_a_control(app: &mut App) {
        *app.world_mut().resource_mut::<Keyboard>() =
            Keyboard { typing: false, focused: true };
    }

    /// A world holding one camera `radius` light years back
    ///
    /// Facing along the angles the map opens at, which is a third of a quarter
    /// turn in each, so a pan that came out along a world axis would be a pan
    /// that ignored where the camera is pointed.
    fn looking(radius: f32) -> App {
        let mut app = world();
        app.world_mut().spawn(OrbitCamera {
            radius,
            target_radius: radius,
            ..default()
        });
        // `orbit_camera` is what settles the rotation, and it is not running
        // here, so it is set to match the angles the camera opens at.
        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        let mut orbit = cameras.single_mut(app.world_mut()).unwrap();
        orbit.rotation =
            Quat::from_euler(EulerRot::YXZ, orbit.yaw, orbit.pitch, 0.);
        app
    }

    /// A world where the map can be panned about
    fn driven() -> App {
        let mut app = looking(100.);
        app.add_systems(Update, pan);
        app
    }

    /// Where the camera has been asked to look
    fn asked(app: &mut App) -> DVec3 {
        app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_center
    }

    /// Panning with `WASD` stays on the plane the map is ruled with
    ///
    /// The camera is pitched down onto the plane, so a pan taken along the way
    /// it faces would sink towards it. Forward is that direction laid flat,
    /// and the altitude the user settled on is theirs to keep.
    #[test]
    fn walking_the_map_holds_its_altitude() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::KeyW]);

        let went = asked(&mut app);
        assert!(went.length() > 0., "the map did not move");
        assert!(went.y.abs() < 1e-9, "the pan sank to {}", went.y);
    }

    /// And it goes the way the camera is facing
    ///
    /// Both of the flat axes carry some of the move, the camera opening turned
    /// between them. A pan along a world axis would leave one of them at zero.
    #[test]
    fn walking_the_map_follows_where_it_is_pointed() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::KeyW]);

        let went = asked(&mut app);
        assert!(went.x.abs() > 0., "went nowhere across");
        assert!(went.z.abs() > 0., "went nowhere along");
    }

    /// `A` and `D` are opposite ways along one line
    #[test]
    fn the_map_pans_both_ways_across() {
        let mut left = driven();
        let mut right = driven();

        frame(&mut left, &[KeyCode::KeyA]);
        frame(&mut right, &[KeyCode::KeyD]);

        let (left, right) = (asked(&mut left), asked(&mut right));
        assert!(left.length() > 0., "the map did not move");
        assert!((left + right).length() < 1e-9, "{left} against {right}");
    }

    /// `QE` leaves the plane, and is the only pair that does
    #[test]
    fn rising_off_the_map_goes_straight_up() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::KeyE]);

        let went = asked(&mut app);
        assert!(went.y > 0., "rose by {}", went.y);
        assert!(went.x.abs() < 1e-9, "wandered to {went}");
        assert!(went.z.abs() < 1e-9, "wandered to {went}");
    }

    /// Two keys held at once cover as much ground as one
    ///
    /// Otherwise the map is quicker along its diagonals than along its axes,
    /// which is a pan that speeds up for being steered.
    #[test]
    fn two_keys_pan_no_faster_than_one() {
        let mut one = driven();
        let mut two = driven();

        frame(&mut one, &[KeyCode::KeyW]);
        frame(&mut two, &[KeyCode::KeyW, KeyCode::KeyD]);

        let (one, two) = (asked(&mut one).length(), asked(&mut two).length());
        assert!(one > 0., "the map did not move");
        assert!((one - two).abs() < one * 1e-6, "{one} against {two}");
    }

    /// Opposite keys held together hold the map still
    #[test]
    fn keys_that_cancel_leave_the_map_where_it_was() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::KeyA, KeyCode::KeyD]);

        assert_eq!(asked(&mut app), DVec3::ZERO);
    }

    /// Panning from further out covers more ground
    ///
    /// The map spans nine orders of magnitude of zoom. A fixed step would
    /// cross a whole system in a frame from inside one and barely register out
    /// at the rim.
    #[test]
    fn panning_covers_ground_in_proportion_to_the_zoom() {
        let mut near = driven();
        let mut far = looking(1000.);
        far.add_systems(Update, pan);

        frame(&mut near, &[KeyCode::KeyW]);
        frame(&mut far, &[KeyCode::KeyW]);

        let (near, far) = (asked(&mut near).length(), asked(&mut far).length());
        assert!((far - near * 10.).abs() < near * 1e-3, "{near} then {far}");
    }

    /// A key held with a modifier is on its way somewhere else
    #[test]
    fn a_chord_does_not_pan_the_map() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::ControlLeft, KeyCode::KeyW]);

        assert_eq!(asked(&mut app), DVec3::ZERO);
    }

    /// Shift-S opens the search rather than panning the map back
    #[test]
    fn asking_for_the_search_does_not_pan_the_map() {
        let mut app = driven();

        frame(&mut app, &[KeyCode::ShiftLeft, KeyCode::KeyS]);

        assert_eq!(asked(&mut app), DVec3::ZERO);
    }

    /// A name being typed is not a map being driven
    #[test]
    fn typing_a_name_does_not_pan_the_map() {
        let mut app = driven();
        type_a_name(&mut app);

        frame(&mut app, &[KeyCode::KeyW]);

        assert_eq!(asked(&mut app), DVec3::ZERO);
    }

    /// A world where the camera can be swung about
    fn swung() -> App {
        let mut app = looking(100.);
        app.add_systems(Update, swing);
        app
    }

    /// Which way the camera has been asked to face
    fn facing(app: &mut App) -> f32 {
        app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_yaw
    }

    /// How far over what it is looking at the camera has been asked to stand,
    /// as a share of the distance it is standing off
    ///
    /// One at straight overhead, nothing level with the plane, and negative
    /// under it. Read rather than the pitch itself, so that a test says where
    /// the camera ends up rather than which way an angle happens to run.
    fn standing_over(app: &mut App) -> f32 {
        -app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_pitch
            .sin()
    }

    /// `Z` and `X` carry the camera round
    #[test]
    fn the_camera_swings_both_ways_round() {
        let mut one = swung();
        let mut other = swung();
        let opened_at = facing(&mut swung());

        frame(&mut one, &[KeyCode::KeyZ]);
        frame(&mut other, &[KeyCode::KeyX]);

        let (one, other) = (facing(&mut one), facing(&mut other));
        assert!(one > opened_at, "stayed at {one}");
        assert!(other < opened_at, "stayed at {other}");
    }

    /// `V` raises it over the plane and `C` lowers it toward one
    #[test]
    fn the_camera_rises_and_falls_over_the_plane() {
        let mut up = swung();
        let mut down = swung();
        let opened_at = standing_over(&mut swung());

        frame(&mut up, &[KeyCode::KeyV]);
        frame(&mut down, &[KeyCode::KeyC]);

        let (up, down) = (standing_over(&mut up), standing_over(&mut down));
        assert!(up > opened_at, "stayed at {up}");
        assert!(down < opened_at, "stayed at {down}");
    }

    /// Opposite keys held together leave the camera facing where it was
    #[test]
    fn keys_that_cancel_leave_the_camera_facing_where_it_was() {
        let mut app = swung();
        let opened_at = (facing(&mut swung()), standing_over(&mut swung()));

        frame(&mut app, &[KeyCode::KeyZ, KeyCode::KeyX]);
        frame(&mut app, &[KeyCode::KeyC, KeyCode::KeyV]);

        assert_eq!((facing(&mut app), standing_over(&mut app)), opened_at);
    }

    /// The camera stops short of straight overhead
    ///
    /// On the point itself the up vector flips as the camera crosses what it
    /// is orbiting, and the whole map turns over.
    #[test]
    fn the_camera_stops_short_of_straight_overhead() {
        let mut app = swung();

        for _ in 0..200 {
            frame(&mut app, &[KeyCode::KeyV]);
        }

        assert!(
            standing_over(&mut app) < 1.,
            "reached {}",
            standing_over(&mut app)
        );
        assert!(
            standing_over(&mut app) > 0.99,
            "only reached {}",
            standing_over(&mut app)
        );
    }

    /// And short of straight under
    #[test]
    fn the_camera_stops_short_of_straight_under() {
        let mut app = swung();

        for _ in 0..200 {
            frame(&mut app, &[KeyCode::KeyC]);
        }

        assert!(
            standing_over(&mut app) > -1.,
            "reached {}",
            standing_over(&mut app)
        );
        assert!(
            standing_over(&mut app) < -0.99,
            "only reached {}",
            standing_over(&mut app)
        );
    }

    /// Carried round far enough, the camera comes back to where it started
    ///
    /// The yaw is not clamped, and it should not be: a key held down is a
    /// camera circling what it is looking at, and stopping it somewhere would
    /// be a wall in the middle of a turn.
    #[test]
    fn the_camera_carries_round_and_round() {
        let mut app = swung();
        let opened_at = facing(&mut swung());

        for _ in 0..300 {
            frame(&mut app, &[KeyCode::KeyZ]);
        }

        let round = std::f32::consts::TAU;
        assert!(
            facing(&mut app) > opened_at + round,
            "only reached {}",
            facing(&mut app)
        );
    }

    /// A name being typed is not a map being swung
    #[test]
    fn typing_a_name_does_not_swing_the_camera() {
        let mut app = swung();
        let opened_at = (facing(&mut swung()), standing_over(&mut swung()));
        type_a_name(&mut app);

        frame(&mut app, &[KeyCode::KeyZ, KeyCode::KeyV]);

        assert_eq!((facing(&mut app), standing_over(&mut app)), opened_at);
    }

    /// A spyglass reaching as far as the map opens at, set however the test
    /// wants
    fn spyglass(lock_camera: bool, follow_camera: bool) -> Spyglass {
        Spyglass {
            radius: Spyglass::OPENING,
            fetch: false,
            clear: true,
            lock_camera,
            follow_camera,
        }
    }

    /// A world where the camera can be pulled in and out, standing `radius`
    /// back
    fn zoomed(radius: f32, spyglass: Spyglass) -> App {
        let mut app = looking(radius);
        app.insert_resource(spyglass);
        app.add_systems(Update, zoom);
        app
    }

    /// How far back the camera has been asked to stand
    fn back(app: &mut App) -> f32 {
        app.world_mut()
            .query::<&OrbitCamera>()
            .single(app.world())
            .unwrap()
            .target_radius
    }

    /// `F` pulls the camera in
    #[test]
    fn a_key_pulls_the_camera_in() {
        let mut app = zoomed(100., spyglass(false, false));

        frame(&mut app, &[KeyCode::KeyF]);

        assert!(back(&mut app) < 100., "stayed at {}", back(&mut app));
    }

    /// `R` pushes it back out
    #[test]
    fn a_key_pushes_the_camera_back() {
        let mut app = zoomed(100., spyglass(false, false));

        frame(&mut app, &[KeyCode::KeyR]);

        assert!(back(&mut app) > 100., "stayed at {}", back(&mut app));
    }

    /// The two undo each other exactly
    ///
    /// Which is what makes the zoom multiplicative rather than a step: in and
    /// out for the same time from anywhere is the distance it started at, and
    /// a fixed step would drift a little further every time it was taken.
    #[test]
    fn zooming_in_and_out_leaves_the_camera_where_it_stood() {
        let mut app = zoomed(100., spyglass(false, false));

        frame(&mut app, &[KeyCode::KeyF]);
        frame(&mut app, &[KeyCode::KeyR]);

        assert!((back(&mut app) - 100.).abs() < 1e-3, "{}", back(&mut app));
    }

    /// A key is worth the same share of the distance wherever it is pressed
    ///
    /// The map spans nine orders of magnitude, and the same fraction of the
    /// way in is the whole point of a zoom that multiplies: a step that is
    /// right out at the rim crosses a system in a frame down close.
    #[test]
    fn zooming_covers_the_same_share_at_any_distance() {
        let mut near = zoomed(10., spyglass(false, false));
        let mut far = zoomed(1e4, spyglass(false, false));

        frame(&mut near, &[KeyCode::KeyF]);
        frame(&mut far, &[KeyCode::KeyF]);

        let (near, far) = (back(&mut near) / 10., back(&mut far) / 1e4);
        assert!((near - far).abs() < 1e-6, "{near} against {far}");
    }

    /// Both keys held hold the camera where it is
    #[test]
    fn keys_that_cancel_leave_the_zoom_where_it_was() {
        let mut app = zoomed(100., spyglass(false, false));

        frame(&mut app, &[KeyCode::KeyF, KeyCode::KeyR]);

        assert_eq!(back(&mut app), 100.);
    }

    /// The zoom stops where the map stops
    ///
    /// Held in as long as a key can be held, the camera comes to rest at the
    /// near end of the range rather than passing through it to a distance
    /// nothing can be drawn at.
    #[test]
    fn zooming_in_stops_at_the_near_end_of_the_map() {
        let mut app = zoomed(MIN_RADIUS, spyglass(false, false));

        for _ in 0..10 {
            frame(&mut app, &[KeyCode::KeyF]);
        }

        assert_eq!(back(&mut app), MIN_RADIUS);
    }

    /// And at the far end
    #[test]
    fn zooming_out_stops_at_the_far_end_of_the_map() {
        let mut app = zoomed(MAX_RADIUS, spyglass(false, false));

        for _ in 0..10 {
            frame(&mut app, &[KeyCode::KeyR]);
        }

        assert_eq!(back(&mut app), MAX_RADIUS);
    }

    /// Held to the spyglass, the camera does not answer a key
    ///
    /// The reach writes the distance back on the next frame, so a zoom taken
    /// anyway is a camera that lurches and returns. The same reason the wheel
    /// stands down.
    #[test]
    fn a_locked_camera_does_not_zoom() {
        let mut app = zoomed(100., spyglass(true, false));

        frame(&mut app, &[KeyCode::KeyF]);

        assert_eq!(back(&mut app), 100.);
    }

    /// Locked while the camera is itself what sets the reach, a key works
    ///
    /// Nothing writes the camera's distance in that case, so there is nothing
    /// for a zoom to be undone by.
    #[test]
    fn a_key_zooms_a_camera_that_sets_the_reach() {
        let mut app = zoomed(100., spyglass(true, true));

        frame(&mut app, &[KeyCode::KeyF]);

        assert!(back(&mut app) < 100., "stayed at {}", back(&mut app));
    }

    /// A name being typed is not a map being zoomed
    #[test]
    fn typing_a_name_does_not_zoom_the_map() {
        let mut app = zoomed(100., spyglass(false, false));
        type_a_name(&mut app);

        frame(&mut app, &[KeyCode::KeyF]);

        assert_eq!(back(&mut app), 100.);
    }

    /// A world holding everything the annotations are drawn from
    fn annotated(names: bool, body_names: bool) -> App {
        let mut app = world();
        app.insert_resource(ShowNames(names));
        app.insert_resource(ShowBodyNames(body_names));
        app.insert_resource(ShowOrbits(true));
        app.insert_resource(ShowGrid(true));
        app.add_systems(Update, toggle);
        app
    }

    /// What the annotations stand at: names, body names, orbits, grid
    fn showing(app: &App) -> (bool, bool, bool, bool) {
        (
            app.world().resource::<ShowNames>().0,
            app.world().resource::<ShowBodyNames>().0,
            app.world().resource::<ShowOrbits>().0,
            app.world().resource::<ShowGrid>().0,
        )
    }

    /// `L` takes the names off the map, and only the names
    #[test]
    fn a_key_takes_the_names_off_the_map() {
        let mut app = annotated(true, true);

        pressed(&mut app, &[KeyCode::KeyL]);

        assert_eq!(showing(&app), (false, false, true, true));
    }

    /// Both kinds of name answer to it, however the pane left them
    ///
    /// The settings keep the galaxy's names and a system's apart, so the two
    /// can disagree. One key over both has to give one answer, and clearing
    /// the sky is the answer worth having: a press that turned one on while
    /// turning the other off would leave the map named either way.
    #[test]
    fn one_key_answers_for_both_kinds_of_name() {
        let mut app = annotated(true, false);

        pressed(&mut app, &[KeyCode::KeyL]);
        assert!(!showing(&app).0, "the systems stayed named");
        assert!(!showing(&app).1, "the bodies stayed named");

        pressed(&mut app, &[KeyCode::KeyL]);
        assert!(showing(&app).0, "the systems were left unnamed");
        assert!(showing(&app).1, "the bodies were left unnamed");
    }

    /// `O` takes the orbit lines, and only those
    #[test]
    fn a_key_takes_the_orbit_lines() {
        let mut app = annotated(true, true);

        pressed(&mut app, &[KeyCode::KeyO]);

        assert_eq!(showing(&app), (true, true, false, true));
    }

    /// `G` takes the ruled plane, and only that
    #[test]
    fn a_key_takes_the_grid() {
        let mut app = annotated(true, true);

        pressed(&mut app, &[KeyCode::KeyG]);

        assert_eq!(showing(&app), (true, true, true, false));
    }

    /// A key held down toggles once rather than every frame
    ///
    /// A key is down for as long as a finger rests on it, which at sixty
    /// frames a second is dozens of them, and an annotation that answered
    /// every one of those would come back on or off by whichever number of
    /// frames the finger happened to take.
    #[test]
    fn a_key_held_down_toggles_once() {
        let mut app = annotated(true, true);

        frame(&mut app, &[KeyCode::KeyG]);
        frame(&mut app, &[KeyCode::KeyG]);

        assert!(!showing(&app).3, "the grid came back");
    }

    /// A second press puts an annotation back
    #[test]
    fn a_second_press_puts_an_annotation_back() {
        let mut app = annotated(true, true);

        pressed(&mut app, &[KeyCode::KeyG]);
        pressed(&mut app, &[KeyCode::KeyG]);

        assert!(showing(&app).3);
    }

    /// Nothing is toggled while a name is being typed
    #[test]
    fn typing_a_name_leaves_the_annotations_alone() {
        let mut app = annotated(true, true);
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::KeyL, KeyCode::KeyO, KeyCode::KeyG]);

        assert_eq!(showing(&app), (true, true, true, true));
    }

    /// A world where the search box can be asked for
    fn barred() -> App {
        let mut app = world();
        app.init_resource::<BarFields>();
        app.add_systems(Update, (open_search, shut_search));
        app
    }

    /// Whether the search box has been asked for
    fn opening(app: &App) -> bool {
        app.world().resource::<BarFields>().opening()
    }

    /// Whether the form has been asked to be put away
    fn shutting(app: &App) -> bool {
        app.world().resource::<BarFields>().shutting()
    }

    /// `/` asks for the search box
    #[test]
    fn a_slash_asks_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(opening(&app));
    }

    /// So does shift-S
    #[test]
    fn a_shifted_s_asks_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::ShiftLeft, KeyCode::KeyS]);

        assert!(opening(&app));
    }

    /// A bare S does not, that being how the map is panned back
    #[test]
    fn a_bare_s_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::KeyS]);

        assert!(!opening(&app));
    }

    /// A shifted slash is a question mark rather than a binding
    #[test]
    fn a_shifted_slash_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::ShiftLeft, KeyCode::Slash]);

        assert!(!opening(&app));
    }

    /// Nor does a slash typed into a field
    ///
    /// Which is the whole of why the field is asked about: the box that would
    /// be opened is the box the slash is being typed into.
    #[test]
    fn a_slash_typed_into_a_field_is_not_a_binding() {
        let mut app = barred();
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(!opening(&app));
    }

    /// An escape asks for the form to be put away
    #[test]
    fn an_escape_puts_the_form_away() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(shutting(&app));
    }

    /// And answers while a name is being typed, alone among the bindings
    ///
    /// A caret in a field is the state it exists to undo, so standing down for
    /// one would leave it unable to do the only thing it does. Nothing is lost
    /// by its answering, an escape being no part of any name.
    #[test]
    fn an_escape_answers_while_a_name_is_being_typed() {
        let mut app = barred();
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(shutting(&app));
    }

    /// It does not ask for the box it is putting away
    ///
    /// The two are one key apart on the same resource, and asking for both in
    /// a frame would leave the form to be shut and opened at once.
    #[test]
    fn an_escape_does_not_ask_for_the_search_box() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Escape]);

        assert!(!opening(&app));
    }

    /// Nor does opening the box ask for it to be put away
    #[test]
    fn a_slash_does_not_put_the_form_away() {
        let mut app = barred();

        pressed(&mut app, &[KeyCode::Slash]);

        assert!(!shutting(&app));
    }

    use crate::systems::selection::{Picked, PickedBody};

    /// A selection holding one thing at each of `places`
    ///
    /// Bodies, which carry a position of their own and can be put anywhere a
    /// test likes. Which kind of thing is picked out says nothing about where
    /// the camera is sent, a body and a system alike being a place.
    fn picked(places: &[DVec3]) -> Selection {
        let mut selection = Selection::default();
        for (id, at) in places.iter().enumerate() {
            selection.pick(
                Picked::Body(PickedBody::new(1, id as i16, "body", *at)),
                true,
            );
        }
        selection
    }

    /// A place a light year out along each axis, and another beside it
    fn somewhere(at: f64) -> DVec3 {
        DVec3::new(at, at, at)
    }

    /// What a frame asked of the camera
    ///
    /// A message is drained by whoever reads it, and nothing in these worlds
    /// does, so it is read here into somewhere a test can look. Both halves of
    /// it: where to go, and what the move had to say about the zoom.
    #[derive(Resource, Default)]
    struct Went {
        places: Vec<DVec3>,
        framings: Vec<Option<f32>>,
    }

    fn note_moves(
        mut moves: MessageReader<MoveCamera>,
        mut went: ResMut<Went>,
    ) {
        for asked in moves.read() {
            let Some(position) = asked.position else { continue };
            went.places.push(position);
            went.framings.push(asked.framing);
        }
    }

    /// A world standing a hundred light years back, holding `selection`
    fn gathered(selection: Selection) -> App {
        let mut app = looking(100.);
        app.insert_resource(selection);
        app.init_resource::<Went>();
        app.add_message::<MoveCamera>();
        app.add_systems(Update, (fly, note_moves).chain());
        app
    }

    /// Point the camera at `at` without moving it there
    ///
    /// Where it has been asked to point rather than where it has reached,
    /// which is what a press counts from.
    fn pointed(app: &mut App, at: DVec3) {
        app.world_mut()
            .query::<&mut OrbitCamera>()
            .single_mut(app.world_mut())
            .unwrap()
            .target_center = at;
    }

    /// Everywhere the camera has been sent
    fn went(app: &App) -> &[DVec3] {
        &app.world().resource::<Went>().places
    }

    /// What each of those moves said about how much to take in
    fn framings(app: &App) -> &[Option<f32>] {
        &app.world().resource::<Went>().framings
    }

    /// Space goes to the first thing picked out
    #[test]
    fn a_press_flies_to_the_first_thing_picked_out() {
        let mut app = gathered(picked(&[somewhere(1.), somewhere(2.)]));

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [somewhere(1.)]);
    }

    /// And on to the next once the camera is there
    #[test]
    fn a_press_flies_on_from_where_the_camera_stands() {
        let mut app = gathered(picked(&[somewhere(1.), somewhere(2.)]));
        pointed(&mut app, somewhere(1.));

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [somewhere(2.)]);
    }

    /// A set is walked one at a time and comes round to the top again
    ///
    /// Which is the whole of what the key is for: three systems marked out
    /// together are looked at one after another without a hand leaving the
    /// keyboard.
    #[test]
    fn pressing_on_walks_the_whole_set_and_comes_round() {
        let places = [somewhere(1.), somewhere(2.), somewhere(3.)];
        let mut app = gathered(picked(&places));

        for _ in 0..4 {
            let at = went(&app).last().copied();
            if let Some(at) = at {
                pointed(&mut app, at);
            }
            pressed(&mut app, &[KeyCode::Space]);
        }

        assert_eq!(went(&app), [places[0], places[1], places[2], places[0]]);
    }

    /// A camera standing somewhere else starts again at the top
    ///
    /// Whatever the user was last looking at, the key means show me what I
    /// picked out, and what they picked out starts at the first of them.
    #[test]
    fn a_press_from_elsewhere_starts_at_the_top() {
        let mut app = gathered(picked(&[somewhere(1.), somewhere(2.)]));
        pointed(&mut app, somewhere(2.));
        pressed(&mut app, &[KeyCode::Space]);
        pointed(&mut app, DVec3::new(500., 0., 0.));

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [somewhere(1.), somewhere(1.)]);
    }

    /// What counts as centered follows the zoom
    ///
    /// A light year off is the far side of the map from inside a system and
    /// is dead center from out among the stars, so the same gap has to answer
    /// differently at the two.
    #[test]
    fn what_counts_as_centered_follows_the_zoom() {
        let selection = picked(&[DVec3::ZERO, somewhere(10.)]);
        let off = DVec3::new(0.1, 0., 0.);

        assert_eq!(looked_at(&selection, off, 100.), Some(0));
        assert_eq!(looked_at(&selection, off, 1.), None);
    }

    /// The nearest of several is what a press counts from
    ///
    /// Seen from far enough out, two things standing close together are both
    /// within tolerance, and the one being looked at is the nearer.
    #[test]
    fn the_nearest_is_what_a_press_counts_from() {
        let places = [DVec3::ZERO, DVec3::new(0.5, 0., 0.)];
        let mut app = gathered(picked(&places));
        pointed(&mut app, DVec3::new(0.4, 0., 0.));

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [places[0]], "counted from the further of them");
    }

    /// Nothing picked out is nowhere to go
    #[test]
    fn a_press_with_nothing_picked_out_flies_nowhere() {
        let mut app = gathered(Selection::default());

        pressed(&mut app, &[KeyCode::Space]);

        assert!(went(&app).is_empty());
    }

    /// A flight says where to look and nothing about how much to take in
    ///
    /// The zoom is the user's, as it is for a double click and for a line in
    /// the bar. Reframing each one would take back whatever they had set on
    /// the way round the set.
    #[test]
    fn flying_leaves_the_zoom_where_the_user_set_it() {
        let mut app = gathered(picked(&[somewhere(1.)]));

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(framings(&app), [None]);
    }

    /// A space typed into a name is not a map being flown
    #[test]
    fn typing_a_name_does_not_fly_the_map() {
        let mut app = gathered(picked(&[somewhere(1.)]));
        type_a_name(&mut app);

        pressed(&mut app, &[KeyCode::Space]);

        assert!(went(&app).is_empty());
    }

    /// The distance a press counts by is the one being zoomed to
    ///
    /// Both halves of the question are asked of where the camera has been
    /// asked to stand. Mid-zoom the distance reached and the distance asked
    /// for part company, and taking one of each would weigh how near something
    /// is against a distance the camera is no longer standing at.
    #[test]
    fn a_press_counts_by_the_distance_the_camera_is_heading_for() {
        let places = [DVec3::ZERO, somewhere(1.)];
        let mut app = gathered(picked(&places));
        pointed(&mut app, DVec3::new(0.5, 0., 0.));
        // Pulling hard back: near enough to be centered by where the zoom is
        // heading, and far from centered by where it has so far reached.
        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        let mut orbit = cameras.single_mut(app.world_mut()).unwrap();
        orbit.radius = 1.;
        orbit.target_radius = 1000.;

        pressed(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [places[1]], "counted by the distance reached");
    }

    /// A control holding the focus is what a space is for, not the map
    ///
    /// Egui reads a space as a click on whatever holds the focus, so a
    /// checkbox tabbed onto and left there would be clicked again by every
    /// press of this key. One of the controls on the settings pane despawns
    /// every system on the map.
    #[test]
    fn a_control_holding_the_focus_does_not_fly_the_map() {
        let mut app = gathered(picked(&[somewhere(1.)]));
        tab_onto_a_control(&mut app);

        pressed(&mut app, &[KeyCode::Space]);

        assert!(went(&app).is_empty());
    }

    /// The letters go on driving the map while it does
    ///
    /// Which is why the two questions are asked apart. A checkbox does nothing
    /// with a W, and standing every binding down whenever anything held the
    /// focus would leave the map undrivable until the user clicked the sky.
    #[test]
    fn a_control_holding_the_focus_still_lets_the_map_be_driven() {
        let mut app = driven();
        tab_onto_a_control(&mut app);

        frame(&mut app, &[KeyCode::KeyW]);

        assert!(asked(&mut app).length() > 0., "the map did not move");
    }

    /// A key held down flies once rather than every frame
    ///
    /// A finger resting on it would otherwise walk the whole set inside a
    /// tenth of a second and leave the camera wherever the count landed.
    #[test]
    fn a_key_held_down_flies_once() {
        let mut app = gathered(picked(&[somewhere(1.), somewhere(2.)]));

        frame(&mut app, &[KeyCode::Space]);
        frame(&mut app, &[KeyCode::Space]);
        frame(&mut app, &[KeyCode::Space]);

        assert_eq!(went(&app), [somewhere(1.)]);
    }
}
