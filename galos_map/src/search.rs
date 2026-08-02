use crate::Db;
use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::Database;
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

/// Where a named system is, or why the map cannot go there
///
/// Both a plain system search and either end of a route need this same
/// answer, and both need to say the same thing when they cannot get it.
async fn locate(db: &Database, name: &str) -> Result<Vec3, String> {
    match DbSystem::fetch_by_name(db, name).await {
        Ok(system) => match system.position {
            Some(p) => Ok(Vec3::new(p.x as f32, p.y as f32, p.z as f32)),
            None => Err(format!("{} has no position on record", system.name)),
        },
        Err(_) => Err(format!("No system named {name}")),
    }
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
                    note.0 = match locate(&db.0, name).await {
                        Ok(position) => {
                            camera_events
                                .write(MoveCamera { position: Some(position) });
                            None
                        }
                        Err(why) => Some(why),
                    };
                });
            }
            // A route needs both ends. Say which one is the problem rather
            // than drawing nothing and leaving the user to guess.
            Searched::Route { start, end, .. } => {
                future::block_on(async {
                    note.0 = locate(&db.0, start)
                        .await
                        .and(locate(&db.0, end).await)
                        .err();
                });
            }
            Searched::Faction { .. } => {
                spyglass.fetch = false;
                despawner.write(Despawn);
            }
        };
    }
}
