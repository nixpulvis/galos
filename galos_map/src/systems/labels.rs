use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::spawn::{ShowNames, Star};
use bevy::camera::visibility::VisibilitySystems;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_rich_text3d::{
    LoadFonts, Text3d, Text3dPlugin, Text3dStyling, TextAnchor, TextAtlas,
};

pub(crate) fn plugin(app: &mut App) {
    app.add_plugins(Text3dPlugin { load_system_fonts: false, ..default() });
    app.insert_resource(LoadFonts {
        font_embedded: vec![include_bytes!("../../assets/gautami.ttf")],
        ..default()
    });
    app.insert_resource(LabelSize(16.));
    app.add_systems(Startup, init_material);
    // `face_camera` and the sizing systems both write a `Transform`, on
    // different entities, so the scheduler cannot run them together whatever
    // is said here. Ordering them costs nothing and fixes which goes first.
    app.add_systems(
        Update,
        (choose_names, respawn, face_camera)
            .chain()
            .in_set(MapSet::Present)
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

/// How far from the camera a system may be before it stops being labelled
const RADIUS: f32 = 100.;

/// How tall a system's name draws, in logical pixels
///
/// The one number that decides label size. Everything else follows from the
/// viewport and where the camera is.
#[derive(Resource)]
pub struct LabelSize(pub f32);

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

/// Average glyph width, in line heights
///
/// Used to guess how wide a name will draw before a mesh for it exists.
/// Generous on purpose: overestimating leaves a gap between two names, and
/// underestimating overlaps them, which is the thing being prevented.
const ADVANCE: f32 = 0.6;

/// How much of a name's own height is kept clear around it
///
/// Names that merely touch are still hard to read apart.
const CROWDING: f32 = 0.35;

/// How strongly population argues for a name being shown
///
/// Weighed against nearness, which is measured in light years, so this is
/// how many light years of nearness one e-fold of population is worth.
const POPULATION_WEIGHT: f32 = 6.;

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

#[derive(Resource)]
pub struct LabelMaterial(Handle<StandardMaterial>);

/// Decide which systems get to show their name
///
/// Every name inside [`RADIUS`] drawn at once is unreadable: the dev
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
    size: Res<LabelSize>,
    show_names: Res<ShowNames>,
    systems: Query<(Entity, &System)>,
    named: Query<Entity, With<Named>>,
) {
    let clear = |commands: &mut Commands| {
        for entity in &named {
            commands.entity(entity).remove::<Named>();
        }
    };

    if !show_names.0 {
        clear(&mut commands);
        return;
    }
    let Ok((orbit, camera)) = camera.single() else { return };
    let Some(viewport) = camera.logical_viewport_size() else { return };
    let cot_half_fov = camera.clip_from_view().y_axis.y;

    // Everything close enough to name and in front of the camera, with the
    // rectangle its name would occupy and how much it deserves one.
    let mut wanted: Vec<(Entity, Rect, f32)> = systems
        .iter()
        .filter_map(|(entity, system)| {
            let position = DVec3::from(system.position);
            let away = (position - orbit.eye).length() as f32;
            if away > RADIUS {
                return None;
            }
            let at = screen_position(orbit, cot_half_fov, viewport, position)?;
            let rect = name_rect(at, &system.name, size.0);
            let screen = Rect::from_corners(Vec2::ZERO, viewport);
            if screen.intersect(rect).is_empty() {
                return None;
            }
            let score = name_score(system.population, away);
            Some((entity, rect, score))
        })
        .collect();

    // Best first, so that what is dropped is dropped in favour of something
    // the viewer wanted more.
    wanted.sort_unstable_by(|a, b| b.2.total_cmp(&a.2));

    let mut kept: Vec<Rect> = Vec::new();
    let mut winners: Vec<Entity> = Vec::new();
    for (entity, rect, _) in wanted {
        if kept.iter().any(|taken| !taken.intersect(rect).is_empty()) {
            continue;
        }
        kept.push(rect);
        winners.push(entity);
    }

    clear(&mut commands);
    for entity in winners {
        commands.entity(entity).insert(Named);
    }
}

/// How much a system deserves to have its name drawn
///
/// Two claims in the same units. Nearness is what the viewer is looking at,
/// and population is what is worth looking at.
fn name_score(population: u64, away: f32) -> f32 {
    (1. + population as f32).ln() * POPULATION_WEIGHT - away
}

/// The screen rectangle a system's name would occupy, with room around it
///
/// The width is a guess from the letter count, since the mesh that would
/// give an exact one is the thing being decided about.
fn name_rect(at: Vec2, name: &str, size: f32) -> Rect {
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
    material: Res<LabelMaterial>,
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
                MeshMaterial3d(material.0.clone()),
                // Placed by `face_camera` before the first draw.
                Transform::default(),
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
    size: Res<LabelSize>,
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
        let height = size.0 * world_per_pixel;
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
    systems: Query<(&GlobalTransform, &Children), With<System>>,
    stars: Query<&GlobalTransform, With<Star>>,
) {
    for (label, drawn, child_of) in &labels {
        if !drawn.get() {
            continue;
        }
        let Ok((system, children)) = systems.get(child_of.parent()) else {
            continue;
        };

        // A name is the system's, so the line points at the system. What it
        // has to begin clear of is whatever is drawn there, which is the
        // stars, and they carry the size they are drawn at where the system
        // deliberately does not. The largest of them, since a system may hold
        // more than one, and they all sit at its own position for now.
        let drawn_radius = children
            .iter()
            .filter_map(|child| stars.get(child).ok())
            .map(|star| star.scale().x)
            .fold(0., f32::max);

        // The label's origin is the left edge of the text, so the line runs
        // from the system straight to where the name begins.
        let from = system.translation();
        let to = label.translation();
        let length = from.distance(to);
        let Some(direction) = (to - from).try_normalize() else { continue };

        // What is drawn ends at its surface, not its centre, so measure the
        // near gap from there to match the one before the first glyph.
        let gap = length * LEADER_GAP;
        let start = drawn_radius + gap;
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

pub fn init_material(
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let handle = assets.add(StandardMaterial {
        base_color_texture: Some(TextAtlas::DEFAULT_IMAGE.clone()),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    });
    commands.insert_resource(LabelMaterial(handle));
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
