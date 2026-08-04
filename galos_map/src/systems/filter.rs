//! Narrowing the map down to what the user is looking for
//!
//! A filter is a question asked of every system, and several are asked at
//! once: a system passes when all of them admit it. What fails is not taken
//! off the map, it is drawn faintly, so a faction is read against the space
//! around it rather than against an empty sky.
//!
//! This is a layer over the map rather than a mode. The spyglass goes on
//! fetching by region, the camera stays where it is, and nothing is
//! despawned. All that changes is how brightly each system is drawn.
//!
//! [`DimTo`] says how faintly, and answers for the whole of it. At zero an
//! excluded system is not drawn at all, and then it is not worth fetching
//! either, which is where the filter reaches the database. Anywhere above
//! zero the excluded systems are wanted on screen, so they have to be fetched
//! to be dimmed: what was never asked for cannot be drawn faintly.

use crate::Db;
use crate::schedule::MapSet;
use crate::systems::System;
use bevy::prelude::*;
use bevy::tasks::futures_lite::future;
use galos_db::Database;
use galos_db::factions::Faction as DbFaction;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.init_resource::<Filters>();
    app.init_resource::<DimTo>();
    app.init_resource::<FilterNote>();
    app.add_message::<Wanted>();
    // Answering what the user asked for, so with the rest of that.
    app.add_systems(Update, resolve.in_set(MapSet::Search));
    // After the systems it marks exist. A system spawned this frame is
    // marked by `spawn` itself, since commands do not land until the next
    // sync point and nothing here could see it in time.
    app.add_systems(
        Update,
        mark.in_set(MapSet::Populate).after(super::spawn::spawn),
    );
}

/// One question asked of every system
///
/// The id is what a system is tested against and the name is what the filter
/// row says it is. Both are settled when the faction is picked out of a list,
/// so neither has to be looked up again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Systems the named faction is present in
    Faction { id: i32, name: String },
}

impl Filter {
    /// Whether this filter admits `system`
    fn admits(&self, system: &System) -> bool {
        match self {
            Filter::Faction { id, .. } => system.factions.contains(id),
        }
    }

    /// What the filter is asking for, as a row can say it
    pub fn name(&self) -> &str {
        match self {
            Filter::Faction { name, .. } => name,
        }
    }

    /// Every system this filter admits, as far as the database knows
    ///
    /// What a filter's panel lists. Asked of the database rather than of the
    /// map, since where a faction is, is most of what is being asked, and the
    /// map holds only what the spyglass has dragged in.
    ///
    /// Here rather than beside the panel that draws it, so that a kind of
    /// filter is one arm of each of these rather than something to be traced
    /// through the modules that happen to use it.
    pub async fn systems(&self, db: &Database) -> Vec<DbSystem> {
        match self {
            Filter::Faction { name, .. } => {
                DbSystem::fetch_faction(db, name).await.unwrap_or_default()
            }
        }
    }
}

/// A filter the user has asked for, before it is known to exist
///
/// What is typed is a name and what a filter tests against is an id, so
/// something has to look one up for the other. That is a database question,
/// asked here rather than in the bar, which draws during egui's own pass and
/// has no business waiting on anything.
///
/// Its own message rather than a [`crate::search::Searched`]: asking for a
/// filter is not searching. The map goes nowhere, fetches nothing in
/// particular, and picks nothing out.
#[derive(Message, Debug)]
pub enum Wanted {
    /// Systems a faction of this name is present in
    Faction { name: String },
}

impl Wanted {
    /// The filter this asks for, or why there is none
    async fn resolve(&self, db: &Database) -> Result<Filter, String> {
        match self {
            Wanted::Faction { name } => {
                match DbFaction::fetch_by_name(db, name).await {
                    Ok(faction) => Ok(Filter::Faction {
                        id: faction.id,
                        name: faction.name,
                    }),
                    Err(_) => Err(format!("No faction named {name}")),
                }
            }
        }
    }
}

/// What to tell the user about the last filter they asked for
///
/// Its own line rather than [`crate::search::SearchNote`], which answers a
/// name typed into the search input. Two unrelated answers sharing one line
/// means each wipes the other: adding a faction would clear a note about a
/// system that was never found, and the note about a faction would be read
/// out under the box that has nothing to do with it.
#[derive(Resource, Default)]
pub struct FilterNote(pub Option<String>);

/// Turn what the user asked for into a filter, or say why not
fn resolve(
    mut wanted: MessageReader<Wanted>,
    mut filters: ResMut<Filters>,
    mut note: ResMut<FilterNote>,
    db: Res<Db>,
) {
    for asked in wanted.read() {
        // Waited on, as a search is. A name resolves against one indexed row
        // and this answers something the user just did.
        future::block_on(async {
            note.0 = match asked.resolve(&db.0).await {
                Ok(filter) => {
                    filters.add(filter);
                    None
                }
                Err(why) => Some(why),
            };
        });
    }
}

/// A filter, and whether it is being applied
///
/// Off without being taken away, so that one can be lifted to see what it was
/// hiding and put back without being typed in again.
#[derive(Debug, Clone)]
pub struct Active {
    pub filter: Filter,
    pub enabled: bool,
}

/// Every filter the user has added
#[derive(Resource, Default)]
pub struct Filters(Vec<Active>);

impl Filters {
    /// Whether every enabled filter admits `system`
    ///
    /// All of them, so filters narrow as they are added. A system passing
    /// some but not others is not what any of them was asking for.
    pub fn admit(&self, system: &System) -> bool {
        self.0
            .iter()
            .filter(|active| active.enabled)
            .all(|active| active.filter.admits(system))
    }

    /// Add `filter`, unless it is already being asked
    ///
    /// Asking the same thing twice narrows nothing and leaves two rows that
    /// have to be turned off one at a time.
    pub fn add(&mut self, filter: Filter) {
        if self.0.iter().any(|active| active.filter == filter) {
            return;
        }
        self.0.push(Active { filter, enabled: true });
    }

    /// Stop asking the filter at `index`
    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    /// The filters in the order they were added
    pub fn iter(&self) -> impl Iterator<Item = &Active> {
        self.0.iter()
    }

    /// Turn the filter at `index` on or off
    pub fn toggle(&mut self, index: usize) {
        if let Some(active) = self.0.get_mut(index) {
            active.enabled = !active.enabled;
        }
    }
}

/// A system no enabled filter admits
///
/// The verdict is one bit because filters are ANDed, so it is carried by a
/// marker and how faintly to draw lives apart in [`DimTo`]. Written when the
/// filters change or a system does, rather than worked out afresh by each of
/// the things that draws from it.
#[derive(Component)]
pub struct Filtered;

/// How brightly a system the filters exclude is drawn
///
/// A fraction of what it would be drawn at unfiltered, so one is untouched
/// and zero is not drawn at all. Reads as what it does: dim to a quarter, dim
/// to nothing.
///
/// Zero is not merely invisible. A star faded to nothing is still a star
/// being drawn, and still one the pointer can land on, so zero hides it
/// outright, which takes its name, its ring and its hit box with it.
#[derive(Resource)]
pub struct DimTo(pub f32);

impl Default for DimTo {
    fn default() -> Self {
        DimTo(DEFAULT_DIM)
    }
}

impl DimTo {
    /// `color` as it should be drawn for a system, given whether it is
    /// excluded
    ///
    /// The colour is left alone and the alpha carries it, so that what is
    /// dimmed reads as standing further back rather than as having changed
    /// into something else.
    ///
    /// For what is painted straight rather than through a material: the two
    /// rings are gizmos, and a gizmo takes its colour at the call.
    pub fn against(&self, color: Srgba, filtered: bool) -> Srgba {
        if filtered {
            Srgba { alpha: color.alpha * self.0, ..color }
        } else {
            color
        }
    }
}

/// How faint an excluded system is to begin with
///
/// Enough to read as background rather than as something picked out, and
/// enough to still be seen: the point of dimming rather than hiding is that
/// the space around a faction stays legible.
const DEFAULT_DIM: f32 = 0.2;

/// Keep the mark on whichever systems the filters exclude
///
/// Only where something has changed. This runs over every system on the map,
/// and the filters are usually quiet, so the common case is a walk that
/// writes nothing.
fn mark(
    filters: Res<Filters>,
    systems: Query<(Entity, Ref<System>, Has<Filtered>)>,
    mut commands: Commands,
) {
    let filters_changed = filters.is_changed();
    for (entity, system, marked) in &systems {
        // A row that has changed may have changed its factions, so it is
        // asked again even while the filters stand still.
        if !filters_changed && !system.is_changed() {
            continue;
        }

        match (filters.admit(&system), marked) {
            (false, false) => {
                commands.entity(entity).insert(Filtered);
            }
            (true, true) => {
                commands.entity(entity).remove::<Filtered>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::system;

    /// A faction filter, by id, called after it
    fn faction(id: i32) -> Filter {
        Filter::Faction { id, name: format!("Faction {id}") }
    }

    /// A system belonging to each of `factions`
    fn member(address: i64, factions: &[i32]) -> System {
        let mut system = system(address);
        system.factions = factions.to_vec();
        system
    }

    /// With nothing asked, everything passes
    #[test]
    fn no_filter_admits_everything() {
        let filters = Filters::default();
        assert!(filters.admit(&member(1, &[])));
        assert!(filters.admit(&member(2, &[7])));
    }

    /// A faction filter admits the systems that faction is in
    #[test]
    fn a_faction_filter_admits_its_members() {
        let mut filters = Filters::default();
        filters.add(faction(7));

        assert!(filters.admit(&member(1, &[7])));
        assert!(filters.admit(&member(2, &[3, 7])));
        assert!(!filters.admit(&member(3, &[3])));
        assert!(!filters.admit(&member(4, &[])));
    }

    /// Two filters admit what passes both
    ///
    /// Which is what a stack of rows reads as, and what makes adding one
    /// narrow the map rather than widen it.
    #[test]
    fn filters_narrow_each_other() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));

        assert!(filters.admit(&member(1, &[7, 9])));
        assert!(!filters.admit(&member(2, &[7])));
        assert!(!filters.admit(&member(3, &[9])));
    }

    /// A filter turned off asks nothing
    #[test]
    fn a_disabled_filter_admits_everything() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.toggle(0);

        assert!(filters.admit(&member(1, &[])));
    }

    /// One of two turned off leaves the other asking
    #[test]
    fn disabling_one_filter_leaves_the_rest() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.toggle(1);

        assert!(filters.admit(&member(1, &[7])));
        assert!(!filters.admit(&member(2, &[9])));
    }

    /// The same filter added twice is asked once
    ///
    /// Two rows saying the same thing narrow nothing and have to be turned
    /// off one at a time.
    #[test]
    fn the_same_filter_is_not_added_twice() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(7));

        assert_eq!(filters.iter().count(), 1);
    }

    /// Removing a filter stops it being asked
    #[test]
    fn a_removed_filter_asks_nothing() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.remove(0);

        assert!(filters.admit(&member(1, &[])));
    }

    /// A world with nothing in it but the filters and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Filters>();
        app.add_systems(Update, mark);
        app
    }

    /// The mark lands on what the filters exclude
    #[test]
    fn the_mark_lands_on_what_is_excluded() {
        let mut app = map();
        let inside = app.world_mut().spawn(member(1, &[7])).id();
        let outside = app.world_mut().spawn(member(2, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();

        assert!(!app.world().entity(inside).contains::<Filtered>());
        assert!(app.world().entity(outside).contains::<Filtered>());
    }

    /// Dropping a filter takes the mark off what it excluded
    #[test]
    fn the_mark_goes_when_the_filter_does() {
        let mut app = map();
        let outside = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();
        app.world_mut().resource_mut::<Filters>().remove(0);
        app.update();

        assert!(!app.world().entity(outside).contains::<Filtered>());
    }

    /// A row that changes underneath a filter is asked again
    ///
    /// A fetch replaces the row of a system already on the map, and the
    /// factions in it are what the filter reads. Without this the mark would
    /// go on answering for the row the system arrived with.
    #[test]
    fn a_changed_row_is_asked_again() {
        let mut app = map();
        let joining = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();
        assert!(app.world().entity(joining).contains::<Filtered>());

        app.world_mut().entity_mut(joining).insert(member(1, &[3, 7]));
        app.update();

        assert!(!app.world().entity(joining).contains::<Filtered>());
    }

    /// A system arriving under a standing filter is marked
    #[test]
    fn a_system_arriving_is_asked() {
        let mut app = map();

        app.world_mut().resource_mut::<Filters>().add(faction(7));
        app.update();

        let outside = app.world_mut().spawn(member(1, &[3])).id();
        app.update();

        assert!(app.world().entity(outside).contains::<Filtered>());
    }
}
