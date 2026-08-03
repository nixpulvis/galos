use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::pointing::{INDICATOR, PointedAt, UNFITTED_SCALE};
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
    app.add_systems(
        Update,
        (fit_name_boxes, tint_pointed_at_names)
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
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
/// In the same light years as everything else, so this is what the focused
/// system's name is worth over one sitting at the camera itself. The best
/// score is always kept, nothing having been placed yet to crowd it out, so
/// a value above the span of the other terms makes the focused name certain
/// rather than merely likely. They span a little over two hundred as they
/// stand, which is why this sits where it does.
const FOCUS_WEIGHT: f32 = 250.;

/// How strongly being pointed at argues for a name being shown
///
/// Above [`FOCUS_WEIGHT`], so that a system under the pointer takes the top
/// of the order from the one the camera was sent to. The best score is
/// always kept, nothing having been placed yet to crowd it out, so the name
/// of whatever is pointed at is certain to be drawn and anything that would
/// have overlapped it gives way instead.
const POINTED_WEIGHT: f32 = 500.;

/// How far the focus bonus reaches, in light years
///
/// It falls to half at this distance. The point the camera orbits is usually
/// a system exactly, so this only has to forgive one that is merely near it.
const FOCUS_REACH: f32 = 2.;

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
/// the centre at the same depth, so sizing by distance draws the corner one
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

/// What a name is drawn in, resting and pointed at
///
/// Two materials rather than one recoloured, because the colour lives on a
/// shared asset: changing it would repaint every name at once. Swapping which
/// handle a label points at repaints only that one.
#[derive(Resource)]
pub struct LabelMaterials {
    resting: Handle<StandardMaterial>,
    pointed_at: Handle<StandardMaterial>,
    /// Drawn for a [`NameBox`], and drawing nothing
    invisible: Handle<StandardMaterial>,
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
    systems: Query<(Entity, &System)>,
    named: Query<Entity, With<Named>>,
    pointing: Query<&PointedAt>,
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
        .filter_map(|(entity, system)| {
            // Pointing at a system asks for its name whatever else has
            // been set, so it answers to neither of the tests below.
            //
            // Only once the pointer has come to rest, though. A system
            // crossed on the way to another would otherwise take a name,
            // and with it the place of the name being reached for.
            let pointed_at = pointing
                .get(entity)
                .is_ok_and(|at| at.settled(time.elapsed_secs()));

            // Names turned off leaves the one under the pointer the only
            // one being asked for.
            if !show_names.0 && !pointed_at {
                return None;
            }

            let position = DVec3::from(system.position);
            let from_focus = (position - orbit.focus).length() as f32;
            // Further out than names were asked to reach, and not the one
            // being pointed at. That exception cannot name something
            // invisible: a system the spyglass hides is not drawn, and what
            // is not drawn cannot be hit, so it cannot be pointed at either.
            if !pointed_at && from_focus > reach {
                return None;
            }
            let at = screen_position(orbit, cot_half_fov, viewport, position)?;
            let rect = name_rect(at, &system.name);
            let screen = Rect::from_corners(Vec2::ZERO, viewport);
            if screen.intersect(rect).is_empty() {
                return None;
            }
            let score = name_score(from_focus, pointed_at);
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
/// Being under the pointer settles it outright. Failing that, one question:
/// how far the system is from what the camera is pointed at.
/// A sharp term picks out the focused system itself, and a flat one puts
/// everything else in order of nearness to it. Strictly falling, so names
/// are awarded nearest first and the closest system to the focus is always
/// the first offered a place.
///
/// Measured from the focus and not from the camera. Everything worth naming
/// sits about one orbit radius from the eye, whichever side of the focus it
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
fn name_score(from_focus: f32, pointed_at: bool) -> f32 {
    let focused = FOCUS_WEIGHT / (1. + (from_focus / FOCUS_REACH).powi(2));
    let pointed = if pointed_at { POINTED_WEIGHT } else { 0. };

    pointed + focused - from_focus
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
                MeshMaterial3d(materials.resting.clone()),
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
    systems: Query<(&GlobalTransform, &Children, Has<PointedAt>), With<System>>,
    stars: Query<&GlobalTransform, With<Star>>,
) {
    for (label, drawn, child_of) in &labels {
        if !drawn.get() {
            continue;
        }
        let Ok((system, children, pointed_at)) = systems.get(child_of.parent())
        else {
            continue;
        };

        // A ring around a system says which one a name belongs to better
        // than a line to it does, and leaves nothing for the line to say.
        if pointed_at {
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

        // Measured out from the ring rather than from the centre, so that
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
    mut commands: Commands,
) {
    commands.insert_resource(NameBoxMesh(
        meshes.add(Mesh::from(Rectangle::new(1., 1.))),
    ));
    // The glyphs are drawn white and unlit, so a material's base colour
    // multiplies straight through them and is what a name comes out.
    let mut label = |tint: Srgba| {
        assets.add(StandardMaterial {
            base_color: tint.into(),
            base_color_texture: Some(TextAtlas::DEFAULT_IMAGE.clone()),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        })
    };

    commands.insert_resource(LabelMaterials {
        resting: label(Srgba::WHITE),
        pointed_at: label(INDICATOR),
        invisible: label(Srgba::NONE),
    });
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

/// Tint a name while its system is pointed at
///
/// Keyed on the system rather than on the name, so that pointing at a star
/// lights its name as well, and both go out together.
pub fn tint_pointed_at_names(
    pointed_at: Query<(), With<PointedAt>>,
    materials: Res<LabelMaterials>,
    mut names: Query<
        (&ChildOf, &mut MeshMaterial3d<StandardMaterial>),
        With<Label>,
    >,
) {
    for (child_of, mut material) in &mut names {
        let wanted = if pointed_at.contains(child_of.parent()) {
            &materials.pointed_at
        } else {
            &materials.resting
        };
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

    /// Names are offered in order of nearness to what is focused
    ///
    /// The score falls the whole way out, so the greedy pass takes the
    /// nearest system first and works outwards. Nothing further away can
    /// take a place from something closer, which is what makes the ordering
    /// something a viewer can predict rather than a ranking to be read.
    #[test]
    fn nearer_systems_are_always_offered_a_name_first() {
        let mut nearer = name_score(0., false);
        for step in 1..=1000 {
            let further =
                name_score(step as f32 * DEFAULT_NAME_RADIUS / 1000., false);
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
    /// it gives way. It has to beat the focused system to do that, which is
    /// the only other claim strong enough to matter.
    #[test]
    fn what_is_pointed_at_outranks_what_is_focused() {
        // Pointed at, and as far out as a name is ever drawn.
        let pointed = name_score(DEFAULT_NAME_RADIUS, true);

        // The focused system itself, which is otherwise the best there is.
        let focused = name_score(0., false);

        assert!(
            pointed > focused,
            "pointed at scored {pointed}, behind the {focused} of the focus"
        );
    }

    /// The focus bonus falls away with distance from what is focused
    ///
    /// Otherwise every system in the neighbourhood would inherit the claim
    /// of the one at the middle of it, and the sharp term would be doing
    /// the flat term's job.
    #[test]
    fn the_focus_bonus_is_local_to_the_focus() {
        let bonus = |d: f32| FOCUS_WEIGHT / (1. + (d / FOCUS_REACH).powi(2));

        assert!(bonus(FOCUS_REACH) < bonus(0.) * 0.6);
        assert!(
            bonus(FOCUS_REACH * 10.) < FOCUS_WEIGHT * 0.05,
            "a system ten reaches out still held {} of the bonus",
            bonus(FOCUS_REACH * 10.)
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
    /// `orbit_camera` places the eye at `focus + rotation * Z * radius`, so
    /// this pins the helper to the convention the camera is written to. A
    /// forward of `+Z` would put the focus behind the camera instead.
    #[test]
    fn depth_agrees_with_where_the_camera_puts_its_eye() {
        let rotation = Quat::from_euler(EulerRot::YXZ, 0.9, -0.4, 0.);
        let focus = DVec3::new(1234.5, -678.9, 4321.);
        let radius = 250f32;
        let eye = focus + (rotation * Vec3::Z * radius).as_dvec3();

        let camera = OrbitCamera { eye, rotation, ..default() };
        assert!(
            (depth(&camera, focus) - radius).abs() < 1e-2,
            "the focus measured {} deep, not the {radius} the camera sits at",
            depth(&camera, focus)
        );
    }
}
