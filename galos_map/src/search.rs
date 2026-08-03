use crate::Db;
use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::systems::despawn::Despawn;
use crate::systems::selection::Selection;
use crate::systems::{Spyglass, System, system_to_vec};
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

/// The row for a named system the map can go to, or why it cannot
///
/// Both a plain system search and either end of a route need this same
/// answer, and both need to say the same thing when they cannot get it.
///
/// The whole row rather than only where it is, since a search is also how a
/// system comes to be selected and the panel describing it has nothing else
/// to read: the map does not fetch the system until the camera arrives.
async fn locate(db: &Database, name: &str) -> Result<DbSystem, String> {
    match DbSystem::fetch_by_name(db, name).await {
        Ok(system) if system.position.is_some() => Ok(system),
        Ok(system) => Err(format!("{} has no position on record", system.name)),
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
    mut selection: ResMut<Selection>,
    db: Res<Db>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    note.0 = match locate(&db.0, name).await {
                        Ok(row) => {
                            camera_events.write(MoveCamera {
                                position: system_to_vec(&row),
                            });
                            // Named is as good as picked out. The map has
                            // nothing to mark until the camera gets there,
                            // but the panel can say what the row says now.
                            if let Ok(system) = System::try_from(&row) {
                                selection.set(system);
                            }
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
