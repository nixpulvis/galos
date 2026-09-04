use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::bodies::spawn::{Body, HeldSystem, Places, Strength};
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::pointing::{INDICATOR, Indicator, PointedAt};
use crate::systems::scale::View;
use crate::systems::selection::{SELECTION, Selected};
use crate::systems::spawn::{ShowNames, StarExposure};
use crate::systems::{Spyglass, System};
use bevy::ecs::entity::EntityHashSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use galos_photometry::{Distance, Magnitude};
use std::ops::RangeInclusive;

pub(crate) fn plugin(app: &mut App) {
    app.insert_resource(NameRadius {
        follow_spyglass: false,
        radius: DEFAULT_NAME_RADIUS,
    });
    app.insert_resource(NameLimit(8.0));
    app.insert_resource(ShowBodyNames(true));
    // Chosen and realised: [`choose_names`] decides which names are drawn and
    // [`respawn`] hangs a token off each, carrying the words. Both in
    // `Update`, before the screen-space paint.
    app.add_systems(
        Update,
        (choose_names, respawn)
            .chain()
            .in_set(MapSet::Present)
            // Both read which system is pointed at, decided this frame.
            .after(super::pointing::point_at)
            // A selected system is named whether or not it is within reach,
            // but only while it is drawn, and that is decided here.
            .after(super::visibility)
            .after(super::scale::size_by_distance)
            .after(super::scale::size_photometrically)
            // A name stands off the mark drawn around what it names, so it
            // wants the mark settled this frame rather than last.
            .after(super::pointing::size_indicators),
    );
    // The names are painted flat, in egui's own pass, from the tokens
    // [`respawn`] put up. Before the chrome so the label layer is registered
    // first and sits beneath the panes rather than over them.
    app.add_systems(
        EguiPrimaryContextPass,
        draw_names.before(crate::ui::chrome),
    );
}

/// How far a name is drawn from what the camera looks at, to begin with
///
/// What [`NameRadius`] starts at, and what the tests measure against.
///
/// Twenty light years is a few dozen systems around whatever is being looked
/// at, which is what a name is for: reading where you are, not labelling the
/// whole of what is loaded. The spyglass reaches much further and should, the
/// stars being worth drawing long after their names would be worth reading.
const DEFAULT_NAME_RADIUS: f32 = 20.;

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
    /// Off to begin with. The spyglass answers how much of the galaxy is
    /// drawn, which is not the same question as how much of it is worth
    /// naming: it reaches hundreds of light years, and names read at that
    /// range are a wall of them over a field nobody is looking at.
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

/// The faintest a star may look and still be named, in the realistic view
///
/// Apparent magnitude, the astronomer's backwards scale: a smaller number is a
/// brighter star, so turning this down names fewer of them, the brightest
/// first. The realistic view's answer to [`NameRadius`] — where the map view
/// holds names to a reach about the center, the sky holds them to a brightness,
/// since that is what a name is worth there. A star still has to be drawn to be
/// named, so past the exposure's floor this only takes names away from what is
/// drawn; it never adds any.
#[derive(Resource)]
pub struct NameLimit(pub f32);

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
/// drawing a glyph. The font is monospaced, so every glyph advances the same
/// and a name of `n` letters is exactly `n` of these across, whichever letters
/// they are. That is also why the names may be drawn in capitals for nothing:
/// a `W` takes the room an `i` does.
///
/// Above the font's own advance of `1233/2048`, so that it errs wide. Erring
/// wide costs a gap between two names, and with it a third name that would
/// have fitted between them; erring narrow overlaps them, which is the thing
/// being prevented.
const ADVANCE: f32 = 0.7;

/// The dark ground a name is set on
///
/// A name is read over a field of stars, and a word whose counters and the
/// gaps between its letters are full of them has no shape to recognise. The
/// ground is what makes the word a figure again.
///
/// Solid, and it has to be. Blended, a ground is ordered against its own
/// words by which mesh is further off, bevy measuring that to the middle of
/// each; a ground's middle is half a name to the side of the words it carries,
/// so the two are apart sideways as well as in depth and the sideways part
/// swings as the camera turns. The order flips mid-rotation and a dark ground
/// over white letters greys them out.
///
/// Depth cannot settle it, and pushing the ground back to try is what went
/// wrong before: far enough back to beat the swing is far enough for
/// perspective to drag it toward the middle of the view, so a name near the
/// edge of the screen wears its ground low and off to the side. Opaque takes
/// the question away with no setback at all. Opaque geometry is drawn before
/// anything blended and writes depth as it goes; the words are blended, test
/// `GreaterEqual`, and clear a ground at their own depth, so the two are left
/// in the one plane and the ground never drifts off the words it carries.
const GROUND: Srgba = Srgba::new(0.03, 0.03, 0.05, 1.);

/// How far the ground reaches past the words, as a fraction of [`NAME_HEIGHT`]
///
/// Enough that the letters are not set against its edge, and no more. A ground
/// wider than the word it carries reads as a box on the map rather than as the
/// word standing clear of what is behind it.
const GROUND_PAD: f32 = 0.3;

/// How much of a name's own height is kept clear around it
///
/// Names that merely touch are still hard to read apart. This is the tight
/// figure, which is what a name gets up close and what it gets at the center
/// of the view at any distance. [`room`] is what widens it elsewhere.
const CROWDING: f32 = 0.35;

/// How far the camera stands off before names are given any more room, in
/// light years
///
/// Below this the map is among a handful of systems, where [`CROWDING`] alone
/// is all the clearance a name wants.
const TIGHT_TO: f32 = 50.;

/// Where names are given all the extra room, in light years
///
/// Three times [`TIGHT_TO`], not ten. The room is eased over `log10`, so a
/// wide gap between the two ends spends most of the range part way: at five
/// hundred the ramp stood at a fifth of the way by a hundred light years and
/// under half by a hundred and fifty, which is where the map is actually
/// flown, so most of the spread was never reaching the view that needed it.
const LOOSE_FROM: f32 = 150.;

/// How many times [`CROWDING`]'s room a name is given at the far end
///
/// A judgement about what a star field should look like rather than something
/// derived, so it wants looking at with the map running. Measured over a six
/// hundred name field at full spread, this places 63 of them: 42% of what the
/// middle offers, 15% of the band around it and 4% at the edge. Twenty four
/// placed 97, which read as a wall.
const SPREAD_BY: f32 = 48.;

/// How far the tightly packed middle reaches, as a fraction of the viewport's
/// height
///
/// The room a name is given relaxes toward the loose figure over this
/// distance, so the middle is a well around what the camera is pointed at
/// rather than a disc with a rim.
///
/// Where [`relaxed`] turns over, so the plateau reaches about this far and the
/// room is nearly all given up by twice it. Two thirds of the height puts the
/// turn around the middle of the way out, which leaves a dense core, a
/// thinning band around it and a sparse edge.
const RELAX: f32 = 0.7;

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

/// Where a system stands relative to what its view names by
///
/// The map view and the realistic view cut the sky on different questions, so
/// each hands [`worth_placing`] its own.
enum Placement {
    /// Map view: within a reach about the center, in light years
    Reach { from_center: f32, reach: f32 },
    /// Realistic view: bright enough to be drawn — apparent magnitude below
    /// the exposure's `floor` — and no fainter than the naming `limit`
    Bright { apparent: f32, floor: f32, limit: f32 },
}

/// Whether a system is near or bright enough to lay a name out for, absent a
/// mark asking for it outright
///
/// Where the two views part. The map view holds names to a neighborhood about
/// the center, since there every star is the same size and nearness is the
/// whole of the ordering. The realistic view holds them to a brightness: a
/// star has to be drawn — apparent magnitude below the exposure floor — to
/// have a mark to name at all, and no fainter than [`NameLimit`], the dial
/// that turns the sky's names down the way [`NameRadius`] turns the map's
/// down. A bright star deep along the line of sight is named where a fixed
/// reach would have dropped it; a faint one is left out however near it sits.
/// What is resident is already bounded by the spyglass, so this is not the
/// whole sky.
fn worth_placing(placement: Placement) -> bool {
    match placement {
        Placement::Reach { from_center, reach } => from_center <= reach,
        Placement::Bright { apparent, floor, limit } => {
            apparent < floor && apparent <= limit
        }
    }
}

/// How strongly standing in front of the middle argues for a name
///
/// A name is drawn over whatever is behind it, so of two systems along nearly
/// one line of sight the near one is the one worth naming: its name covers
/// the field beyond it, where naming the far one lays a name over the near
/// stars as well and hides what is in front.
///
/// In the same light years as everything else, and at one it exactly answers
/// the penalty a system pays for standing off the middle. A system directly
/// in front of the middle therefore scores as one at the middle does, and one
/// directly behind pays that distance twice.
const NEARER_WEIGHT: f32 = 1.;

/// How far the center bonus reaches, in light years
///
/// It falls to half at this distance. The point the camera orbits is usually
/// a system exactly, so this only has to forgive one that is merely near it.
const CENTER_REACH: f32 = 2.;

/// How much a magnitude of brightness above the drawn floor is worth to a
/// system's name in the realistic view
///
/// There a star is drawn at a size of `PSF_GROWTH·ln(energy)` (see
/// [`super::scale`]), and `ln(energy)` is, up to a constant, the margin by
/// which the star's apparent magnitude clears the exposure's floor. Weighing a
/// name by that margin climbs the same ladder its star's size does, which is
/// the prominence the note on the system weight foresaw a name should follow.
/// Scaled so the drawn range leads the field and capped at [`CENTER_WEIGHT`],
/// so a system's name keeps to the band it occupies when position alone
/// decides it: below a body's ([`INSIDE_WEIGHT`]) and below what is pointed at
/// or picked out ([`POINTED_WEIGHT`]).
const BRIGHT_NAME_WEIGHT: f32 = 20.;

/// How much of the map view's nearness ordering the realistic view keeps
///
/// Brightness leads there; nearness to the middle stays on at this fraction of
/// its strength, ordering stars of a like brightness without overriding a
/// brighter one. Small enough that the whole of it cannot make up a magnitude
/// of [`BRIGHT_NAME_WEIGHT`], so the order is brightness first and position
/// second.
const SECONDARY_NEARNESS: f32 = 0.05;

/// One thing in the running for a name, as [`choose_names`] weighs it
///
/// Spelled out once. Both of the questions it answers are asked of the same
/// five things, and written into the query they read as a wall rather than as
/// a list of claims.
type Candidate<'a, T> = (
    Entity,
    T,
    &'a Strength,
    &'a Visibility,
    &'a Indicator,
    Has<Filtered>,
    Has<crate::systems::route::Hop>,
);

/// A system whose name has won a place on screen
///
/// Awarded by [`choose_names`] and read by [`respawn`], which hangs a name
/// token on whatever has one and takes the token from whatever does not. A
/// name that would not be readable never gets a token at all.
#[derive(Component)]
pub struct Named;

/// A name token: the pointer handle and word-carrier for one drawn name
///
/// Hung off whatever it names. It renders nothing itself — [`draw_names`]
/// paints the words in screen space — and exists so the pointer can be caught
/// over a name and the words kept between the frame a name is chosen and the
/// frame it is drawn.
#[derive(Component)]
pub struct Label;

/// The words a name token is set to
///
/// What [`choose_names`] won a place for, kept on the token so [`draw_names`]
/// can paint it and [`super::pointing`] can size the area that catches the
/// pointer, both from the one string.
#[derive(Component)]
pub struct PlateText(pub String);

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

/// The egui layer the map's own annotations are painted into
///
/// One background layer for the rings, the names, their grounds and the
/// leaders alike: a single painter list shared by [`draw_names`] and the ring
/// systems, filled in the order those systems run — the rings first, beneath
/// the grounds — so nothing about the stacking is left to how egui happens to
/// order two separate layers. Background, so the whole of it sits under the
/// chrome and over the map.
pub(crate) fn annotations_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Background, egui::Id::new("map-annotations"))
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

/// What the realistic view weighs a name by, beyond where it stands
///
/// Whether that view is drawn at all, and the floor its stars are measured
/// from, which together say how bright each system looks and so — through
/// [`BRIGHT_NAME_WEIGHT`] — how much its brightness is worth to its name.
/// Bundled so [`choose_names`] stays within Bevy's system parameter limit.
#[derive(bevy::ecs::system::SystemParam)]
pub(crate) struct Sky<'w> {
    view: Res<'w, View>,
    exposure: Res<'w, StarExposure>,
    limit: Res<'w, NameLimit>,
}

/// Decide which systems get to show their name
///
/// In the map view, held to whichever is nearer of [`NameRadius`] and the
/// spyglass, since a system the spyglass has hidden has nothing to put a name
/// against. In the realistic view the cut is brightness instead: any star
/// drawn — one that clears the exposure's floor — may be named wherever it
/// stands, since there brightness is what a name is worth (see [`name_score`]).
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
/// The best win — nearest in the map view, brightest in the realistic one —
/// overlap something already kept. Greedy rather than optimal: the best
/// arrangement of a few hundred overlapping rectangles is not worth solving
/// each frame, and taking them in order of what the viewer most wants to see
/// gives them the ones that matter.
pub(crate) fn choose_names(
    mut commands: Commands,
    camera: Query<(&OrbitCamera, &Camera)>,
    radius: Res<NameRadius>,
    spyglass: Res<Spyglass>,
    show_names: Res<ShowNames>,
    show_body_names: Res<ShowBodyNames>,
    systems: Query<Candidate<'_, &'static System>>,
    bodies: Query<(Entity, &Body, &Indicator)>,
    // Where a body stands, read the same way [`face_camera`] reads it. Which
    // names are drawn is decided by packing their boxes, and where each one
    // lands is decided there; taken from two different answers about where a
    // body is, the packing settles a screen the names are then drawn onto
    // somewhere else, and through a zoom the two are a quarter of the way
    // apart. Names that were laid out clear of each other come out over one
    // another, and which of them wins changes every frame.
    places: Places,
    named: Query<Entity, With<Named>>,
    pointing: Query<&PointedAt>,
    selection: Query<(), With<Selected>>,
    holding: Res<HeldSystem>,
    time: Res<Time<Real>>,
    sky: Sky,
    mut layout: Local<Layout>,
) {
    let clear = |commands: &mut Commands| {
        for entity in &named {
            // The system may have been evicted this frame, so drop the marker
            // gracefully rather than panicking on a despawned entity.
            commands.entity(entity).try_remove::<Named>();
        }
    };
    // What is named already, which is worth something to it: the packing runs
    // afresh every frame, so without this two names of much the same standing
    // trade one place between them for as long as they are both on screen.
    let already: EntityHashSet = named.iter().collect();

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
        .find(|(_, body, _)| body.primary)
        .map(|(_, body, _)| body.address);

    // Where a jump is measured from, for the stops that say one. Looked up in
    // the same query the layout goes over: the system the camera is standing
    // in is one of the systems on the map.
    let from = holding
        .of()
        .and_then(|held| systems.get(held).ok())
        .map(|(_, system, ..)| system.position());

    // What a name has to fall inside some of to be worth laying out at all.
    let screen = Rect::from_corners(Vec2::ZERO, viewport);

    // Where the camera is pointed, which is the tightly packed middle that
    // [`room`] measures out from. The point the camera orbits projects here
    // by construction, so this is that point without the projection.
    let middle = viewport / 2.;

    let Layout { wanted, packing, rings } = &mut *layout;
    rings.clear();

    // Everything close enough to name and in front of the camera, with the
    // rectangle its name would occupy and how much it deserves one.
    wanted.clear();
    wanted.extend(systems.iter().filter_map(|candidate| {
        let (entity, system, mark, visibility, indicator, filtered, hop) =
            candidate;
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
        let stands = mark.0 > 0. && carried != Some(system.address);

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

        // In the realistic view a star's brightness is what sizes it, so it
        // both admits the name and, through `name_score`, leads it. Read the
        // apparent magnitude and the exposure's floor the same way
        // `size_photometrically` reads them; nothing in the map view, where
        // every star is drawn the same size.
        let floor = sky.exposure.zero_point() as f32;
        let apparent = matches!(*sky.view, View::Realistic).then(|| {
            Magnitude(system.absolute_magnitude())
                .apparent(Distance::light_years(orbit.eye.distance(position)))
                .0 as f32
        });
        // The margin above the floor is what the weight reads; `None` leaves
        // the map view scored on position alone. See `name_score`.
        let brightness = apparent.map(|a| (floor - a).max(0.));

        // Worth laying out at all, absent a mark asking for it outright: within
        // the reach in the map view, bright enough to draw and inside the
        // naming limit in the realistic one. See `worth_placing`.
        let placement = match apparent {
            Some(apparent) => {
                Placement::Bright { apparent, floor, limit: sky.limit.0 }
            }
            None => Placement::Reach { from_center, reach },
        };
        if !pointed_at && !selected && !hop && !worth_placing(placement) {
            return None;
        }
        let at = screen_position(orbit, cot_half_fov, viewport, position)?;
        // The words that will be set rather than the name alone. A stop
        // carries the jump to it, which is a good part of the width again,
        // and room granted for less than that is a name laid over the next
        // one along.
        let rect =
            name_rect_of(at, plate_width(system, hop, from), indicator.0);
        if screen.intersect(rect).is_empty() {
            return None;
        }
        // How far in front of the middle the system stands, in light years.
        // Measured down the view rather than to the eye, so two systems side
        // by side on screen are compared on which is nearer and not on which
        // sits further from the middle of the window.
        let ahead = orbit.radius
            - depth(orbit, position) / crate::space::LIGHT_YEAR as f32;
        if selected {
            rings.push((entity, ringed(at, indicator.0)));
        }

        let score = name_score(
            LabelWeight::System { from_center, ahead, brightness },
            pointed_at,
            selected,
            already.contains(&entity),
        );

        // Grown after the test above rather than before it, so that what is
        // laid out at all is still decided by the name itself. The room is
        // for the packing to read and nothing else.
        let apart = (at - middle).length();
        let rect = rect.inflate(room(orbit.radius, apart, viewport));
        Some((entity, rect, score))
    }));

    // Everything inside the system the camera is in. Its own switch, since a
    // system's name and a body's are asked for at different moments: the sky
    // is read by name and a system inside is read by looking, so one is off
    // until wanted and the other on until it is in the way.
    for (entity, body, indicator) in &bodies {
        let Some(place) = places.of(entity) else { continue };
        let Some(spot) = screen_position(orbit, cot_half_fov, viewport, place)
        else {
            continue;
        };
        let rect = name_rect_of(spot, capitals(&body.name), indicator.0);
        if screen.intersect(rect).is_empty() {
            continue;
        }
        let apart = (spot - middle).length();
        let rect = rect.inflate(room(orbit.radius, apart, viewport));

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
        if selected {
            rings.push((entity, ringed(spot, indicator.0)));
        }
        if !worth_naming(true, show_body_names.0, false, pointed_at, selected) {
            continue;
        }

        // How large the body itself looks, rather than the mark drawn to
        // aim at it.
        let depth = depth(orbit, place).max(1.);
        let score = name_score(
            LabelWeight::Body {
                under: body.ancestors,
                star: body.star,
                primary: body.primary,
                apparent: looks(
                    body.radius,
                    world_per_pixel(cot_half_fov, viewport.y, depth),
                ),
            },
            pointed_at,
            selected,
            already.contains(&entity),
        );
        wanted.push((entity, rect, score));
    }

    let winners = place(wanted, viewport, packing, rings);

    // Only what changed hands. Nearly every name is the same name it was
    // last frame, and taking one away to give it straight back moves its
    // system between archetypes twice for nothing, and reads as a change to
    // anything watching for one.
    for entity in &named {
        if !winners.contains(&entity) {
            commands.entity(entity).try_remove::<Named>();
        }
    }
    for entity in winners {
        if !named.contains(entity) {
            // A winner picked from this frame's query may already be gone by the
            // time the command runs, if eviction despawned it; try_insert lets
            // that pass rather than crashing choose_names.
            commands.entity(entity).try_insert(Named);
        }
    }
}

/// How large a bucket in the placement grid is, in pixels
///
/// About one name's rectangle, so a name falls across roughly four buckets
/// and is tested only against what those four hold. A ten letter name is some
/// 92 by 20 pixels at [`NAME_HEIGHT`], and the width is what to size by,
/// names being far wider than they are tall.
const BUCKET: f32 = NAME_HEIGHT * 8.;

/// The scratch a frame of names is laid out in
///
/// Held across frames so that a couple of thousand candidates and the grid
/// they are packed into are not allocated and thrown away sixty times a
/// second. Both are cleared before they are read, so nothing carries over but
/// the memory.
#[derive(Default)]
pub(crate) struct Layout {
    /// The candidates, with the rectangle each would occupy and its score
    wanted: Vec<(Entity, Rect, f32)>,
    /// Where the names already laid out sit
    packing: Packing,
    /// The rings drawn around what is picked out, and what each belongs to
    rings: Vec<(Entity, Rect)>,
}

/// Where the names already laid out sit, bucketed by screen position
///
/// A candidate overlaps only what is near it, so testing it against every
/// name placed so far asks a question about the far side of the screen to
/// learn about a rectangle twenty pixels wide.
#[derive(Default)]
struct Packing {
    /// Indices into `rects`, by bucket
    buckets: Vec<Vec<u32>>,
    rects: Vec<Rect>,
    /// Buckets across the viewport
    columns: usize,
    /// Buckets down it
    rows: usize,
}

impl Packing {
    /// Empty the grid and size it to the viewport, keeping its allocations
    fn reset(&mut self, viewport: Vec2) {
        self.columns = ((viewport.x / BUCKET).ceil() as usize).max(1);
        self.rows = ((viewport.y / BUCKET).ceil() as usize).max(1);
        self.rects.clear();
        self.buckets.resize_with(self.columns * self.rows, Vec::new);
        for bucket in &mut self.buckets {
            bucket.clear();
        }
    }

    /// The buckets a rectangle falls across, as inclusive ranges
    ///
    /// Clamped to the grid rather than dropped outside it. A name is laid out
    /// once any part of it is on screen, so a rectangle running off an edge is
    /// ordinary and still has to be tested against its neighbors.
    fn span(
        &self,
        rect: Rect,
    ) -> (RangeInclusive<usize>, RangeInclusive<usize>) {
        let bucket = |along: f32, of: usize| {
            ((along / BUCKET).max(0.) as usize).min(of.saturating_sub(1))
        };
        (
            bucket(rect.min.x, self.columns)..=bucket(rect.max.x, self.columns),
            bucket(rect.min.y, self.rows)..=bucket(rect.max.y, self.rows),
        )
    }

    /// Whether a rectangle clears every name already placed
    fn clear_of(&self, rect: Rect) -> bool {
        let (columns, rows) = self.span(rect);
        for row in rows {
            for column in columns.clone() {
                for &taken in &self.buckets[row * self.columns + column] {
                    if !self.rects[taken as usize].intersect(rect).is_empty() {
                        return false;
                    }
                }
            }
        }
        true
    }

    /// Take a rectangle's place, in every bucket it falls across
    fn hold(&mut self, rect: Rect) {
        let (columns, rows) = self.span(rect);
        let taken = self.rects.len() as u32;
        self.rects.push(rect);
        for row in rows {
            for column in columns.clone() {
                self.buckets[row * self.columns + column].push(taken);
            }
        }
    }
}

/// The square on screen a ring drawn at `at` takes up
///
/// [`Indicator`] is a radius in pixels, a mark being nine pixels across
/// whether the camera is a light year off or fifty thousand, so the ring is
/// the same size on screen wherever what it rings has got to.
fn ringed(at: Vec2, radius: f32) -> Rect {
    Rect::from_center_size(at, Vec2::splat(radius * 2.))
}

/// Which candidates fit, taken best first
///
/// Greedy rather than optimal. The best arrangement of a few hundred
/// overlapping rectangles is not worth solving each frame, and taking them in
/// the order the viewer wants them gives them the ones that matter.
fn place(
    candidates: &mut [(Entity, Rect, f32)],
    viewport: Vec2,
    packing: &mut Packing,
    rings: &[(Entity, Rect)],
) -> EntityHashSet {
    // Best first, so that what is dropped is dropped in favour of something
    // the viewer wanted more. Ties are settled by which entity it is, which
    // is arbitrary but the same arbitrary answer every frame.
    //
    // Something has to settle them, and the order they were gathered in
    // cannot: handing one of a tied pair a `Named` moves it to another
    // archetype and so to another place in the order they are read in, and the
    // two would trade one place between them every frame, which is a flicker
    // rather than a choice.
    candidates.sort_unstable_by(|a, b| b.2.total_cmp(&a.2).then(a.0.cmp(&b.0)));

    packing.reset(viewport);
    let mut winners = EntityHashSet::default();
    for (entity, rect, _) in candidates.iter() {
        // A ring is what the map is pointing at, so it keeps its place and
        // the name gives way. Never to its own ring: a name stands off the
        // ring of what it names by construction, but the room it is granted
        // reaches back over it, and a thing picked out losing its own name to
        // its own mark is the one answer that helps nobody.
        let ringed_over = rings.iter().any(|(of, ring)| {
            of != entity && !ring.intersect(*rect).is_empty()
        });

        if ringed_over || !packing.clear_of(*rect) {
            continue;
        }
        packing.hold(*rect);
        winners.insert(*entity);
    }
    winners
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

/// How much better a name has to be to take a place already held
///
/// Two and a half, which is a tenth of what a body's own size can argue for
/// it. Enough that a name is not handed back and forth, and small enough that
/// a body a step further up the system, or plainly larger, still takes it.
///
/// What it is for. The names are packed afresh every frame, best first, and
/// whatever will not fit is dropped; nothing carries over. Two bodies of the
/// same standing are then told apart by their size alone, and Neptune and
/// Uranus are within a twentieth of each other: measured, their names changed
/// hands on sixty frames of a slow zoom, swapping back on the next frame
/// every time, with the camera moving as little as a six-hundredth of the way
/// in between. A place worth holding is worth holding on to.
const HELD_WEIGHT: f32 = 2.5;

/// How many steps down a system the ordering tells apart
///
/// A star, its planets, their moons, and whatever a scan puts under those.
/// Deeper than this and everything is as deep as everything else, which costs
/// nothing: the records go four or five down at the most, and the step it
/// would take is smaller than the room a name needs.
const DEEPEST: u8 = 4;

/// How large a body looks, as a radius in logical pixels
///
/// Its own size, rather than the mark [`super::pointing`] draws to aim at it.
/// That mark is floored so there is always something to point at, and from
/// anywhere a whole system is in view nearly everything in it sits at the
/// floor: Pluto is under it from a light second and a quarter out and is
/// twenty thousand light seconds from the star. Measured by the mark, Pluto
/// and Charon are the same size, and which of the two is named comes down to
/// which entity it is.
fn looks(radius: f32, per_pixel: f32) -> f32 {
    radius / per_pixel.max(f32::MIN_POSITIVE)
}

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
/// Then how large it looks, which is [`looks`] and not the mark drawn to aim
/// at it. Bigger first, which is nearly always the order the viewer would have
/// chosen: the worlds before the specks.
///
/// The four are nested rather than added together. [`INSIDE_WEIGHT`] is cut
/// into a step per depth and one more for the arrival star, being a star takes
/// half a step, and the size argues over what is left, so nothing carries
/// enough to cross a claim above it. The whole of it stays inside
/// [`INSIDE_WEIGHT`], so nothing inside a system outranks whatever is pointed
/// at or picked out.

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
/// In the map view nothing about how notable a system is enters here: every
/// star is drawn the same size unless population scaling is turned on, so a
/// name argued from prominence would answer a question the picture never
/// asked. The realistic view is where that prominence is real — a star's size
/// there *is* its brightness (see [`super::scale::size_photometrically`]) — so
/// brightness earns the name too, carried in on [`LabelWeight::System`] and
/// weighed by [`BRIGHT_NAME_WEIGHT`]. Both follow the one prominence, as the
/// note here long promised they should.
/// What a name's weight is worked out from
///
/// The two are weighed on different things because they are seen in different
/// places. A system is out in the sky, where where it stands is the whole of
/// what distinguishes it. A body is only ever drawn while the camera is
/// inside its system, where every one of them stands in much the same place,
/// so what it is has to do the distinguishing instead.
///
/// Held apart here rather than in two functions, so that what they share, the
/// marks and how the terms are added up, cannot be changed for one and left
/// behind for the other.
enum LabelWeight {
    /// A system in the sky. Where it stands relative to the middle of the
    /// view — light years off it, and light years in front of it — and, in the
    /// realistic view, how far its apparent magnitude clears the drawn floor,
    /// which is what sizes its star and so leads its name. Nothing in the map
    /// view, where every star is the same size and position alone orders them.
    System { from_center: f32, ahead: f32, brightness: Option<f32> },
    /// A body inside the system the camera is in, by what it is: how far
    /// under the arrival star it orbits, whether it is a star, whether it is
    /// the one the system arrives at, and how large it looks
    Body { under: u8, star: bool, primary: bool, apparent: f32 },
}

/// How much a thing deserves the name it is asking for
///
/// One number line for both, since [`choose_names`] sorts systems and bodies
/// together and hands the best of them what room there is.
///
/// `held` is whether the name is drawn already, which is worth
/// [`HELD_WEIGHT`] to it. Added here with the rest so that what a name is
/// worth is one sum in one place, whatever kind of thing is asking.
fn name_score(
    weight: LabelWeight,
    pointed_at: bool,
    selected: bool,
    held: bool,
) -> f32 {
    let standing = match weight {
        LabelWeight::System { from_center, ahead, brightness } => {
            let centered =
                CENTER_WEIGHT / (1. + (from_center / CENTER_REACH).powi(2));
            let nearness = centered - from_center + NEARER_WEIGHT * ahead;
            match brightness {
                // The realistic view sizes a star by how far it clears the
                // limiting magnitude, so that margin leads its name; nearness
                // to the middle is a secondary that orders stars of a like
                // brightness. Both are bounded — the brightness to
                // [`CENTER_WEIGHT`], the nearness to a magnitude's worth
                // ([`BRIGHT_NAME_WEIGHT`]) — so brightness always leads and the
                // whole stays inside the band a body ([`INSIDE_WEIGHT`]) and a
                // mark ([`POINTED_WEIGHT`]) sit above, even in a wide view where
                // `from_center` runs to thousands of light years and unbounded
                // would sink a bright far star below a dim near one.
                Some(margin) => {
                    let bright =
                        (BRIGHT_NAME_WEIGHT * margin).min(CENTER_WEIGHT);
                    let near = (SECONDARY_NEARNESS * nearness)
                        .clamp(-BRIGHT_NAME_WEIGHT, BRIGHT_NAME_WEIGHT);
                    bright + near
                }
                None => nearness,
            }
        }
        LabelWeight::Body { under, star, primary, apparent } => {
            // A rank apiece for the depths, and one over them all for the
            // star the system arrives at.
            let step = INSIDE_WEIGHT / (DEEPEST + 2) as f32;
            let rank = if primary {
                DEEPEST + 1
            } else {
                DEEPEST - under.min(DEEPEST)
            };
            let sun = if star { step / 2. } else { 0. };
            let size = (step / 2.) * apparent / (apparent + BODY_NAME_REACH);

            rank as f32 * step + sun + size
        }
    };

    let holding = if held { HELD_WEIGHT } else { 0. };

    marked_score(pointed_at, selected) + standing + holding
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
    name_rect_of(at, name.chars().count(), clear)
}

/// How much further apart names are held than [`CROWDING`] holds them, in
/// pixels
///
/// Zero up close and zero at the center of the view, so the near map is left
/// exactly as it was and whatever the camera is pointed at stays packed as
/// tightly as it ever was. It grows with how far the camera stands off and
/// with how far from the center a name sits.
///
/// This is room around a name and no part of the name itself. A name is drawn
/// from the left edge and middle that [`name_rect_of`] works out, neither of
/// which the margin enters, so widening it moves no character on screen. It
/// only makes names compete harder for a place.
fn room(radius: f32, apart: f32, viewport: Vec2) -> f32 {
    NAME_HEIGHT
        * CROWDING
        * (SPREAD_BY - 1.)
        * pulled_back(radius)
        * relaxed(apart, viewport)
}

/// How far the camera has pulled back, from none of the way to all of it
///
/// Eased over `log10` of the radius, the map reading distance in e-folds
/// wherever it reads it at all, and smoothed so both ends are flat. A linear
/// ramp would twitch the whole field as the camera drifted across either one.
fn pulled_back(radius: f32) -> f32 {
    let e_folds = |ly: f32| ly.max(1.).log10();
    smoothed(
        (e_folds(radius) - e_folds(TIGHT_TO))
            / (e_folds(LOOSE_FROM) - e_folds(TIGHT_TO)),
    )
}

/// How far a name has relaxed out of the tightly packed middle
///
/// Cubed inside the exponential, and that is what makes a middle at all. A
/// plain `1 - exp(-x)` is steepest at zero, so the room starts growing at once
/// and the part of the view that should be densest is where the falloff bites
/// hardest: raising the spread then thins the middle as fast as the edge and
/// the field comes out evenly sparse, which is the thing being fixed rather
/// than a milder version of it.
///
/// Cubed, the curve leaves the middle alone, turns over around [`RELAX`] and
/// is spent soon after. A dense core reads against a sparse edge, and it still
/// never quite arrives, so the corners are not all given exactly the same room
/// and do not read as a border.
fn relaxed(apart: f32, viewport: Vec2) -> f32 {
    let out = apart / (RELAX * viewport.y).max(1.);
    1. - (-out * out * out).exp()
}

/// A smoothstep, held to its ends
fn smoothed(t: f32) -> f32 {
    let t = t.clamp(0., 1.);
    t * t * (3. - 2. * t)
}

/// The same rectangle, for whoever knows how wide a name is without holding it
///
/// A name's width is its letter count, the font being monospaced, so what the
/// layout is measuring is a number and not any particular words. The sky is
/// laid out every frame and setting the words to count them is a heap
/// allocation per system per frame, thrown away as soon as it is measured.
pub(super) fn name_rect_of(at: Vec2, letters: usize, clear: f32) -> Rect {
    let size = NAME_HEIGHT;
    let width = letters as f32 * ADVANCE * size;
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

/// Give a name token to everything that has won a name, and take it from the
/// rest
///
/// [`choose_names`] decides; this only carries the decision out. A `Named`
/// system or body gets a [`Label`] token hung off it carrying the words to
/// set, which [`draw_names`] paints and [`super::pointing`] catches the
/// pointer over. A thing without a `Named` has none, which keeps the work to
/// the names actually drawn.
///
/// The words are set again where they have moved on. What a system's plate
/// says is not its name alone: a stop carries the jump to it, and a system
/// becomes a stop and stops being one without ever losing the name it holds.
#[allow(clippy::too_many_arguments)]
pub(crate) fn respawn(
    mut commands: Commands,
    named: Query<
        (Entity, &System, Option<&Children>, Has<crate::systems::route::Hop>),
        With<Named>,
    >,
    // Where the jump to a stop is measured from, the system the camera stands
    // in. Asked of every system so the answer matches the one [`choose_names`]
    // granted room for on the frame the camera arrives, before the system it
    // arrives in has let go of its own name.
    standing_in: Query<&System>,
    holding: Res<HeldSystem>,
    named_bodies: Query<(Entity, &Body, Option<&Children>), With<Named>>,
    // Whatever lost its name since this last ran, rather than everything that
    // does not have one. Nearly every name is the name it was last frame, and
    // asking the other way round walks the whole sky to find the handful to
    // take a token from.
    mut unnamed: RemovedComponents<Named>,
    children_of: Query<&Children>,
    tokens: Query<Entity, With<Label>>,
    // The words on a token already up, which only a system's ever change.
    mut words: Query<&mut PlateText, With<Label>>,
) {
    for entity in unnamed.read() {
        // Nothing where the thing itself has gone, which is the other way a
        // name is lost. Its token went with it.
        let Ok(children) = children_of.get(entity) else { continue };
        for child in children.iter() {
            if tokens.contains(child) {
                commands.entity(child).despawn();
            }
        }
    }

    // The same token, hung off whatever it names.
    for (entity, body, children) in &named_bodies {
        set_name(
            &mut commands,
            &mut words,
            &tokens,
            entity,
            children,
            body.name.to_uppercase(),
        );
    }

    // Where a jump is measured from. Nothing where the map is holding no
    // system, which is also when nothing is a stop.
    let from = holding
        .of()
        .and_then(|held| standing_in.get(held).ok())
        .map(|held| held.position());

    for (entity, system, children, hop) in &named {
        set_name(
            &mut commands,
            &mut words,
            &tokens,
            entity,
            children,
            plate_for(system, hop, from),
        );
    }
}

/// Hang a name token off `entity`, or set the words on the one already there
///
/// Written only where the words moved, so a token is not touched on the frames
/// its name says the same thing.
fn set_name(
    commands: &mut Commands,
    words: &mut Query<&mut PlateText, With<Label>>,
    tokens: &Query<Entity, With<Label>>,
    entity: Entity,
    children: Option<&Children>,
    wanted: String,
) {
    let token = children
        .into_iter()
        .flat_map(|children| children.iter())
        .find(|child| tokens.contains(*child));

    if let Some(token) = token {
        if let Ok(mut plate) = words.get_mut(token)
            && plate.0 != wanted
        {
            plate.0 = wanted;
        }
        return;
    }

    let token = commands.spawn(nameplate(wanted)).id();
    commands.entity(entity).add_child(token);
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
    plate_words(&system.name, jump_to(system, stop, from))
}

/// How many letters that plate sets
///
/// What [`plate_for`] would come to, for whoever is laying a name out rather
/// than setting one. The layout wants a width and a width is a letter count,
/// so the words themselves are never wanted and the whole sky is measured
/// without a string being built for any of it.
///
/// A stop is the exception and sets its words to be counted. What it says is a
/// number formatted to a place, whose length is a question about rounding, and
/// there are two stops on a map against a sky of systems.
pub(super) fn plate_width(
    system: &System,
    stop: bool,
    from: Option<DVec3>,
) -> usize {
    match jump_to(system, stop, from) {
        Some(jump) => plate_words(&system.name, Some(jump)).chars().count(),
        None => capitals(&system.name),
    }
}

/// How far the jump to `system` is, where it is a stop that says one
///
/// `from` is where a jump is measured from, which is the system the camera is
/// standing in. Nothing where the map is holding none, which is also when
/// nothing is a stop.
fn jump_to(system: &System, stop: bool, from: Option<DVec3>) -> Option<f64> {
    from.filter(|_| stop).map(|from| (system.position() - from).length())
}

/// How many letters `name` sets in capitals
///
/// Without setting them. Nearly every letter is one letter in capitals, and a
/// few are two, which [`char::to_uppercase`] answers a letter at a time.
fn capitals(name: &str) -> usize {
    name.chars().map(|letter| letter.to_uppercase().count()).sum()
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

/// A name token hung off whatever it names
///
/// A bare handle rather than a plate of meshes: the words are painted in
/// screen space by [`draw_names`], and this only holds them, catches the
/// pointer over the name, and keeps the thing in the hierarchy for the paint
/// to find. One token for a system and for a body alike, differing only in
/// what hangs it and what decided it was worth drawing.
fn nameplate(words: String) -> impl Bundle {
    (
        Label,
        PlateText(words),
        // What catches the pointer over a name. The area is worked out on
        // screen by [`super::pointing`], from the same rectangle
        // [`choose_names`] laid this name out in, so a name catches over
        // exactly the room it was granted.
        Pickable::default(),
        // No mesh hangs off a token; it draws nothing itself. Hidden keeps it
        // out of every render pass while leaving it in the hierarchy for the
        // pointer to be caught over and for [`draw_names`] to reach its parent.
        Transform::default(),
        Visibility::Hidden,
    )
}

/// Paint every drawn name flat on the screen
///
/// The names, their grounds and the lines joining them to what they name are
/// annotation, not light, and are drawn in screen space with egui rather than
/// as meshes out at the galaxy coordinates their systems sit at. A glyph mesh
/// transformed by `view_proj · model` in f32 at a system's ~1e17 m sits torn
/// apart on hardware that keeps fewer bits through the multiply (see
/// `docs/night-sky.md`); a name projected to a pixel on the CPU and painted
/// there does not, and comes out crisp at the window's own scale besides.
///
/// The layout is [`choose_names`]' and reaches here through the [`Label`]
/// tokens [`respawn`] hangs off whatever wins a name: each carries the words to
/// set, and its parent says where on screen the name goes and what colour it
/// comes out. Painted in the background layer, under the chrome and over the
/// map.
pub fn draw_names(
    mut contexts: EguiContexts,
    camera: Query<(&OrbitCamera, &Camera)>,
    tokens: Query<(&PlateText, &ChildOf), With<Label>>,
    // The marks that colour a name and where its system stands. Spelled
    // `Without<Label>` so the scheduler can prove the token query disjoint
    // from these; a token is neither a system nor a body.
    systems: Query<
        (
            &System,
            &Indicator,
            Has<PointedAt>,
            Has<Selected>,
            Has<Filtered>,
            Has<crate::systems::route::Hop>,
        ),
        Without<Label>,
    >,
    bodies: Query<
        (&Indicator, Has<PointedAt>, Has<Selected>),
        (With<Body>, Without<Label>),
    >,
    places: Places,
    dim: Res<DimTo>,
) -> Result {
    let Ok((orbit, camera)) = camera.single() else { return Ok(()) };
    let Some(viewport) = camera.logical_viewport_size() else { return Ok(()) };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    let ctx = contexts.ctx_mut()?;
    // Under the chrome, which draws in the same order, and over the map, which
    // every egui layer is drawn over. Ordered before the chrome in the
    // schedule so its layer is registered first and sits beneath the panes.
    let painter = ctx.layer_painter(annotations_layer());
    // The one face the chrome is set in as well, so a name on the map and the
    // same name in the bar are the one typeface. Egui's default monospace is
    // Hack, which is the face the chrome is set in.
    let font = egui::FontId::new(NAME_HEIGHT, egui::FontFamily::Monospace);

    for (words, child_of) in &tokens {
        let thing = child_of.parent();
        // Where the thing stands on screen, how large its mark is, what colour
        // its name comes out, and whether a line is drawn to it. A system is
        // out in the sky; a body is read off the grid holding it, the way
        // [`choose_names`] reads it, so the name is placed against the view it
        // is about to be drawn into.
        let (at, clear, tint, leader) =
            if let Ok((system, indicator, pointed, selected, filtered, hop)) =
                systems.get(thing)
            {
                let Some(at) = screen_position(
                    orbit,
                    cot_half_fov,
                    viewport,
                    DVec3::from(system.position),
                ) else {
                    continue;
                };
                let tint = faded(
                    marked_tint(pointed, selected),
                    if filtered { dim.opacity() } else { 1. },
                );
                (at, indicator.0, tint, !(pointed || selected || hop))
            } else if let Ok((indicator, pointed, selected)) = bodies.get(thing)
            {
                let Some(place) = places.of(thing) else { continue };
                let Some(at) =
                    screen_position(orbit, cot_half_fov, viewport, place)
                else {
                    continue;
                };
                (
                    at,
                    indicator.0,
                    marked_tint(pointed, selected),
                    !(pointed || selected),
                )
            } else {
                continue;
            };

        // The name sits up and to the right of what it names, clear of the
        // mark drawn around it, by the same figures [`name_rect_of`] lays it
        // out with. Its origin is the left edge of the words, vertically
        // centred.
        let left = at.x + clear + NAME_HEIGHT * GAP;
        let middle = at.y - NAME_HEIGHT * RISE;

        let color = color32(tint);
        let galley =
            painter.layout_no_wrap(words.0.clone(), font.clone(), color);
        let origin = egui::pos2(left, middle - galley.size().y / 2.);

        // The dark ground the name is read against, sized to the words it
        // carries and drawn first so the letters land on it. Opaque, so the
        // field of stars a name is read over does not fill its counters.
        let pad = NAME_HEIGHT * GROUND_PAD;
        painter.rect_filled(
            egui::Rect::from_min_size(origin, galley.size()).expand(pad),
            0.,
            color32(GROUND),
        );
        painter.galley(origin, galley, color);

        // The line joining the thing to its name, begun clear of the mark and
        // stopped short of the words, with the same air at each end. A name
        // marked out is ringed instead and needs no line.
        if leader {
            let to = Vec2::new(left, middle);
            let length = at.distance(to);
            if let Some(direction) = (to - at).try_normalize() {
                let gap = length * LEADER_GAP;
                let start = clear + gap;
                let end = length - gap;
                if start < end {
                    let a = at + direction * start;
                    let b = at + direction * end;
                    painter.line_segment(
                        [egui::pos2(a.x, a.y), egui::pos2(b.x, b.y)],
                        egui::Stroke::new(1.0_f32, color32(LEADER_COLOR)),
                    );
                }
            }
        }
    }

    Ok(())
}

/// What colour a name comes out for the marks on it
///
/// A name is drawn the colour of the ring around its star, so a system marked
/// out is one thing in two places. Selection wins over pointing where both
/// apply, as it does for the ring: the pointer will move on, and the selection
/// is what was asked for.
fn marked_tint(pointed_at: bool, selected: bool) -> Srgba {
    if selected {
        SELECTION
    } else if pointed_at {
        INDICATOR
    } else {
        Srgba::WHITE
    }
}

/// `tint` at `strength` of full
///
/// The alpha carries it, not the colour: a name dimmed by darkening would go
/// black against the sky and read as a hole rather than as something standing
/// further back.
fn faded(tint: Srgba, strength: f32) -> Srgba {
    Srgba { alpha: tint.alpha * strength, ..tint }
}

/// An sRGB colour as egui knows it
///
/// [`Srgba`] channels are already gamma-encoded, the space [`egui::Color32`]
/// holds, so they cross straight over; the alpha is not premultiplied on
/// either side.
pub(crate) fn color32(color: Srgba) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        (color.red * 255.) as u8,
        (color.green * 255.) as u8,
        (color.blue * 255.) as u8,
        (color.alpha * 255.) as u8,
    )
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

    /// A plate is measured at the width it will be set at
    ///
    /// What the layout rests on. The sky is measured without any of its words
    /// being set, so nothing holds the count to the string but this: room
    /// granted for fewer letters than a plate sets is a name laid over the
    /// next one along.
    #[test]
    fn a_plate_is_measured_at_the_width_it_sets() {
        for name in ["sol", "SOL", "Shinrarta Dezhra", "LHS 3447", "straße"] {
            let system = crate::systems::tests::named(1, name);

            assert_eq!(
                plate_width(&system, false, None),
                plate_for(&system, false, None).chars().count(),
                "{name} was granted the room for something else"
            );
        }
    }

    /// A system called `name`, standing `away` light years along the x axis
    fn stop(name: &str, away: f64) -> System {
        let mut system = crate::systems::tests::named(1, name);
        system.position = [away, 0., 0.];
        system
    }

    /// And so is a stop, whose plate says the jump to it as well
    ///
    /// The jump is a number set to a place, so how many letters it takes is a
    /// question about rounding rather than about magnitude: 9.96 light years
    /// is set as four and 9.94 as three.
    #[test]
    fn a_stop_is_measured_at_the_width_it_sets() {
        let from = Some(DVec3::ZERO);
        for away in [0.04, 6.74, 9.94, 9.96, 99.99, 123.45, 1234.5] {
            let system = stop("lung", away);

            assert_eq!(
                plate_width(&system, true, from),
                plate_for(&system, true, from).chars().count(),
                "a stop {away} off was granted the room for something else"
            );
        }
    }

    /// A letter whose capital is two letters takes the room of two
    ///
    /// Which is the whole reason the count is taken a letter at a time rather
    /// than off the length of the name.
    #[test]
    fn a_letter_that_doubles_in_capitals_is_counted_twice() {
        assert_eq!(capitals("straße"), 7);
        assert_eq!(capitals("straße"), "straße".to_uppercase().chars().count());
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

    /// A body of `apparent` pixels, one of a system's own planets
    fn planet(apparent: f32, held: bool) -> f32 {
        name_score(
            LabelWeight::Body {
                under: 1,
                star: false,
                primary: false,
                apparent,
            },
            false,
            false,
            held,
        )
    }

    /// A name already drawn is not handed over to a rival of much the same size
    ///
    /// The packing runs afresh every frame and keeps nothing, so two bodies of
    /// the same standing are told apart by their size alone. Neptune and
    /// Uranus are within a twentieth of each other, and measured they traded
    /// one place between them on sixty frames of one slow zoom.
    #[test]
    fn a_name_already_drawn_holds_its_place_against_its_like() {
        // A hair larger, which is Uranus against Neptune.
        let held = planet(100., true);
        let rival = planet(103., false);

        assert!(
            held > rival,
            "a name drawn at {held} gave way to one worth {rival}"
        );
    }

    /// And gives it up to one that plainly deserves it more
    ///
    /// Holding a place is worth something and not everything. A body that
    /// reads as a world where the one named is a speck has earned the name,
    /// and so has anything a step further up the system.
    #[test]
    fn a_name_gives_way_to_one_that_deserves_it() {
        let speck = planet(0.5, true);
        let world = planet(1e4, false);
        assert!(world > speck, "a speck kept the name off a world");

        // A step up the system beats the whole of what a size can argue,
        // holding or not.
        let moon = name_score(
            LabelWeight::Body {
                under: 2,
                star: false,
                primary: false,
                apparent: 1e4,
            },
            false,
            false,
            true,
        );
        assert!(
            planet(0., false) > moon,
            "a moon holding a name kept it off the planet it goes round"
        );
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
            let parent = name_score(
                LabelWeight::Body {
                    under: under,
                    star: false,
                    primary: false,
                    apparent: 0.,
                },
                false,
                false,
                false,
            );
            let child = name_score(
                LabelWeight::Body {
                    under: under + 1,
                    star: false,
                    primary: false,
                    apparent: 1e4,
                },
                false,
                false,
                false,
            );

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
        let star = name_score(
            LabelWeight::Body {
                under: 1,
                star: true,
                primary: false,
                apparent: 0.,
            },
            false,
            false,
            false,
        );
        let planet = name_score(
            LabelWeight::Body {
                under: 1,
                star: false,
                primary: false,
                apparent: 1e4,
            },
            false,
            false,
            false,
        );

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
        let deeper = name_score(
            LabelWeight::Body {
                under: 2,
                star: true,
                primary: false,
                apparent: 1e4,
            },
            false,
            false,
            false,
        );
        let higher = name_score(
            LabelWeight::Body {
                under: 1,
                star: false,
                primary: false,
                apparent: 0.,
            },
            false,
            false,
            false,
        );

        assert!(higher > deeper, "{higher} did not beat {deeper}");
    }

    /// Past the last step everything is as deep as everything else
    ///
    /// The records go four or five down at the most, and a step small enough
    /// to tell the rest apart is smaller than the room a name takes.
    #[test]
    fn the_deepest_step_is_the_last_one_told_apart() {
        let deep = name_score(
            LabelWeight::Body {
                under: DEEPEST,
                star: false,
                primary: false,
                apparent: 0.,
            },
            false,
            false,
            false,
        );
        let deeper = name_score(
            LabelWeight::Body {
                under: DEEPEST + 3,
                star: false,
                primary: false,
                apparent: 0.,
            },
            false,
            false,
            false,
        );

        assert_eq!(deep, deeper);
    }

    /// At one depth, the larger is named first
    #[test]
    fn the_larger_of_two_at_one_depth_is_named_first() {
        let world = name_score(
            LabelWeight::Body {
                under: 2,
                star: false,
                primary: false,
                apparent: 1e4,
            },
            false,
            false,
            false,
        );
        let speck = name_score(
            LabelWeight::Body {
                under: 2,
                star: false,
                primary: false,
                apparent: 0.1,
            },
            false,
            false,
            false,
        );

        assert!(world > speck, "the world scored {world} against {speck}");
    }

    /// The larger of a pair under the mark's floor is named first
    ///
    /// Pluto and Charon go round the same point, so nothing above size tells
    /// them apart, and a mark is floored at four pixels so there is always
    /// something to point at. Scored by that mark the two tie exactly and
    /// which is named comes down to which entity it is, which is how Charon
    /// took the name.
    #[test]
    fn the_larger_of_a_pair_under_the_marks_floor_is_named_first() {
        // A hundred light seconds off the pair, where Pluto draws a twentieth
        // of a pixel across and Charon about half of that. Both are far under
        // the four the mark stops at.
        let per_pixel = 2.3e7;
        let scored = |radius| {
            name_score(
                LabelWeight::Body {
                    under: 2,
                    star: false,
                    primary: false,
                    apparent: looks(radius, per_pixel),
                },
                false,
                false,
                false,
            )
        };

        let pluto = scored(1.153e6);
        let charon = scored(603_500.);

        assert!(
            looks(1.153e6, per_pixel) < 4.,
            "Pluto drew larger than the floor a mark stops at"
        );
        assert!(pluto > charon, "Pluto scored {pluto} against {charon}");
    }

    /// The star a system arrives at outranks everything else in the system
    ///
    /// Whatever the rest have going for them: the shallowest place, being
    /// stars themselves, and drawing as large as a body ever does. Its name
    /// is the one that says which system this is.
    #[test]
    fn the_arrival_star_outranks_the_whole_system() {
        let primary = name_score(
            LabelWeight::Body {
                under: 0,
                star: true,
                primary: true,
                apparent: 0.,
            },
            false,
            false,
            false,
        );

        // The best anything else can do, which is a second star sitting as
        // shallow as the arrival star and filling the view.
        let best = name_score(
            LabelWeight::Body {
                under: 0,
                star: true,
                primary: false,
                apparent: 1e9,
            },
            false,
            false,
            false,
        );
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
        let arrival = name_score(
            LabelWeight::Body {
                under: 1,
                star: true,
                primary: true,
                apparent: floor,
            },
            false,
            false,
            false,
        );
        let other = name_score(
            LabelWeight::Body {
                under: 1,
                star: true,
                primary: false,
                apparent: floor,
            },
            false,
            false,
            false,
        );

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
        let star = name_score(
            LabelWeight::Body {
                under: 0,
                star: true,
                primary: false,
                apparent: 1e4,
            },
            false,
            false,
            false,
        );

        assert!(
            name_score(
                LabelWeight::Body {
                    under: DEEPEST,
                    star: false,
                    primary: false,
                    apparent: 0.
                },
                true,
                false,
                false,
            ) > star
        );
        assert!(
            name_score(
                LabelWeight::Body {
                    under: DEEPEST,
                    star: false,
                    primary: false,
                    apparent: 0.
                },
                false,
                true,
                false,
            ) > star
        );
    }

    /// A system weight from where it stands, in the map view — no brightness
    fn placed(from_center: f32, ahead: f32) -> LabelWeight {
        LabelWeight::System { from_center, ahead, brightness: None }
    }

    /// And in the realistic view, `margin` magnitudes above the drawn floor
    fn lit(from_center: f32, ahead: f32, margin: f32) -> LabelWeight {
        LabelWeight::System { from_center, ahead, brightness: Some(margin) }
    }

    /// Of two systems along one line of sight, the near one is named first
    ///
    /// A name is drawn over whatever stands behind it, so naming the far one
    /// lays it over the near stars as well and hides what is in front. Both
    /// of these stand the same distance off the middle, one ahead of it and
    /// one behind, so the only thing separating them is which is nearer.
    #[test]
    fn a_system_in_front_is_named_before_one_behind() {
        let away = 20.;
        let front = name_score(placed(away, away), false, false, false);
        let back = name_score(placed(away, -away), false, false, false);

        assert!(front > back, "{front} was no better than {back}");
    }

    /// Standing in front of the middle answers the cost of standing off it
    ///
    /// What [`NEARER_WEIGHT`] at one buys, and what keeps the near half of a
    /// view competing with the middle rather than with the far half. The two
    /// part by the center bonus falling away and by nothing else.
    #[test]
    fn a_system_in_front_of_the_middle_scores_as_one_at_it() {
        let away = 20.;
        let bonus = CENTER_WEIGHT / (1. + (away / CENTER_REACH).powi(2));

        let at = name_score(placed(0., 0.), false, false, false);
        let front = name_score(placed(away, away), false, false, false);

        assert!(
            (front - (at - CENTER_WEIGHT + bonus)).abs() < 1e-3,
            "{front} against {at} less a bonus of {bonus}"
        );
    }

    /// Names are offered in order of nearness to the center
    ///
    /// The score falls the whole way out, so the greedy pass takes the
    /// nearest system first and works outwards. Nothing further away can
    /// take a place from something closer, which is what makes the ordering
    /// something a viewer can predict rather than a ranking to be read.
    #[test]
    fn nearer_systems_are_always_offered_a_name_first() {
        let mut nearer = name_score(placed(0., 0.), false, false, false);
        for step in 1..=1000 {
            let further = name_score(
                placed(step as f32 * DEFAULT_NAME_RADIUS / 1000., 0.),
                false,
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

    /// The realistic view names the brighter of two systems first
    ///
    /// There a star's size is its brightness, so the name follows it: two
    /// systems the same distance off the middle are told apart by how far each
    /// clears the drawn floor, brighter first.
    #[test]
    fn a_brighter_system_is_named_first_in_the_realistic_view() {
        let score = |w| name_score(w, false, false, false);

        let bright = score(lit(5., 0., 8.));
        let faint = score(lit(5., 0., 2.));

        assert!(bright > faint, "{bright} was no better than {faint}");
    }

    /// Stars of a like brightness are still ordered by nearness to the middle
    ///
    /// Brightness leads, but does not settle it alone: where two clear the
    /// floor by the same margin, the nearer the center is named first.
    #[test]
    fn a_like_brightness_falls_back_on_nearness() {
        let score = |w| name_score(w, false, false, false);

        let near = score(lit(0., 0., 5.));
        let far = score(lit(DEFAULT_NAME_RADIUS, 0., 5.));

        assert!(near > far, "{near} was no better than {far}");
    }

    /// Brightness leads position: a bright star off the middle outranks a
    /// faint one at it
    ///
    /// What naming primarily by magnitude means. In the map view the central
    /// system always wins; in the realistic view a far brighter star takes the
    /// name first, and only a like brightness falls back on nearness.
    #[test]
    fn brightness_leads_position_in_the_realistic_view() {
        let score = |w| name_score(w, false, false, false);

        let bright_edge = score(lit(DEFAULT_NAME_RADIUS, 0., 10.));
        let faint_center = score(lit(0., 0., 0.));

        assert!(
            bright_edge > faint_center,
            "{bright_edge} against {faint_center}"
        );
    }

    /// Brightness leads even in a wide view, where nearness would run away
    ///
    /// With the spyglass wide, `from_center` reaches thousands of light years,
    /// and unbounded the nearness term would sink a bright far star below a dim
    /// near one. Bounded to a magnitude's worth, brightness keeps the lead.
    #[test]
    fn brightness_leads_even_in_a_wide_view() {
        let score = |w| name_score(w, false, false, false);

        let bright_far = score(lit(5000., -5000., 8.));
        let dim_near = score(lit(0., 0., 1.));

        assert!(bright_far > dim_near, "{bright_far} against {dim_near}");
    }

    /// However bright, a system's name keeps below a body's and below a mark
    ///
    /// The realistic weight is capped to the band the map view's uses, so the
    /// orderings the rest of the module rests on hold: a body drawn inside the
    /// held system, and anything pointed at or picked out, still come first.
    #[test]
    fn a_bright_system_keeps_to_its_band() {
        let bright = name_score(lit(0., 0., 1e3), false, false, false);

        assert!(bright < INSIDE_WEIGHT, "{bright} reached a body's band");
        assert!(bright < POINTED_WEIGHT, "{bright} reached a point's");
    }

    /// Pointing still settles a name against the brightest star in the sky
    #[test]
    fn pointing_outranks_the_brightest_system() {
        let pointed =
            name_score(lit(DEFAULT_NAME_RADIUS, 0., 0.), true, false, false);
        let bright = name_score(lit(0., 0., 1e3), false, false, false);

        assert!(pointed > bright, "{pointed} against {bright}");
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

    /// The map view lays a name out within the reach and no further
    ///
    /// Where every star is the same size, nearness is the whole of it, so a
    /// system past the reach about the center is dropped before it is scored.
    #[test]
    fn the_map_view_places_names_within_the_reach() {
        let reach = |from_center| {
            worth_placing(Placement::Reach { from_center, reach: 20. })
        };
        assert!(reach(10.));
        assert!(!reach(30.));
    }

    /// The realistic view lays out any star bright enough to be drawn, however
    /// far off the center it stands
    ///
    /// Brightness earns the place there, not a reach: a bright star deep along
    /// the line of sight — Dubhe past Merak — is laid out where the map view
    /// would have dropped it, and a star too faint to clear the floor is left
    /// out.
    #[test]
    fn the_realistic_view_places_by_brightness_not_reach() {
        let drawn = |apparent| {
            worth_placing(Placement::Bright { apparent, floor: 8., limit: 12. })
        };
        assert!(drawn(2.));
        assert!(!drawn(8.));
    }

    /// The naming limit turns the sky's names down from the faint end
    ///
    /// What the dial is for: a star still drawn but fainter than the limit is
    /// left unnamed, the brighter ones kept, the way turning Name Radius down
    /// keeps the near systems and drops the far.
    #[test]
    fn the_name_limit_drops_the_faint() {
        let named = |apparent| {
            worth_placing(Placement::Bright { apparent, floor: 10., limit: 5. })
        };
        assert!(named(3.));
        assert!(!named(7.));
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
        let pointed =
            name_score(placed(DEFAULT_NAME_RADIUS, 0.), true, false, false);

        // The system at the center, which is otherwise the best there is.
        let centered = name_score(placed(0., 0.), false, false, false);

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
        let selected =
            name_score(placed(DEFAULT_NAME_RADIUS, 0.), false, true, false);
        let pointed = name_score(placed(0., 0.), true, false, false);

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
        let both =
            name_score(placed(DEFAULT_NAME_RADIUS, 0.), true, true, false);
        let selected =
            name_score(placed(DEFAULT_NAME_RADIUS, 0.), false, true, false);

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

    /// A minimal world for the name tokens, holding no system
    ///
    /// Nothing is being flown into, so no system is the one the map is holding
    /// and no token says a jump.
    fn plated() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<HeldSystem>();
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

    /// A 1080p screen, which is what the figures in the plan are quoted at
    const VIEWPORT: Vec2 = Vec2::new(1920., 1080.);

    fn candidate(index: u32, rect: Rect, score: f32) -> (Entity, Rect, f32) {
        (Entity::from_raw_u32(index).expect("a test entity"), rect, score)
    }

    fn won<const N: usize>(indices: [u32; N]) -> EntityHashSet {
        indices
            .into_iter()
            .map(|index| Entity::from_raw_u32(index).expect("a test entity"))
            .collect()
    }

    fn placing(candidates: &mut [(Entity, Rect, f32)]) -> EntityHashSet {
        ringing(candidates, &[])
    }

    fn ringing(
        candidates: &mut [(Entity, Rect, f32)],
        rings: &[(Entity, Rect)],
    ) -> EntityHashSet {
        place(candidates, VIEWPORT, &mut Packing::default(), rings)
    }

    /// Seeded, so a failure can be looked at twice
    fn noise() -> impl FnMut() -> f32 {
        let mut seed = 0x2545_f491_u32;
        move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        }
    }

    /// A scattered field of ten letter names, scored without regard to where
    /// they sit, so that what survives is decided by the room they are given
    /// and by nothing else
    ///
    /// Scattered rather than a lattice. A lattice quantizes the answer, since
    /// room that grows by less than one step of it changes nothing, and a
    /// star field is not laid out on a grid anyway. One seed, so the two
    /// standoffs are handed the same positions and the same scores and differ
    /// only in the room.
    fn scattered(radius: f32) -> Vec<(Entity, Rect, f32)> {
        let mut next = noise();
        (1..=600)
            .map(|index| {
                let at = Vec2::new(next() * VIEWPORT.x, next() * VIEWPORT.y);
                let apart = (at - VIEWPORT / 2.).length();
                let rect = name_rect_of(at, 10, 0.)
                    .inflate(room(radius, apart, VIEWPORT));
                candidate(index, rect, next())
            })
            .collect()
    }

    /// A name clear of everything already placed is kept
    #[test]
    fn a_name_clear_of_the_rest_is_kept() {
        let mut candidates = [
            candidate(1, Rect::new(0., 0., 10., 10.), 2.),
            candidate(2, Rect::new(20., 0., 30., 10.), 1.),
        ];

        assert_eq!(placing(&mut candidates), won([1, 2]));
    }

    /// A name over one already placed is dropped
    #[test]
    fn a_name_over_one_already_placed_is_dropped() {
        let mut candidates = [
            candidate(1, Rect::new(0., 0., 10., 10.), 2.),
            candidate(2, Rect::new(5., 5., 15., 15.), 1.),
        ];

        assert_eq!(placing(&mut candidates), won([1]));
    }

    /// Of two names that overlap, the better scored one is kept
    ///
    /// Whichever order they arrive in. What the viewer wanted more decides,
    /// and the order candidates are gathered in is an accident of which
    /// archetype each system sits in.
    #[test]
    fn the_better_scored_of_two_that_overlap_is_kept() {
        let better = candidate(1, Rect::new(0., 0., 10., 10.), 10.);
        let worse = candidate(2, Rect::new(5., 5., 15., 15.), 1.);

        assert_eq!(placing(&mut [better, worse]), won([1]));
        assert_eq!(placing(&mut [worse, better]), won([1]));
    }

    /// A name at the center of the view is given no more room than it ever was
    ///
    /// However far the camera stands off. What it is pointed at is what is
    /// being read, so that packs as tightly pulled back as it does up close.
    #[test]
    fn a_name_at_the_center_is_given_no_extra_room() {
        for radius in [0.1, 1., 10., TIGHT_TO, LOOSE_FROM, 50_000.] {
            assert_eq!(room(radius, 0., VIEWPORT), 0., "standing off {radius}");
        }
    }

    /// Up close every name is given the room it always was
    #[test]
    fn up_close_no_name_is_given_extra_room() {
        for apart in [0., 100., 540., VIEWPORT.length()] {
            assert_eq!(
                room(TIGHT_TO, apart, VIEWPORT),
                0.,
                "{apart} pixels out"
            );
        }
    }

    /// The middle of a wide view is a plateau and not just the top of a slope
    ///
    /// What [`relaxed`] is cubed for. A tenth of the way out should still be
    /// packed about as tightly as the middle itself, where a curve steepest at
    /// zero would already have given away a good part of the room.
    #[test]
    fn the_middle_of_a_wide_view_is_a_plateau() {
        let near = room(LOOSE_FROM, VIEWPORT.y / 10., VIEWPORT);
        let out = room(LOOSE_FROM, VIEWPORT.y / 2., VIEWPORT);

        assert!(
            near < out / 10.,
            "a tenth of the way out already gave up {near} of {out}"
        );
    }

    /// Pulled back, a name off the center is given more room than one on it
    #[test]
    fn pulled_back_the_edges_are_held_further_apart_than_the_middle() {
        let middle = room(LOOSE_FROM, 0., VIEWPORT);
        let edge = room(LOOSE_FROM, VIEWPORT.y / 2., VIEWPORT);

        assert!(edge > middle, "the edge got {edge} against {middle}");
    }

    /// The room only ever grows, with the standoff and with the distance out
    ///
    /// A name that tightened as the camera pulled further back would read as
    /// the field deciding something, which it is not.
    #[test]
    fn room_grows_with_both_distances_and_gives_none_back() {
        let mut widest = 0.;
        for radius in [0.1, 25., TIGHT_TO, 100., 250., LOOSE_FROM, 50_000.] {
            let wide = room(radius, 400., VIEWPORT);
            assert!(wide >= widest, "standing off {radius} gave room back");
            widest = wide;
        }

        let mut widest = 0.;
        for apart in [0., 50., 100., 200., 400., 800., 1_600.] {
            let wide = room(LOOSE_FROM, apart, VIEWPORT);
            assert!(wide >= widest, "{apart} pixels out gave room back");
            widest = wide;
        }
    }

    /// The far corner of a wide view is given very nearly the whole spread
    ///
    /// [`relaxed`] approaches its end rather than reaching it, so the corner
    /// is short of [`SPREAD_BY`] by the tail of an exponential and no more.
    #[test]
    fn the_far_corner_of_a_wide_view_is_given_the_whole_spread() {
        let tight = NAME_HEIGHT * CROWDING;
        let corner = tight + room(LOOSE_FROM, VIEWPORT.length(), VIEWPORT);

        assert!(
            corner > tight * (SPREAD_BY - 0.1),
            "the corner was given {corner} against a full {}",
            tight * SPREAD_BY
        );
    }

    /// Pulling the camera back leaves fewer names on screen
    ///
    /// The two halves together. [`room`] widens the rectangles and [`place`]
    /// fits fewer of them, and neither says this on its own.
    #[test]
    fn pulling_the_camera_back_leaves_fewer_names() {
        let close = placing(&mut scattered(TIGHT_TO)).len();
        let far = placing(&mut scattered(LOOSE_FROM)).len();

        assert!(far < close, "{far} names pulled back against {close} close");
    }

    /// A wide view keeps the middle denser than the edges
    ///
    /// Which is the whole point of measuring the room out from the center.
    /// Scored without regard to position, so the margin is what decides and
    /// not the score, where the map's own [`CENTER_WEIGHT`] would decide it
    /// twice over.
    #[test]
    fn a_wide_view_keeps_the_middle_denser_than_the_edges() {
        let candidates = scattered(LOOSE_FROM);
        let kept = placing(&mut candidates.clone());

        let share = |inside: bool| {
            let (mut all, mut won) = (0, 0);
            for (entity, rect, _) in &candidates {
                let apart = (rect.center() - VIEWPORT / 2.).length();
                if (apart < VIEWPORT.y / 4.) != inside {
                    continue;
                }
                all += 1;
                won += usize::from(kept.contains(entity));
            }
            won as f32 / all as f32
        };

        let middle = share(true);
        let edges = share(false);

        assert!(middle > edges, "the middle kept {middle} against {edges}");
    }

    /// A ring takes the name of anything else that would cover it
    ///
    /// What the map is pointing at keeps its place and the name gives way,
    /// since a name over the ring hides the one mark saying which system is
    /// picked out.
    #[test]
    fn a_ring_hides_a_name_drawn_over_it() {
        let ring = (
            candidate(9, Rect::new(0., 0., 0., 0.), 0.).0,
            ringed(Vec2::new(50., 50.), 20.),
        );

        let over = candidate(1, Rect::new(40., 40., 90., 60.), 1.);
        let clear = candidate(2, Rect::new(200., 40., 250., 60.), 1.);

        assert_eq!(ringing(&mut [over, clear], &[ring]), won([2]));
    }

    /// A ring never takes the name of what it rings
    ///
    /// A name stands off the ring of what it names, but the room it is
    /// granted reaches back over it, and a system picked out losing its name
    /// to its own mark helps nobody.
    #[test]
    fn a_ring_leaves_the_name_of_what_it_rings_alone() {
        let owner = candidate(1, Rect::new(40., 40., 90., 60.), 1.);
        let ring = (owner.0, ringed(Vec2::new(50., 50.), 20.));

        assert_eq!(ringing(&mut [owner], &[ring]), won([1]));
    }

    /// The grid chooses what a plain linear scan chooses
    ///
    /// Bucketing changes how the question is asked and not the answer, so the
    /// two agree over any spread of rectangles. Pseudo random rather than hand
    /// picked, since what is hand picked is what was already thought of, and
    /// seeded so that a failure can be looked at twice.
    #[test]
    fn the_grid_chooses_what_a_linear_scan_chooses() {
        // What `place` did before it bucketed.
        fn scanning(candidates: &mut [(Entity, Rect, f32)]) -> EntityHashSet {
            candidates.sort_unstable_by(|a, b| {
                b.2.total_cmp(&a.2).then(a.0.cmp(&b.0))
            });

            let mut kept: Vec<Rect> = Vec::new();
            let mut winners = EntityHashSet::default();
            for (entity, rect, _) in candidates.iter() {
                if kept.iter().any(|taken| !taken.intersect(*rect).is_empty()) {
                    continue;
                }
                kept.push(*rect);
                winners.insert(*entity);
            }
            winners
        }

        let mut seed = 0x2545_f491_u32;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };

        // Up to a couple of thousand, which is what the dense parts of the
        // database offer, and rectangles wide enough to run off the edges,
        // which is what the grid has to clamp rather than drop.
        for names in [1_u32, 20, 500, 2_000] {
            let mut candidates: Vec<_> = (1..=names)
                .map(|index| {
                    let left = next() * VIEWPORT.x;
                    let top = next() * VIEWPORT.y;
                    let width = 40. + next() * 120.;
                    let rect =
                        Rect::new(left, top, left + width, top + NAME_HEIGHT);
                    candidate(index, rect, next())
                })
                .collect();

            assert_eq!(
                placing(&mut candidates.clone()),
                scanning(&mut candidates),
                "the grid and a scan parted over {names} names"
            );
        }
    }

    /// Two names that only touch are both kept
    ///
    /// Which pins the room [`CROWDING`] already leaves around a name: the
    /// margin is inside each rectangle, so rectangles meeting edge to edge
    /// are two names with their full clearance between them.
    #[test]
    fn two_names_that_only_touch_are_both_kept() {
        let mut candidates = [
            candidate(1, Rect::new(0., 0., 10., 10.), 2.),
            candidate(2, Rect::new(10., 0., 20., 10.), 1.),
        ];

        assert_eq!(placing(&mut candidates), won([1, 2]));
    }
}
