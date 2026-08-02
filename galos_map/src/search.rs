use crate::Db;
use crate::camera::MoveCamera;
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.add_message::<Searched>();
    app.add_systems(Update, searched);
}

/// A collection of search messages for responding to the user's UI
/// interactions.
#[derive(Message, Debug)]
pub enum Searched {
    System { name: String },
    Faction { name: String },
    Route { start: String, end: String, range: String },
}

/// Move the camera to the searched system
///
/// A system for responding to [`Searched`] messages.
/// - On [`Searched::System`] the camera is moved to the searched system and
/// letting the `fetch` system's `fetch_around_camera` logic handle the rest.
/// - On [`Searched::Faction`] we disable the spyglass's fetch and send
/// a [`Despawn`] message for all systems.
pub fn searched(
    mut search_events: MessageReader<Searched>,
    mut camera_events: MessageWriter<MoveCamera>,
    mut despawner: MessageWriter<Despawn>,
    mut spyglass: ResMut<Spyglass>,
    db: Res<Db>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    if let Ok(origin) =
                        DbSystem::fetch_by_name(&db.0, &name).await
                    {
                        if let Some(p) = origin.position {
                            let position =
                                Vec3::new(p.x as f32, p.y as f32, p.z as f32);
                            camera_events
                                .write(MoveCamera { position: Some(position) });
                        }
                    }
                });
            }
            Searched::Faction { .. } => {
                spyglass.fetch = false;
                despawner.write(Despawn);
            }
            _ => {}
        };
    }
}
