use crate::camera::MoveCamera;
use crate::systems::despawn::Despawn;
use crate::systems::Spyglass;
use crate::Db;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.add_event::<Searched>();
    app.add_systems(Update, searched);
}

/// A collection of search events for responding to the user's UI
/// interactions.
#[derive(Event, Debug)]
pub enum Searched {
    System { name: String },
    Faction { name: String },
    Route { start: String, end: String, range: String },
}

/// Move the camera to the searched system
///
/// A system for responding to [`Searched`] events.
/// - On [`Searched::System`] the camera is moved to the searched system and
/// letting the `fetch` system's `fetch_around_camera` logic handle the rest.
/// - On [`Searched::Faction`] we disable the spyglass's fetch and send
/// a [`Despawn`] event for all systems.
pub fn searched(
    mut search_events: EventReader<Searched>,
    mut camera_events: EventWriter<MoveCamera>,
    mut despawner: EventWriter<Despawn>,
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
                                .send(MoveCamera { position: Some(position) });
                        }
                    }
                });
            }
            Searched::Faction { .. } => {
                spyglass.fetch = false;
                despawner.send(Despawn);
            }
            _ => {}
        };
    }
}
