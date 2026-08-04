use crate::Db;
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use elite_journal::system::Coordinate;
use galos_db::Database;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.add_message::<Searched>();
    app.init_resource::<SearchNote>();
    app.init_resource::<SearchResults>();
    app.init_resource::<Plot>();
    app.add_systems(Update, searched.in_set(MapSet::Search));
}

/// How many systems a search answers with
///
/// Enough that the one wanted is among them, and few enough that the database
/// is asked for a screenful rather than for every system holding a common
/// fragment. `%col%` matches better than a hundred thousand of them.
const RESULTS: i64 = 25;

/// What the last search turned up, for the bar to offer
///
/// The database's own rows rather than the map's [`System`]s. Roughly three
/// quarters of the systems on record have no position, and those are worth
/// showing: a user searching a name wants to know it exists even where the
/// map cannot draw it. A map [`System`] is a thing with somewhere to be, so
/// the unplaceable ones would have to be dropped to hold them here, and the
/// answer would be missing the systems it most needs to account for.
#[derive(Resource, Default)]
pub struct SearchResults(Vec<DbSystem>);

impl SearchResults {
    /// The systems on offer, best first
    pub fn iter(&self) -> impl Iterator<Item = &DbSystem> {
        self.0.iter()
    }

    /// Whether there is anything to offer
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Offer `found` instead of whatever was on offer
    pub fn set(&mut self, found: Vec<DbSystem>) {
        self.0 = found;
    }

    /// Put the list away
    ///
    /// What a search turned up answers the name it was asked about, so it is
    /// no answer at all once that name is being typed over. Picking one of
    /// them out does not put the list away: choosing is what it is for, and
    /// several may be chosen from the one list.
    pub fn clear(&mut self) {
        self.0.clear();
    }
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
///
/// A focus is not one of these. Asking for a focus names something and the
/// map neither goes there, fetches it, nor picks it out, so it is asked for
/// by [`crate::systems::focus::Wanted`] instead.
#[derive(Message, Debug)]
pub enum Searched {
    System { name: String },
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
/// - On [`Searched::System`] the systems that might be meant are looked up and
/// left as a list for the user to choose from. Nothing is picked out by the
/// search itself, however exactly the name was typed: a search says which
/// systems are on record under that name and a click says which of them is
/// meant. Keeping the two apart is what lets a set be gathered across
/// searches, since a search that picked something out would let go of
/// everything gathered before it. The camera is left where it is for the same
/// reason, and the map has a control of its own for going there.
/// - On [`Searched::Route`] both ends are resolved, and which of them could
/// not be is what the form is told.
///
/// The search is measured from where the camera is looking, so that a common
/// fragment answers with the systems in front of the user rather than with
/// whichever ones the database reached first.
pub fn searched(
    mut search_events: MessageReader<Searched>,
    mut note: ResMut<SearchNote>,
    mut results: ResMut<SearchResults>,
    mut plot: ResMut<Plot>,
    camera: Query<&OrbitCamera>,
    db: Res<Db>,
) {
    let near = camera
        .single()
        .map(|camera| Coordinate {
            x: camera.center.x,
            y: camera.center.y,
            z: camera.center.z,
        })
        .ok();

    for event in search_events.read() {
        match event {
            Searched::System { name, .. } => {
                future::block_on(async {
                    let found =
                        DbSystem::search_by_name(&db.0, name, near, RESULTS)
                            .await
                            .unwrap_or_default();

                    // Whatever was on offer answered the last name asked
                    // about, and this is a new one. What is picked out is
                    // left alone: it was picked out by a click rather than
                    // by the search before it, and a search is not a reason
                    // to let go of it.
                    results.clear();
                    note.0 = if found.is_empty() {
                        Some(format!("No system named {name}"))
                    } else {
                        results.set(found);
                        None
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
        };
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A system the database might answer a search with
    pub(crate) fn row(name: &str) -> DbSystem {
        DbSystem {
            address: name.len() as i64,
            name: name.to_owned(),
            position: Some(Coordinate { x: 0., y: 0., z: 0. }),
            population: 0,
            security: None,
            government: None,
            allegiance: None,
            primary_economy: None,
            secondary_economy: None,
            factions: vec![],
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
        }
    }
}
