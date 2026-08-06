use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space::Galaxy;
use crate::systems::System;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_message::<Despawn>();
    app.add_systems(
        Update,
        despawn.in_set(MapSet::Populate).after(super::spawn::spawn),
    );
}

#[derive(Message)]
pub struct Despawn;

pub fn despawn(
    mut commands: Commands,
    systems: Query<(Entity, &System)>,
    camera: Query<Entity, With<OrbitCamera>>,
    galaxy: Res<Galaxy>,
    mut events: MessageReader<Despawn>,
) {
    for _ in events.read() {
        // Up out of whatever it was standing in first. A camera that has
        // descended into a system is a child of it, and despawning a system
        // takes its children with it.
        if let Ok(eye) = camera.single() {
            commands.entity(eye).insert(ChildOf(galaxy.0));
        }
        for (entity, _) in systems.iter() {
            commands.entity(entity).despawn();
        }
    }
}
