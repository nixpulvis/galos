//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::{Apparent, Body};
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{
    Label, depth, depth_of, name_rect, screen_offset, screen_position,
    world_per_pixel,
};
use crate::systems::selection::Selected;
use crate::systems::spawn::Shell;
use bevy::camera::RenderTarget;
use bevy::ecs::entity::EntityHashMap;
use bevy::math::DVec3;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{PointerId, PointerLocation, PointerMap};
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (point_at, size_indicators, point_the_cursor)
            .in_set(MapSet::Present)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_uniformly)
            // A mark is taken from the shell that is drawn this frame rather
            // than the one that was drawn last, as it is taken from the size
            // settled this frame.
            .after(super::spawn::shells),
    );
    // Answers where the pointer is before anything asks, which is what a
    // picking backend is and where bevy expects one to run.
    app.add_systems(
        PreUpdate,
        hits.in_set(bevy::picking::PickingSystems::Backend),
    );
    // Reads where a star ended up rather than deciding it, so it waits for
    // the transforms to be worked out, as `labels::leaders` does.
    // Both read where something inside a system ended up rather than
    // deciding it, and a body's place is written by `big_space` during
    // `PostUpdate`. Sized any earlier and a body just spawned would be
    // measured from a transform that has never been worked out, which is the
    // origin: it would be marked as though it sat on the camera.
    app.add_systems(
        PostUpdate,
        (size_bodies, ring).chain().after(TransformSystems::Propagate),
    );
    app.add_observer(start_drag);
    app.add_observer(track_drag);
}

/// The color everything pointed at is drawn in
///
/// One color for the ring, the name and the line between them, so that a
/// system being pointed at reads as one thing rather than as three that
/// happen to have all changed at once.
pub const INDICATOR: Srgba = Srgba::new(1., 0.82, 0.35, 1.);

/// How much wider than its star a system's indicator is drawn
///
/// Far enough out to read as something around the star rather than as part
/// of it.
const INDICATOR_MARGIN: f32 = 1.5;

/// The smallest an indicator may be, as a radius in logical pixels
///
/// A star draws small enough at a distance that an indicator hugging it
/// would be a dot, and it is the indicator that has to be aimed at. Held in
/// pixels because that is what aiming is done in, so the target stays the
/// same size to the hand at every zoom.
const INDICATOR_MIN_RADIUS: f32 = 9.5;

/// The smallest a body's mark may be, as a radius in logical pixels
///
/// Smaller than [`INDICATOR_MIN_RADIUS`], and for the opposite reason. A
/// system stands alone in the sky and a mark that swamps it costs nothing;
/// bodies are drawn packed inside one, and at the range they first appear the
/// whole system is some eighty pixels across. A floor as generous as a
/// system's would draw every body in it as one blob.
///
/// So it is small: enough to aim at a moon that is drawn at a fraction of a
/// pixel, not so much that a system reads as a smear. What answers a crowded
/// system is flying further in, which is what the map is for.
const BODY_MIN_RADIUS: f32 = 4.;

/// How wide the mark saying which way along a route a stop lies is, in pixels
///
/// Small. It is read once, to tell one of two stops from the other, and then
/// not looked at again.
const ICON: f32 = 14.;

/// The mark for `hop`, about `at`, as lines in pixels
///
/// The two triangles every machine that has ever played anything back puts on
/// its buttons: onward for the stop ahead and back the way it came for the one
/// behind. Nobody has to work out what they mean, and they read at a glance
/// rather than on inspection, which a single arrow at this size does not: its
/// head is a few pixels and its tail is a line like every other line drawn.
///
/// Filled by ruling them. Gizmos have only lines to draw with, and a hollow
/// triangle this small is mostly the gap in the middle of it, so each is drawn
/// as a row of lines from its flat side out to wherever the two sloping sides
/// have met by that row.
fn transport(hop: &crate::systems::route::Hop, at: Vec2) -> Vec<[Vec2; 2]> {
    let way = match hop {
        crate::systems::route::Hop::Next => 1.,
        crate::systems::route::Hop::Last => -1.,
    };
    let wing = ICON * 0.5;
    let half = ICON * 0.35;
    let rows = (half * 2.).round().max(1.) as usize;

    let mut ruled = Vec::with_capacity((rows + 1) * 2);
    for base in [-wing, 0.] {
        for row in 0..=rows {
            let y = half * (2. * row as f32 / rows as f32 - 1.);
            let reach = wing * (1. - (y / half).abs());

            ruled.push([
                at + Vec2::new(base * way, y),
                at + Vec2::new((base + reach) * way, y),
            ]);
        }
    }

    ruled
}

/// How far the mark sits below the line the name is set on, in pixels
///
/// A name is set on a baseline and these are triangles standing on nothing, so
/// squaring their tops with the letters leaves them reading high. A little
/// under, and the two settle onto the one line.
const ICON_DROP: f32 = 1.5;

/// How much fainter the stub is than the mark it leads to
const STUB_FADE: f32 = 0.6;

/// How far in from the edge of the view a stub pointing at a stop sits
///
/// At the edge rather than about the star. The edge is where the eye goes to
/// ask what lies off that way, which is the question the stub answers, and
/// around the star it is clutter laid over the one thing the view is of.
const STUB_EDGE: f32 = 40.;

/// And how long it is drawn, in pixels
///
/// Long enough to read as pointing out of the view rather than as a tick in
/// the corner of it.
const STUB_LENGTH: f32 = 60.;

/// How many line segments a ring is drawn with
///
/// Bevy draws a gizmo circle with thirty two unless it is told otherwise,
/// which is a ring visibly cornered by the time one is a few hundred pixels
/// across, and a mark around a body is exactly that once the body is worth
/// looking at. At this it is off by a hundredth of a pixel there.
///
/// Rings are drawn one per thing marked out and a handful of things are ever
/// marked out at once, so the count is nothing beside the orbits.
pub(super) const RING_POINTS: u32 = 256;

/// How much air a body's mark leaves around it, as a fraction of the body
///
/// A ring drawn on a body's own outline sits on the silhouette and reads as
/// part of it rather than as something around it, which is the whole of what a
/// mark is for.
///
/// A fraction, because a gap has to be read against what it is a gap around: a
/// couple of pixels tells a mark from a moon and disappears entirely against a
/// planet a hundred and sixty pixels wide. A tenth is a few pixels where a
/// body is small enough for a few pixels to show and grows with it from there.
///
/// Gentler than [`INDICATOR_MARGIN`], which is what a system's shell is given.
/// A shell is a handful of pixels across and half again of it is still a
/// handful; a planet filling the view would be ringed off the edge of the
/// screen.
const BODY_MARGIN: f32 = 0.1;

/// The least air it leaves, as a radius in pixels
///
/// Half of [`BODY_MIN_RADIUS`], which is what a body too small to see is
/// marked at, so the gap where the body is too small for a tenth of it to show
/// is half the mark it gets at range.
const BODY_AIR: f32 = BODY_MIN_RADIUS / 2.;

/// How large a thing's mark is, as a radius in logical pixels
///
/// The one answer behind both the ring drawn around it and the area that
/// answers the pointer over it, so what can be clicked is exactly what is
/// shown.
///
/// Carried by systems and by the bodies inside them alike. What each is
/// worked out from differs — a system's mark is a floor it almost never
/// leaves, a body's is the size it is actually drawn — but what the number
/// means, and everything that reads it, does not.
///
/// Pixels, because that is what the mark is specified in and what aiming is
/// done in: [`INDICATOR_MIN_RADIUS`] is a distance to the hand rather than a
/// distance in the world. A ring is drawn in the world and so converts this
/// back at the moment of drawing, which is the only place the two units meet.
///
/// Held on the system itself. It once sat on an invisible sphere hung off the
/// system for a ray to be thrown at, and that sphere had to be a size in
/// metres, which is what a system is not: a mark is nine pixels across
/// whether the camera is a light year away or fifty thousand.
#[derive(Component, Default)]
pub struct Indicator(pub f32);

/// How long the pointer must rest on a system before it is asking about it
///
/// Crossing a system on the way to another is not pointing at it. A name
/// takes its place from the ones around it, so a claim staked in passing
/// takes away the very name that was being reached for.
const DWELL: f32 = 0.25;

/// The button that answers for whatever is under the pointer
///
/// Picking knows it as [`PointerButton::Primary`], and [`ButtonInput`] knows
/// it by where it sits, so the two names are put together here. What a press
/// selects and what a press clears are then the same button by construction
/// rather than by two files happening to agree.
pub const PRIMARY: MouseButton = MouseButton::Left;

/// How far a pointer may travel while pressed before it is dragging
///
/// The line between the two things a press can mean. Short of it the press
/// is a click, and answers with whatever is under the pointer; past it the
/// press is moving the map, and asks nothing of what it sweeps across.
///
/// Logical pixels, so it is the same distance to the hand whatever the
/// display density.
pub(super) const DRAG_THRESHOLD: f32 = 5.;

/// How far a pointer has travelled since it was last pressed
///
/// Kept on the pointer rather than in one shared slot, so a second pointer
/// cannot answer for the first and the measurement dies with the pointer.
#[derive(Component, Default)]
pub(super) struct DragDistance(pub f32);

/// Start measuring a pointer's travel when one of its buttons goes down
fn start_drag(
    press: On<Pointer<Press>>,
    pointers: Res<PointerMap>,
    mut commands: Commands,
) {
    let Some(pointer) = pointers.get_entity(press.pointer_id) else { return };
    commands.entity(pointer).insert(DragDistance(0.));
}

/// Keep the furthest a pointer has been from where it was pressed
fn track_drag(
    moved: On<Pointer<Drag>>,
    pointers: Res<PointerMap>,
    mut dragged: Query<&mut DragDistance>,
) {
    let Some(pointer) = pointers.get_entity(moved.pointer_id) else { return };
    let Ok(mut travelled) = dragged.get_mut(pointer) else { return };
    travelled.0 = travelled.0.max(moved.distance.length());
}

/// A system the pointer is over
///
/// Carried by the system rather than by whatever the pointer actually
/// landed on, which may be its star or its name, so that anything wanting
/// to draw a system as pointed at asks one question.
#[derive(Component)]
pub struct PointedAt {
    /// When the pointer came to rest here, in seconds since startup
    since: f32,
}

impl PointedAt {
    /// Reached at `now`, in seconds since startup
    pub fn reached(now: f32) -> Self {
        Self { since: now }
    }

    /// Whether the pointer has rested here long enough to be asking
    ///
    /// What is shown of a system costs nothing and answers at once. What is
    /// shown of its neighbours does not, so that waits.
    pub fn settled(&self, now: f32) -> bool {
        now - self.since >= DWELL
    }
}

/// Mark the one thing the pointer is on
///
/// Read from what is hovered rather than from coming and going, so that the
/// choice between two things under the pointer at once is made in one place
/// instead of falling to whichever event happened to arrive last.
///
/// A name is the thing it names, and wins over a mark. Names are drawn over
/// everything, so a name under the pointer is what the eye says is being
/// pointed at, whatever happens to lie nearer the camera behind it. The one
/// thing that beats a name is whatever the thing it names goes round: a label
/// laid across a star or a planet is a label for something else rather than a
/// reason to stop aiming at what it was laid across.
///
/// Between stars, an admitted one wins, and only then the nearer of the two,
/// as it would if they blocked each other. A filter says which systems the
/// user is working with, and the rest are drawn faintly to be the space those
/// are read against; letting that space take the pointer off an admitted
/// system would have the background answer for the thing in front of it.
///
/// Reachable across [`super`], since what is drawn for a system is ordered
/// after it: a ring, a tint and a selection all answer what this decides, and
/// reading it a frame late is a mark trailing the pointer. No further than
/// that, though, which is as far as [`DragDistance`] goes and as far as
/// anything asks.
pub(super) fn point_at(
    hovered: Res<HoverMap>,
    time: Res<Time<Real>>,
    buttons: Res<ButtonInput<MouseButton>>,
    dragged: Query<&DragDistance>,
    names: Query<&ChildOf, With<Label>>,
    bodies: Query<&Body>,
    marked: Query<(), With<Indicator>>,
    filtered: Query<(), With<Filtered>>,
    pointed_at: Query<Entity, With<PointedAt>>,
    mut commands: Commands,
) {
    // A pointer dragging the map is holding the view, not asking about
    // what it happens to sweep over, and rings and names lighting up under
    // a turning map are noise.
    //
    // Past the same travel that tells a click from a drag, so the two agree:
    // within it the press is a click and what it is on stays pointed at,
    // and beyond it the press is a drag and answers nothing. Only while a
    // button is down, since the distance a pointer last travelled outlives
    // the press that measured it.
    let holding = buttons.any_pressed([
        MouseButton::Left,
        MouseButton::Right,
        MouseButton::Middle,
    ]);
    if holding && dragged.iter().any(|far| far.0 > DRAG_THRESHOLD) {
        for system in &pointed_at {
            commands.entity(system).remove::<PointedAt>();
        }
        return;
    }

    let mut named: Option<Entity> = None;
    // What is admitted, and only then what is nearest. A system the filters
    // admit is what the user asked to be looking at, so one lying behind
    // another they did not ask for is still the one they are pointing at:
    // the dim star in front is the background the filter is read against, and
    // background that answers the pointer is background in the way.
    let mut nearest: Option<(Entity, bool, f32)> = None;
    // Whatever is inside a system, weighed by four things in this order: how
    // far down the system it sits, whether it is a star, whether it was
    // reached by its name, and then the nearest.
    //
    // A parent before whatever goes round it, however the two are drawn. A
    // star is what its system is named for and a planet is what its moons are
    // named after, so one behind something crossing it — or behind that
    // thing's name — is still what was being aimed at.
    //
    // Then a star, which the depth cannot settle: a system's stars and the
    // planets that go round the pair of them all hang off the point in the
    // middle and count the same ancestors.
    //
    // Then a name, for the reason a name wins anywhere: it is drawn over
    // everything, so a name under the pointer is what the eye says is being
    // pointed at. Then depth, a body being a real thing at a real size, so one
    // in front of another is simply the one nearer.
    let mut inside: Option<(Entity, (u8, bool, bool, f32))> = None;

    for hits in hovered.values() {
        for (entity, hit) in hits.iter() {
            // A name is the thing it names, wherever it happens to be drawn.
            let (thing, by_name) = match names.get(*entity) {
                Ok(name) => (name.parent(), true),
                Err(_) => (*entity, false),
            };

            if let Ok(body) = bodies.get(thing) {
                let rank = (body.ancestors, !body.star, !by_name, hit.depth);
                if inside.is_none_or(|(_, was)| rank < was) {
                    inside = Some((thing, rank));
                }
            } else if by_name {
                // A system's name, which is drawn over the whole sky and over
                // every mark in it.
                named = Some(thing);
            } else if marked.contains(thing) {
                let dim = filtered.contains(thing);
                let better = nearest.is_none_or(|(_, was_dim, depth)| {
                    (dim, hit.depth) < (was_dim, depth)
                });
                if better {
                    nearest = Some((thing, dim, hit.depth));
                }
            }
        }
    }

    // A system's name over everything, since a name is drawn over everything.
    // Then whatever is inside a system over the mark standing for the system
    // as a whole: once the camera is close enough to see a body, the body is
    // the thing being pointed at and the system is the place it is in.
    let wanted = named
        .or(inside.map(|(body, _)| body))
        .or(nearest.map(|(system, ..)| system));
    let mut already = false;
    for system in &pointed_at {
        if Some(system) == wanted {
            // Left as it is, so that resting somewhere goes on counting
            // rather than starting over every frame.
            already = true;
        } else {
            commands.entity(system).remove::<PointedAt>();
        }
    }
    if let Some(system) = wanted
        && !already
    {
        commands.entity(system).insert(PointedAt::reached(time.elapsed_secs()));
    }
}

/// Work out how large each system's mark is, in pixels
///
/// One answer, read back off [`Indicator`] both by what draws the ring and by
/// what answers the pointer, so the mark and the area that catches cannot
/// come apart.
///
/// A shell is drawn in metres and holds a size that changes with the camera,
/// so it is measured into pixels here and the larger of that and the floor
/// wins. Where the shell is too small to aim at, which is nearly everywhere,
/// the floor is the whole of the answer.
///
/// A shell that is not drawn is not measured. [`super::shells`] takes one away
/// once the camera is inside the system, and a mark taken from a sphere
/// nobody can see would put the whole viewport up as one system's target.
pub fn size_indicators(
    camera: Query<(&OrbitCamera, &Camera)>,
    mut systems: Query<(&System, &Children, &mut Indicator)>,
    shells: Query<(&Transform, &Visibility), With<Shell>>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (system, children, mut indicator) in &mut systems {
        let drawn = children
            .iter()
            .filter_map(|child| shells.get(child).ok())
            .filter(|(_, shown)| **shown != Visibility::Hidden)
            .map(|(shell, _)| shell.scale.x)
            .fold(0., f32::max);

        // A metre, which is as near as the camera may be pulled to anything.
        // What the floor is for is the sign rather than the distance.
        let into_view = depth(orbit, DVec3::from(system.position)).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);
        let shell = drawn * INDICATOR_MARGIN / per_pixel;

        indicator.0 = shell.max(INDICATOR_MIN_RADIUS);
    }
}

/// Work out how large each body's mark is, in pixels
///
/// A body is drawn at the size it is, so unlike a system its mark is mostly
/// the thing itself: the disc a sphere projects to is its own outline, and
/// aiming at that is aiming at the body. The floor only takes over where a
/// body is drawn too small to hit.
///
/// Measured from the body's own [`GlobalTransform`], which [`big_space`]
/// writes relative to the camera and which is exact this close in. A body
/// carries no galactic position to ask about, and this is better than one:
/// the arithmetic never leaves the neighbourhood the camera is standing in.
///
/// Which is why this runs in `PostUpdate`: that transform is written there,
/// and a body sized before its own place is known is sized from the origin.
/// Until then a body's mark is nothing at all, so a body that has just
/// appeared catches nothing rather than catching everywhere.
pub fn size_bodies(
    camera: Query<(&GlobalTransform, &OrbitCamera, &Camera)>,
    mut bodies: Query<(&GlobalTransform, &Body, &mut Indicator)>,
) {
    let Ok((eye, orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (at, body, mut indicator) in &mut bodies {
        let offset = (at.translation() - eye.translation()).as_dvec3();
        // A metre, which is as near as the camera may be pulled to anything.
        let into_view = depth_of(orbit, offset).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);

        indicator.0 = body_mark(body.radius, per_pixel);
    }
}

/// How large a body of `radius` is marked, where a pixel covers `per_pixel`
///
/// Its own size, which for a sphere is also its outline, and air around it:
/// [`BODY_MARGIN`] of the body or [`BODY_AIR`], whichever is the more. Until
/// both are too small to aim at and the floor takes over, which is where the
/// air is already the whole of the mark.
fn body_mark(radius: f32, per_pixel: f32) -> f32 {
    let drawn = radius / per_pixel;
    let air = (drawn * BODY_MARGIN).max(BODY_AIR);

    (drawn + air).max(BODY_MIN_RADIUS)
}

/// How wide a mark of `radius` pixels is out where its system stands
///
/// A ring is world geometry, so what is held in pixels is spoken back into
/// metres at the moment of drawing. The one place the two meet, and both
/// rings go through it, so neither can disagree with what is caught.
pub(super) fn drawn_radius(
    orbit: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    position: DVec3,
    radius: f32,
) -> f32 {
    let offset = crate::space::metres(position - orbit.eye);
    drawn_radius_of(orbit, cot_half_fov, viewport, offset, radius)
}

/// How wide a mark of `radius` pixels is, `offset` metres from the eye
///
/// What [`drawn_radius`] is written on, and what anything already holding its
/// own place relative to the camera asks: everything inside a system does.
pub(super) fn drawn_radius_of(
    orbit: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    offset: DVec3,
    radius: f32,
) -> f32 {
    // A metre, which is as near as the camera may be pulled to anything.
    let into_view = depth_of(orbit, offset).max(1.);
    radius * world_per_pixel(cot_half_fov, viewport.y, into_view)
}

/// Say what the pointer is over, measured on screen
///
/// A picking backend, which is to say it answers one question: which entities
/// lie under each pointer. Everything downstream of that — what is hovered,
/// what a click lands on, whether the cursor turns — is bevy's and is not
/// touched by how the answer is arrived at.
///
/// Arrived at here by projecting each system to the screen and comparing
/// distances in pixels, rather than by throwing a ray at a mesh standing in
/// for it. What is being aimed at is a mark nine pixels across; putting that
/// in the world only to ask a ray about it is a long way round, and at this
/// map's scale it does not survive the trip. A ray is built from a point at
/// the near plane and one at `near / f32::EPSILON`, and it is met by
/// inverting the target's transform and multiplying three of its lengths
/// together. In metres, with a system's mark drawn `1e15` across and stars
/// `1e17` apart, all three of those overflow a float. The map was briefly
/// unclickable in three separate ways for this reason.
///
/// Screen space has none of that: a projection and a distance, both in
/// pixels, both small. It is also where the sizes were decided to begin with.
///
/// Bodies are still met by rays, and rightly. They are drawn at their own
/// size, they have shapes worth hitting rather than a disc standing in for
/// them, and the numbers stay ordinary.
fn hits(
    pointers: Query<(&PointerId, &PointerLocation)>,
    window: Query<Entity, With<PrimaryWindow>>,
    cameras: Query<(
        Entity,
        &Camera,
        &RenderTarget,
        &OrbitCamera,
        &GlobalTransform,
    )>,
    systems: Query<(Entity, &System, &Indicator, &ViewVisibility)>,
    bodies: Query<(
        Entity,
        &Body,
        &GlobalTransform,
        &Indicator,
        &ViewVisibility,
    )>,
    labels: Query<(Entity, &ChildOf), With<Label>>,
    mut hits: MessageWriter<PointerHits>,
) {
    let Ok((eye, camera, target, orbit, eye_at)) = cameras.single() else {
        return;
    };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    let caught = |camera: Entity, position: DVec3| HitData {
        camera,
        depth: depth(orbit, position),
        position: None,
        normal: None,
        extra: None,
    };

    // Which name hangs off what. A name is caught through the answer worked
    // out for the thing it names rather than through a second one worked out
    // for itself, so a name cannot catch anywhere its own thing is not, and a
    // name whose thing is not drawn is not caught at all.
    //
    // Keyed the other way round than it is asked, because that is the way
    // round the asking happens: the sky runs to thousands of systems and a
    // handful of them wear a name, so the few are gathered here and the many
    // are looked up as they are gone over anyway.
    let mut named = EntityHashMap::default();
    for (label, child_of) in &labels {
        named.insert(child_of.parent(), label);
    }

    for (pointer, at) in &pointers {
        let Some(at) = at.location() else { continue };
        if !at.is_in_viewport(camera, target, &window) {
            continue;
        }
        let at = at.position;
        let mut picks = Vec::new();

        for (entity, system, indicator, drawn) in &systems {
            // What is not drawn is not there to be pointed at. The spyglass
            // hides a system by writing its visibility, and one hidden is one
            // the user has said they are not looking at.
            if !drawn.get() {
                continue;
            }
            let position = DVec3::from(system.position);
            let Some(on_screen) =
                screen_position(orbit, cot_half_fov, viewport, position)
            else {
                continue;
            };
            let hit = caught(eye, position);
            if on_screen.distance(at) <= indicator.0 {
                picks.push((entity, hit.clone()));
            }
            // A name is caught over the rectangle it was given, which is the
            // same one [`super::labels::choose_names`] laid out to keep names
            // from touching. So the areas that catch cannot overlap either,
            // and a name is clickable over exactly the room it was granted.
            if let Some(label) = named.get(&entity)
                && name_rect(on_screen, &system.name, indicator.0).contains(at)
            {
                picks.push((*label, hit));
            }
        }

        // Everything inside a system, measured from the camera rather than
        // from the galaxy. A body is drawn at its own size, so its mark is
        // its own outline, and one drawn over another is settled by which is
        // nearer, exactly as two overlapping spheres would be.
        for (entity, body, body_at, indicator, drawn) in &bodies {
            if !drawn.get() {
                continue;
            }
            let offset =
                (body_at.translation() - eye_at.translation()).as_dvec3();
            let Some(on_screen) =
                screen_offset(orbit, cot_half_fov, viewport, offset)
            else {
                continue;
            };
            let hit = HitData {
                camera: eye,
                depth: depth_of(orbit, offset),
                position: None,
                normal: None,
                extra: None,
            };
            if on_screen.distance(at) <= indicator.0 {
                picks.push((entity, hit.clone()));
            }
            // A body's name as a system's, and over the same rectangle. What
            // differs between the two is only where the thing being named
            // ended up, which is answered before a name is asked about.
            if let Some(label) = named.get(&entity)
                && name_rect(on_screen, &body.name, indicator.0).contains(at)
            {
                picks.push((*label, hit));
            }
        }

        hits.write(PointerHits::new(*pointer, picks, camera.order as f32));
    }
}

/// Show a pointing cursor while anything worth clicking is under the pointer
///
/// Read from what is hovered rather than from coming and going, so that
/// moving straight from one system to the next cannot leave the cursor
/// behind whichever of the two events happens to arrive last.
pub fn point_the_cursor(
    hovered: Res<HoverMap>,
    clickable: Query<(), Or<(With<Indicator>, With<Label>)>>,

    window: Query<Entity, With<PrimaryWindow>>,
    mut commands: Commands,
) {
    let Ok(window) = window.single() else { return };
    let over_something = hovered
        .values()
        .flat_map(|hits| hits.keys())
        .any(|entity| clickable.contains(*entity));

    commands.entity(window).insert(if over_something {
        CursorIcon::System(SystemCursorIcon::Pointer)
    } else {
        CursorIcon::default()
    });
}

/// Ring the system the pointer is over
///
/// Drawn as a gizmo rather than a mesh because it lasts exactly as long as
/// the pointer rests there and follows the camera while it does.
///
/// Turned to face the camera, so it reads as a ring around the star rather
/// than as a hoop the star is sitting inside.
///
/// It goes out with the shell as the camera comes inside the system, as
/// [`super::selection::ring`] does and for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn ring(
    mut gizmos: Gizmos,
    camera: Query<(&OrbitCamera, &Camera)>,
    seen_as: Res<Apparent>,
    // A selected system is already ringed, in its own color. Ringing it
    // again for being pointed at would draw one circle over the other and
    // read as the selection having been lost.
    pointed_at: Query<
        (Entity, &GlobalTransform, &System, &Indicator, Has<Filtered>),
        (With<PointedAt>, Without<Selected>),
    >,
    // Whatever inside a system is pointed at, which carries no filter and no
    // galactic position of its own.
    inside: Query<
        (&GlobalTransform, &Indicator),
        (With<Body>, With<PointedAt>, Without<Selected>),
    >,
    // The stops the route reaches from here. Ringed whether or not anything
    // is pointing at them, that being the whole of what the mark is for, and
    // whatever the filters say, as they are drawn regardless of those too.
    hops: Query<(
        Entity,
        &GlobalTransform,
        &System,
        &Indicator,
        &crate::systems::route::Hop,
        Has<Selected>,
    )>,
    // The system the camera is standing in, which the lines to the stops
    // leave from.
    standing_in: Query<&GlobalTransform, With<System>>,
    eye_at: Query<&GlobalTransform, With<Camera>>,
    dim: Res<DimTo>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    // Where the line to each stop leaves from, which is the system the camera
    // is standing in. Nothing to draw from where the map holds nothing.
    let from = seen_as
        .of()
        .and_then(|held| standing_in.get(held).ok())
        .map(|at| at.translation());

    // Everything about a stop is drawn where the camera can see it rather than
    // where the stop actually is. A stop is a jump away, and standing inside a
    // system the camera is measuring in metres: a ring out at its true distance
    // is past the far plane, and a line between here and there reads as depth
    // rather than as a mark. So the stop is projected to the screen and drawn
    // back in the plane the star stands in, which puts the whole of it in front
    // of the camera at a depth whose scale is already worked out.
    if let (Some(from), Ok(eye)) = (from, eye_at.single()) {
        let right = orbit.rotation * Vec3::X;
        let up = orbit.rotation * Vec3::Y;
        let ahead = orbit.rotation * Vec3::NEG_Z;
        let here = (from - eye.translation()).as_dvec3();
        let pixel = drawn_radius_of(orbit, cot_half_fov, viewport, here, 1.);
        let middle =
            eye.translation() + ahead * (from - eye.translation()).dot(ahead);
        // A point `at` pixels from the middle of the screen, drawn out there.
        let placed = |at: Vec2| middle + (right * at.x - up * at.y) * pixel;

        for (entity, at, _, indicator, hop, picked) in &hops {
            let standing = seen_as.standing(entity);
            if standing <= 0. {
                continue;
            }
            // A stop picked out is ringed by the selection in its own color,
            // and what is drawn beside it follows that rather than standing
            // there in another.
            let hue = if picked {
                super::selection::SELECTION
            } else {
                crate::systems::route::HOP
            };
            let color = super::selection::going(hue, standing);

            let there = (at.translation() - eye.translation()).as_dvec3();
            let landed = super::labels::screen_offset(
                orbit,
                cot_half_fov,
                viewport,
                there,
            )
            .map(|at| at - viewport * 0.5)
            .filter(|at| at.abs().cmple(viewport * 0.5).all());

            // Which way the stop lies across the view. Both of the things this
            // draws lie along it, so following one leads to the other and the
            // handover between them moves nothing. Nothing to say for a stop
            // lying straight out through the middle, which has no across to it.
            let across =
                landed.and_then(|at| at.try_normalize()).or_else(|| {
                    (at.translation() - from).try_normalize().and_then(
                        |toward| {
                            Vec2::new(toward.dot(right), -toward.dot(up))
                                .try_normalize()
                        },
                    )
                });
            let Some(across) = across else { continue };

            // Where the mark saying which way this stop lies goes, which each
            // of the two ways of drawing a stop settles for itself.
            let icon = match landed {
                // On screen: the ring, and nothing else. The line was only
                // ever there to find the stop with, and once the stop is
                // found it is a line pointing at a thing already in sight.
                // Which of the two stops this is belongs on the ring itself.
                Some(place) => {
                    // Drawn here whether or not the stop is picked out, in
                    // whichever color `hue` settled on. A stop is drawn where
                    // the camera can see it rather than where it is, and
                    // [`super::selection::ring`] draws where a thing is: a
                    // selection ringed out at its true distance is a ring
                    // nobody sees. So this rings every stop and the selection
                    // leaves stops alone.
                    let ringed = indicator.0.max(INDICATOR_MIN_RADIUS);
                    gizmos
                        .circle(
                            Isometry3d::new(placed(place), orbit.rotation),
                            ringed * pixel,
                            color,
                        )
                        .resolution(RING_POINTS);

                    // Under the name and starting where it starts. The name is
                    // laid out from the mark by these same two figures, so they
                    // are read from where it reads them rather than guessed at
                    // a second time.
                    let left = indicator.0
                        + super::labels::NAME_HEIGHT * super::labels::GAP;
                    place + Vec2::new(left + ICON * 0.5, ICON_DROP)
                }
                // Nowhere to be seen: a stub at the edge saying which way to
                // turn, run out along the same axis a leader would lie on.
                None => {
                    let half = viewport * 0.5;
                    let edge = (half.x / across.x.abs())
                        .min(half.y / across.y.abs())
                        - STUB_EDGE;

                    gizmos.line(
                        placed(across * (edge - STUB_LENGTH)),
                        placed(across * edge),
                        // Fainter than the mark it leads to. It is there to be
                        // glanced along rather than read.
                        color.with_alpha(color.alpha() * STUB_FADE),
                    );

                    // At the inner end, which is the end that is looked at:
                    // the outer one is against the border of the view.
                    across * (edge - STUB_LENGTH - ICON)
                }
            };

            // The same mark either way, so it is drawn in one place.
            for rule in transport(hop, icon) {
                gizmos.line(placed(rule[0]), placed(rule[1]), color);
            }
        }
    }

    // Whatever inside a system is pointed at, which is drawn where it stands
    // and needs none of the above: it is in here with the camera.
    if let Ok(eye) = eye_at.single() {
        for (at, indicator) in &inside {
            let offset = (at.translation() - eye.translation()).as_dvec3();
            let radius = drawn_radius_of(
                orbit,
                cot_half_fov,
                viewport,
                offset,
                indicator.0,
            );

            gizmos
                .circle(
                    Isometry3d::new(at.translation(), orbit.rotation),
                    radius,
                    INDICATOR,
                )
                .resolution(RING_POINTS);
        }
    }

    for (entity, at, system, indicator, filtered) in &pointed_at {
        let standing = seen_as.standing(entity);
        if standing <= 0. {
            continue;
        }
        // Drawn at what the pointer is tested against, so the ring is the
        // outline of the very area that catches.
        let radius = drawn_radius(
            orbit,
            cot_half_fov,
            viewport,
            DVec3::from(system.position),
            indicator.0,
        );

        gizmos
            .circle(
                Isometry3d::new(at.translation(), orbit.rotation),
                radius,
                super::selection::going(
                    dim.against(INDICATOR, filtered),
                    standing,
                ),
            )
            .resolution(RING_POINTS);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::ecs::entity::EntityHashMap;
    use bevy::picking::backend::HitData;
    use bevy::picking::pointer::PointerId;

    /// Crossing a system does not count as pointing at it
    ///
    /// This is the whole reason for the wait. A name takes its place from
    /// the ones around it, so a system claiming one in passing takes it
    /// from whatever the pointer was on its way to.
    #[test]
    fn passing_over_a_system_is_not_pointing_at_it() {
        let brushed = PointedAt::reached(10.);

        assert!(!brushed.settled(10.), "counted the instant it was reached");
        assert!(
            !brushed.settled(10. + DWELL * 0.9),
            "counted before the pointer had settled"
        );
    }

    /// Resting on one does
    ///
    /// Measured a shade past the wait rather than exactly on it, since a
    /// subtraction of two floats does not land on the boundary and it is
    /// not the boundary that matters.
    #[test]
    fn resting_on_a_system_is_pointing_at_it() {
        let rested = PointedAt::reached(10.);

        assert!(rested.settled(10. + DWELL * 1.1));
        assert!(rested.settled(10. + DWELL * 10.));
    }

    /// A map with `stars` under the pointer at once, each `(dim, depth)`
    ///
    /// Answers which system each of them stands for, in the order given.
    /// Every star is a system carrying the child the pointer actually hits,
    /// as the sky is built, and the hits are what a picking backend would
    /// have reported for those children.
    fn pointed(stars: &[(bool, f32)]) -> (App, Vec<Entity>) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let mut over = EntityHashMap::default();
        let systems = stars
            .iter()
            .map(|(dim, depth)| {
                let system = app.world_mut().spawn(Indicator(0.)).id();
                if *dim {
                    app.world_mut().entity_mut(system).insert(Filtered);
                }
                over.insert(
                    system,
                    HitData {
                        camera: Entity::PLACEHOLDER,
                        depth: *depth,
                        position: None,
                        normal: None,
                        extra: None,
                    },
                );
                system
            })
            .collect();

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();
        (app, systems)
    }

    /// Which of `stars` came out pointed at
    fn points_at(app: &App, stars: &[Entity]) -> Vec<bool> {
        stars
            .iter()
            .map(|star| app.world().entity(*star).contains::<PointedAt>())
            .collect()
    }

    /// An admitted system is pointed at through a dim one nearer the camera
    ///
    /// The dim ones are the space a filter is read against. One of them
    /// answering the pointer over an admitted system behind it would put the
    /// background in the way of the thing it is there to set off.
    #[test]
    fn an_admitted_system_is_pointed_at_through_a_dim_one() {
        let (app, stars) = pointed(&[(true, 1.), (false, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![false, true]);
    }

    /// Between two admitted, the nearer is still the one pointed at
    ///
    /// Being admitted decides between a dim star and a lit one, and depth
    /// goes on deciding everything it decided before.
    #[test]
    fn between_two_admitted_the_nearer_is_pointed_at() {
        let (app, stars) = pointed(&[(false, 1.), (false, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![true, false]);
    }

    /// And between two dim ones, likewise
    ///
    /// Neither of them admitted, so the rule that decided it before is the
    /// whole of what is left.
    #[test]
    fn between_two_dim_ones_the_nearer_is_pointed_at() {
        let (app, stars) = pointed(&[(true, 1.), (true, 50.)]);

        assert_eq!(points_at(&app, &stars), vec![true, false]);
    }

    /// A camera at the origin, looking down `-Z`
    fn looking() -> OrbitCamera {
        OrbitCamera { eye: DVec3::ZERO, rotation: Quat::IDENTITY, ..default() }
    }

    /// The cotangent of half the vertical field of view, for a default lens
    ///
    /// What `Camera::clip_from_view` answers, worked out here rather than
    /// built from a window so the sizing can be tested without one.
    fn cot_half_fov() -> f32 {
        1. / (PerspectiveProjection::default().fov / 2.).tan()
    }

    /// A mark is the same size to the hand however far off its system is
    ///
    /// The whole reason it is held in pixels. Nine pixels at the near end of
    /// the map and nine at the far end, where the two ends are fourteen
    /// orders of magnitude apart.
    #[test]
    fn a_mark_is_the_same_size_to_the_hand_at_every_zoom() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);
        let mark = INDICATOR_MIN_RADIUS;

        // A metre off, and the width of the galaxy off.
        for away in [1f64, 1e6, 1e12, 1e18, 1e21] {
            let position = DVec3::new(0., 0., -away);
            let drawn =
                drawn_radius(&camera, cot_half_fov(), viewport, position, mark);
            // Back into pixels, the way the ring's size is arrived at.
            let per_pixel = world_per_pixel(
                cot_half_fov(),
                viewport.y,
                depth(&camera, position).max(1.),
            );

            assert!(
                (drawn / per_pixel - mark).abs() < mark * 1e-3,
                "a {mark} pixel mark {away}m off came back {} pixels",
                drawn / per_pixel
            );
        }
    }

    /// A mark stays a number at the far end of the map
    ///
    /// What the ray this replaced could not do. Aiming at a system meant
    /// inverting a transform scaled to metres and multiplying three of its
    /// lengths together, and at these distances all of that overflows a
    /// float and comes back as an infinity or as nothing at all.
    #[test]
    fn a_mark_across_the_galaxy_is_still_a_number() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);

        // A hundred thousand light years, in the metres the map is drawn in.
        let across = DVec3::new(0., 0., -9.46e20);
        let drawn = drawn_radius(
            &camera,
            cot_half_fov(),
            viewport,
            across,
            INDICATOR_MIN_RADIUS,
        );

        assert!(drawn.is_finite(), "the mark came back {drawn}");
        assert!(drawn > 0., "the mark collapsed to {drawn}");
    }

    /// A system dead ahead lands in the middle of the screen
    ///
    /// Which is what the pointer is measured against, so a mark that is not
    /// where its system is drawn is a mark that catches somewhere else.
    #[test]
    fn a_system_ahead_is_marked_where_it_is_drawn() {
        let camera = looking();
        let viewport = Vec2::new(1280., 720.);
        let ahead = DVec3::new(0., 0., -9.46e17);

        let at = screen_position(&camera, cot_half_fov(), viewport, ahead)
            .expect("a system in front of the camera is on screen");

        assert!(
            at.distance(viewport / 2.) < 1e-3,
            "a system dead ahead landed at {at}"
        );
    }

    /// A body of `radius` metres at `depth` metres, marked
    fn marked(radius: f32, depth: f32) -> f32 {
        let viewport = Vec2::new(1280., 720.);
        body_mark(radius, world_per_pixel(cot_half_fov(), viewport.y, depth))
    }

    /// How wide a body of `radius` metres is drawn at `depth` metres
    fn drawn(radius: f32, depth: f32) -> f32 {
        let viewport = Vec2::new(1280., 720.);
        radius / world_per_pixel(cot_half_fov(), viewport.y, depth)
    }

    /// A body is marked at the size it is drawn
    ///
    /// Which for a sphere is its own outline, so aiming at the mark and
    /// aiming at the body are the same act. Twice as far off is half as
    /// large, as anything drawn in perspective is.
    #[test]
    fn a_body_is_marked_at_the_size_it_is_drawn() {
        // An Earth, near enough to fill a good part of the view.
        let near = marked(6.371e6, 5e7);
        let far = marked(6.371e6, 1e8);

        assert!(near > BODY_MIN_RADIUS, "the floor answered instead: {near}");
        assert!(
            (near / far - 2.).abs() < 1e-3,
            "half the distance marked {near} against {far}"
        );
    }

    /// A body twice the size is marked twice as large
    #[test]
    fn a_larger_body_is_marked_larger() {
        let small = marked(6.371e6, 5e7);
        let large = marked(1.2742e7, 5e7);

        assert!((large / small - 2.).abs() < 1e-3);
    }

    /// A body's mark always stands clear of the body
    ///
    /// Which is what a ring around one has to do to read as a ring. Swept from
    /// a moon drawn at a hundredth of a pixel to a planet filling the view,
    /// since the mark was the body's own outline and met it exactly at every
    /// size.
    #[test]
    fn a_body_is_marked_clear_of_itself() {
        // An Earth from a light hour off down to a few radii away.
        for depth in [1.08e12, 1e11, 1e10, 1e9, 5e7, 2e7] {
            let mark = marked(6.371e6, depth);
            let body = drawn(6.371e6, depth);
            let air = (body * BODY_MARGIN).max(BODY_AIR);

            assert!(
                mark - body >= air - 1e-3,
                "a body drawn {body} wide was marked at {mark}, \
                 which is {} of air against the {air} it is owed",
                mark - body
            );
        }
    }

    /// The least air around one is half the mark it gets from far off
    ///
    /// So the gap where a body is too small for a fraction of it to show is of
    /// a piece with the dot such a body is drawn as, rather than a second
    /// number chosen on its own.
    #[test]
    fn the_least_air_is_half_the_furthest_mark() {
        assert_eq!(BODY_AIR * 2., BODY_MIN_RADIUS);
    }

    /// The air grows with the body
    ///
    /// The whole of what was wrong before. A couple of pixels reads against a
    /// moon and vanishes against a planet a hundred and sixty across, which is
    /// a ring drawn on the silhouette.
    #[test]
    fn the_air_around_a_body_grows_with_it() {
        let close = marked(6.371e6, 2e7) - drawn(6.371e6, 2e7);
        let off = marked(6.371e6, 1e9) - drawn(6.371e6, 1e9);

        assert!(close > off * 4., "{close} of air against {off}");
        assert!(close > 8., "a planet filling the view got {close} of air");
    }

    /// A body too small to see is still worth aiming at
    ///
    /// A moon at the far side of a system is drawn at a fraction of a pixel,
    /// and something drawn at nothing can never be pointed at. The floor is
    /// what makes it reachable without flying to it first.
    #[test]
    fn a_body_too_small_to_see_is_still_worth_aiming_at() {
        // A moon a light hour off, which is a hundredth of a pixel.
        let mark = marked(1.7e6, 1.08e12);

        assert_eq!(mark, BODY_MIN_RADIUS);
    }

    /// A body's floor is well short of a system's
    ///
    /// Bodies are drawn packed inside one system, so a floor as generous as
    /// the one a system stands alone with would draw a whole system as a
    /// single blob of overlapping marks.
    #[test]
    fn a_body_is_not_marked_as_broadly_as_a_system() {
        assert!(BODY_MIN_RADIUS < INDICATOR_MIN_RADIUS);
    }

    /// A world holding one system and one body, both under the pointer
    ///
    /// The body deeper than the system, so that anything preferring the
    /// nearer of the two would answer with the system and fail the test.
    fn inside(body_depth: f32) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let system = app.world_mut().spawn(Indicator(0.)).id();
        let body = app
            .world_mut()
            .spawn((
                Indicator(0.),
                Body {
                    address: 1,
                    name: String::new(),
                    id: 1,
                    class: String::new(),
                    radius: 1e6,
                    ancestors: 0,
                    primary: false,
                    star: false,
                },
            ))
            .id();

        let hit = |depth| HitData {
            camera: Entity::PLACEHOLDER,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut over = EntityHashMap::default();
        over.insert(system, hit(1.));
        over.insert(body, hit(body_depth));

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();
        (app, system, body)
    }

    /// A body with `id`, named under `ancestors` of them
    fn body(id: i16, ancestors: u8) -> Body {
        Body {
            address: 1,
            name: String::new(),
            id,
            class: String::new(),
            radius: 1e6,
            ancestors,
            primary: false,
            star: false,
        }
    }

    /// A star with `id`, named under `ancestors` of them
    fn sun(id: i16, ancestors: u8) -> Body {
        Body { star: true, ..body(id, ancestors) }
    }

    /// A world holding two bodies under the pointer, `over` steps down the
    /// system and `under` steps down
    ///
    /// The one further up is the deeper of the two in the view, so that
    /// anything preferring the nearer answers with the other and fails.
    fn crossing(over: u8, under: u8) -> (App, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let star = app.world_mut().spawn((Indicator(0.), body(1, over))).id();
        let moon = app.world_mut().spawn((Indicator(0.), body(2, under))).id();

        let hit = |depth| HitData {
            camera: Entity::PLACEHOLDER,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut over = EntityHashMap::default();
        over.insert(star, hit(50.));
        over.insert(moon, hit(1.));

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();
        (app, star, moon)
    }

    /// A parent is pointed at through whatever goes round it
    ///
    /// A star is what its system is named for and a planet is what its moons
    /// are named after, and something a pixel across passing over either is
    /// not what the user was aiming at.
    ///
    /// Every step down: a star through its planet, and that planet through
    /// its own moon.
    #[test]
    fn a_parent_is_pointed_at_through_what_goes_round_it() {
        for (over, under) in [(0, 1), (1, 2), (2, 3)] {
            let (app, parent, child) = crossing(over, under);

            assert!(
                app.world().entity(parent).contains::<PointedAt>(),
                "the one {over} down was not pointed at"
            );
            assert!(
                !app.world().entity(child).contains::<PointedAt>(),
                "the one {under} down answered for what it goes round"
            );
        }
    }

    /// A star answers before a planet that goes round the pair of it
    ///
    /// Which the depth cannot settle. Shinrarta Dezhra's two stars and its
    /// three outer planets all hang off the point at the middle and count one
    /// ancestor apiece, so being a star is what is left to ask.
    ///
    /// The planet nearer the camera, so that anything going by depth alone
    /// answers with it and fails.
    #[test]
    fn a_star_answers_before_its_own_siblings() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let star = app.world_mut().spawn((Indicator(0.), sun(1, 1))).id();
        let planet = app.world_mut().spawn((Indicator(0.), body(2, 1))).id();

        let hit = |depth| HitData {
            camera: Entity::PLACEHOLDER,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut over = EntityHashMap::default();
        over.insert(star, hit(50.));
        over.insert(planet, hit(1.));

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();

        assert!(app.world().entity(star).contains::<PointedAt>());
        assert!(!app.world().entity(planet).contains::<PointedAt>());
    }

    /// A name reaches the thing it names, and what that goes round beats it
    ///
    /// A name is drawn over everything, so a name under the pointer is
    /// ordinarily what the eye says is being pointed at. A moon's name laid
    /// across the star it goes round is a label for something else, though,
    /// rather than a reason to stop aiming at the star.
    #[test]
    fn a_star_is_pointed_at_through_the_name_of_a_moon() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let star = app.world_mut().spawn((Indicator(0.), body(1, 0))).id();
        let moon = app.world_mut().spawn((Indicator(0.), body(2, 2))).id();
        let label = app.world_mut().spawn(Label).id();
        app.world_mut().entity_mut(moon).add_child(label);

        let hit = |depth| HitData {
            camera: Entity::PLACEHOLDER,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut over = EntityHashMap::default();
        over.insert(star, hit(50.));
        over.insert(label, hit(1.));

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();

        assert!(
            app.world().entity(star).contains::<PointedAt>(),
            "the star was not pointed at"
        );
        assert!(
            !app.world().entity(moon).contains::<PointedAt>(),
            "a moon's name answered for the star it was drawn over"
        );
    }

    /// Between two of a kind, a name still wins over a disc
    ///
    /// Which is the reading a name is given anywhere: it is drawn over
    /// everything, so one under the pointer is what is being pointed at.
    #[test]
    fn a_body_is_pointed_at_by_its_name_over_another_body() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<ButtonInput<MouseButton>>();
        app.add_systems(Update, point_at);

        let near = app.world_mut().spawn((Indicator(0.), body(1, 2))).id();
        let far = app.world_mut().spawn((Indicator(0.), body(2, 2))).id();
        let label = app.world_mut().spawn(Label).id();
        app.world_mut().entity_mut(far).add_child(label);

        let hit = |depth| HitData {
            camera: Entity::PLACEHOLDER,
            depth,
            position: None,
            normal: None,
            extra: None,
        };
        let mut over = EntityHashMap::default();
        over.insert(near, hit(1.));
        over.insert(label, hit(50.));

        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
        app.update();

        assert!(app.world().entity(far).contains::<PointedAt>());
        assert!(!app.world().entity(near).contains::<PointedAt>());
    }

    /// A body is pointed at through the system holding it
    ///
    /// Once the camera is close enough to see what is inside a system, what
    /// is inside it is what is being pointed at. The mark standing for the
    /// system as a whole still sits at its centre, and answering with that
    /// would put the place in front of the thing in it.
    #[test]
    fn a_body_is_pointed_at_through_the_system_holding_it() {
        let (app, system, body) = inside(50.);

        assert!(
            app.world().entity(body).contains::<PointedAt>(),
            "the body was not pointed at"
        );
        assert!(
            !app.world().entity(system).contains::<PointedAt>(),
            "the system answered over the body inside it"
        );
    }
}
