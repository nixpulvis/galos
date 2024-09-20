use super::ShowNames;
use super::System;
use bevy::prelude::*;
use bevy_mod_billboard::Billboard;
use bevy_mod_billboard::BillboardLockAxis;
use bevy_mod_billboard::BillboardTextBundle;
use bevy_panorbit_camera::PanOrbitCamera;

const SCALE: f32 = 0.02;
const SIZE: f32 = 64.;
const RADIUS: f32 = 35.;

/// Spawn and despawn system labels
pub fn respawn(
    mut commands: Commands,
    camera: Query<&Transform, With<PanOrbitCamera>>,
    systems: Query<(Entity, &System, &Transform, Option<&Children>)>,
    billboards: Query<Entity, With<Billboard>>,
    show_names: Res<ShowNames>,
    asset_server: Res<AssetServer>,
) {
    let font = asset_server.load("gautami.ttf");
    let camera_translation = camera.single().translation;

    for (system_entity, system, system_transform, children) in systems.iter() {
        let d = camera_translation.distance(system_transform.translation);

        if d > RADIUS {
            if let Some(children) = children {
                for &billboard_entity in children.iter() {
                    if let Ok(billboard_entity) =
                        billboards.get(billboard_entity)
                    {
                        commands
                            .entity(system_entity)
                            .remove_children(&[billboard_entity]);
                        commands.entity(billboard_entity).despawn();
                    }
                }
            }
        } else {
            if children.map_or(true, |c| c.iter().len() == 0) {
                let mut system_entity = commands.entity(system_entity);
                let mut commands = system_entity.commands();
                let billboard = {
                    let mut billboard_entity = commands.spawn((
                        BillboardTextBundle {
                            transform: Transform::from_scale(Vec3::splat(
                                SCALE,
                            ))
                            .with_translation(Vec3::new(3., 0., 0.)),
                            text: Text::from_section(
                                system.name.clone(),
                                TextStyle {
                                    font_size: SIZE,
                                    font: font.clone(),
                                    color: Color::WHITE,
                                },
                            )
                            .with_justify(JustifyText::Left),
                            ..default()
                        },
                        BillboardLockAxis::default(),
                    ));

                    if !show_names.0 {
                        billboard_entity.insert(Visibility::Hidden);
                    }

                    billboard_entity.id()
                };

                system_entity.add_child(billboard);
            }
        }
    }
}

pub fn scale(
    camera: Query<&Transform, With<PanOrbitCamera>>,
    mut labels: Query<
        &mut Transform,
        (With<Billboard>, Without<PanOrbitCamera>),
    >,
) {
    let camera_translation = camera.single().translation;

    for mut label in &mut labels {
        let d = camera_translation.distance(label.translation);
        label.scale = Vec3::splat(0.75 * d.ln() * SCALE); //Vec3::splat(d.ln() / 25.);
    }
}

/// Add visibility components when ShowName changes
pub fn visibility(
    mut commands: Commands,
    billboards: Query<(Entity, &Billboard)>,
    show_names: Res<ShowNames>,
) {
    if show_names.is_changed() {
        // TODO: remove iter()?
        for (entity, _) in billboards.iter() {
            if show_names.0 {
                commands.entity(entity).insert(Visibility::Visible);
            } else {
                commands.entity(entity).insert(Visibility::Hidden);
            }
        }
    }
}
