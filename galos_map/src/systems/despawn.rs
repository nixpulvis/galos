use crate::systems::System;
use bevy::prelude::*;

pub fn plugin(app: &mut App) {
    app.add_message::<Despawn>();
    app.add_systems(Update, despawn.after(super::spawn::spawn));
}

#[derive(Message)]
pub struct Despawn;

pub fn despawn(
    mut commands: Commands,
    systems: Query<(Entity, &System)>,
    mut events: MessageReader<Despawn>,
) {
    for _ in events.read() {
        for (entity, _) in systems.iter() {
            commands.entity(entity).despawn();
        }
    }
}
