use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::filter::{DimTo, Filtered};
use crate::systems::pointing::{INDICATOR, PointedAt, UNFITTED_SCALE};
use crate::systems::selection::{SELECTION, Selected};
use crate::systems::spawn::{ShowNames, Star};
use crate::systems::{Spyglass, System};
use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::entity::EntityHashSet;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_rich_text3d::{
    LoadFonts, Text3d, Text3dDimensionOut, Text3dPlugin, Text3dStyling,
    TextAnchor, TextAtlas,
};

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(Text3dPlugin { load_system_fonts: false, ..default() });
    app.insert_resource(LoadFonts {
        font_embedded: vec![include_bytes!("../../assets/gautami.ttf")],
        ..default()
    });
    app.insert_resource(NameRadius {
        follow_spyglass: true,
        radius: DEFAULT_NAME_RADIUS,
    });
    app.add_systems(Startup, init_materials);
    app.add_systems(Update, redim.in_set(MapSet::Present));
    app.add_systems(
        Update,
        (fit_name_boxes, tint_marked_names)
            .in_set(MapSet::Present)
            .after(super::pointing::point_at)
            // A name is spawned in the colour of a system at rest, so one
            // that appears because its system has just been marked out
            // draws untinted for a frame unless the tint follows the spawn.
            // Both of these want the name that exists rather than the one
            // asked for last frame.
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
            .after(super::scale::size_uniformly),
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

/// World size of a label at unit scale
const SIZE: f32 = 64.;

/// Depth floor for the label size, in light years
///
/// Size is proportional to depth, so a system level with the camera would
/// draw at nothing and one just behind it at a negative size, which is
/// mirrored. Anything this close is inside the near plane regardless.
const MIN_DEPTH: f32 = 0.1;

/// How far a name is drawn from what the camera looks at, to begin with
///
/// What [`NameRadius`] starts at, and what the tests measure against.
const DEFAULT_NAME_RADIUS: f32 = 100.;

/// How tall a system's name draws, in logical pixels
///
/// The line box, which for a single line of text is the size the font is
/// set at. The one number that decides how large a name is; everything else
/// follows from the viewport and where the camera is.
const NAME_HEIGHT: f32 = 12.;

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
    /// Never past the spyglass while it is in force, since a system it has
    /// hidden has nothing to put a name against. Overriding it draws
    /// everything loaded, and then the asking is the only limit.
    pub fn reach(&self, spyglass: &Spyglass) -> f32 {
        if self.follow_spyglass {
            spyglass.radius
        } else if spyglass.disabled {
            self.radius
        } else {
            self.radius.min(spyglass.radius)
        }
    }
}

/// Sideways gap between a star and its label, in text heights
const GAP: f32 = 0.75;

/// How far a label sits above its star, in text heights
const RISE: f32 = 1.0;

/// The family name inside `assets/gautami.ttf`, used to select it in
/// [`Text3dStyling`].
const FONT: &str = "Gautami";

/// Colour of the line joining a star to its name
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
/// Whether two names overlap is decided from their rectangles, and a name's
/// width depends on which glyphs it is made of. The width it will draw at is
/// known exactly, in `Text3dDimensionOut`, but only once the text mesh has
/// been built, and building it is what [`choose_names`] is deciding whether
/// to do. So the width is guessed from the letter count instead.
///
/// Named for the typographic advance, how far the pen moves along after
/// drawing a glyph. An `i` advances a little and a `W` a lot, so this is an
/// average over a name rather than a measurement of one.
///
/// Deliberately above that average, so that it errs wide. Erring wide costs
/// a gap between two names, and with it a third name that would have fitted
/// between them; erring narrow overlaps them, which is the thing being
/// prevented. Over the names of a hundred and fifty systems the widest ran
/// to a little under seven tenths, which is where this sits.
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
/// Two ways to be passed over and two to be asked for regardless.
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
fn worth_naming(
    shown: bool,
    filtered: bool,
    pointed_at: bool,
    selected: bool,
) -> bool {
    pointed_at || selected || (shown && !filtered)
}

/// How far the center bonus reaches, in light years
///
/// It falls to half at this distance. The point the camera orbits is usually
/// a system exactly, so this only has to forgive one that is merely near it.
const CENTER_REACH: f32 = 2.;

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

/// How far in front of the camera a point is, in light years
///
/// Depth into the view, which is not the same as the distance to the camera.
/// A point at the corner of the screen is further from the eye than one at
/// the center at the same depth, so sizing by distance draws the corner one
/// larger. At the corner of a 16:9 viewport with a quarter-turn field of
/// view, distance is about 1.31 times the depth.
///
/// Both ends come from [`OrbitCamera`], which publishes an absolute position
/// and a rotation during `Update`. The camera's `GlobalTransform` answers
/// neither question: it is written in `PostUpdate`, so it lags a frame, and
/// it holds a position relative to the floating origin rather than to the
/// galaxy. Negative behind the camera.
pub(super) fn depth(camera: &OrbitCamera, point: DVec3) -> f32 {
    let forward = (camera.rotation * Vec3::NEG_Z).as_dvec3();
    (point - camera.eye).dot(forward) as f32
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
pub(super) fn world_per_pixel(
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
    let offset = point - camera.eye;
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
/// A material per colour rather than one recoloured per name, because the
/// colour lives on a shared asset: changing it would repaint every name at
/// once. Swapping which handle a label points at repaints only that one.
///
/// Two sets of the same three: full strength, and whatever [`DimTo`] asks for
/// a name whose system the filters exclude. The dim set is recoloured in
/// place when that moves, which is the case where repainting every name at
/// once is exactly what is wanted.
#[derive(Resource)]
pub struct LabelMaterials {
    bright: [Handle<StandardMaterial>; 3],
    dim: [Handle<StandardMaterial>; 3],
    /// Drawn for a [`NameBox`], and drawing nothing
    invisible: Handle<StandardMaterial>,
}

/// Which colour a name is drawn in, given what its system is
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
    /// A name comes out the colour of the ring drawn around its star, so that
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

/// The unit rectangle every [`NameBox`] is stretched from
#[derive(Resource)]
pub struct NameBoxMesh(Handle<Mesh>);

/// How far behind its name a hit box sits, in the name's own units
///
/// Enough that the two never argue over which is in front, and far less
/// than the gap to anything else.
const BOX_DEPTH: f32 = 1.;

/// The box behind a name that catches the pointer
///
/// A name's mesh is one quad per glyph, so a ray aimed between two letters,
/// or at the space between two words, falls through it to whatever is
/// behind. Probing along a name a letter at a time found this: aimed at the
/// middle, only two thirds of names were hit at all.
///
/// This is a single rectangle covering the whole name, which is what the
/// pointer is actually tested against. Invisible, being drawn in a fully
/// transparent material, since picking only considers what is being drawn
/// and hiding it would take it out of the running.
#[derive(Component)]
pub struct NameBox;

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
    systems: Query<(Entity, &System, &Visibility, Has<Filtered>)>,
    named: Query<Entity, With<Named>>,
    pointing: Query<&PointedAt>,
    selection: Query<(), With<Selected>>,
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

    // Everything close enough to name and in front of the camera, with the
    // rectangle its name would occupy and how much it deserves one.
    let mut wanted: Vec<(Entity, Rect, f32)> = systems
        .iter()
        .filter_map(|(entity, system, visibility, filtered)| {
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

            if !worth_naming(show_names.0, filtered, pointed_at, selected) {
                return None;
            }

            let position = DVec3::from(system.position);
            let from_center = (position - orbit.center).length() as f32;
            // Further out than names were asked to reach, and not one of the
            // two the map is marking out.
            if !pointed_at && !selected && from_center > reach {
                return None;
            }
            let at = screen_position(orbit, cot_half_fov, viewport, position)?;
            let rect = name_rect(at, &system.name);
            let screen = Rect::from_corners(Vec2::ZERO, viewport);
            if screen.intersect(rect).is_empty() {
                return None;
            }
            let score = name_score(from_center, pointed_at, selected);
            Some((entity, rect, score))
        })
        .collect();

    // Best first, so that what is dropped is dropped in favour of something
    // the viewer wanted more.
    wanted.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));

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
    let pointed = if pointed_at { POINTED_WEIGHT } else { 0. };
    let picked = if selected { SELECTED_WEIGHT } else { 0. };
    picked.max(pointed) + centered - from_center
}

/// The screen rectangle a system's name would occupy, with room around it
///
/// The width is a guess from the letter count, since the mesh that would
/// give an exact one is the thing being decided about.
fn name_rect(at: Vec2, name: &str) -> Rect {
    let size = NAME_HEIGHT;
    let width = name.chars().count() as f32 * ADVANCE * size;
    let margin = size * CROWDING;

    // `face_camera` puts a name up and to the right of its system by these
    // same multiples of its height.
    let left = at.x + size * GAP;
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
pub fn respawn(
    mut commands: Commands,
    named: Query<(Entity, &System, Option<&Children>), With<Named>>,
    unnamed: Query<&Children, (With<System>, Without<Named>)>,
    labels: Query<Entity, With<Label>>,
    materials: Res<LabelMaterials>,
    box_mesh: Res<NameBoxMesh>,
) {
    for children in &unnamed {
        for child in children.iter() {
            if let Ok(label) = labels.get(child) {
                commands.entity(label).despawn();
            }
        }
    }

    for (entity, system, children) in &named {
        let labelled = children
            .is_some_and(|c| c.iter().any(|child| labels.contains(child)));
        if labelled {
            continue;
        }

        let label = commands
            .spawn((
                Label,
                Text3d::new(system.name.clone()),
                Text3dStyling {
                    size: SIZE,
                    font: FONT.into(),
                    color: Srgba::WHITE,
                    // The anchor says where the text sits relative to the
                    // entity, not which edge of the text lands on it.
                    // CENTER_RIGHT puts the name to the right of its system
                    // rather than straddling it, leaving room for the gap
                    // below.
                    anchor: TextAnchor::CENTER_RIGHT,
                    ..default()
                },
                Mesh3d::default(),
                // Whatever the system is; `tint_marked_names` runs after
                // this and settles it before the name is drawn.
                MeshMaterial3d(materials.get(Tint::Resting, false).clone()),
                // Placed by `face_camera` before the first draw.
                Transform::default(),
            ))
            .with_child((
                NameBox,
                Mesh3d(box_mesh.0.clone()),
                MeshMaterial3d(materials.invisible.clone()),
                // Sized by `fit_name_boxes` once the name has a mesh to
                // measure, and catching nothing until then, which is the
                // right answer for a name not yet drawn.
                Transform::from_scale(Vec3::splat(UNFITTED_SCALE)),
                Pickable::default(),
            ))
            .id();

        commands.entity(entity).add_child(label);
    }
}

/// Turn each label to the camera and place it beside its system
///
/// A label is a child of the system it names, which carries neither a size
/// nor a rotation of its own, so everything written here is what the label
/// is drawn with. That a system is never rotated is what lets the camera's
/// rotation be written straight into a slot that is read as local.
pub fn face_camera(
    camera: Query<(&OrbitCamera, &Camera)>,
    systems: Query<&System, Without<Label>>,
    // `Without<System>` is already true of any label. It is spelled out so
    // the scheduler can prove this query is disjoint from the one above, and
    // from every other system that reads a star's transform.
    mut labels: Query<
        (&mut Transform, &ChildOf),
        (With<Label>, Without<System>),
    >,
) {
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    for (mut label, child_of) in &mut labels {
        let Ok(system) = systems.get(child_of.parent()) else { continue };

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

        // Offset along the camera's own axes, so the label keeps sitting up
        // and to the right on screen however the view is orbited. Both are
        // multiples of the height, so they are fixed pixel gaps too.
        let offset = orbit.rotation * Vec3::X * (height * GAP)
            + orbit.rotation * Vec3::Y * (height * RISE);

        label.scale = Vec3::splat(scale);
        label.translation = offset;
        label.rotation = orbit.rotation;
    }
}

/// Join each star to its name with a line
///
/// The name sits off to one side, which is ambiguous once stars are close
/// together. Drawn as a gizmo rather than a mesh because both ends move
/// every frame the camera does, and there is nothing to keep between frames.
///
/// A line only exists where its name is drawn. Names are hidden by the
/// spyglass, by the names toggle, and by facing away from the camera, and a
/// line answering to anything less than the drawn text outlives one of them.
pub fn leaders(
    mut gizmos: Gizmos,
    labels: Query<(&GlobalTransform, &ViewVisibility, &ChildOf), With<Label>>,
    systems: Query<
        (&GlobalTransform, &Children, Has<PointedAt>, Has<Selected>),
        With<System>,
    >,
    stars: Query<&GlobalTransform, With<Star>>,
) {
    for (label, drawn, child_of) in &labels {
        if !drawn.get() {
            continue;
        }
        let Ok((system, children, pointed_at, selected)) =
            systems.get(child_of.parent())
        else {
            continue;
        };

        // A ring around a system says which one a name belongs to better
        // than a line to it does, and leaves nothing for the line to say.
        // Either ring answers, and a name the colour of the ring it belongs
        // to has already said which star it came from.
        if pointed_at || selected {
            continue;
        }

        // A name is the system's, so the line points at the system. What it
        // has to begin clear of is the star drawn there, which carries the
        // size it is drawn at where the system deliberately does not. The
        // largest of them, since a system may hold more than one, and they
        // all sit at its own position for now.
        let Some(edge) = children
            .iter()
            .filter_map(|child| stars.get(child).ok())
            .map(|star| star.scale().x)
            .reduce(f32::max)
        else {
            continue;
        };

        // The label's origin is the left edge of the text, so the line runs
        // from the system straight to where the name begins.
        let from = system.translation();
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
    mut meshes: ResMut<Assets<Mesh>>,
    dim: Res<DimTo>,
    mut commands: Commands,
) {
    commands.insert_resource(NameBoxMesh(
        meshes.add(Mesh::from(Rectangle::new(1., 1.))),
    ));
    let mut label = |tint: Srgba| assets.add(name_material(tint));

    commands.insert_resource(LabelMaterials {
        bright: Tint::ALL.map(|tint| label(tint.color())),
        dim: Tint::ALL.map(|tint| label(faded(tint.color(), dim.0))),
        invisible: label(Srgba::NONE),
    });
}

/// How a name is painted in `tint`
///
/// The glyphs are drawn white and unlit, so a material's base colour
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
/// The colour is left alone and the alpha carries it, since a name dimmed by
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

/// Stretch each hit box over the name it stands behind
///
/// The extent comes from the name's own mesh, so a box only gets its size
/// once there is a name to measure. A name is anchored so that its glyphs
/// run rightwards from its origin, which is where the offset comes from.
pub fn fit_name_boxes(
    names: Query<&Text3dDimensionOut, With<Label>>,
    mut boxes: Query<(&mut Transform, &ChildOf), With<NameBox>>,
) {
    for (mut hit_box, child_of) in &mut boxes {
        let Ok(name) = names.get(child_of.parent()) else { continue };
        let width = name.dimension.x;
        if width <= 0. {
            continue;
        }

        hit_box.translation = Vec3::new(width / 2., 0., -BOX_DEPTH);
        hit_box.scale = Vec3::new(width, name.dimension.y.max(SIZE), 1.);
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
    materials: Res<LabelMaterials>,
    mut names: Query<
        (&ChildOf, &mut MeshMaterial3d<StandardMaterial>),
        With<Label>,
    >,
) {
    for (child_of, mut material) in &mut names {
        let Ok((pointed_at, selected, filtered)) =
            systems.get(child_of.parent())
        else {
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

        assert!((depth(&camera, ahead) - 100.).abs() < 1e-3);
        assert!((depth(&camera, corner) - 100.).abs() < 1e-3);
        assert!(
            corner.length() > 130.,
            "the corner point is only {} away, too close to tell the two apart",
            corner.length()
        );
    }

    /// A system the filters exclude gives up its name
    ///
    /// A name is read or it is not, so one belonging to a dimmed star is not
    /// dimly readable, it is clutter over what the user asked to see.
    #[test]
    fn an_filtered_system_is_not_named() {
        assert!(!worth_naming(true, true, false, false));
    }

    /// And keeps it while it is pointed at or picked out
    ///
    /// Either is asking for the system by name, which is the one thing a
    /// name is for.
    #[test]
    fn a_marked_system_is_named_through_a_filter() {
        assert!(worth_naming(true, true, true, false));
        assert!(worth_naming(true, true, false, true));
    }

    /// The names toggle bars one the same way, and yields the same way
    #[test]
    fn the_names_toggle_bars_and_yields_as_a_filter_does() {
        assert!(!worth_naming(false, false, false, false));
        assert!(worth_naming(false, false, true, false));
        assert!(worth_naming(false, false, false, true));
    }

    /// A system the filters admit is named when names are on, and not when off
    #[test]
    fn an_admitted_system_follows_the_toggle() {
        assert!(worth_naming(true, false, false, false));
        assert!(!worth_naming(false, false, false, false));
    }

    /// Marked out beats both at once
    #[test]
    fn a_marked_system_is_named_with_everything_against_it() {
        assert!(worth_naming(false, true, true, false));
        assert!(worth_naming(false, true, false, true));
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

    /// A spyglass of a given reach, in force unless said otherwise
    fn spyglass(radius: f32, disabled: bool) -> Spyglass {
        Spyglass { fetch: true, radius, disabled, lock_camera: false }
    }

    /// Names reach no further than the spyglass shows
    ///
    /// A system the spyglass has hidden has nothing to put a name against,
    /// so one drawn for it would build a mesh, hold its place against the
    /// others, and never appear.
    #[test]
    fn names_reach_no_further_than_the_spyglass() {
        let asked = NameRadius { follow_spyglass: false, radius: 200. };

        assert_eq!(asked.reach(&spyglass(30., false)), 30.);
    }

    /// Following takes the spyglass's answer whatever it is
    #[test]
    fn following_the_spyglass_takes_its_reach() {
        let following = NameRadius { follow_spyglass: true, radius: 5. };

        for radius in [7., 30., 4_000.] {
            assert_eq!(following.reach(&spyglass(radius, false)), radius);
        }
    }

    /// Overriding the spyglass lets names be asked for beyond it
    ///
    /// Everything loaded is drawn then, so there is nothing left for the
    /// spyglass to say about which of it may be named.
    #[test]
    fn overriding_the_spyglass_lifts_the_ceiling() {
        let asked = NameRadius { follow_spyglass: false, radius: 200. };

        assert_eq!(asked.reach(&spyglass(30., true)), 200.);
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
    /// marked out, and the ring and the name it earns are drawn the colour
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
        assert!(
            (depth(&camera, center) - radius).abs() < 1e-2,
            "the center measured {} deep, not the {radius} the camera sits at",
            depth(&camera, center)
        );
    }
}
