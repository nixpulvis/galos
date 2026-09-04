//! What it looks like to point at a system
//!
//! A system can be reached by its star or by its name, and both mean the
//! same thing, so both mark the system itself. Everything drawn for a system
//! then answers to one component and they cannot disagree: pointing at a
//! name rings its star, and pointing at a star lights its name.

use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::spawn::{Body, HeldSystem, Places, Strength};
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::labels::{
    Label, PlateText, color32, depth, depth_of, name_rect, screen_offset,
    screen_position, world_per_pixel,
};
use crate::systems::selection::Selected;
use crate::systems::spawn::Shell;
use bevy::camera::RenderTarget;
use bevy::camera::visibility::ViewVisibility;
use bevy::ecs::entity::EntityHashMap;
use bevy::math::DVec3;
use bevy::picking::backend::{HitData, PointerHits};
use bevy::picking::hover::HoverMap;
use bevy::picking::pointer::{PointerId, PointerLocation, PointerMap};
use bevy::prelude::*;
use bevy::window::{CursorIcon, PrimaryWindow, SystemCursorIcon};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (point_at, size_indicators, point_the_cursor)
            .in_set(MapSet::Present)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_photometrically),
    );
    // Answers where the pointer is before anything asks, which is what a
    // picking backend is and where bevy expects one to run.
    app.add_systems(
        PreUpdate,
        hits.in_set(bevy::picking::PickingSystems::Backend),
    );
    // A body's mark is read by what packs the names, which runs here, so it is
    // settled here too. Where a body stands is asked of the grid holding it
    // rather than of the transform `big_space` writes during `PostUpdate`, so
    // there is nothing to wait for: a body carries the cell and the offset it
    // was spawned with, and both are answers before anything is drawn.
    app.add_systems(
        Update,
        size_bodies.in_set(MapSet::Present).before(super::labels::choose_names),
    );
    // Painted flat in screen space with egui, in the same pass and the same
    // way [`super::labels::draw_names`] paints the names, and before them so
    // the ring layer sits beneath the name grounds. A ring drawn as a mesh out
    // at a system's galaxy coordinate tears in f32; see `docs/night-sky.md`.
    app.add_systems(
        EguiPrimaryContextPass,
        ring.before(super::labels::draw_names),
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

/// The largest an indicator may be, as a radius in logical pixels
///
/// A ring is a mark around a system, not a halo over it. Left uncapped it is
/// [`INDICATOR_MARGIN`] times the shell behind it, and that shell has no fixed
/// size on screen: a bare system held at [`crate::camera`]'s zoom floor is
/// drawn at its whole stand-in extent, and a bright star's point spread swells
/// the same way, either of which would circle half the view. So the ring is
/// held to a small fixed margin however large the shell grows — a touch above
/// the floor, enough that a mark just short of being flown into still reads as
/// ringed, and no more.
const INDICATOR_MAX_RADIUS: f32 = 14.;

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
fn triangle(hop: &crate::systems::route::Hop, at: Vec2) -> Vec<[Vec2; 2]> {
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

/// How wide a ring's stroke is painted, in logical pixels
///
/// A hair bolder than the leader lines [`super::labels::draw_names`] paints at
/// one pixel, so a ring reads as the mark it is rather than as another leader.
/// Painted flat in screen space, so this is pixels on the glass and holds its
/// weight at every zoom.
pub(crate) const RING_STROKE: f32 = 1.5;

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
            commands.entity(system).try_remove::<PointedAt>();
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
            commands.entity(system).try_remove::<PointedAt>();
        }
    }
    if let Some(system) = wanted
        && !already
    {
        commands
            .entity(system)
            .try_insert(PointedAt::reached(time.elapsed_secs()));
    }
}

/// Work out how large each system's mark is, in pixels
///
/// One answer, read back off [`Indicator`] both by what draws the ring and by
/// what answers the pointer, so the mark and the area that catches cannot
/// come apart.
///
/// A shell is drawn in metres and holds a size that changes with the camera,
/// so it is measured into pixels here and held between a floor and a ceiling
/// (see [`system_mark`]). Where the shell is too small to aim at, which is
/// nearly everywhere, the floor is the whole of the answer; where it has
/// swelled toward being flown into, or a bright star's point spread has run
/// away with it, the ceiling holds the ring to a mark rather than a halo.
///
/// A shell that is not drawn is not measured: a mark taken from a sphere
/// nobody can see would put the whole viewport up as one system's target.
pub fn size_indicators(
    camera: Query<(&OrbitCamera, &Camera)>,
    mut systems: Query<
        (&System, &Transform, &Visibility, &Strength, &mut Indicator),
        With<Shell>,
    >,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (system, shell, view, mark, mut indicator) in &mut systems {
        // Off the frame the mark cannot be aimed at, so the pixels it would
        // take are not worked out; held at the floor so a hidden or off-screen
        // system is no easier to hit than an absent one. A hidden system reads
        // as off the frame here, its inherited visibility being what culling
        // asks first.
        if *view == Visibility::Hidden {
            if indicator.0 != INDICATOR_MIN_RADIUS {
                indicator.0 = INDICATOR_MIN_RADIUS;
            }
            continue;
        }

        let drawn = if mark.0 > 0. { shell.scale.x } else { 0. };

        // A metre, which is as near as the camera may be pulled to anything.
        // What the floor is for is the sign rather than the distance.
        let into_view = depth(orbit, DVec3::from(system.position)).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);
        // Only where it moved, as everything asked of every system every frame
        // is. Nothing watches a mark for changes today, and writing one
        // regardless is how that stops being safe without anyone meaning it to.
        let wanted = system_mark(drawn, per_pixel);
        if indicator.0 != wanted {
            indicator.0 = wanted;
        }
    }
}

/// How large a system's mark is, where its shell is `drawn` metres across and a
/// pixel covers `per_pixel`
///
/// The shell measured into pixels and stood off by [`INDICATOR_MARGIN`], then
/// held between [`INDICATOR_MIN_RADIUS`] and [`INDICATOR_MAX_RADIUS`]. The floor
/// keeps a system too small to aim at aimable; the ceiling keeps one drawn large
/// — a bare system at the zoom floor, or a bright star's point spread — from
/// wearing a ring that circles the view rather than the star.
fn system_mark(drawn: f32, per_pixel: f32) -> f32 {
    let shell = drawn * INDICATOR_MARGIN / per_pixel.max(f32::MIN_POSITIVE);
    shell.clamp(INDICATOR_MIN_RADIUS, INDICATOR_MAX_RADIUS)
}

/// Work out how large each body's mark is, in pixels
///
/// A body is drawn at the size it is, so unlike a system its mark is mostly
/// the thing itself: the disc a sphere projects to is its own outline, and
/// aiming at that is aiming at the body. The floor only takes over where a
/// body is drawn too small to hit.
///
/// Measured to where the body stands out in the galaxy, which [`Places`] reads
/// off the grid holding it. A body carries the cell and the offset it was
/// spawned with, so where it is can be answered before anything is drawn.
///
/// Which is what lets this run during `Update`. The mark decides how far a
/// name stands off what it names, and `super::labels::choose_names` packs the
/// names here; taken from the transform `big_space` writes in `PostUpdate`,
/// the mark would be a frame old, and a zoom covers near a quarter of the
/// distance it has left every frame.
pub fn size_bodies(
    camera: Query<(&OrbitCamera, &Camera)>,
    places: Places,
    mut bodies: Query<(Entity, &Body, &mut Indicator)>,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (entity, body, mut indicator) in &mut bodies {
        let Some(place) = places.of(entity) else { continue };
        // A metre, which is as near as the camera may be pulled to anything.
        let into_view = depth(orbit, place).max(1.);
        let per_pixel = world_per_pixel(cot_half_fov, viewport.y, into_view);

        // Only where it moved, as a system's mark is.
        let wanted = body_mark(body.radius, per_pixel);
        if indicator.0 != wanted {
            indicator.0 = wanted;
        }
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

/// The most systems the picker reports under one pointer at once
///
/// A pointer over a crowded sky can fall within the mark radius of thousands
/// of overlapping systems, the same crowding the labels thin themselves
/// against, here on the marks. Putting every one through the hover pipeline
/// each frame is what a zoomed-out view over the galactic core otherwise
/// stalls on, and only the nearest can be pointed at — a mark under a mark is
/// settled by depth. So the picker keeps that many and no more, enough for
/// [`point_at`] to settle its precedence and past what any hand could aim
/// between. The rest are a system you would have to fly in to tell apart.
const MAX_HITS: usize = 256;

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
    systems: Query<(Entity, &System, &Indicator, &Visibility)>,
    bodies: Query<(Entity, &GlobalTransform, &Indicator, &ViewVisibility)>,
    labels: Query<(Entity, &ChildOf, &PlateText), With<Label>>,
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
    // The words with it, so a name is caught over what is drawn rather than
    // over what it was drawn from: a stop's plate says the jump to it as well
    // as its name, and that is a good part of the width again.
    let mut named = EntityHashMap::default();
    for (label, child_of, words) in &labels {
        named.insert(child_of.parent(), (label, words.0.as_str()));
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
            if *drawn == Visibility::Hidden {
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
            if let Some((label, said)) = named.get(&entity)
                && name_rect(on_screen, said, indicator.0).contains(at)
            {
                picks.push((*label, hit));
            }
        }

        // Everything inside a system, measured from the camera rather than
        // from the galaxy. A body is drawn at its own size, so its mark is
        // its own outline, and one drawn over another is settled by which is
        // nearer, exactly as two overlapping spheres would be.
        for (entity, body_at, indicator, drawn) in &bodies {
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
            if let Some((label, said)) = named.get(&entity)
                && name_rect(on_screen, said, indicator.0).contains(at)
            {
                picks.push((*label, hit));
            }
        }

        // Report only the nearest few, so a pointer resting on the dense core
        // does not put a crowd of overlapping sub-pixel marks through the
        // hover pipeline every frame. The nearest is the one pointed at; the
        // rest behind it change nothing but the cost.
        if picks.len() > MAX_HITS {
            picks.sort_by(|(_, a), (_, b)| a.depth.total_cmp(&b.depth));
            picks.truncate(MAX_HITS);
        }
        hits.write(PointerHits::new(*pointer, picks, camera.order as f32));
    }
}

/// Show a pointing cursor while anything worth clicking is under the pointer
///
/// Read from what is hovered rather than from coming and going, so that
/// moving straight from one system to the next cannot leave the cursor
/// behind whichever of the two events happens to arrive last.
///
/// Written only where it changed. What sets the cursor on the window looks at
/// the icon only when it has been marked as written, and an insert is a write
/// whether or not the icon differs, so writing regardless asks the platform to
/// set the same cursor over again every frame.
pub fn point_the_cursor(
    hovered: Res<HoverMap>,
    clickable: Query<(), Or<(With<Indicator>, With<Label>)>>,
    // Whatever the window is showing, which is nothing until this has run
    // once.
    window: Query<(Entity, Option<&CursorIcon>), With<PrimaryWindow>>,
    mut commands: Commands,
) {
    let Ok((window, shown)) = window.single() else { return };
    let over_something = hovered
        .values()
        .flat_map(|hits| hits.keys())
        .any(|entity| clickable.contains(*entity));

    let wanted = if over_something {
        CursorIcon::System(SystemCursorIcon::Pointer)
    } else {
        CursorIcon::default()
    };
    if shown != Some(&wanted) {
        commands.entity(window).insert(wanted);
    }
}

/// Ring the system the pointer is over
///
/// Painted flat in screen space with egui, in the same pass and the same way
/// [`super::labels::draw_names`] paints the names and the leaders that join a
/// name to what it names. A ring drawn as a mesh out at a system's ~1e17 m
/// coordinate tears in the f32 clip transform (see `docs/night-sky.md`); a
/// circle painted at a projected pixel holds its shape at every zoom.
///
/// It goes out with the shell as the camera comes inside the system, as
/// [`super::selection::ring`] does and for the same reason.
#[allow(clippy::too_many_arguments)]
pub fn ring(
    mut contexts: EguiContexts,
    camera: Query<(&OrbitCamera, &Camera)>,
    holding: Res<HeldSystem>,
    // A selected system is already ringed, in its own color. Ringing it
    // again for being pointed at would draw one circle over the other and
    // read as the selection having been lost.
    pointed_at: Query<
        (&System, &Strength, &Indicator, Has<Filtered>),
        (With<PointedAt>, Without<Selected>),
    >,
    // Whatever inside a system is pointed at, read off the grid holding it the
    // way its name is, so it carries no filter and no galactic position of its
    // own.
    inside: Query<
        (Entity, &Indicator),
        (With<Body>, With<PointedAt>, Without<Selected>),
    >,
    // The stops the routes reach from here. Ringed whether or not anything
    // is pointing at them, that being the whole of what the mark is for, and
    // whatever the filters say, as they are drawn regardless of those too.
    hops: Query<(
        &System,
        &Strength,
        &Indicator,
        &crate::systems::route::Hop,
        Has<Selected>,
    )>,
    places: Places,
    dim: Res<DimTo>,
) -> Result {
    let Ok((orbit, camera)) = camera.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    // The camera's own axes, for turning which way a stop lies into which way
    // its stub runs across the view.
    let right = orbit.rotation * Vec3::X;
    let up = orbit.rotation * Vec3::Y;

    let ctx = contexts.ctx_mut()?;
    let painter = ctx.layer_painter(super::labels::annotations_layer());
    let stroke = |color: Srgba| egui::Stroke::new(RING_STROKE, color32(color));
    // A point given in pixels from the middle of the screen, laid out in screen
    // space. A stop's marks are placed about the middle, where the leader they
    // stand in for runs from.
    let middle = viewport * 0.5;
    let placed = |at: Vec2| egui::pos2(middle.x + at.x, middle.y + at.y);

    // The stops the routes reach from here, while the map is holding a system,
    // which is what a stop is reached from and what [`super::selection::ring`]
    // stands back for.
    if holding.of().is_some() {
        for (system, mark, indicator, hop, picked) in &hops {
            let standing = mark.0;
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

            let there = DVec3::from(system.position) - orbit.eye;
            let landed = screen_offset(orbit, cot_half_fov, viewport, there)
                .map(|at| at - middle)
                .filter(|at| at.abs().cmple(middle).all());

            // Where the mark saying which way this stop lies goes, which each
            // of the two ways of drawing a stop settles for itself.
            let icon = match landed {
                // On screen: the ring, and nothing else. The line was only
                // ever there to find the stop with, and once the stop is
                // found it is a line pointing at a thing already in sight.
                // Which of the two stops this is belongs on the ring itself.
                Some(place) => {
                    let ringed = indicator.0.max(INDICATOR_MIN_RADIUS);
                    painter.circle_stroke(placed(place), ringed, stroke(color));

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
                    // Which way the stop lies across the view, which is the
                    // axis the stub is run out along. Asked only here: a stop
                    // in sight is drawn where it is, and needs no direction to
                    // be found by. Nothing to say for one lying straight out
                    // through the middle, which has no across to it.
                    let Some(across) =
                        there.as_vec3().try_normalize().and_then(|toward| {
                            Vec2::new(toward.dot(right), -toward.dot(up))
                                .try_normalize()
                        })
                    else {
                        continue;
                    };

                    let edge = (middle.x / across.x.abs())
                        .min(middle.y / across.y.abs())
                        - STUB_EDGE;

                    painter.line_segment(
                        [
                            placed(across * (edge - STUB_LENGTH)),
                            placed(across * edge),
                        ],
                        // Fainter than the mark it leads to. It is there to be
                        // glanced along rather than read.
                        stroke(color.with_alpha(color.alpha() * STUB_FADE)),
                    );

                    // At the inner end, which is the end that is looked at:
                    // the outer one is against the border of the view.
                    across * (edge - STUB_LENGTH - ICON)
                }
            };

            // The same mark either way, so it is drawn in one place.
            for rule in triangle(hop, icon) {
                painter.line_segment(
                    [placed(rule[0]), placed(rule[1])],
                    stroke(color),
                );
            }
        }
    }

    // Whatever inside a system is pointed at, read off the grid holding it, as
    // its name is, so it is placed against the view it is drawn into.
    for (entity, indicator) in &inside {
        let Some(place) = places.of(entity) else { continue };
        let Some(at) = screen_position(orbit, cot_half_fov, viewport, place)
        else {
            continue;
        };
        painter.circle_stroke(
            egui::pos2(at.x, at.y),
            indicator.0,
            stroke(INDICATOR),
        );
    }

    // The system the pointer is on, drawn where it lands on screen rather than
    // out at its own coordinate, where a mesh ring would tear.
    for (system, mark, indicator, filtered) in &pointed_at {
        let standing = mark.0;
        if standing <= 0. {
            continue;
        }
        let Some(at) = screen_position(
            orbit,
            cot_half_fov,
            viewport,
            DVec3::from(system.position),
        ) else {
            continue;
        };
        painter.circle_stroke(
            egui::pos2(at.x, at.y),
            indicator.0,
            stroke(super::selection::going(
                dim.as_drawn(INDICATOR, filtered),
                standing,
            )),
        );
    }

    Ok(())
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

    /// A system's ring holds a small margin however large its shell grows
    ///
    /// The reported trouble: a bare system at the zoom floor, and a bright
    /// star's point spread, drew a shell large enough that a ring
    /// [`INDICATOR_MARGIN`] times it circled the view. The floor keeps a mark
    /// aimable and the ceiling keeps it a mark, whatever the shell does between.
    #[test]
    fn a_system_ring_never_becomes_a_halo() {
        assert_eq!(system_mark(0., 1.), INDICATOR_MIN_RADIUS);
        assert_eq!(system_mark(1e12, 1.), INDICATOR_MAX_RADIUS);
        // In between, the shell is stood off by the margin and passed through.
        assert_eq!(system_mark(8., 1.), 8. * INDICATOR_MARGIN);
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

    /// How many marks were written
    #[derive(Resource, Default)]
    struct Marks(usize);

    fn count_marks(
        mut marks: ResMut<Marks>,
        systems: Query<(), (Changed<Indicator>, With<System>)>,
    ) {
        marks.0 += systems.iter().count();
    }

    /// A world holding a camera and a system with a shell drawn around it
    ///
    /// The shell is sized so its ring lands in the responsive band — over
    /// [`INDICATOR_MIN_RADIUS`] and under [`INDICATOR_MAX_RADIUS`] — once the
    /// camera has come in, and at the floor when it stands off. A ring pinned at
    /// either bound says nothing about where the camera is, so a shell that
    /// lifts off the floor as the camera nears is what leaves the mark with
    /// anything to say.
    fn sized() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Marks>();
        app.add_systems(Update, (size_indicators, count_marks).chain());
        app.world_mut().spawn((looking(), crate::systems::tests::seeing()));

        // Down the axis the camera looks along, a mark being measured by how
        // far into the view its system lies rather than by how far off it is.
        let mut standing = crate::systems::tests::system(1);
        standing.position = [0., 0., -5.];
        let wide = (0.034 * crate::space::LIGHT_YEAR) as f32;
        app.world_mut().spawn((
            standing,
            Shell,
            Indicator::default(),
            Transform::from_scale(Vec3::splat(wide)),
            Strength::default(),
            Visibility::Visible,
        ));
        app
    }

    /// How many marks have been written so far
    fn marks(app: &App) -> usize {
        app.world().resource::<Marks>().0
    }

    /// A frame that moves nothing leaves a system's mark alone
    ///
    /// The mark is asked of every system every frame, and nothing watches one
    /// for changes today. Writing it regardless is how that stops being safe
    /// without anyone meaning it to.
    #[test]
    fn a_resting_frame_leaves_a_mark_alone() {
        let mut app = sized();

        // The system arriving is a change of its own, and the frame after it
        // is the first that could be said to be resting.
        app.update();
        app.update();
        let settled = marks(&app);

        app.update();
        assert_eq!(marks(&app), settled, "wrote a mark that had not moved");
    }

    /// And a camera that has moved still remarks it
    ///
    /// Which is what the mark is worked out for: it is held in pixels, so
    /// where the camera stands is the whole of what decides it.
    #[test]
    fn a_mark_is_written_again_when_the_camera_moves() {
        let mut app = sized();
        app.update();
        app.update();
        let settled = marks(&app);

        let mut cameras = app.world_mut().query::<&mut OrbitCamera>();
        cameras.single_mut(app.world_mut()).unwrap().eye =
            DVec3::new(0., 0., -2.);
        app.update();

        assert!(marks(&app) > settled, "left a mark at the size it was");
    }

    /// How many times the cursor was written
    #[derive(Resource, Default)]
    struct Sets(usize);

    fn count_sets(
        mut sets: ResMut<Sets>,
        windows: Query<(), Changed<CursorIcon>>,
    ) {
        sets.0 += windows.iter().count();
    }

    /// A world holding a window, with nothing under the pointer
    fn windowed() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HoverMap>();
        app.init_resource::<Sets>();
        app.world_mut().spawn(PrimaryWindow);
        app.add_systems(Update, (point_the_cursor, count_sets).chain());
        app
    }

    /// Put `entity` under the pointer
    fn hover(app: &mut App, entity: Entity) {
        let mut over = EntityHashMap::default();
        over.insert(
            entity,
            HitData {
                camera: Entity::PLACEHOLDER,
                depth: 0.,
                position: None,
                normal: None,
                extra: None,
            },
        );
        let mut hovered = HoverMap::default();
        hovered.insert(PointerId::Mouse, over);
        app.insert_resource(hovered);
    }

    /// What the window is showing
    fn cursor(app: &mut App) -> Option<CursorIcon> {
        let mut windows = app
            .world_mut()
            .query_filtered::<&CursorIcon, With<PrimaryWindow>>();
        windows.iter(app.world()).next().cloned()
    }

    /// How many times the cursor has been written so far
    fn sets(app: &App) -> usize {
        app.world().resource::<Sets>().0
    }

    /// The cursor points while something worth clicking is under it
    #[test]
    fn the_cursor_points_at_what_can_be_clicked() {
        let mut app = windowed();
        let system = app.world_mut().spawn(Indicator(0.)).id();
        hover(&mut app, system);

        app.update();

        assert_eq!(
            cursor(&mut app),
            Some(CursorIcon::System(SystemCursorIcon::Pointer))
        );
    }

    /// And rests over sky with nothing in it
    #[test]
    fn the_cursor_rests_over_empty_sky() {
        let mut app = windowed();

        app.update();

        assert_eq!(cursor(&mut app), Some(CursorIcon::default()));
    }

    /// A frame that moves the pointer onto something writes the cursor
    #[test]
    fn reaching_something_worth_clicking_writes_the_cursor() {
        let mut app = windowed();
        app.update();
        app.update();
        let settled = sets(&app);

        let system = app.world_mut().spawn(Indicator(0.)).id();
        hover(&mut app, system);
        app.update();

        assert!(sets(&app) > settled, "the cursor was left resting");
    }

    /// A frame that moves the pointer nowhere leaves the cursor alone
    ///
    /// What sets the cursor on the window looks at the icon only where it has
    /// been marked as written, so writing it regardless asks the platform to
    /// set the same cursor over again every frame.
    #[test]
    fn a_resting_frame_leaves_the_cursor_alone() {
        let mut app = windowed();
        let system = app.world_mut().spawn(Indicator(0.)).id();
        hover(&mut app, system);

        // The first frame writes whatever the window was not already showing,
        // and the second is the first that could be said to be resting.
        app.update();
        app.update();
        let settled = sets(&app);

        app.update();
        assert_eq!(sets(&app), settled, "wrote a cursor that had not changed");
    }
}
