//! Picking the map out against the rest of the sky
//!
//! A focus is a question asked of every system, and several are asked at
//! once: a system is in focus when any one of them admits it. Each adds to
//! what is picked out rather than cutting into it, every focus being
//! something the user asked to see. What none of them admits is not taken off
//! the map, it is drawn faintly, so a faction is read against the space
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
    app.init_resource::<Focuses>();
    app.init_resource::<Asked>();
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
/// The id is what a system is tested against and the name is what the focus
/// row says it is. Both are settled when the faction is picked out of a list,
/// so neither has to be looked up again.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Focus {
    /// Systems the named faction is present in
    Faction { id: i32, name: String },
    /// The systems a plotted route runs through
    ///
    /// Unlike a faction, this is nothing a system knows about itself. A route
    /// is worked out rather than recorded, so the focus carries the answer it
    /// came back with: the addresses it runs through, in the order they are
    /// travelled.
    ///
    /// That order is the whole of what a route is, so it is what is kept.
    /// Holding them sorted instead would make asking whether a system is on
    /// the route a search rather than a walk, and would lose the sequence in
    /// exchange: a route is tens of systems long, so the walk costs little,
    /// and nothing else could put them back in order afterwards.
    ///
    /// `label` is what its row says, settled when the route landed, since it
    /// names the two ends as the database spells them rather than as they
    /// were typed.
    Route { label: String, systems: Vec<i64> },
    /// The systems the user picked out by hand
    ///
    /// A copy of what was selected rather than a reading of the selection as
    /// it stands. Taking a copy is what makes the focus worth having: the
    /// rings and the rows can be let go of and those systems stay picked out
    /// against the rest.
    ///
    /// `label` says how many, a hand-picked set having no name of its own.
    Systems { label: String, systems: Vec<i64> },
}

impl Focus {
    /// Whether this focus admits `system`
    fn admits(&self, system: &System) -> bool {
        match self {
            Focus::Faction { id, .. } => system.factions.contains(id),
            Focus::Route { systems, .. } => systems.contains(&system.address),
            Focus::Systems { systems, .. } => systems.contains(&system.address),
        }
    }

    /// Whether what it admits has an order of its own
    ///
    /// A route is travelled from one end to the other, so its systems are a
    /// sequence and reading them in any other order loses what they are. A
    /// faction's are a set, with nothing in them to say which comes first, so
    /// whoever lists those may put them in whatever order suits the reader.
    pub fn ordered(&self) -> bool {
        matches!(self, Focus::Route { .. })
    }

    /// Where the system at `address` falls in what this admits
    ///
    /// Nothing for a focus with no order of its own, and nothing for a
    /// system it does not admit at all.
    pub fn place_of(&self, address: i64) -> Option<usize> {
        match self {
            Focus::Faction { .. } | Focus::Systems { .. } => None,
            Focus::Route { systems, .. } => {
                systems.iter().position(|on| *on == address)
            }
        }
    }

    /// What the focus is asking for, as a row can say it
    pub fn name(&self) -> &str {
        match self {
            Focus::Faction { name, .. } => name,
            Focus::Route { label, .. } | Focus::Systems { label, .. } => label,
        }
    }

    /// Every system this focus admits, as far as the database knows
    ///
    /// What a focus's panel lists. Asked of the database rather than of the
    /// map, since where a faction is, is most of what is being asked, and the
    /// map holds only what the spyglass has dragged in.
    ///
    /// Here rather than beside the panel that draws it, so that a kind of
    /// focus is one arm of each of these rather than something to be traced
    /// through the modules that happen to use it.
    pub async fn systems(&self, db: &Database) -> Vec<DbSystem> {
        match self {
            Focus::Faction { name, .. } => {
                DbSystem::fetch_faction(db, name).await.unwrap_or_default()
            }
            Focus::Route { systems, .. } | Focus::Systems { systems, .. } => {
                DbSystem::fetch_many(db, systems).await.unwrap_or_default()
            }
        }
    }
}

/// A focus the user has asked for, before it is known to exist
///
/// What is typed is a name and what a focus tests against is an id, so
/// something has to look one up for the other. That is a database question,
/// asked here rather than in the bar, which draws during egui's own pass and
/// has no business waiting on anything.
///
/// Its own message rather than a [`crate::search::Searched`]: asking for a
/// focus is not searching. The map goes nowhere, fetches nothing in
/// particular, and picks nothing out.
#[derive(Message, Debug)]
pub enum Wanted {
    /// Systems a faction of this name is present in
    Faction { name: String },
}

impl Wanted {
    /// The focus this asks for, or why there is none
    async fn resolve(&self, db: &Database) -> Result<Focus, String> {
        match self {
            Wanted::Faction { name } => {
                match DbFaction::fetch_by_name(db, name).await {
                    Ok(faction) => Ok(Focus::Faction {
                        id: faction.id,
                        name: faction.name,
                    }),
                    Err(_) => Err(format!("No faction named {name}")),
                }
            }
        }
    }
}

/// What became of the last focus asked for
///
/// Three states rather than an error or nothing, because the field that asked
/// has to know the difference between not yet answered and answered well. It
/// cannot know either at the moment it asks: a name is looked up against the
/// database a frame later, so the field is still holding what was typed when
/// the answer arrives.
///
/// Its own resource rather than [`crate::search::SearchNote`], which answers a
/// name typed into the search input. Two unrelated answers sharing one line
/// means each wipes the other: adding a faction would clear a note about a
/// system that was never found, and the note about a faction would be read
/// out under the box that has nothing to do with it.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub enum Asked {
    /// Nothing has been asked for, or the answer has been acted on
    #[default]
    Nothing,
    /// What was asked for is standing in a row of its own
    Added,
    /// Why there is nothing to stand there
    Trouble(String),
}

impl Asked {
    /// Whether the field that asked has done its job
    ///
    /// A focus that was added is a row on screen now, so what was typed to
    /// ask for it has been answered and the field is free for the next one.
    ///
    /// A name that resolved to nothing is left where it was typed. It is
    /// most likely nearly right, and clearing it makes the user type the
    /// whole of it again to find out which part was wrong.
    pub fn answered(&self) -> bool {
        matches!(self, Asked::Added)
    }
}

/// Turn what the user asked for into a focus, or say why not
fn resolve(
    mut wanted: MessageReader<Wanted>,
    mut focuses: ResMut<Focuses>,
    mut answer: ResMut<Asked>,
    db: Res<Db>,
) {
    for asked in wanted.read() {
        // Waited on, as a search is. A name resolves against one indexed row
        // and this answers something the user just did.
        future::block_on(async {
            *answer = match asked.resolve(&db.0).await {
                Ok(focus) => {
                    focuses.add(focus);
                    Asked::Added
                }
                Err(why) => Asked::Trouble(why),
            };
        });
    }
}

/// A focus, and whether it is being applied
///
/// Off without being taken away, so that one can be lifted to see what it was
/// hiding and put back without being typed in again.
#[derive(Debug, Clone)]
pub struct Active {
    pub focus: Focus,
    pub enabled: bool,
}

/// Every focus the user has added
#[derive(Resource, Default)]
pub struct Focuses(Vec<Active>);

impl Focuses {
    /// Whether any enabled focus admits `system`
    ///
    /// Any of them, so each one adds to what is shown rather than cutting
    /// into it. Every focus is something the user asked to see, and a second
    /// ask is a second thing wanted, not a condition on the first: asking for
    /// a faction and then for a route means both, where taking the systems
    /// they share would usually mean nothing at all, the two rarely
    /// overlapping.
    ///
    /// Nothing asked for admits everything. A map with no focus on it is a
    /// map showing the sky rather than an empty one.
    pub fn admit(&self, system: &System) -> bool {
        let mut asked =
            self.0.iter().filter(|active| active.enabled).peekable();
        if asked.peek().is_none() {
            return true;
        }
        asked.any(|active| active.focus.admits(system))
    }

    /// Add `focus`, unless it is already being asked
    ///
    /// Asking the same thing twice picks out nothing further and leaves two
    /// rows that have to be turned off one at a time.
    pub fn add(&mut self, focus: Focus) {
        if self.0.iter().any(|active| active.focus == focus) {
            return;
        }
        self.0.push(Active { focus, enabled: true });
    }

    /// Add `focus` in place of whatever else of its kind is being asked
    ///
    /// For a kind there can only be one of. The map draws one route at a
    /// time, replacing the line as each is plotted, so a second route focus
    /// standing beside the first would go on picking out the systems of a
    /// route whose line was rubbed out when the next one was plotted.
    ///
    /// Factions do not go through here. Several of them at once is the whole
    /// point of a focus being a focus.
    pub fn replace(&mut self, focus: Focus) {
        let kind = std::mem::discriminant(&focus);
        self.0.retain(|active| std::mem::discriminant(&active.focus) != kind);
        self.0.push(Active { focus, enabled: true });
    }

    /// Stop asking the focus at `index`
    pub fn remove(&mut self, index: usize) {
        if index < self.0.len() {
            self.0.remove(index);
        }
    }

    /// The focuses in the order they were added
    pub fn iter(&self) -> impl Iterator<Item = &Active> {
        self.0.iter()
    }

    /// Turn the focus at `index` on or off
    pub fn toggle(&mut self, index: usize) {
        if let Some(active) = self.0.get_mut(index) {
            active.enabled = !active.enabled;
        }
    }

    /// How many focuses are being held, turned on or not
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether none is held at all, which is a map showing the whole sky
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Whether any focus is turned on
    ///
    /// Which is whether the map is picking anything out. With every focus
    /// off the sky is drawn whole, as it is with none of them held at all.
    pub fn any_enabled(&self) -> bool {
        self.0.iter().any(|active| active.enabled)
    }

    /// Turn every focus off, or every one back on
    ///
    /// Off while any of them is on, since that is the question the control
    /// answers: show me the sky as it is, and then put back what I was
    /// looking at. All of them come back rather than the ones that were on
    /// before, so the two clicks are one gesture and its undo rather than a
    /// state to be remembered.
    pub fn toggle_all(&mut self) {
        let on = self.any_enabled();
        for active in &mut self.0 {
            active.enabled = !on;
        }
    }

    /// Stop asking every focus
    pub fn clear(&mut self) {
        self.0.clear();
    }
}

/// A system no enabled focus admits
///
/// The verdict is one bit, whichever of them admitted it and however many
/// did, so it is carried by a marker, and how faintly to draw is [`DIMMED`],
/// the same for all of them. Written when the focuses change or a system
/// does, rather than worked out afresh by each of the things that draws from
/// it.
#[derive(Component)]
pub struct Unfocused;

/// How brightly a system no focus admits is drawn
///
/// A fraction of what it would be drawn at in focus. Faint enough to read as
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
pub fn dim(color: Srgba, unfocused: bool) -> Srgba {
    if unfocused {
        Srgba { alpha: color.alpha * DIMMED, ..color }
    } else {
        color
    }
}

/// Keep the mark on whichever systems the focuses exclude
///
/// Only where something has changed. This runs over every system on the map,
/// and the focuses are usually quiet, so the common case is a walk that
/// writes nothing.
fn mark(
    focuses: Res<Focuses>,
    systems: Query<(Entity, Ref<System>, Has<Unfocused>)>,
    mut commands: Commands,
) {
    let focuses_changed = focuses.is_changed();
    for (entity, system, marked) in &systems {
        // A row that has changed may have changed its factions, so it is
        // asked again even while the focuses stand still.
        if !focuses_changed && !system.is_changed() {
            continue;
        }

        match (focuses.admit(&system), marked) {
            (false, false) => {
                commands.entity(entity).insert(Unfocused);
            }
            (true, true) => {
                commands.entity(entity).remove::<Unfocused>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::tests::system;

    /// A faction focus, by id, called after it
    fn faction(id: i32) -> Focus {
        Focus::Faction { id, name: format!("Faction {id}") }
    }

    /// A system belonging to each of `factions`
    fn member(address: i64, factions: &[i32]) -> System {
        let mut system = system(address);
        system.factions = factions.to_vec();
        system
    }

    /// A focus that was added has answered the field that asked for it
    #[test]
    fn an_added_focus_frees_the_field() {
        assert!(Asked::Added.answered());
    }

    /// A name that resolved to nothing has not
    ///
    /// The field goes on holding it, since it is most likely nearly right and
    /// clearing it makes the user type the whole of it again to find out
    /// which part was wrong.
    #[test]
    fn a_name_that_failed_leaves_the_field_alone() {
        let trouble = Asked::Trouble("No faction named Zargon".to_owned());

        assert!(!trouble.answered());
    }

    /// Nor has a question nobody has asked
    #[test]
    fn nothing_asked_frees_nothing() {
        assert!(!Asked::default().answered());
        assert_eq!(Asked::default(), Asked::Nothing);
    }

    /// With nothing asked, everything passes
    #[test]
    fn no_focus_admits_everything() {
        let focuses = Focuses::default();
        assert!(focuses.admit(&member(1, &[])));
        assert!(focuses.admit(&member(2, &[7])));
    }

    /// A faction focus admits the systems that faction is in
    #[test]
    fn a_faction_focus_admits_its_members() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));

        assert!(focuses.admit(&member(1, &[7])));
        assert!(focuses.admit(&member(2, &[3, 7])));
        assert!(!focuses.admit(&member(3, &[3])));
        assert!(!focuses.admit(&member(4, &[])));
    }

    /// Two focuses admit what passes either
    ///
    /// Each row is something the user asked to see, so a second row shows a
    /// second thing rather than cutting into the first. Two factions asked
    /// for together and ANDed would answer with the systems both are present
    /// in, which is usually none of them.
    #[test]
    fn focuses_add_to_each_other() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));

        assert!(focuses.admit(&member(1, &[7, 9])));
        assert!(focuses.admit(&member(2, &[7])));
        assert!(focuses.admit(&member(3, &[9])));
        assert!(!focuses.admit(&member(4, &[3])));
    }

    /// A focus turned off asks nothing
    #[test]
    fn a_disabled_focus_admits_everything() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.toggle(0);

        assert!(focuses.admit(&member(1, &[])));
    }

    /// Turning them all off shows the sky whole, and holds on to every focus
    ///
    /// Which is the point of the row: lift the whole set to see what it was
    /// dimming, without having to type any of it in again.
    #[test]
    fn turning_them_all_off_admits_everything() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));

        focuses.toggle_all();

        assert!(!focuses.any_enabled());
        assert_eq!(focuses.len(), 2);
        assert!(focuses.admit(&member(1, &[3])));
    }

    /// And the same gesture puts every one of them back
    ///
    /// All of them rather than the ones that were on before, so the second
    /// click undoes the first rather than restoring a state nobody chose.
    #[test]
    fn turning_them_all_on_asks_every_one() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));
        focuses.toggle(1);

        focuses.toggle_all();
        focuses.toggle_all();

        assert!(focuses.admit(&member(1, &[7])));
        assert!(focuses.admit(&member(2, &[9])));
        assert!(!focuses.admit(&member(3, &[3])));
    }

    /// One left on is enough for the set to read as asking
    ///
    /// So the row turns the rest off with it rather than turning the one
    /// that is off back on, which would take two clicks to reach the sky.
    #[test]
    fn one_left_on_turns_them_all_off() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));
        focuses.toggle(1);

        focuses.toggle_all();

        assert!(!focuses.any_enabled());
    }

    /// Clearing them takes every focus away
    #[test]
    fn clearing_leaves_nothing_held() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));

        focuses.clear();

        assert_eq!(focuses.len(), 0);
        assert!(focuses.admit(&member(1, &[3])));
    }

    /// One of two turned off leaves the other asking
    #[test]
    fn disabling_one_focus_leaves_the_rest() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));
        focuses.toggle(1);

        assert!(focuses.admit(&member(1, &[7])));
        assert!(!focuses.admit(&member(2, &[9])));
    }

    /// A hand-picked set holding the systems at `addresses`
    fn gathered(addresses: &[i64]) -> Focus {
        Focus::Systems {
            label: format!("{} systems", addresses.len()),
            systems: addresses.to_vec(),
        }
    }

    /// A hand-picked set admits the systems that were picked
    #[test]
    fn a_gathered_set_admits_what_was_picked() {
        let mut focuses = Focuses::default();
        focuses.add(gathered(&[7, 3]));

        assert!(focuses.admit(&member(3, &[])));
        assert!(focuses.admit(&member(7, &[])));
        assert!(!focuses.admit(&member(9, &[])));
    }

    /// And has no order for a panel to list it in
    ///
    /// Unlike a route, which is travelled from one end to the other. A set is
    /// gathered up, and the order it happened to be clicked in says nothing
    /// worth holding a list to.
    #[test]
    fn a_gathered_set_has_no_order_of_its_own() {
        assert!(!gathered(&[7, 3]).ordered());
        assert_eq!(gathered(&[7, 3]).place_of(3), None);
    }

    /// A route between the systems at `addresses`
    fn route(addresses: &[i64]) -> Focus {
        Focus::Route { label: "A -> B".to_owned(), systems: addresses.to_vec() }
    }

    /// A route focus admits the systems it runs through
    ///
    /// Nothing a system knows about itself, unlike a faction: a route is
    /// worked out, so the focus carries the answer it came back with.
    #[test]
    fn a_route_focus_admits_what_it_runs_through() {
        let mut focuses = Focuses::default();
        focuses.add(route(&[7, 3, 9]));

        assert!(focuses.admit(&member(3, &[])));
        assert!(focuses.admit(&member(9, &[])));
        assert!(!focuses.admit(&member(4, &[])));
    }

    /// However the addresses are ordered, the answer is the same
    ///
    /// The order a route is kept in is the order it is travelled, which is
    /// what its panel lists it in, and says nothing about who is on it.
    #[test]
    fn a_route_admits_the_same_whatever_order_it_came_in() {
        let mut forwards = Focuses::default();
        forwards.add(route(&[1, 5, 9]));
        let mut backwards = Focuses::default();
        backwards.add(route(&[9, 5, 1]));

        for address in [1, 5, 9, 2, 7] {
            let system = member(address, &[]);
            assert_eq!(forwards.admit(&system), backwards.admit(&system));
        }
    }

    /// A route says where each of its systems falls along it
    ///
    /// Which is what its panel lists them in. The order a route is travelled
    /// is the whole of what a route is.
    #[test]
    fn a_route_knows_the_order_it_is_travelled() {
        let asked = route(&[9, 3, 7]);

        assert_eq!(asked.place_of(9), Some(0));
        assert_eq!(asked.place_of(3), Some(1));
        assert_eq!(asked.place_of(7), Some(2));
        assert_eq!(asked.place_of(4), None);
    }

    /// A route has an order and a faction has none
    ///
    /// A faction's systems are a set, with nothing in them to say which comes
    /// first, so whoever lists those may order them to suit the reader.
    #[test]
    fn only_a_route_carries_an_order() {
        assert!(route(&[1, 2]).ordered());
        assert!(!faction(7).ordered());
        assert_eq!(faction(7).place_of(1), None);
    }

    /// A second route takes the place of the first
    ///
    /// One route is drawn at a time, so two route focuses would ask about a
    /// line that is no longer there and narrow the map to whatever the two
    /// happened to share.
    #[test]
    fn a_route_replaces_the_route_before_it() {
        let mut focuses = Focuses::default();
        focuses.replace(route(&[1, 2]));
        focuses.replace(route(&[8, 9]));

        assert_eq!(focuses.iter().count(), 1);
        assert!(focuses.admit(&member(9, &[])));
        assert!(!focuses.admit(&member(1, &[])));
    }

    /// And leaves the factions where they are
    ///
    /// Several factions at once is the whole point of a focus being a
    /// focus, so only the kind that replaces itself does.
    #[test]
    fn a_route_leaves_the_factions_alone() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(9));
        focuses.replace(route(&[1, 2]));

        assert_eq!(focuses.iter().count(), 3);
    }

    /// A route stands beside the factions rather than cutting into them
    ///
    /// Both were asked for, and a route through a faction's space rarely
    /// keeps to it. Taking only what the two share would answer a plotted
    /// route with the handful of its systems that faction happens to hold.
    #[test]
    fn a_route_and_a_faction_ask_together() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.replace(route(&[1, 2]));

        let mut on_both = member(1, &[7]);
        on_both.factions = vec![7];
        assert!(focuses.admit(&on_both));
        // On the route, though the faction is nowhere near it.
        assert!(focuses.admit(&member(2, &[])));
        // The faction's, though the route runs elsewhere.
        assert!(focuses.admit(&member(5, &[7])));
        // Neither, so neither asked for it.
        assert!(!focuses.admit(&member(6, &[3])));
    }

    /// Two focuses that differ are told apart, and equal ones are not
    ///
    /// The bar keys each row on its focus rather than on where the row sits,
    /// so that dropping one does not hand its identity, and whatever egui was
    /// remembering against it, to the row that moves up into its place. That
    /// rests on this: two focuses that are not the same must not hash the
    /// same, and one that is must.
    #[test]
    fn focuses_are_told_apart_by_what_they_ask() {
        use std::collections::HashSet;

        let asked = [faction(7), faction(9), route(&[1, 2]), route(&[3, 4])];
        let distinct: HashSet<_> = asked.iter().collect();
        assert_eq!(distinct.len(), asked.len());

        let same: HashSet<_> = [faction(7), faction(7)].into_iter().collect();
        assert_eq!(same.len(), 1);
    }

    /// The same focus added twice is asked once
    ///
    /// Two rows saying the same thing narrow nothing and have to be turned
    /// off one at a time.
    #[test]
    fn the_same_focus_is_not_added_twice() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.add(faction(7));

        assert_eq!(focuses.iter().count(), 1);
    }

    /// Removing a focus stops it being asked
    #[test]
    fn a_removed_focus_asks_nothing() {
        let mut focuses = Focuses::default();
        focuses.add(faction(7));
        focuses.remove(0);

        assert!(focuses.admit(&member(1, &[])));
    }

    /// A world with nothing in it but the focuses and the mark
    fn map() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Focuses>();
        app.add_systems(Update, mark);
        app
    }

    /// The mark lands on what the focuses exclude
    #[test]
    fn the_mark_lands_on_what_is_excluded() {
        let mut app = map();
        let inside = app.world_mut().spawn(member(1, &[7])).id();
        let outside = app.world_mut().spawn(member(2, &[3])).id();

        app.world_mut().resource_mut::<Focuses>().add(faction(7));
        app.update();

        assert!(!app.world().entity(inside).contains::<Unfocused>());
        assert!(app.world().entity(outside).contains::<Unfocused>());
    }

    /// Dropping a focus takes the mark off what it excluded
    #[test]
    fn the_mark_goes_when_the_focus_does() {
        let mut app = map();
        let outside = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Focuses>().add(faction(7));
        app.update();
        app.world_mut().resource_mut::<Focuses>().remove(0);
        app.update();

        assert!(!app.world().entity(outside).contains::<Unfocused>());
    }

    /// A row that changes underneath a focus is asked again
    ///
    /// A fetch replaces the row of a system already on the map, and the
    /// factions in it are what the focus reads. Without this the mark would
    /// go on answering for the row the system arrived with.
    #[test]
    fn a_changed_row_is_asked_again() {
        let mut app = map();
        let joining = app.world_mut().spawn(member(1, &[3])).id();

        app.world_mut().resource_mut::<Focuses>().add(faction(7));
        app.update();
        assert!(app.world().entity(joining).contains::<Unfocused>());

        app.world_mut().entity_mut(joining).insert(member(1, &[3, 7]));
        app.update();

        assert!(!app.world().entity(joining).contains::<Unfocused>());
    }

    /// A system arriving under a standing focus is marked
    #[test]
    fn a_system_arriving_is_asked() {
        let mut app = map();

        app.world_mut().resource_mut::<Focuses>().add(faction(7));
        app.update();

        let outside = app.world_mut().spawn(member(1, &[3])).id();
        app.update();

        assert!(app.world().entity(outside).contains::<Unfocused>());
    }
}
