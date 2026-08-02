use crate::Db;
use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.add_message::<Searched>();
    app.init_resource::<SearchNote>();
    app.add_systems(Update, searched.in_set(MapSet::Search));
}

/// What to tell the user about the last system they searched for
///
/// Roughly three quarters of the systems on record have no coordinates, Sol
/// among them. Flying to one is impossible, and doing nothing at all reads
/// exactly like the name not being in the database.
#[derive(Resource, Default)]
pub struct SearchNote(pub Option<String>);

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
    mut note: ResMut<SearchNote>,
    db: Res<Db>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    note.0 = match DbSystem::fetch_by_name(&db.0, &name).await {
                        Ok(origin) => match origin.position {
                            Some(p) => {
                                let position = Vec3::new(
                                    p.x as f32, p.y as f32, p.z as f32,
                                );
                                camera_events.write(MoveCamera {
                                    position: Some(position),
                                });
                                None
                            }
                            None => Some(format!(
                                "{} has no position on record",
                                origin.name
                            )),
                        },
                        Err(_) => Some(format!("No system named {name}")),
                    };
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
