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
    app.add_systems(Startup, init_material);
    // `face_camera` and the sizing systems both write a `Transform`, on
    // different entities, so the scheduler cannot run them together whatever
    // is said here. Ordering them costs nothing and fixes which goes first.
    app.add_systems(
        Update,
        (respawn, visibility, face_camera)
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

/// Tuning factor for how large labels draw, before distance is applied
///
/// A world scale, applied to the label alone. A label is a child of its
/// system, which carries no size, so this is the size it is drawn at.
const SCALE: f32 = 0.0032;

/// How far from the camera a system may be before it stops being labelled
const RADIUS: f32 = 100.;

/// Sideways gap between a star and its label, in text heights
const GAP: f32 = 0.75;

/// How far a label sits above its star, in text heights
const RISE: f32 = 1.0;

/// Distance floor for the label scale curve
///
/// Scale grows with `ln(distance)`, which is zero at 1 and negative below
/// that. A label at zero scale is invisible and one at negative scale is
/// mirrored, so the curve is clamped before it reaches either.
const MIN_DISTANCE: f32 = 2.;

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

/// A marker for system name labels
#[derive(Component)]
pub struct Label;

#[derive(Resource)]
pub struct LabelMaterial(Handle<StandardMaterial>);

/// Spawn and despawn system labels
pub fn respawn(
    mut commands: Commands,
    camera: Query<&OrbitCamera>,
    systems: Query<(Entity, &System, Option<&Children>)>,
    labels: Query<Entity, With<Label>>,
    show_names: Res<ShowNames>,
    material: Res<LabelMaterial>,
) {
    let Ok(camera) = camera.single() else { return };
    let eye = camera.eye;

    for (system_entity, system, children) in systems.iter() {
        let d = eye.distance(DVec3::from(system.position)) as f32;

        if d > RADIUS {
            if let Some(children) = children {
                for child in children.iter() {
                    if let Ok(label_entity) = labels.get(child) {
                        commands.entity(label_entity).despawn();
                    }
                }
            }
        } else {
            let labelled = children
                .is_some_and(|c| c.iter().any(|child| labels.contains(child)));
            if !labelled {
                let label = {
                    let mut label_entity = commands.spawn((
                        Label,
                        Text3d::new(system.name.clone()),
                        Text3dStyling {
                            size: SIZE,
                            font: FONT.into(),
                            color: Srgba::WHITE,
                            // The anchor says where the text sits relative
                            // to the entity, not which edge of the text
                            // lands on it. CENTER_RIGHT puts the name to
                            // the right of its star rather than straddling
                            // it, leaving room for the gap below.
                            anchor: TextAnchor::CENTER_RIGHT,
                            ..default()
                        },
                        Mesh3d::default(),
                        MeshMaterial3d(material.0.clone()),
                        // Placed by `face_camera` before the first draw.
                        Transform::default(),
                    ));

                    if !show_names.0 {
                        label_entity.insert(Visibility::Hidden);
                    }

                    label_entity.id()
                };

                commands.entity(system_entity).add_child(label);
            }
        }
    }
}

/// Turn each label to the camera and place it beside its system
///
/// A label is a child of the system it names, which carries neither a size
/// nor a rotation of its own, so everything written here is what the label
/// is drawn with. That a system is never rotated is what lets the camera's
/// rotation be written straight into a slot that is read as local.
pub fn face_camera(
    camera: Query<&OrbitCamera>,
    systems: Query<&System, Without<Label>>,
    // `Without<System>` is already true of any label. It is spelled out so
    // the scheduler can prove this query is disjoint from the one above, and
    // from every other system that reads a star's transform.
    mut labels: Query<
        (&mut Transform, &ChildOf),
        (With<Label>, Without<System>),
    >,
) {
    let Ok(camera) = camera.single() else { return };

    for (mut label, child_of) in &mut labels {
        let Ok(system) = systems.get(child_of.parent()) else { continue };

        // Measure to the star, not to the label's offset within it.
        let d = camera.eye.distance(DVec3::from(system.position)) as f32;
        let scale = 0.75 * d.max(MIN_DISTANCE).ln() * SCALE;

        // Offset along the camera's own axes, so the label keeps sitting up
        // and to the right on screen however the view is orbited.
        let height = SIZE * scale;
        let offset = camera.rotation * Vec3::X * (height * GAP)
            + camera.rotation * Vec3::Y * (height * RISE);

        label.scale = Vec3::splat(scale);
        label.translation = offset;
        label.rotation = camera.rotation;
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

/// Add visibility components when ShowName changes
pub fn visibility(
    mut commands: Commands,
    labels: Query<Entity, With<Label>>,
    show_names: Res<ShowNames>,
) {
    if show_names.is_changed() {
        for entity in &labels {
            if show_names.0 {
                commands.entity(entity).insert(Visibility::Inherited);
            } else {
                commands.entity(entity).insert(Visibility::Hidden);
            }
        }
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
