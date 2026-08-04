use crate::Db;
use crate::schedule::MapSet;
use crate::systems::despawn::Despawn;
use crate::systems::selection::Selection;
use crate::systems::{Spyglass, System};
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::Database;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.add_message::<Searched>();
    app.init_resource::<SearchNote>();
    app.init_resource::<Plot>();
    app.add_systems(Update, searched.in_set(MapSet::Search));
}

/// What to tell the user about the last name they searched for
///
/// Roughly three quarters of the systems on record have no coordinates, Sol
/// among them. Flying to one is impossible, and doing nothing at all reads
/// exactly like the name not being in the database.
#[derive(Resource, Default)]
pub struct SearchNote(pub Option<String>);

/// How the route last asked for is getting on
///
/// Routing is worked out against the database in the background, and until
/// it comes back nothing is drawn. Neither is anything drawn for a route
/// that was asked for and does not exist, so without somewhere to say which
/// is which, a plot still being worked out and one that failed look exactly
/// alike: nothing happens either way.
#[derive(Resource, Default, PartialEq, Eq)]
pub enum Plot {
    /// Nothing has been asked for, or what was asked for is drawn
    #[default]
    Nothing,
    /// Asked for, and not yet come back
    Working,
    /// Why the route could not be plotted
    Trouble(String),
}

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

/// Answer what the user asked for
///
/// A system for responding to [`Searched`] messages.
/// - On [`Searched::System`] the named system is picked out, and the camera
/// is left where it is. Naming a system is asking which one it is, not
/// asking to be taken there, and the map has a control of its own for that.
/// - On [`Searched::Faction`] we disable the spyglass's fetch and send
/// a [`Despawn`] message for all systems.
pub fn searched(
    mut search_events: MessageReader<Searched>,
    mut despawner: MessageWriter<Despawn>,
    mut spyglass: ResMut<Spyglass>,
    mut note: ResMut<SearchNote>,
    mut plot: ResMut<Plot>,
    mut selection: ResMut<Selection>,
    db: Res<Db>,
) {
    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    note.0 = match locate(&db.0, name).await {
                        Ok(row) => {
                            // The map has nothing to mark until the system
                            // is fetched, but the row the name resolved
                            // against says everything a panel would.
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
                    *plot = match locate(&db.0, start)
                        .await
                        .and(locate(&db.0, end).await)
                    {
                        Ok(_) => Plot::Working,
                        Err(why) => Plot::Trouble(why),
                    };
                });
            }
            Searched::Faction { .. } => {
                spyglass.fetch = false;
                despawner.write(Despawn);
            }
        };
    }
}
