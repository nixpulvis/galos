use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::spawn::ShowNames;
use bevy::prelude::*;
use bevy_panorbit_camera::PanOrbitCamera;
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
    // Labels follow their star's transform. `face_camera` in particular
    // divides by the scale the `scale` systems give a star, and needs this
    // frame's value rather than the previous frame's.
    app.add_systems(
        Update,
        (respawn, visibility, face_camera)
            .chain()
            .in_set(MapSet::Present)
            .after(super::scale::scale_systems)
            .after(super::scale::scale_stars),
    );
}

/// World size of a label at unit scale
const SIZE: f32 = 64.;

/// Tuning factor for how large labels draw, before distance is applied
///
/// This is a world scale. Labels used to inherit the star's scale, so the
/// equivalent value here is roughly a tenth of what it was.
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

/// A marker for system name labels
#[derive(Component)]
pub struct Label;

#[derive(Resource)]
pub struct LabelMaterial(Handle<StandardMaterial>);

/// Spawn and despawn system labels
pub fn respawn(
    mut commands: Commands,
    camera: Query<&Transform, With<PanOrbitCamera>>,
    systems: Query<(Entity, &System, &Transform, Option<&Children>)>,
    labels: Query<Entity, With<Label>>,
    show_names: Res<ShowNames>,
    material: Res<LabelMaterial>,
) {
    let Ok(camera) = camera.single() else { return };
    let camera_translation = camera.translation;

    for (system_entity, system, system_transform, children) in systems.iter() {
        let d = camera_translation.distance(system_transform.translation);

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

/// Turn labels toward the camera, and place them beside their star
///
/// Labels are children of their system, so they inherit the scale that
/// [`super::scale`] gives the star. Every value here is therefore divided by
/// that scale to land on the intended world size and offset. Systems are
/// unrotated, so the camera's rotation can be copied straight in.
pub fn face_camera(
    camera: Query<&Transform, With<PanOrbitCamera>>,
    systems: Query<&Transform, (With<System>, Without<Label>)>,
    // `Without<System>` and `Without<PanOrbitCamera>` are already true of any
    // label. They are spelled out so the scheduler can prove this query is
    // disjoint from the two above, and from every other system that reads a
    // star's or the camera's transform.
    mut labels: Query<
        (&mut Transform, &ChildOf),
        (With<Label>, Without<System>, Without<PanOrbitCamera>),
    >,
) {
    let Ok(camera) = camera.single() else { return };

    for (mut label, child_of) in &mut labels {
        let Ok(system) = systems.get(child_of.parent()) else { continue };

        // Measure to the star, not to the label's offset within it.
        let d = camera.translation.distance(system.translation);
        let parent_scale = system.scale.x.max(f32::EPSILON);
        let scale = 0.75 * d.max(MIN_DISTANCE).ln() * SCALE;

        // Offset along the camera's own axes, so the label keeps sitting up
        // and to the right on screen however the view is orbited.
        let height = SIZE * scale;
        let offset = camera.rotation * Vec3::X * (height * GAP)
            + camera.rotation * Vec3::Y * (height * RISE);

        label.scale = Vec3::splat(scale / parent_scale);
        label.translation = offset / parent_scale;
        label.rotation = camera.rotation;
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
