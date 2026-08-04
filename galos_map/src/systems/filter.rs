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
//! [`DIMMED`] says how faintly, the same for all of them. What is excluded is
//! still wanted on screen, so it is still fetched: a system that was never
//! asked for cannot be drawn faintly, and the space around a faction is the
//! thing being drawn.

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
    /// The systems a plotted route runs through
    ///
    /// Unlike a faction, this is nothing a system knows about itself. A route
    /// is worked out rather than recorded, so the filter carries the answer:
    /// the addresses it came back with, sorted, so that asking whether a
    /// system is on it is a search rather than a walk. The sky it is asked
    /// about runs to thousands and a route to tens.
    ///
    /// `label` is what its row says, settled when the route landed, since it
    /// names the two ends as the database spells them rather than as they
    /// were typed.
    Route { label: String, systems: Vec<i64> },
}

impl Filter {
    /// Whether this filter admits `system`
    fn admits(&self, system: &System) -> bool {
        match self {
            Filter::Faction { id, .. } => system.factions.contains(id),
            Filter::Route { systems, .. } => {
                systems.binary_search(&system.address).is_ok()
            }
        }
    }

    /// What the filter is asking for, as a row can say it
    pub fn name(&self) -> &str {
        match self {
            Filter::Faction { name, .. } => name,
            Filter::Route { label, .. } => label,
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
            Filter::Route { systems, .. } => {
                DbSystem::fetch_many(db, systems).await.unwrap_or_default()
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

    /// Add `filter` in place of whatever else of its kind is being asked
    ///
    /// For a kind there can only be one of. The map draws one route at a
    /// time, replacing the line as each is plotted, so a second route filter
    /// standing beside the first would ask about a line that is no longer
    /// there and narrow the map to nothing between them.
    ///
    /// Factions do not go through here. Several of them at once is the whole
    /// point of a filter being a filter.
    pub fn replace(&mut self, filter: Filter) {
        let kind = std::mem::discriminant(&filter);
        self.0.retain(|active| std::mem::discriminant(&active.filter) != kind);
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
/// marker, and how faintly to draw is [`DIMMED`], the same for all of them. Written when the
/// filters change or a system does, rather than worked out afresh by each of
/// the things that draws from it.
#[derive(Component)]
pub struct Filtered;

/// How brightly a system the filters exclude is drawn
///
/// A fraction of what it would be drawn at unfiltered. Faint enough to read as
/// background rather than as something picked out, and bright enough to still
/// be read, which is the whole point of dimming rather than hiding: the space
/// around a faction stays legible.
///
/// One number rather than a setting. What it is worth setting to is the same
/// answer every time, and a control that only ever moves between "right" and
/// "wrong" is a control that costs more than it gives.
pub const DIMMED: f32 = 0.15;

/// `color` as it should be drawn for a system, given whether it is excluded
///
/// The colour is left alone and the alpha carries it, so that what is dimmed
/// reads as standing further back rather than as having changed into something
/// else.
///
/// For what is painted straight rather than through a material: the two rings
/// are gizmos, and a gizmo takes its colour at the call.
pub fn dim(color: Srgba, filtered: bool) -> Srgba {
    if filtered {
        Srgba { alpha: color.alpha * DIMMED, ..color }
    } else {
        color
    }
}

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

    /// A route between the systems at `addresses`
    fn route(addresses: &[i64]) -> Filter {
        let mut systems = addresses.to_vec();
        systems.sort_unstable();
        Filter::Route { label: "A -> B".to_owned(), systems }
    }

    /// A route filter admits the systems it runs through
    ///
    /// Nothing a system knows about itself, unlike a faction: a route is
    /// worked out, so the filter carries the answer it came back with.
    #[test]
    fn a_route_filter_admits_what_it_runs_through() {
        let mut filters = Filters::default();
        filters.add(route(&[7, 3, 9]));

        assert!(filters.admit(&member(3, &[])));
        assert!(filters.admit(&member(9, &[])));
        assert!(!filters.admit(&member(4, &[])));
    }

    /// However the addresses arrived, the answer is the same
    ///
    /// They are searched rather than walked, which needs them sorted, and
    /// nothing about where a route came from says they will be.
    #[test]
    fn a_route_admits_the_same_whatever_order_it_came_in() {
        let mut forwards = Filters::default();
        forwards.add(route(&[1, 5, 9]));
        let mut backwards = Filters::default();
        backwards.add(route(&[9, 5, 1]));

        for address in [1, 5, 9, 2, 7] {
            let system = member(address, &[]);
            assert_eq!(forwards.admit(&system), backwards.admit(&system));
        }
    }

    /// A second route takes the place of the first
    ///
    /// One route is drawn at a time, so two route filters would ask about a
    /// line that is no longer there and narrow the map to whatever the two
    /// happened to share.
    #[test]
    fn a_route_replaces_the_route_before_it() {
        let mut filters = Filters::default();
        filters.replace(route(&[1, 2]));
        filters.replace(route(&[8, 9]));

        assert_eq!(filters.iter().count(), 1);
        assert!(filters.admit(&member(9, &[])));
        assert!(!filters.admit(&member(1, &[])));
    }

    /// And leaves the factions where they are
    ///
    /// Several factions at once is the whole point of a filter being a
    /// filter, so only the kind that replaces itself does.
    #[test]
    fn a_route_leaves_the_factions_alone() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.add(faction(9));
        filters.replace(route(&[1, 2]));

        assert_eq!(filters.iter().count(), 3);
    }

    /// A route narrows the factions rather than replacing them
    #[test]
    fn a_route_and_a_faction_ask_together() {
        let mut filters = Filters::default();
        filters.add(faction(7));
        filters.replace(route(&[1, 2]));

        let mut on_both = member(1, &[7]);
        on_both.factions = vec![7];
        assert!(filters.admit(&on_both));
        // On the route, but not the faction's.
        assert!(!filters.admit(&member(2, &[])));
        // The faction's, but not on the route.
        assert!(!filters.admit(&member(5, &[7])));
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
