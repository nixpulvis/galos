use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::bodies::spawn::{ApparentSize, Body};
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::pointing::{INDICATOR, Indicator, PointedAt};
use crate::systems::selection::{SELECTION, Selected};
use crate::systems::spawn::{Shell, ShowNames};
use crate::systems::{Spyglass, System};
use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::entity::EntityHashSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_rich_text3d::{
    LoadFonts, Text3d, Text3dPlugin, Text3dSegment, Text3dStyling, TextAnchor,
    TextAtlas,
};

pub(crate) fn plugin(app: &mut App) {
    // Only if it is not already there, and appended rather than set. The
    // ruled plane sets its own numbers in the same face and asks for it the
    // same way, and whichever of the two is added first should not decide
    // whether the other's face is loaded.
    if !app.is_plugin_added::<Text3dPlugin>() {
        app.add_plugins(Text3dPlugin { load_system_fonts: false, ..default() });
    }
    app.world_mut()
        .get_resource_or_insert_with(LoadFonts::default)
        .font_embedded
        .push(epaint_default_fonts::HACK_REGULAR);
    app.insert_resource(NameRadius {
        follow_spyglass: true,
        radius: DEFAULT_NAME_RADIUS,
    });
    app.insert_resource(ShowBodyNames(true));
    app.add_systems(Startup, init_materials);
    app.add_systems(Update, redim.in_set(MapSet::Present));
    app.add_systems(
        Update,
        tint_marked_names
            .in_set(MapSet::Present)
            .after(super::pointing::point_at)
            // A name is spawned in the color of a system at rest, so one
            // that appears because its system has just been marked out
            // draws untinted for a frame unless the tint follows the spawn.
            // This wants the name that exists rather than the one asked for
            // last frame.
            .after(respawn),
    );
    // `face_camera` and the sizing systems both write a `Transform`, on
    // different entities, so the scheduler cannot run them together whatever
    // is said here. Ordering them costs nothing and fixes which goes first.
    app.add_systems(
        Update,
        (choose_names, respawn, face_camera)
            .chain()
            .in_set(MapSet::Present)
            // Both read which system is pointed at, which is decided this
            // frame rather than last.
            .after(super::pointing::point_at)
            // A selected system is named whether or not it is within reach,
            // but only while it is drawn, and that is decided here.
            .after(super::visibility)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_uniformly)
            // A name stands off the mark drawn around what it names, so it
            // wants the mark settled this frame rather than last.
            .after(super::pointing::size_indicators),
    );
    // `leaders` reads where a label ended up rather than deciding it, and
    // neither of the two answers it needs exists during `Update`. A label's
    // `GlobalTransform` is computed from the local one in `PostUpdate`, and
    // its `ViewVisibility` is not settled until everything has had a chance
    // to hide it. Reading either any earlier draws last frame's line.
    app.add_systems(
        PostUpdate,
        leaders
            .after(TransformSystems::Propagate)
            .after(VisibilitySystems::MarkNewlyHiddenEntitiesInvisible),
    );
}

/// World size of a run of text at unit scale
///
/// The line box a text mesh is built at, which whoever places it scales down
/// to the height they want it drawn at.
pub(crate) const SIZE: f32 = 64.;

/// Depth floor for the label size, in metres
///
/// Size is proportional to depth, so a system level with the camera would
/// draw at nothing and one just behind it at a negative size, which is
/// mirrored. Anything this close is inside the near plane regardless.
///
/// A metre. What it guards against is the sign, not any particular distance,
/// and the camera cannot be pulled nearer than this to what it looks at.
pub(crate) const MIN_DEPTH: f32 = 1.;

/// How far a name is drawn from what the camera looks at, to begin with
///
/// What [`NameRadius`] starts at, and what the tests measure against.
const DEFAULT_NAME_RADIUS: f32 = 100.;

/// How tall a system's name draws, in logical pixels
///
/// The line box, which for a single line of text is the size the font is
/// set at. The one number that decides how large a name is; everything else
/// follows from the viewport and where the camera is.
pub(super) const NAME_HEIGHT: f32 = 12.;

/// How far from what the camera looks at a system may be and still be named
///
/// Worth reaching for, where the size of a name is not: turning it up asks
/// for more of what is around, and what will not fit is dropped rather than
/// drawn over, so asking for more of it costs only the asking.
#[derive(Resource)]
pub struct NameRadius {
    /// Take the spyglass's reach rather than the one below
    ///
    /// On to begin with. The spyglass already answers how much of the
    /// galaxy is being looked at, and a name belongs to something drawn, so
    /// there is rarely a second answer worth giving.
    pub follow_spyglass: bool,
    /// How far names reach when not following, in light years
    pub radius: f32,
}

impl NameRadius {
    /// How far names actually reach, given what the spyglass is doing
    ///
    /// Never past the spyglass while it is clearing, since a system it has
    /// hidden has nothing to put a name against. A spyglass that is not
    /// clearing draws everything loaded, and then the asking is the only
    /// limit.
    pub fn reach(&self, spyglass: &Spyglass) -> f32 {
        if self.follow_spyglass {
            spyglass.radius
        } else if !spyglass.clear {
            self.radius
        } else {
            self.radius.min(spyglass.radius)
        }
    }
}

/// Whether the things inside a system are named
///
/// On to begin with, unlike the systems' own names. A system's name is one of
/// thousands and is off until asked for, to keep the sky readable; a body's is
/// one of a handful and is only ever drawn once the camera has flown in to
/// look at them, which is most of what flying in is for.
#[derive(Resource)]
pub struct ShowBodyNames(pub bool);

/// Sideways gap between a star and its label, in text heights
pub(super) const GAP: f32 = 0.75;

/// How far a label sits above its star, in text heights
const RISE: f32 = 1.0;

/// The family name inside [`epaint_default_fonts::HACK_REGULAR`], used to
/// select it in [`Text3dStyling`]
///
/// The same face egui draws the chrome in, so a name on the map and the same
/// name in the bar are the one typeface. Monospaced, which is what [`ADVANCE`]
/// rests on.
pub(crate) const FONT: &str = "Hack";

/// Color of the line joining a star to its name
///
/// Dimmer than the text so it reads as a connector rather than as content.
const LEADER_COLOR: Srgba = Srgba::new(1., 1., 1., 0.35);

/// Air left at each end of the line, as a fraction of its full span
///
/// The same gap sits between the line and the body as between the line and
/// the first glyph, so the connector reads as detached from both.
const LEADER_GAP: f32 = 0.15;

/// How wide a character is taken to be, as a fraction of the font size
///
/// Whether two names overlap is decided from their rectangles, and the width
/// a name will draw at is known exactly, in `Text3dDimensionOut`, but only
/// once the text mesh has been built, and building it is what [`choose_names`]
/// is deciding whether to do. So the width is reckoned from the letter count
/// instead.
///
/// Named for the typographic advance, how far the pen moves along after
/// drawing a glyph. [`FONT`] is monospaced, so every glyph advances the same
/// and a name of `n` letters is exactly `n` of these across, whichever letters
/// they are. That is also why the names may be drawn in capitals for nothing:
/// a `W` takes the room an `i` does.
///
/// Above the font's own advance of `1233/2048`, so that it errs wide. Erring
/// wide costs a gap between two names, and with it a third name that would
/// have fitted between them; erring narrow overlaps them, which is the thing
/// being prevented.
const ADVANCE: f32 = 0.7;

/// How much of a name's own height is kept clear around it
///
/// Names that merely touch are still hard to read apart.
const CROWDING: f32 = 0.35;

/// How strongly being what the camera looks at argues for a name being shown
///
/// In the same light years as everything else, so this is what the name of
/// the system at the center is worth over one sitting at the camera itself.
/// The best score is always kept, nothing having been placed yet to crowd it
/// out, so a value above the span of the other terms makes that name certain
/// rather than merely likely. They span a little over two hundred as they
/// stand, which is why this sits where it does.
const CENTER_WEIGHT: f32 = 250.;

/// How strongly being pointed at argues for a name being shown
///
/// Above [`CENTER_WEIGHT`], so that a system under the pointer takes the top
/// of the order from the one the camera was sent to. The best score is
/// always kept, nothing having been placed yet to crowd it out, so the name
/// of whatever is pointed at is certain to be drawn and anything that would
/// have overlapped it gives way instead.
const POINTED_WEIGHT: f32 = 500.;

/// How strongly being selected argues for a name being shown
///
/// Above [`POINTED_WEIGHT`], for the same reason the ring of a selection is
/// drawn in place of the ring of a point: a selection is what the user asked
/// to keep, and a point is wherever the pointer happens to be resting. Where
/// the two names would overlap, the one that lasts holds its place.
///
/// Far enough above it to clear [`CENTER_WEIGHT`] as well, since a point may
/// be sitting on the center and drawing that bonus with it while the
/// selection is out at a distance and paying for it. The margin left over is
/// what that distance may be, which is [`DEFAULT_NAME_RADIUS`] over. Past
/// there a point on the center takes the top of the order back, as it already
/// does from the center itself at [`POINTED_WEIGHT`].
const SELECTED_WEIGHT: f32 = 1000.;

/// Whether a system is asked for a name at all
///
/// Two ways to be passed over, two to be asked for regardless, and one that
/// settles it whatever the other four say.
///
/// A name is read or it is not; there is no faint reading of one. So a system
/// the filters exclude gives its name up rather than keeping it dimly: a sky
/// of faint names over dim stars has nothing legible in it, and what the
/// filters admit is what the user asked to be able to read. The toggle says
/// the same thing about every system at once.
///
/// Being marked out beats both. Pointing at a system or picking it out is
/// asking for it by name, which is the one thing a name is for, and neither
/// the toggle nor a filter has any business refusing it.
///
/// `stands` beats being marked out. It is whether the map is still standing a
/// mark in for the system, and the name is part of that mark: once the camera
/// is inside, what is drawn there is the system itself, and the things in it
/// carry their own names. A name left hanging over them would be the label of
/// a shell that is no longer drawn.
///
/// The star a system arrives at ends it as surely as the mark fading does, and
/// sooner: it is named for the system, so from the moment it is drawn the
/// system's name is already on screen. The two would otherwise both be laid
/// out, the same words a few pixels apart, for the whole band over which the
/// contents arrive before the mark begins to go.
fn worth_naming(
    stands: bool,
    shown: bool,
    filtered: bool,
    pointed_at: bool,
    selected: bool,
) -> bool {
    stands && (pointed_at || selected || (shown && !filtered))
}

/// How far the center bonus reaches, in light years
///
/// It falls to half at this distance. The point the camera orbits is usually
/// a system exactly, so this only has to forgive one that is merely near it.
const CENTER_REACH: f32 = 2.;

/// One thing in the running for a name, as [`choose_names`] weighs it
///
/// Spelled out once. Both of the questions it answers are asked of the same
/// five things, and written into the query they read as a wall rather than as
/// a list of claims.
type Candidate<'a, T> = (
    Entity,
    T,
    &'a Visibility,
    &'a Indicator,
    Has<Filtered>,
    Has<crate::systems::route::Hop>,
);

/// What [`respawn`] needs of a system that has won a name
///
/// Whether it already carries the plate, and whether the name has a jump to
/// say as well as a system.
type Plated = (
    Entity,
    &'static System,
    Option<&'static Children>,
    Has<crate::systems::route::Hop>,
);

/// Whatever a name is hung off, as [`leaders`] reads it
///
/// Where it is, what is drawn under it to begin clear of, and the three ways
/// of being marked out that leave a line with nothing to say.
type Anchor = (
    &'static GlobalTransform,
    Option<&'static Children>,
    Has<PointedAt>,
    Has<Selected>,
    Has<crate::systems::route::Hop>,
);

/// A system whose name has won a place on screen
///
/// Awarded by [`choose_names`] and read by [`respawn`], which spawns a label
/// for a system that has one and takes the label away from a system that
/// does not. A name that would not be readable never gets a mesh built for
/// it at all.
#[derive(Component)]
pub struct Named;

/// A marker for system name labels
#[derive(Component)]
pub struct Label;

/// How far in front of the camera a point is, in metres
///
/// Depth into the view, which is not the same as the distance to the camera.
/// A point at the corner of the screen is further from the eye than one at
/// the center at the same depth, so sizing by distance draws the corner one
/// larger. At the corner of a 16:9 viewport with a quarter-turn field of
/// view, distance is about 1.31 times the depth.
///
/// In metres although `point` is given in light years, because every caller
/// wants it in order to work out a size to draw something at, and what is
/// drawn is measured in metres. [`screen_position`] takes the same projection
/// on its own rather than through this, since a place on screen is a ratio of
/// two lengths and does not care which unit either is in.
///
/// Both ends come from [`OrbitCamera`], which publishes an absolute position
/// and a rotation during `Update`. The camera's `GlobalTransform` answers
/// neither question: it is written in `PostUpdate`, so it lags a frame, and
/// it holds a position relative to the floating origin rather than to the
/// galaxy. Negative behind the camera.
pub(super) fn depth(camera: &OrbitCamera, point: DVec3) -> f32 {
    depth_of(camera, crate::space::metres(point - camera.eye))
}

/// How far in front of the camera something `offset` from the eye is
///
/// The same measurement as [`depth`], for whatever already knows where it is
/// relative to the eye rather than where it is in the galaxy. Everything
/// inside a system does: [`big_space`] writes a `GlobalTransform` measured
/// from the floating origin, which is the camera, and it is exact near it.
///
/// Answers in whatever unit `offset` is given in.
pub(crate) fn depth_of(camera: &OrbitCamera, offset: DVec3) -> f32 {
    let forward = (camera.rotation * Vec3::NEG_Z).as_dvec3();
    offset.dot(forward) as f32
}

/// How much world one logical pixel covers, at a given depth
///
/// A perspective view widens with depth, so a pixel spans more world the
/// further in it is measured. Multiplying a size in pixels by this gives the
/// world size that draws at it, which is what makes a label hold its size on
/// screen however far away the system is.
///
/// `cot_half_fov` is `Camera::clip_from_view().y_axis.y`, which glam fills
/// with `1 / tan(fov_y / 2)`. The vertical field of view is what the
/// viewport's height is divided into; aspect ratio lives in the matrix's x
/// axis and does not enter.
pub(crate) fn world_per_pixel(
    cot_half_fov: f32,
    viewport_height: f32,
    depth: f32,
) -> f32 {
    2. * depth / (cot_half_fov * viewport_height)
}

/// Where a point lands on screen, in logical pixels from the top left
///
/// [`None`] for anything level with the camera or behind it, which has no
/// place on screen to land on.
///
/// The camera's own axes turn the offset to a point into how far right, how
/// far up, and how far in it lies. The first two divided by what a pixel
/// covers at that depth are the offset from the middle of the viewport, in
/// pixels. Screen y counts downwards, where the camera's counts up.
pub(super) fn screen_position(
    camera: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    point: DVec3,
) -> Option<Vec2> {
    screen_offset(camera, cot_half_fov, viewport, point - camera.eye)
}

/// Where something `offset` from the eye lands on screen
///
/// What [`screen_position`] is written on, and what anything already holding
/// its own place relative to the camera asks directly.
///
/// The unit does not matter so long as it is one unit: a place on screen is a
/// length over a length, and the two cancel. So a system may ask in light
/// years and a body in metres, and both are answered in pixels.
pub(crate) fn screen_offset(
    camera: &OrbitCamera,
    cot_half_fov: f32,
    viewport: Vec2,
    offset: DVec3,
) -> Option<Vec2> {
    let depth = offset.dot((camera.rotation * Vec3::NEG_Z).as_dvec3()) as f32;
    if depth <= 0. {
        return None;
    }

    let right = offset.dot((camera.rotation * Vec3::X).as_dvec3()) as f32;
    let up = offset.dot((camera.rotation * Vec3::Y).as_dvec3()) as f32;
    let per_pixel = world_per_pixel(cot_half_fov, viewport.y, depth);

    Some(viewport / 2. + Vec2::new(right, -up) / per_pixel)
}

/// What a name may be drawn in
///
/// A material per color rather than one recolored per name, because the
/// color lives on a shared asset: changing it would repaint every name at
/// once. Swapping which handle a label points at repaints only that one.
///
/// Two sets of the same three: full strength, and whatever [`DimTo`] asks for
/// a name whose system the filters exclude. The dim set is recolored in
/// place when that moves, which is the case where repainting every name at
/// once is exactly what is wanted.
#[derive(Resource)]
pub struct LabelMaterials {
    bright: [Handle<StandardMaterial>; 3],
    dim: [Handle<StandardMaterial>; 3],
}

/// Which color a name is drawn in, given what its system is
///
/// Named rather than numbered, for the reason [`super::spawn::Hue`] is: the
/// two sets are laid out in [`Tint::ALL`] order and indexed by the tint.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Tint {
    Resting,
    PointedAt,
    Selected,
}

impl Tint {
    /// Every tint, in the order the sets hold them
    const ALL: [Tint; 3] = [Tint::Resting, Tint::PointedAt, Tint::Selected];

    /// What a name of this tint comes out
    ///
    /// A name comes out the color of the ring drawn around its star, so that
    /// a system marked out is one thing in two places rather than two answers
    /// that have to be matched up.
    const fn color(self) -> Srgba {
        match self {
            Tint::Resting => Srgba::WHITE,
            Tint::PointedAt => INDICATOR,
            Tint::Selected => SELECTION,
        }
    }
}

impl LabelMaterials {
    /// The handle for `tint`, at the strength `dimmed` asks for
    fn get(&self, tint: Tint, dimmed: bool) -> &Handle<StandardMaterial> {
        let set = if dimmed { &self.dim } else { &self.bright };
        &set[tint as usize]
    }
}

/// Decide which systems get to show their name
///
/// Held to whichever is nearer of [`NameRadius`] and the spyglass, since a
/// system the spyglass has hidden has nothing to put a name against.
///
/// Every name inside that reach drawn at once is unreadable: the dev
/// database holds a couple of thousand systems within a hundred light years
/// and a full one holds many times that, so they pile into each other. This
/// keeps the ones that fit.
///
/// Worked in screen pixels, because that is where the crowding happens. Two
/// systems light years apart share a pixel when the camera is far enough
/// away, and two a stone's throw apart fill the screen when it is close.
///
/// Nearest and most populous win, and the rest are dropped where they would
/// overlap something already kept. Greedy rather than optimal: the best
/// arrangement of a few hundred overlapping rectangles is not worth solving
/// each frame, and taking them in order of what the viewer most wants to see
/// gives them the ones that matter.
pub fn choose_names(
    mut commands: Commands,
    camera: Query<(&OrbitCamera, &Camera)>,
    radius: Res<NameRadius>,
    spyglass: Res<Spyglass>,
    show_names: Res<ShowNames>,
    show_body_names: Res<ShowBodyNames>,
    systems: Query<Candidate<'_, &'static System>>,
    bodies: Query<(Entity, &Body, &GlobalTransform, &Indicator)>,
    eye_at: Query<&GlobalTransform, With<Camera>>,
    named: Query<Entity, With<Named>>,
    pointing: Query<&PointedAt>,
    selection: Query<(), With<Selected>>,
    seen_as: Res<ApparentSize>,
    time: Res<Time<Real>>,
) {
    let clear = |commands: &mut Commands| {
        for entity in &named {
            commands.entity(entity).remove::<Named>();
        }
    };

    let Ok((orbit, camera)) = camera.single() else {
        clear(&mut commands);
        return;
    };
    let Some(viewport) = camera.logical_viewport_size() else {
        clear(&mut commands);
        return;
    };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    let reach = radius.reach(&spyglass);

    // Which system, if any, has the star it arrives at drawn inside it. That
    // star is named for the system, so while it is drawn the system's own name
    // is the same words a second time over the same place, and which of the
    // two survives the layout comes down to whichever scored higher as the
    // camera moved. What that looks like is a name flickering between two
    // labels a few pixels apart.
    let carried = bodies
        .iter()
        .find(|(_, body, _, _)| body.primary)
        .map(|(_, body, _, _)| body.address);

    // Where a jump is measured from, for the stops that say one. Looked up in
    // the same query the layout goes over: the system the camera is standing
    // in is one of the systems on the map.
    let from = seen_as
        .of()
        .and_then(|held| systems.get(held).ok())
        .map(|(_, system, ..)| system.position());

    // Everything close enough to name and in front of the camera, with the
    // rectangle its name would occupy and how much it deserves one.
    let mut wanted: Vec<(Entity, Rect, f32)> = systems
        .iter()
        .filter_map(|(entity, system, visibility, indicator, filtered, hop)| {
            // Pointing at a system asks for its name whatever else has
            // been set, so it answers to neither of the tests below.
            //
            // Only once the pointer has come to rest, though. A system
            // crossed on the way to another would otherwise take a name,
            // and with it the place of the name being reached for.
            let pointed_at = pointing
                .get(entity)
                .is_ok_and(|at| at.settled(time.elapsed_secs()));

            // Selecting one asks the same, and for longer: the whole point
            // of a selection is that it stays marked out while the user
            // moves around it, and a mark that says which star without
            // saying which system is half an answer.
            //
            // Only while it is drawn, though. Unlike a point, a selection
            // outlives the spyglass hiding its star, and a name is laid out
            // in screen space whether or not it is rendered, so one asked
            // for by a hidden star would take the place of a name that
            // could be read.
            let selected =
                selection.contains(entity) && *visibility != Visibility::Hidden;

            // And the map still standing a mark in for the system at all,
            // which beats every other claim: past there the system is drawn
            // rather than marked, and its name belongs to the mark. The star
            // it arrives at being drawn ends it just as surely, that star
            // carrying the name from there on.
            let stands = seen_as.standing_for(entity) > 0.
                && carried != Some(system.address);

            // A stop a route reaches is named whatever else is set. It is
            // one of two systems out of the whole sky that the viewer is being
            // told to look at, and a mark saying look there without saying
            // where there is is half an answer.
            if !worth_naming(
                stands,
                show_names.0,
                filtered,
                pointed_at || hop,
                selected,
            ) {
                return None;
            }

            let position = DVec3::from(system.position);
            let from_center = (position - orbit.center).length() as f32;
            // Further out than names were asked to reach, and not one of the
            // two the map is marking out.
            if !pointed_at && !selected && !hop && from_center > reach {
                return None;
            }
            let at = screen_position(orbit, cot_half_fov, viewport, position)?;
            // The words that will be set rather than the name alone. A stop
            // carries the jump to it, which is a good part of the width again,
            // and room granted for less than that is a name laid over the next
            // one along.
            let rect =
                name_rect(at, &plate_for(system, hop, from), indicator.0);
            let screen = Rect::from_corners(Vec2::ZERO, viewport);
            if screen.intersect(rect).is_empty() {
                return None;
            }
            let score = name_score(from_center, pointed_at, selected);
            Some((entity, rect, score))
        })
        .collect();

    // Everything inside the system the camera is in. Its own switch, since a
    // system's name and a body's are asked for at different moments: the sky
    // is read by name and a system inside is read by looking, so one is off
    // until wanted and the other on until it is in the way.
    if let Ok(eye) = eye_at.single() {
        for (entity, body, at, indicator) in &bodies {
            let offset = (at.translation() - eye.translation()).as_dvec3();
            let Some(place) =
                screen_offset(orbit, cot_half_fov, viewport, offset)
            else {
                continue;
            };
            let rect = name_rect(place, &body.name, indicator.0);
            let screen = Rect::from_corners(Vec2::ZERO, viewport);
            if screen.intersect(rect).is_empty() {
                continue;
            }

            // Pointing at a body or picking it out is asking for it by name,
            // which the switch has no more business refusing than it does for
            // a system.
            let pointed_at = pointing
                .get(entity)
                .is_ok_and(|at| at.settled(time.elapsed_secs()));
            // The same question the sky is asked, with the two terms a body
            // cannot answer settled: it is drawn as itself rather than stood
            // in for, so there is no mark whose going takes its name, and the
            // filters are about systems.
            let selected = selection.contains(entity);
            if !worth_naming(
                true,
                show_body_names.0,
                false,
                pointed_at,
                selected,
            ) {
                continue;
            }

            let score = body_name_score(
                indicator.0,
                body.ancestors,
                body.star,
                body.primary,
                pointed_at,
                selected,
            );
            wanted.push((entity, rect, score));
        }
    }

    // Best first, so that what is dropped is dropped in favour of something
    // the viewer wanted more. Ties are settled by which entity it is, which
    // is arbitrary but the same arbitrary answer every frame.
    //
    // Something has to settle them, and the order they were gathered in
    // cannot: a body drawn too small to measure scores exactly the floor, so
    // a system's moons are routinely tied, and handing one of them a `Named`
    // moves it to another archetype and so to another place in the order they
    // are read in. Two names would then trade one place between them every
    // frame, which is a flicker rather than a choice.
    wanted.sort_unstable_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));

    let mut kept: Vec<Rect> = Vec::new();
    let mut winners = EntityHashSet::default();
    for (entity, rect, _) in wanted {
        if kept.iter().any(|taken| !taken.intersect(rect).is_empty()) {
            continue;
        }
        kept.push(rect);
        winners.insert(entity);
    }

    // Only what changed hands. Nearly every name is the same name it was
    // last frame, and taking one away to give it straight back moves its
    // system between archetypes twice for nothing, and reads as a change to
    // anything watching for one.
    for entity in &named {
        if !winners.contains(&entity) {
            commands.entity(entity).remove::<Named>();
        }
    }
    for entity in winners {
        if !named.contains(entity) {
            commands.entity(entity).insert(Named);
        }
    }
}

/// What being inside the system the camera is in is worth to a name
///
/// Cut into a step for each of [`DEEPEST`], so a star takes nearly the whole
/// of it and each step down takes a step less.
///
/// Above [`CENTER_WEIGHT`] and below [`POINTED_WEIGHT`], which places bodies
/// where they belong in the order. A body outranks the ordinary run of
/// systems, since bodies are only ever drawn while the camera is inside a
/// system and what is in front of the viewer is then what they came to look
/// at. It never outranks what is pointed at or picked out, whichever that is.
const INSIDE_WEIGHT: f32 = 300.;

/// How large a body has to look to be worth half of what its size can argue
///
/// In logical pixels of radius. Around the size a body stops being a dot and
/// starts being a disc, so the bodies that read as worlds are named first and
/// the specks last.
const BODY_NAME_REACH: f32 = 20.;

/// How many steps down a system the ordering tells apart
///
/// A star, its planets, their moons, and whatever a scan puts under those.
/// Deeper than this and everything is as deep as everything else, which costs
/// nothing: the records go four or five down at the most, and the step it
/// would take is smaller than the room a name needs.
const DEEPEST: u8 = 4;

/// How much a body deserves to have its name drawn
///
/// Four claims, each settled before the next is asked.
///
/// The star the system arrives at before anything else in it. It is what the
/// system is named for, what everything else is measured from, and where the
/// camera is sent; a name for it is the one name that says which system this
/// is. It takes a rank of its own above the deepest, so nothing below can
/// reach it however large it draws or however shallow it sits.
///
/// Then a parent before whatever goes round it, whatever the two are drawn at:
/// a star before its planets, and a planet before its moons. A moon the camera
/// happens to be beside is drawn larger than the star at the middle of the
/// system and is not more worth naming than it. `under` is how many ancestors
/// the scan named it under, which counts up the same way.
///
/// Then a star, which the depth cannot settle. A system's stars and the
/// planets that go round the pair of them all hang off the point in the
/// middle and count the same ancestors, and a star is what the system is
/// named for.
///
/// Then its own apparent size. Bigger first, which is nearly always the order
/// the viewer would have chosen: the worlds before the specks.
///
/// The four are nested rather than added together. [`INSIDE_WEIGHT`] is cut
/// into a step per depth and one more for the arrival star, being a star takes
/// half a step, and the size argues over what is left, so nothing carries
/// enough to cross a claim above it. The whole of it stays inside
/// [`INSIDE_WEIGHT`], so nothing inside a system outranks whatever is pointed
/// at or picked out.
fn body_name_score(
    apparent: f32,
    under: u8,
    star: bool,
    primary: bool,
    pointed_at: bool,
    selected: bool,
) -> f32 {
    // A rank apiece for the depths, and one over them all for the star the
    // system arrives at.
    let step = INSIDE_WEIGHT / (DEEPEST + 2) as f32;
    let rank = if primary { DEEPEST + 1 } else { DEEPEST - under.min(DEEPEST) };
    let depth = rank as f32 * step;
    let sun = if star { step / 2. } else { 0. };
    let size = (step / 2.) * apparent / (apparent + BODY_NAME_REACH);

    marked_score(pointed_at, selected) + depth + sun + size
}

/// How much a system deserves to have its name drawn
///
/// Being marked out settles it outright, and being selected outranks being
/// under the pointer. Failing either, one question: how far the system is
/// from the center the camera orbits.
///
/// A sharp term picks out the system at the center itself, and a flat one
/// puts everything else in order of nearness to it. Strictly falling, so
/// names are awarded nearest first and the closest system to the center is
/// always the first offered a place.
///
/// Measured from the center and not from the camera. Everything worth naming
/// sits about one orbit radius from the eye, whichever side of the center it
/// is on, so a distance measured from there is near enough the same for all
/// of them and separates nothing.
///
/// Nothing about how notable a system is enters here. What makes one worth
/// picking out of a crowd is what makes its star draw larger, and that is
/// [`super::scale`]'s to decide: it is population today and a setting the
/// viewer turns off by default. A name that argued from population while
/// every star was drawn the same size was answering a question nobody had
/// asked. Should prominence earn a name, it should be the same prominence
/// that earns a star its size, so that both follow whatever that becomes.
fn name_score(from_center: f32, pointed_at: bool, selected: bool) -> f32 {
    let centered = CENTER_WEIGHT / (1. + (from_center / CENTER_REACH).powi(2));
    marked_score(pointed_at, selected) + centered - from_center
}

/// What being marked out is worth, to a system and a body alike
///
/// The greater of the two rather than the sum: a system both pointed at and
/// picked out is asked for once, and adding them would put it above a pair of
/// systems that between them are asked for twice.
fn marked_score(pointed_at: bool, selected: bool) -> f32 {
    let pointed = if pointed_at { POINTED_WEIGHT } else { 0. };
    let picked = if selected { SELECTED_WEIGHT } else { 0. };

    picked.max(pointed)
}

/// The screen rectangle a name would occupy, with room around it
///
/// The width is a guess from the letter count, since the mesh that would
/// give an exact one is the thing being decided about.
///
/// `clear` is the radius of the mark drawn around whatever is being named, in
/// pixels, which the name stands off rather than overlapping. What is drawn
/// there is a ring around a system picked out or pointed at, and the shell
/// inside that ring; up close either is far wider than the gap a name is
/// otherwise given, and a name laid over its own system is a name that cannot
/// be read.
pub(super) fn name_rect(at: Vec2, name: &str, clear: f32) -> Rect {
    let size = NAME_HEIGHT;
    let width = name.chars().count() as f32 * ADVANCE * size;
    let margin = size * CROWDING;

    // `face_camera` puts a name up and to the right of what it names by this
    // same standoff and these same multiples of its height.
    let left = at.x + clear + size * GAP;
    let middle = at.y - size * RISE;

    Rect::new(
        left - margin,
        middle - size / 2. - margin,
        left + width + margin,
        middle + size / 2. + margin,
    )
}

/// Give a label to every system that has won a name, and take it from the
/// rest
///
/// [`choose_names`] decides; this only carries the decision out. A system
/// without a [`Named`] has no label, which is what keeps the mesh cost to
/// the names actually drawn, and means nothing has to be hidden after the
/// fact.
///
/// A name already up is set again where the words have moved on. What a
/// system's plate says is not its name alone: a stop carries the jump to it,
/// and a system becomes a stop and stops being one without ever losing the
/// name it holds. Left alone, a plate would go on saying whatever was true the
/// frame it was spawned.
#[allow(clippy::too_many_arguments)]
pub fn respawn(
    mut commands: Commands,
    named: Query<Plated, With<Named>>,
    // Where the jump to a stop is measured from, which is the system the
    // camera is standing in. Asked of every system rather than only the ones
    // without names, so that the answer is the same one [`choose_names`]
    // granted room for on the frame the camera arrives and the system it is
    // arriving in has not yet let go of its own name.
    standing_in: Query<&System>,
    seen_as: Res<ApparentSize>,
    named_bodies: Query<(Entity, &Body, Option<&Children>), With<Named>>,
    // Whatever lost its name since this last ran, rather than everything that
    // does not have one. Nearly every name is the same name it was last frame,
    // and asking the other way round is a walk of the whole sky, and of every
    // child hung under it, to find the handful with a plate to take down.
    mut unnamed: RemovedComponents<Named>,
    children_of: Query<&Children>,
    labels: Query<Entity, With<Label>>,
    // The words on a plate already up, which only a system's ever change.
    mut plates: Query<&mut Text3d, With<Label>>,
    materials: Res<LabelMaterials>,
) {
    for entity in unnamed.read() {
        // Nothing where the thing itself has gone, which is the other way a
        // name is lost. Its children went with it, plate and all.
        let Ok(children) = children_of.get(entity) else { continue };
        for child in children.iter() {
            if let Ok(label) = labels.get(child) {
                commands.entity(label).despawn();
            }
        }
    }

    // The same nameplate, hung off whatever it names.
    for (entity, body, children) in &named_bodies {
        let labelled = children
            .is_some_and(|c| c.iter().any(|child| labels.contains(child)));
        if labelled {
            continue;
        }

        let label = commands
            .spawn(nameplate(body.name.to_uppercase(), &materials))
            .id();
        commands.entity(entity).add_child(label);
    }

    // Where a jump is measured from. Nothing where the map is holding no
    // system, which is also when nothing is a stop.
    let from = seen_as
        .of()
        .and_then(|held| standing_in.get(held).ok())
        .map(|held| held.position());

    for (entity, system, children, hop) in &named {
        let wanted = plate_for(system, hop, from);

        let plate = children
            .into_iter()
            .flat_map(|children| children.iter())
            .find(|child| labels.contains(*child));

        // Written only where the words moved. Setting a plate again rebuilds
        // its mesh, and this runs over every name every frame.
        if let Some(plate) = plate {
            if let Ok(mut words) = plates.get_mut(plate)
                && said(&words) != Some(wanted.as_str())
            {
                *words = Text3d::new(wanted);
            }
            continue;
        }

        let label = commands.spawn(nameplate(wanted, &materials)).id();

        commands.entity(entity).add_child(label);
    }
}

/// What the plate for `system` says, `stop` being whether a route reaches it
///
/// `from` is where a jump is measured from, which is the system the camera is
/// standing in. Nothing where the map is holding none, which is also when
/// nothing is a stop.
///
/// Asked wherever a name is laid out as well as where one is set, since what
/// is drawn is what has to be given room and what has to be caught over.
pub(super) fn plate_for(
    system: &System,
    stop: bool,
    from: Option<DVec3>,
) -> String {
    let jump =
        from.filter(|_| stop).map(|from| (system.position() - from).length());

    plate_words(&system.name, jump)
}

/// A name, and `jump` beside it where there is one
///
/// A stop says how far the jump to it is. That is the one number the viewer
/// wants of a system they are being told to go to next, and the name alone
/// does not carry it. In light years, as every distance the map states is.
fn plate_words(name: &str, jump: Option<f64>) -> String {
    match jump {
        Some(jump) => format!("{} {jump:.1} Ly", name.to_uppercase()),
        None => name.to_uppercase(),
    }
}

/// What a plate says, if it says anything this wrote
///
/// A plate is one run of text, [`nameplate`] having set it that way, and
/// anything else is not a plate this put up.
pub(super) fn said(words: &Text3d) -> Option<&str> {
    match words.segments.as_slice() {
        [(Text3dSegment::String(said), _)] => Some(said),
        _ => None,
    }
}

/// The name of a thing, drawn beside it
///
/// One plate for a system and for a body alike: what differs is only what it
/// is hung off and what decided it was worth drawing.
///
/// Given the words to set rather than making them. Names are put in capitals,
/// as everything else the map says out loud is, and the font being monospaced
/// that costs no width. A unit standing beside one is not a name and is spelt
/// the way a unit is spelt, so whoever knows which is which says so.
fn nameplate(name: String, materials: &LabelMaterials) -> impl Bundle {
    (
        Label,
        Text3d::new(name),
        Text3dStyling {
            size: SIZE,
            font: FONT.into(),
            color: Srgba::WHITE,
            // The anchor says where the text sits relative to the entity,
            // not which edge of the text lands on it. CENTER_RIGHT puts the
            // name to the right of what it names rather than straddling it,
            // leaving room for the gap below.
            anchor: TextAnchor::CENTER_RIGHT,
            ..default()
        },
        Mesh3d::default(),
        // Whatever the thing is; `tint_marked_names` runs after this and
        // settles it before the name is drawn.
        MeshMaterial3d(materials.get(Tint::Resting, false).clone()),
        // Placed by `face_camera` before the first draw.
        Transform::default(),
        // What catches the pointer over a name. The area is worked out on
        // screen by `super::pointing`, from the same rectangle
        // `choose_names` laid this name out in, so a name catches over
        // exactly the room it was granted rather than over the quads its
        // glyphs happen to occupy.
        Pickable::default(),
    )
}

/// Turn each label to the camera and place it beside its system
///
/// A label is a child of the system it names, which carries neither a size
/// nor a rotation of its own, so everything written here is what the label
/// is drawn with. That a system is never rotated is what lets the camera's
/// rotation be written straight into a slot that is read as local.
pub fn face_camera(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<(&System, &Indicator), Without<Label>>,
    // `Without<System>` is already true of any label. It is spelled out so
    // the scheduler can prove this query is disjoint from the one above, and
    // from every other system that reads a star's transform.
    bodies: Query<(&GlobalTransform, &Indicator), Without<Label>>,
    eye_at: Query<&GlobalTransform, (With<Camera>, Without<Label>)>,
    mut labels: Query<
        (&mut Transform, &ChildOf),
        (With<Label>, Without<System>),
    >,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (mut label, child_of) in &mut labels {
        let Ok((system, indicator)) = systems.get(child_of.parent()) else {
            // A name hung off something inside a system, which knows where it
            // is relative to the camera rather than where it is in the
            // galaxy.
            let Ok(eye) = eye_at.single() else { continue };
            let Ok((at, indicator)) = bodies.get(child_of.parent()) else {
                continue;
            };

            let offset = (at.translation() - eye.translation()).as_dvec3();
            let into_view = depth_of(orbit, offset).max(MIN_DEPTH);
            let world_per_pixel =
                world_per_pixel(cot_half_fov, viewport.y, into_view);

            let height = NAME_HEIGHT * world_per_pixel;
            // Clear of the body itself rather than a fixed step from its
            // middle. A body is drawn at the size it is, so a name set the
            // gap a system's name is set at would sit inside anything larger
            // than a speck. Measured from the mark, which is the outline the
            // pointer is tested against, so the name stands off exactly what
            // is drawn.
            let clear = indicator.0 * world_per_pixel;
            let offset = orbit.rotation * Vec3::X * (clear + height * GAP)
                + orbit.rotation * Vec3::Y * (height * RISE);

            // The plate is drawn at the body's own scale otherwise, and a
            // body's scale is its radius in metres.
            label.scale = Vec3::splat(height / SIZE / at.scale().x.max(1e-6));
            label.translation = offset / at.scale().x.max(1e-6);
            label.rotation = orbit.rotation;
            continue;
        };

        // Measured to the system, not to the label's offset within it, so
        // that every name on screen is sized against the same view.
        let into_view =
            depth(orbit, DVec3::from(system.position)).max(MIN_DEPTH);
        let world_per_pixel =
            world_per_pixel(cot_half_fov, viewport.y, into_view);

        // The line box is exactly `SIZE` tall, so this is the height the
        // name draws at, in pixels, whatever the camera is doing.
        let height = NAME_HEIGHT * world_per_pixel;
        let scale = height / SIZE;

        // Clear of the mark drawn around the system, as a body's name is
        // clear of the body. Up close that mark is a ring tens of pixels wide
        // with the shell inside it, and a name set a fixed step from the
        // middle would be drawn over both.
        let clear = indicator.0 * world_per_pixel;

        // Offset along the camera's own axes, so the label keeps sitting up
        // and to the right on screen however the view is orbited. All three
        // are pixel measurements taken into the world, so they are fixed
        // pixel gaps.
        let offset = orbit.rotation * Vec3::X * (clear + height * GAP)
            + orbit.rotation * Vec3::Y * (height * RISE);

        label.scale = Vec3::splat(scale);
        label.translation = offset;
        label.rotation = orbit.rotation;
    }
}

/// Join each thing to its name with a line
///
/// The name sits off to one side, which is ambiguous once things are close
/// together. Drawn as a gizmo rather than a mesh because both ends move
/// every frame the camera does, and there is nothing to keep between frames.
///
/// A line only exists where its name is drawn. Names are hidden by the
/// spyglass, by the names toggle, and by facing away from the camera, and a
/// line answering to anything less than the drawn text outlives one of them.
///
/// A system's name and a body's are the same name drawn the same way, so both
/// get one. What differs is only what the line has to begin clear of.
pub fn leaders(
    mut gizmos: Gizmos,
    labels: Query<(&GlobalTransform, &ViewVisibility, &ChildOf), With<Label>>,
    named: Query<Anchor, Or<(With<System>, With<Body>)>>,
    shells: Query<&GlobalTransform, With<Shell>>,
) {
    for (label, drawn, child_of) in &labels {
        if !drawn.get() {
            continue;
        }
        let Ok((at, children, pointed_at, selected, hop)) =
            named.get(child_of.parent())
        else {
            continue;
        };

        // A ring around a system says which one a name belongs to better
        // than a line to it does, and leaves nothing for the line to say.
        // Either ring answers, and a name the color of the ring it belongs
        // to has already said which star it came from.
        //
        // A stop a route reaches is ringed as well, and carries the mark
        // saying which way it lies between the ring and the name, which joins
        // the two of them more plainly than a line drawn under it could.
        if pointed_at || selected || hop {
            continue;
        }

        // What the line has to begin clear of is whatever is drawn where the
        // name points. For a system that is the shell hung under it, which
        // carries the size it is drawn at where the system deliberately does
        // not, and the largest of them where it holds more than one. A body is
        // drawn at its own size and needs nothing looked up, so folding its own
        // scale in as the starting figure answers both: a system's own scale is
        // a metre and loses to any shell, and a body has no shell to lose to.
        let edge = children
            .into_iter()
            .flat_map(|children| children.iter())
            .filter_map(|child| shells.get(child).ok())
            .map(|shell| shell.scale().x)
            .fold(at.scale().x, f32::max);

        // The label's origin is the left edge of the text, so the line runs
        // from the thing straight to where the name begins.
        let from = at.translation();
        let to = label.translation();
        let length = from.distance(to);
        let Some(direction) = (to - from).try_normalize() else { continue };

        // Measured out from the ring rather than from the center, so that
        // the air before the line matches the air after it.
        let gap = length * LEADER_GAP;
        let start = edge + gap;
        let end = length - gap;
        if start >= end {
            continue;
        }

        gizmos.line(
            from + direction * start,
            from + direction * end,
            LEADER_COLOR,
        );
    }
}

pub fn init_materials(
    mut assets: ResMut<Assets<StandardMaterial>>,
    dim: Res<DimTo>,
    mut commands: Commands,
) {
    let mut label = |tint: Srgba| assets.add(name_material(tint));

    commands.insert_resource(LabelMaterials {
        bright: Tint::ALL.map(|tint| label(tint.color())),
        dim: Tint::ALL.map(|tint| label(faded(tint.color(), dim.0))),
    });
}

/// How a name is painted in `tint`
///
/// The glyphs are drawn white and unlit, so a material's base color
/// multiplies straight through them and is what a name comes out.
fn name_material(tint: Srgba) -> StandardMaterial {
    StandardMaterial {
        base_color: tint.into(),
        base_color_texture: Some(TextAtlas::DEFAULT_IMAGE.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    }
}

/// `tint` at `strength` of full
///
/// The color is left alone and the alpha carries it, since a name dimmed by
/// darkening would go black against the sky and read as a hole rather than as
/// something standing further back.
fn faded(tint: Srgba, strength: f32) -> Srgba {
    Srgba { alpha: tint.alpha * strength, ..tint }
}

/// Repaint the dimmed tints when the slider moves
///
/// The handles stay as they are, so no name has to be told which material it
/// is pointing at.
fn redim(
    dim: Res<DimTo>,
    materials: Res<LabelMaterials>,
    mut assets: ResMut<Assets<StandardMaterial>>,
) {
    if !dim.is_changed() {
        return;
    }

    for (handle, tint) in materials.dim.iter().zip(Tint::ALL) {
        if let Some(mut material) = assets.get_mut(handle) {
            *material = name_material(faded(tint.color(), dim.0));
        }
    }
}

/// Tint a name for what its system is
///
/// Keyed on the system rather than on the name, so that pointing at a star
/// lights its name as well, and both go out together.
///
/// Selection wins over pointing where both apply, as it does for the ring:
/// the pointer will move on, and the selection is what was asked for.
///
/// A name dims with the system it belongs to, marked out or not. One rule
/// with no exceptions, and a name that stayed bright over a dimmed star would
/// read as the filter having let go of it.
pub fn tint_marked_names(
    systems: Query<
        (Has<PointedAt>, Has<Selected>, Has<Filtered>),
        With<System>,
    >,
    // What is inside a system answers the same two marks. No filter: a
    // filter is a question asked of systems, and nothing asks it of a body.
    bodies: Query<(Has<PointedAt>, Has<Selected>), With<Body>>,
    materials: Res<LabelMaterials>,
    mut names: Query<
        (&ChildOf, &mut MeshMaterial3d<StandardMaterial>),
        With<Label>,
    >,
) {
    for (child_of, mut material) in &mut names {
        let marked = systems.get(child_of.parent()).ok().or_else(|| {
            bodies
                .get(child_of.parent())
                .ok()
                .map(|(pointed_at, selected)| (pointed_at, selected, false))
        });
        let Some((pointed_at, selected, filtered)) = marked else {
            continue;
        };
        let tint = if selected {
            Tint::Selected
        } else if pointed_at {
            Tint::PointedAt
        } else {
            Tint::Resting
        };

        let wanted = materials.get(tint, filtered);
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stop says the jump to it, and anything else says its name alone
    #[test]
    fn only_a_stop_says_how_far_off_it_is() {
        assert_eq!(plate_words("lung", Some(6.74)), "LUNG 6.7 Ly");
        assert_eq!(plate_words("lung", None), "LUNG");
    }

    /// A plate says back what it was set to
    ///
    /// Which is what tells a plate whose words have moved on from one that is
    /// still saying the right thing, and what the room a name is granted and
    /// the area it is caught over are measured from.
    #[test]
    fn a_plate_says_back_what_it_was_set_to() {
        let resting = plate_words("lung", None);
        let stop = plate_words("lung", Some(6.74));

        assert_eq!(said(&Text3d::new(&resting)), Some(resting.as_str()));
        assert_eq!(said(&Text3d::new(&stop)), Some(stop.as_str()));
        assert_ne!(said(&Text3d::new(&resting)), Some(stop.as_str()));
    }

    /// A camera at the origin, looking the way `Quat::IDENTITY` faces
    fn camera(rotation: Quat) -> OrbitCamera {
        OrbitCamera { eye: DVec3::ZERO, rotation, ..default() }
    }

    /// Two points at the same depth measure the same, however far apart
    ///
    /// This is the whole distinction. Both points below sit a hundred light
    /// years into the view, but the second is a third further from the eye,
    /// and measuring the distance would size it a third larger for it.
    #[test]
    fn depth_ignores_how_far_off_axis_a_point_is() {
        let camera = camera(Quat::IDENTITY);
        let ahead = DVec3::new(0., 0., -100.);
        let corner = DVec3::new(80., 45., -100.);
        // A hundred light years, in the metres depth answers in.
        let hundred = (100. * crate::space::LIGHT_YEAR) as f32;

        assert!((depth(&camera, ahead) - hundred).abs() < hundred * 1e-5);
        assert!((depth(&camera, corner) - hundred).abs() < hundred * 1e-5);
        assert!(
            corner.length() > 130.,
            "the corner point is only {} away, too close to tell the two apart",
            corner.length()
        );
    }

    /// A body asks the same question with nothing standing in for it
    ///
    /// What the bodies pass: always standing, never filtered. The switch then
    /// decides, and being asked for by name beats it.
    #[test]
    fn a_body_is_named_by_the_switch_or_by_being_asked_for() {
        assert!(worth_naming(STANDS, true, false, false, false));
        assert!(!worth_naming(STANDS, false, false, false, false));
        assert!(worth_naming(STANDS, false, false, true, false));
        assert!(worth_naming(STANDS, false, false, false, true));
    }

    /// A system the map is still standing a mark in for
    ///
    /// Which is every system but the one the camera has flown into, so it is
    /// what the tests below are about.
    const STANDS: bool = true;

    /// A system the filters exclude gives up its name
    ///
    /// A name is read or it is not, so one belonging to a dimmed star is not
    /// dimly readable, it is clutter over what the user asked to see.
    #[test]
    fn a_filtered_system_is_not_named() {
        assert!(!worth_naming(STANDS, true, true, false, false));
    }

    /// And keeps it while it is pointed at or picked out
    ///
    /// Either is asking for the system by name, which is the one thing a
    /// name is for.
    #[test]
    fn a_marked_system_is_named_through_a_filter() {
        assert!(worth_naming(STANDS, true, true, true, false));
        assert!(worth_naming(STANDS, true, true, false, true));
    }

    /// The names toggle bars one the same way, and yields the same way
    #[test]
    fn the_names_toggle_bars_and_yields_as_a_filter_does() {
        assert!(!worth_naming(STANDS, false, false, false, false));
        assert!(worth_naming(STANDS, false, false, true, false));
        assert!(worth_naming(STANDS, false, false, false, true));
    }

    /// A system the filters admit is named when names are on, and not when off
    #[test]
    fn an_admitted_system_follows_the_toggle() {
        assert!(worth_naming(STANDS, true, false, false, false));
        assert!(!worth_naming(STANDS, false, false, false, false));
    }

    /// Marked out beats both at once
    #[test]
    fn a_marked_system_is_named_with_everything_against_it() {
        assert!(worth_naming(STANDS, false, true, true, false));
        assert!(worth_naming(STANDS, false, true, false, true));
    }

    /// A system the camera has come inside is not named at all
    ///
    /// Its name is part of the mark standing in for it, and there is no mark
    /// left: what is drawn there is the system itself, with the things in it
    /// carrying their own names.
    #[test]
    fn a_system_the_camera_is_inside_is_not_named() {
        assert!(!worth_naming(false, true, false, false, false));
        assert!(!worth_naming(false, true, false, true, false));
        assert!(!worth_naming(false, true, false, false, true));
    }

    /// A name is laid out clear of the mark drawn around what it names
    ///
    /// A ring around a system picked out or pointed at is drawn at exactly
    /// that mark, with the shell inside it, and up close the mark runs to
    /// hundreds of pixels. A name given the same gap at every zoom would be
    /// laid over both.
    #[test]
    fn a_name_stands_off_the_mark_around_it() {
        let at = Vec2::new(500., 300.);

        for mark in [0., 9.5, 40., 300.] {
            let rect = name_rect(at, "SOL", mark);

            assert!(
                rect.min.x > at.x + mark,
                "a mark {mark} wide left the name starting at {}",
                rect.min.x - at.x
            );
        }
    }

    /// A parent is named before whatever goes round it
    ///
    /// Whatever the two are drawn at. A moon the camera happens to be beside
    /// is drawn larger than the star at the middle of the system, and the
    /// star is still what the system is named for.
    ///
    /// Every step down, so this covers a planet over its moons as much as a
    /// star over its planets. Each is drawn at nothing against the one below
    /// it filling the view, which is the hardest way round for it to hold.
    #[test]
    fn a_parent_is_named_before_what_goes_round_it() {
        for under in 0..DEEPEST {
            let parent = body_name_score(0., under, false, false, false, false);
            let child =
                body_name_score(1e4, under + 1, false, false, false, false);

            assert!(
                parent > child,
                "one {under} down scored {parent} against the {child} of one \
                 under it"
            );
        }
    }

    /// A star is named before a planet that goes round the pair of it
    ///
    /// Which the depth cannot settle. Shinrarta Dezhra's two stars and its
    /// three outer planets all hang off the point at the middle and count one
    /// ancestor apiece, and the stars are what the system is named for.
    #[test]
    fn a_star_is_named_before_its_own_siblings() {
        // The star drawn at nothing, and the planet filling the view.
        let star = body_name_score(0., 1, true, false, false, false);
        let planet = body_name_score(1e4, 1, false, false, false, false);

        assert!(star > planet, "the star scored {star} against {planet}");
    }

    /// And a star still gives way to what it goes round
    ///
    /// The depth is asked before anything else, so a star out at the second
    /// step is under a planet at the first however either is drawn. Nothing on
    /// record looks like that, and the ordering says so rather than leaving it
    /// to how the terms happen to add up.
    #[test]
    fn depth_is_asked_before_being_a_star() {
        let deeper = body_name_score(1e4, 2, true, false, false, false);
        let higher = body_name_score(0., 1, false, false, false, false);

        assert!(higher > deeper, "{higher} did not beat {deeper}");
    }

    /// Past the last step everything is as deep as everything else
    ///
    /// The records go four or five down at the most, and a step small enough
    /// to tell the rest apart is smaller than the room a name takes.
    #[test]
    fn the_deepest_step_is_the_last_one_told_apart() {
        let deep = body_name_score(0., DEEPEST, false, false, false, false);
        let deeper =
            body_name_score(0., DEEPEST + 3, false, false, false, false);

        assert_eq!(deep, deeper);
    }

    /// At one depth, the larger is named first
    #[test]
    fn the_larger_of_two_at_one_depth_is_named_first() {
        let world = body_name_score(1e4, 2, false, false, false, false);
        let speck = body_name_score(0.1, 2, false, false, false, false);

        assert!(world > speck, "the world scored {world} against {speck}");
    }

    /// The star a system arrives at outranks everything else in the system
    ///
    /// Whatever the rest have going for them: the shallowest place, being
    /// stars themselves, and drawing as large as a body ever does. Its name
    /// is the one that says which system this is.
    #[test]
    fn the_arrival_star_outranks_the_whole_system() {
        let primary = body_name_score(0., 0, true, true, false, false);

        // The best anything else can do, which is a second star sitting as
        // shallow as the arrival star and filling the view.
        let best = body_name_score(1e9, 0, true, false, false, false);
        assert!(
            primary > best,
            "the arrival star scored {primary}, under the {best} of another"
        );
    }

    /// The arrival star wins a pair the rest of the order cannot separate
    ///
    /// Shinrarta Dezhra is the case: two stars, both hanging off the point in
    /// the middle so both are named one ancestor down, both stars, and both
    /// drawn at the floor a mark cannot go under while the whole system is on
    /// screen. Every term but the arrival tied exactly, so which of the two
    /// names was drawn came down to which row the database returned first.
    #[test]
    fn the_arrival_star_wins_a_pair_nothing_else_separates() {
        // What `pointing` will not draw a mark under, which both stars sit at
        // from anywhere the whole system is in view.
        let floor = 4.;
        let arrival = body_name_score(floor, 1, true, true, false, false);
        let other = body_name_score(floor, 1, true, false, false, false);

        assert!(
            arrival > other,
            "the arrival star scored {arrival} against the {other} of the \
             star beside it"
        );
    }

    /// And all of it gives way to what is pointed at or picked out
    ///
    /// Being marked out is the user asking for something by name, which is
    /// the one thing a name is for, and it beats every claim a thing makes
    /// for itself.
    #[test]
    fn a_marked_body_outranks_a_star_at_rest() {
        let star = body_name_score(1e4, 0, true, false, false, false);

        assert!(body_name_score(0., DEEPEST, false, false, true, false) > star);
        assert!(body_name_score(0., DEEPEST, false, false, false, true) > star);
    }

    /// Names are offered in order of nearness to the center
    ///
    /// The score falls the whole way out, so the greedy pass takes the
    /// nearest system first and works outwards. Nothing further away can
    /// take a place from something closer, which is what makes the ordering
    /// something a viewer can predict rather than a ranking to be read.
    #[test]
    fn nearer_systems_are_always_offered_a_name_first() {
        let mut nearer = name_score(0., false, false);
        for step in 1..=1000 {
            let further = name_score(
                step as f32 * DEFAULT_NAME_RADIUS / 1000.,
                false,
                false,
            );
            assert!(
                further < nearer,
                "a system {}ly out scored {further}, beating the {nearer} \
                 of one closer in",
                step as f32 * DEFAULT_NAME_RADIUS / 1000.
            );
            nearer = further;
        }
    }

    /// A spyglass of a given reach, clearing unless said otherwise
    fn spyglass(radius: f32, clear: bool) -> Spyglass {
        Spyglass {
            fetch: true,
            radius,
            clear,
            lock_camera: false,
            follow_camera: false,
        }
    }

    /// Names reach no further than the spyglass shows
    ///
    /// A system the spyglass has hidden has nothing to put a name against,
    /// so one drawn for it would build a mesh, hold its place against the
    /// others, and never appear.
    #[test]
    fn names_reach_no_further_than_the_spyglass() {
        let asked = NameRadius { follow_spyglass: false, radius: 200. };

        assert_eq!(asked.reach(&spyglass(30., true)), 30.);
    }

    /// Following takes the spyglass's answer whatever it is
    #[test]
    fn following_the_spyglass_takes_its_reach() {
        let following = NameRadius { follow_spyglass: true, radius: 5. };

        for radius in [7., 30., 4_000.] {
            assert_eq!(following.reach(&spyglass(radius, true)), radius);
        }
    }

    /// A spyglass that does not clear lets names be asked for beyond it
    ///
    /// Everything loaded is drawn then, so there is nothing left for the
    /// spyglass to say about which of it may be named.
    #[test]
    fn a_spyglass_that_does_not_clear_lifts_the_ceiling() {
        let asked = NameRadius { follow_spyglass: false, radius: 200. };

        assert_eq!(asked.reach(&spyglass(30., false)), 200.);
    }

    /// What the pointer is on takes the top of the order
    ///
    /// The best score is kept before anything has been placed, so the name
    /// under the pointer is always drawn, and whatever would have overlapped
    /// it gives way. It has to beat the system at the center to do that,
    /// which is the only other claim strong enough to matter.
    #[test]
    fn what_is_pointed_at_outranks_what_is_centered() {
        // Pointed at, and as far out as a name is ever drawn.
        let pointed = name_score(DEFAULT_NAME_RADIUS, true, false);

        // The system at the center, which is otherwise the best there is.
        let centered = name_score(0., false, false);

        assert!(
            pointed > centered,
            "pointed at scored {pointed}, behind the {centered} at the center"
        );
    }

    /// What is selected outranks what the pointer is on
    ///
    /// Where the two names would overlap, one of them is dropped, and it
    /// should be the one that goes away by itself when the pointer moves.
    /// Measured with the selection as far out as a name is ever drawn and
    /// the point on the center, so nothing but the two claims decides it.
    #[test]
    fn what_is_selected_outranks_what_is_pointed_at() {
        let selected = name_score(DEFAULT_NAME_RADIUS, false, true);
        let pointed = name_score(0., true, false);

        assert!(
            selected > pointed,
            "selected scored {selected}, behind the {pointed} of a point"
        );
    }

    /// Pointing at what is already selected does not unseat it
    ///
    /// The two claims are one claim, since both are the same system being
    /// marked out, and the ring and the name it earns are drawn the color
    /// of the selection either way.
    #[test]
    fn pointing_at_a_selection_leaves_it_where_it_is() {
        let both = name_score(DEFAULT_NAME_RADIUS, true, true);
        let selected = name_score(DEFAULT_NAME_RADIUS, false, true);

        assert_eq!(both, selected);
    }

    /// The center bonus falls away with distance from what is centered
    ///
    /// Otherwise every system in the neighbourhood would inherit the claim
    /// of the one at the middle of it, and the sharp term would be doing
    /// the flat term's job.
    #[test]
    fn the_center_bonus_is_local_to_the_center() {
        let bonus = |d: f32| CENTER_WEIGHT / (1. + (d / CENTER_REACH).powi(2));

        assert!(bonus(CENTER_REACH) < bonus(0.) * 0.6);
        assert!(
            bonus(CENTER_REACH * 10.) < CENTER_WEIGHT * 0.05,
            "a system ten reaches out still held {} of the bonus",
            bonus(CENTER_REACH * 10.)
        );
    }

    /// A pixel covers the visible world at that depth, divided by the height
    #[test]
    fn a_pixel_covers_its_share_of_the_view() {
        let fov = std::f32::consts::FRAC_PI_4;
        let cot = 1. / (fov / 2.).tan();
        let depth = 100.;

        // What the viewport spans at that depth, from the field of view
        // alone, is what its pixels divide between them.
        let visible = 2. * depth * (fov / 2.).tan();
        let expected = visible / 1080.;

        assert!(
            (world_per_pixel(cot, 1080., depth) - expected).abs() < 1e-6,
            "a pixel covered {}, not {expected}",
            world_per_pixel(cot, 1080., depth)
        );
    }

    /// A label sized this way holds its apparent size at any depth
    ///
    /// This is what #58 asks for. What the eye sees is the world size over
    /// the depth, and the world size is proportional to depth, so the depth
    /// cancels and the same number of pixels is left at every range.
    #[test]
    fn a_label_holds_its_apparent_size_at_any_depth() {
        let (cot, height, pixels) = (2.414, 1080., 16.);
        let apparent = |d: f32| pixels * world_per_pixel(cot, height, d) / d;

        let near = apparent(1.);
        for depth in [10., 1_000., 100_000.] {
            assert!(
                (apparent(depth) - near).abs() < 1e-9,
                "{depth}ly away it subtended {}, against {near} at one",
                apparent(depth)
            );
        }
    }

    /// A point behind the camera measures negative
    #[test]
    fn depth_is_negative_behind_the_camera() {
        let camera = camera(Quat::IDENTITY);
        assert!(depth(&camera, DVec3::new(0., 0., 100.)) < 0.);
    }

    /// Whatever the camera orbits sits exactly its own radius deep
    ///
    /// `orbit_camera` places the eye at `center + rotation * Z * radius`, so
    /// this pins the helper to the convention the camera is written to. A
    /// forward of `+Z` would put the center behind the camera instead.
    #[test]
    fn depth_agrees_with_where_the_camera_puts_its_eye() {
        let rotation = Quat::from_euler(EulerRot::YXZ, 0.9, -0.4, 0.);
        let center = DVec3::new(1234.5, -678.9, 4321.);
        let radius = 250f32;
        let eye = center + (rotation * Vec3::Z * radius).as_dvec3();

        let camera = OrbitCamera { eye, rotation, ..default() };
        // The radius is a distance the camera is set up in, which is light
        // years; the depth comes back in the metres it is drawn in.
        let expected = (radius as f64 * crate::space::LIGHT_YEAR) as f32;
        assert!(
            (depth(&camera, center) - expected).abs() < expected * 1e-5,
            "the center measured {} deep, not the {expected} the camera sits at",
            depth(&camera, center)
        );
    }

    /// A world holding the colors a plate is set in, and nothing else
    ///
    /// Nothing is being flown into, so no system is the one the map is holding
    /// and no plate says a jump.
    fn plated() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<StandardMaterial>>();
        app.init_resource::<DimTo>();
        app.init_resource::<ApparentSize>();
        app.add_systems(Startup, init_materials);
        app
    }

    /// Take the name off whatever holds one, as [`choose_names`] does
    ///
    /// By command, which is the whole of what is being tested in
    /// [`a_name_dropped_this_frame_is_answered_this_frame`]: a removal has to
    /// have landed and been recorded before [`respawn`] reads for it.
    fn drop_names(named: Query<Entity, With<Named>>, mut commands: Commands) {
        for entity in &named {
            commands.entity(entity).remove::<Named>();
        }
    }

    /// How many plates are up
    fn up(app: &mut App) -> usize {
        let mut labels =
            app.world_mut().query_filtered::<Entity, With<Label>>();
        labels.iter(app.world()).count()
    }

    /// A system that has won a name
    fn winner(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((crate::systems::tests::named(1, "Sol"), Named))
            .id()
    }

    /// A system that has won a name is given a plate
    ///
    /// What the rest of these rest on. A system without a [`Named`] has no
    /// label at all, so there is nothing to hide and no mesh built for a name
    /// that would not be read.
    #[test]
    fn a_system_that_wins_a_name_is_given_a_plate() {
        let mut app = plated();
        app.add_systems(Update, respawn);
        winner(&mut app);

        app.update();

        assert_eq!(up(&mut app), 1, "the winner went unnamed");
    }

    /// And keeps it for as long as it holds the name
    ///
    /// The other half of what the plate is looked up for. A plate torn down
    /// and put back would rebuild its mesh every frame.
    #[test]
    fn a_system_that_keeps_its_name_keeps_its_plate() {
        let mut app = plated();
        app.add_systems(Update, respawn);
        winner(&mut app);

        app.update();
        app.update();

        assert_eq!(up(&mut app), 1, "the plate was put up twice over");
    }

    /// A system that loses its name loses its plate
    #[test]
    fn a_system_that_loses_its_name_loses_its_plate() {
        let mut app = plated();
        app.add_systems(Update, respawn);
        let system = winner(&mut app);

        app.update();
        app.world_mut().entity_mut(system).remove::<Named>();
        app.update();

        assert_eq!(up(&mut app), 0, "the plate outlived the name");
    }

    /// A name taken away this frame is answered this frame
    ///
    /// [`choose_names`] takes a name away by command and this runs after it in
    /// the same frame, so the removal has landed and been recorded by the time
    /// it is read for. Answered a frame late, every name the layout dropped
    /// would be drawn once more over whatever took its place.
    #[test]
    fn a_name_dropped_this_frame_is_answered_this_frame() {
        let mut app = plated();
        app.add_systems(Update, respawn);
        winner(&mut app);

        // Up first, so that what the next frame does is take it down.
        app.update();
        assert_eq!(up(&mut app), 1);

        app.add_systems(Update, drop_names.before(respawn));
        app.update();

        assert_eq!(up(&mut app), 0, "the plate outlived the frame it was lost");
    }

    /// A system despawned outright takes its plate with it
    ///
    /// The other way a name is lost, and the one where there is nothing left
    /// to look up: the children went with the system.
    #[test]
    fn a_system_that_goes_takes_its_plate_with_it() {
        let mut app = plated();
        app.add_systems(Update, respawn);
        let system = winner(&mut app);

        app.update();
        app.world_mut().entity_mut(system).despawn();
        app.update();

        assert_eq!(up(&mut app), 0, "the plate outlived its system");
    }
}
