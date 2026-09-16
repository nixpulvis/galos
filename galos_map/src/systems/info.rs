//! What the map knows about a system, written out
//!
//! Pointing at a system and picking one out are both answered on the map
//! itself, by a ring and a name. This is the long form of the same answer,
//! and the user asks for it deliberately, from the mark beside the name of
//! whatever is picked out. A panel is kept until they shut it, and as many
//! of them stand open at once as they care to open, so two systems can be
//! read side by side.
//!
//! A panel holds a [`System`] value rather than an entity, for the reason a
//! selection does: a system flown away from is despawned, and a panel opened
//! for it has no reason to go with it.

use crate::camera::{MoveCamera, OrbitCamera};
use crate::schedule::MapSet;
use crate::systems::System;
use crate::systems::bodies::mark_if_moved;
use crate::systems::filter::{Filter, Filters};
use crate::systems::route::graph::Crossing;
use crate::systems::selection::{Picked, Selection};
use crate::ui::MARGIN;
use crate::ui::SystemAction;
use crate::{Factions, Names, Populated};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::tasks::{AsyncComputeTaskPool, Task};
use bevy_egui::egui::{Context, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use chrono::{DateTime, Utc};
use elite_journal::body::{Composition, Material, Orbit, Spin};
use galos_index::meta::{Body as DbBody, Economies, Star as DbStar, Surface};
use galos_photometry::{Distance, Magnitude};
use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::time::Duration;

pub fn plugin(app: &mut App) {
    app.init_resource::<Panels>();
    app.init_resource::<StarClasses>();
    app.init_resource::<FactionNames>();
    app.add_systems(Update, refresh.in_set(MapSet::Present));
    app.add_systems(Update, name_factions.in_set(MapSet::Present));
    app.add_systems(Update, fill_filters.in_set(MapSet::Present));
    // `ui::chrome` concludes at its end whether the pointer is busy with the
    // UI, from every window drawn in the pass so far. Drawn before it, these
    // are counted in the same frame they are shown rather than the next.
    app.add_systems(
        EguiPrimaryContextPass,
        panels.after(crate::ui::lettering).before(crate::ui::chrome),
    );
}

/// How wide a panel stands
///
/// Wide enough for a position and for the longest of the names a field is
/// answered with, so that the two columns do not shift from one system to
/// the next.
///
/// A panel comes out at this taken up to a whole character, the title bar
/// being lettered and filled out to its end.
const WIDTH: f32 = 230.;

/// What the title bar spends on the fold arrow, the close mark and the gaps
/// around them
///
/// The rest of [`WIDTH`] is the title's. Measured rather than worked out, egui
/// laying its own title bar out, and held down by
/// `a_short_title_leaves_the_width_alone`.
const TITLE_MARKS: f32 = 46.;

/// How much of a title a panel has room for, in characters
///
/// A window is at least as wide as its title bar needs, so what a panel is
/// called is what decides how wide it stands. Cut to this, a route's panel
/// keeps the width every other panel has.
///
/// Enough characters to cover the room rather than as many as fit inside
/// it, since the title is padded out to fill the bar. A bar a fraction of a
/// character narrower than what stands under it is a panel that draws itself
/// in the moment it is folded away into the bar, and back out when it opens.
fn titling(ctx: &Context) -> usize {
    crate::ui::covering(ctx, egui::TextStyle::Body, WIDTH - TITLE_MARKS)
}

/// What a panel is called, laid out across its title bar
///
/// Lettered as the panel's own contents are. A title set for a heading stands
/// half again as tall as everything under it, which reads as a title bar
/// borrowed from some other window rather than as the top of this one.
///
/// Cut to the room there is, and then filled out to it with the spaces the cut
/// left over: egui centres a title between the marks either side of it, and a
/// title that fills the bar has nothing left to centre, so it stands at the
/// left where a name is read from.
fn titled(ctx: &Context, title: &str) -> egui::RichText {
    let room = titling(ctx);
    let said = crate::ui::shortened(title, room);
    let spare = room.saturating_sub(said.chars().count());

    egui::RichText::new(format!("{said}{}", " ".repeat(spare)))
        .text_style(egui::TextStyle::Body)
}

/// The least room a panel is ever offered for what it holds, in pixels
///
/// A viewport too short to hold a panel is a viewport too short to hold
/// anything; what it gets is a couple of rows and a scroll bar rather than a
/// window egui refuses to draw.
const LEAST: f32 = 64.;

/// The box a panel is kept inside: the viewport, a margin off every edge
///
/// Where it may stand, and how tall: the room under it is measured from here.
fn kept_inside(ctx: &Context) -> egui::Rect {
    ctx.content_rect().shrink(crate::ui::MARGIN)
}

/// The room a panel standing at `top` has for what it holds, in pixels
///
/// Down to the bottom of the box it is kept inside, less `frame` — how much of
/// a panel is the window round it, which is [`Panels::frame`] and is measured
/// rather than worked out.
///
/// Measured from where the panel actually stands, which is where it stood last
/// frame: a panel the user has dragged halfway down the screen is capped by
/// the room where it is rather than by the room the tiling would have given it.
fn room_under(ctx: &Context, id: egui::Id, at: egui::Pos2, frame: f32) -> f32 {
    let stood =
        ctx.memory(|memory| memory.area_rect(id).map(|rect| rect.top()));
    (kept_inside(ctx).bottom() - stood.unwrap_or(at.y) - frame).max(LEAST)
}

/// The window a panel stands in, put where the tiling says
///
/// `at` is where its right hand top corner goes, that being the corner the
/// tiling works from: panels stand against the right edge of the viewport, and
/// a window is at least as wide as its title bar needs, so where its left edge
/// falls is not known until it has been drawn.
///
/// `placed` is [`Panel::placed`]: a panel is put where the tiling says on the
/// frame it opens, and asked for `at` as a default after that, so that one
/// dragged somewhere stays where it was dragged.
///
/// `room` is what it has for what it holds, and is both the height it opens
/// filling where it has more to show than that and the height a dragged one is
/// held to. Egui does its own clamping against the box a window is constrained
/// to, and does not take the window's own margins off when it does, so the
/// room is the figure to trust.
///
/// The title is cut to the room there is for it, both ends of a route kept.
/// A panel as wide as its name is a panel the tiling cannot place and the
/// user cannot read two of side by side.
fn framed<'open>(
    ctx: &Context,
    title: &str,
    id: egui::Id,
    at: egui::Pos2,
    room: f32,
    placed: bool,
    showing: &'open mut bool,
) -> egui::Window<'open> {
    let window = egui::Window::new(titled(ctx, title))
        .id(id)
        .open(showing)
        // The height alone. A panel is as wide as its two columns and its
        // title bar need and no wider, so there is nothing to drag there; how
        // much of a long list to show is the user's business.
        .resizable([false, true])
        .pivot(egui::Align2::RIGHT_TOP)
        // Over the chrome, which is what a window is: the pane and the bar
        // stand where the map put them and a panel stands where the user
        // dragged it, so where the two meet the window is the one on top.
        // Said here rather than left to a window's own `Order::Middle`, which
        // is where the chrome sits: same-order areas are stacked by which was
        // last interacted with, and a panel would slide under the pane the
        // moment the pane was touched.
        .order(egui::Order::Foreground)
        // The width alone. Left unsaid it is `Style::default_area_size`, 600,
        // which will not fit where a panel is asked to be placed, so egui
        // slides the window somewhere it does and remembers it there.
        .default_width(WIDTH)
        // The room, both ways. Preferred, so a panel with more to show than
        // fits opens filling it rather than at egui's own default of four
        // hundred pixels with the screen empty under it; and at most, so a
        // height the user has dragged is held to it and a viewport shrinking
        // under a tall panel brings it back inside.
        .default_height(room)
        .max_height(room)
        // On the screen, wherever it was dragged to.
        .constrain_to(kept_inside(ctx));

    if placed { window.default_pos(at) } else { window.current_pos(at) }
}

/// Lay a panel's contents out over the whole of the window
///
/// [`WIDTH`] is what a panel asks for, and a window is at least as wide as its
/// title bar needs. A route is titled with the names of both its ends, which
/// is wider, and egui hands the extra room to the contents to use or leave.
///
/// They take it. A line in a list is a control the width of the list, so a
/// list laid out to [`WIDTH`] inside a wider window stops short of the frame
/// and leaves a band of empty panel down the right hand side, which reads as
/// a margin nobody chose.
fn spread(ui: &mut Ui) {
    ui.set_min_width(WIDTH);
}

/// A panel's contents: across the whole of the window, and scrolled inside the
/// `room` there is for them — answering how tall they came out
///
/// A panel is as tall as what it holds — four rows of a body's orbit, or four
/// hundred systems of a faction's holdings — and nothing about a window bounds
/// that, so a long one ran off the bottom of the viewport with the rest of it
/// out of reach. Scrolled, the panel stops at the room there is and the bar
/// carries the rest.
///
/// The room rather than `ui.available_height()`, which is what the window has
/// already decided to be: read from there a long panel can never ask for more
/// than it was given last frame, so it never grows into the room it has.
///
/// No height is imposed: [`crate::ui::scrolling`] grows to what it is given
/// and stops at what is in it, so a panel of four rows is four rows tall.
/// Which is what the tiling wants — it steps the next panel by the tallest
/// drawn — and what a fixed height would take away.
///
/// What comes back is the height the contents came out at, which is the other
/// half of measuring [`Panels::frame`].
fn inside(
    ui: &mut Ui,
    id: egui::Id,
    room: f32,
    contents: impl FnOnce(&mut Ui),
) -> f32 {
    spread(ui);
    crate::ui::scrolling(ui, room, id, contents);
    ui.min_rect().height()
}

/// What the user has a panel open for
///
/// A list rather than a map, since there are only ever a handful of them and
/// what matters about the order is which places in the tiling are free.
#[derive(Resource, Default)]
pub struct Panels {
    open: Vec<Panel>,
    /// How tall the tallest panel drawn came out
    ///
    /// How far down the next one opens. The tallest rather than the last,
    /// since a system's panel and a filter's are not the same height and a
    /// step measured from the shorter would open the next panel on top of a
    /// taller one already standing.
    ///
    /// Measured rather than guessed because a guess one line short is exactly
    /// the overlap the tiling is for. Nothing until a panel has been drawn,
    /// which leaves the first of a session laid out as taking no room. It
    /// still opens where it asked to, since egui only moves a window to bring
    /// it back inside the viewport.
    height: f32,
    /// How wide the widest panel drawn came out
    ///
    /// How far across the next column of them opens, and the widest for the
    /// reason the height is the tallest. A panel is at least [`WIDTH`] and as
    /// much wider as its title bar needs, which for a route is the names of
    /// both its ends, so a column stepped by what a panel asks for would open
    /// the next one over the top of a wide one already standing.
    ///
    /// Only the columns. Where a panel opens down the edge does not depend on
    /// this, panels being placed by the corner they are tiled against.
    width: f32,
    /// How much of a panel is the window round it: its title bar, its frame
    /// and its margins
    ///
    /// What comes off the room before the room is offered to the contents,
    /// which is what [`room_under`] takes it for. Measured — the height a
    /// panel came out at, less the height its contents came out at — rather
    /// than read off the style: the title bar is egui's to lay out, and its
    /// own clamping of a window against the box it is constrained to leaves
    /// the window's margins out, so a figure worked out from the style was
    /// a few pixels short and a full-height panel hung that far past the
    /// bottom of the viewport.
    ///
    /// One number for every panel, they all being the same window. Nothing
    /// until one has been drawn, which leaves the first frame of a session
    /// offering the room the window itself takes up as well; the frame after
    /// has it right, and a panel that has to shrink by a title bar's worth
    /// does it before it is ever seen.
    frame: f32,
}

/// One open panel
struct Panel {
    /// What it is about
    subject: Subject,
    /// Which place in the tiling it stands in
    slot: usize,
    /// Whether it has been drawn since it was opened
    ///
    /// A panel is put where the tiling says on the frame it opens, and left
    /// wherever it is after that, so that dragging one somewhere holds. Egui
    /// goes on remembering where a window was for the whole session, shut
    /// windows included, so a panel opened a second time would otherwise come
    /// back to the place it had rather than to the place it was just given.
    placed: bool,
}

/// What a panel is about
///
/// Two kinds, sharing the tiling and the window because they are the same
/// gesture answered: the mark on a row in the bar, opening the long form of
/// what that row names.
enum Subject {
    /// One system, and everything the map knows about it
    System(System),
    /// One star, and everything the map knows about it
    ///
    /// The row rather than the entity, for the reason a system's panel holds
    /// one: a star is despawned the moment the camera leaves the system it is
    /// in, and a panel opened for it has no reason to go with it.
    Star(DbStar),
    /// One body, likewise
    Body(DbBody),
    /// One filter, and the systems it admits
    ///
    /// The systems are fetched once the panel is open, by [`fill_filters`],
    /// and are nothing at all until they arrive. They come from the database
    /// rather than from the map, since the point of the list is to say where
    /// a faction is, and the map holds only what the spyglass has reached.
    ///
    /// `legs` is what a trip is made of, empty for everything else. A trip is
    /// one filter here -- the route it is flown as, every stop in order --
    /// and the legs are how its list is broken up: each named, with its own
    /// stops drawn in under it.
    Filter { filter: Filter, legs: Vec<Filter>, systems: Option<Vec<System>> },
}

impl Subject {
    /// What the panel is titled
    fn title(&self) -> &str {
        match self {
            Subject::System(system) => &system.name,
            Subject::Star(star) => &star.name,
            Subject::Body(body) => &body.name,
            Subject::Filter { filter, .. } => filter.name(),
        }
    }

    /// The identity egui remembers a panel's place by
    ///
    /// Not the title. A system's row is replaced by every fetch that covers
    /// it, so a window named for the row would forget where it was dragged
    /// to; what makes two panels the same panel is what they are about.
    fn id(&self) -> egui::Id {
        match self {
            Subject::System(system) => {
                egui::Id::new(("system-panel", system.address))
            }
            // Which system as well as which of its numbering, an id being
            // one system's own and every system having a body one.
            Subject::Star(star) => {
                egui::Id::new(("star-panel", star.system_address, star.id))
            }
            Subject::Body(body) => {
                egui::Id::new(("body-panel", body.system_address, body.id))
            }
            // A trip's own panel is keyed on the trip and the ship rather
            // than on the joined route, because the joined route *grows*:
            // its legs land one at a time and the panel is rebuilt off
            // whatever the bar now holds ([`crate::ui::trip_now`]). Keyed
            // on the filter, every leg that landed would have been a new
            // window, dropped back into the tiling and losing wherever the
            // user had dragged the last one.
            Subject::Filter { filter, .. } => match filter
                .trip()
                .zip(filter.ship())
            {
                Some((trip, ship)) => egui::Id::new(("trip-panel", trip, ship)),
                None => egui::Id::new(("filter-panel", filter)),
            },
        }
    }
}

impl Panels {
    /// Open a panel describing `system`
    ///
    /// A system already being read about is left where it is rather than
    /// opened a second time, since two windows describing one system are two
    /// copies of one answer.
    pub fn open_system(&mut self, system: System) {
        self.push(Subject::System(system));
    }

    /// Open a panel describing `star`
    pub fn open_star(&mut self, star: DbStar) {
        self.push(Subject::Star(star));
    }

    /// Open a panel describing `body`
    pub fn open_body(&mut self, body: DbBody) {
        self.push(Subject::Body(body));
    }

    /// Open a panel listing what `filter` admits
    pub fn open_filter(&mut self, filter: Filter) {
        self.push(Subject::Filter { filter, legs: Vec::new(), systems: None });
    }

    /// Say what a whole trip holds, leg by leg
    ///
    /// `whole` is the route the trip is flown as and `legs` are the routes it
    /// is made of, in the order they are flown. One panel about the trip
    /// rather than one per leg: each leg has a row of its own in the bar and
    /// a panel of its own behind it.
    pub fn open_trip(&mut self, whole: Filter, legs: Vec<Filter>) {
        self.push(Subject::Filter { filter: whole, legs, systems: None });
    }

    fn push(&mut self, subject: Subject) {
        if self.open.iter().any(|panel| panel.subject.id() == subject.id()) {
            return;
        }
        let slot = self.free();
        self.open.push(Panel { subject, slot, placed: false });
    }

    /// The first place in the tiling nothing stands in
    ///
    /// So a panel shut hands its place on, and the next one opened lands in
    /// the gap rather than below everything the user has already read and put
    /// away.
    fn free(&self) -> usize {
        // One more place than there are panels open, so one of them is free
        // however the rest are spread out.
        (0..=self.open.len())
            .find(|slot| self.open.iter().all(|panel| panel.slot != *slot))
            .expect("more places than panels")
    }
}

/// Which place down and across the tiling the panel at `slot` stands in
///
/// Down the right hand edge until another would not fit above the bottom of
/// the viewport, then across into a fresh column to its left, and back to the
/// corner once the viewport is full. Answers in places rather than in pixels,
/// so how large a panel is stays the caller's business.
///
/// Filling the last place is the one time two panels are left on top of each
/// other. There is nowhere else for the next one to go, and shrinking every
/// panel to make room would be a poor trade for the one the user is reading.
fn tile(slot: usize, down: usize, across: usize) -> (usize, usize) {
    let down = down.max(1);
    let slot = slot % (down * across.max(1));
    (slot % down, slot / down)
}

/// What each faction the map has had to name is called
///
/// A [`System`] carries the ids of the factions present in it, since that is
/// what is asked of it in bulk: which of them a filter admits, over every
/// system drawn, every frame. What they are called is asked for rarely and a
/// panel at a time, so it is looked up when a panel wants it and kept.
///
/// Kept for the session. A faction's name does not change, and there are only
/// as many of them here as the user has opened panels for.
#[derive(Resource, Default)]
pub struct FactionNames(HashMap<i32, String>);

impl FactionNames {
    /// What the faction with `id` is called, if it has been looked up
    pub fn get(&self, id: i32) -> Option<&str> {
        self.0.get(&id).map(String::as_str)
    }
}

/// Look up the names of any factions an open panel cannot name yet
///
/// One query for everything unnamed across every open panel, and none at all
/// once they are named, which is every frame but the one after a panel opens.
///
/// Asked for and waited on, as a search is. This is the answer to something
/// the user just did, it is a primary key lookup over a handful of ids, and a
/// panel that filled itself in a moment later would be a panel that looked
/// broken when it opened.
fn name_factions(
    mut names: ResMut<FactionNames>,
    panels: Res<Panels>,
    factions: Res<Factions>,
) {
    let wanted: Vec<i32> = panels
        .open
        .iter()
        .filter_map(|panel| match &panel.subject {
            Subject::System(system) => Some(system),
            // A filter panel lists systems by name and says nothing about
            // whose they are, and nothing inside a system belongs to anyone:
            // a faction holds a system, not a rock in one.
            Subject::Star(_) | Subject::Body(_) | Subject::Filter { .. } => {
                None
            }
        })
        .flat_map(|system| system.factions.iter().copied())
        .filter(|id| names.get(*id).is_none())
        .collect();
    if wanted.is_empty() {
        return;
    }

    // From the resident faction table, which the whole galaxy's names are held
    // in, so a panel names its factions without a fetch.
    for id in wanted {
        if let Some(name) = factions.name(id) {
            names.0.insert(id, name.to_string());
        }
    }
}

/// Keep each panel on whatever the map last heard about its system
///
/// A panel is drawn from the row it was opened with, and a fetch replaces
/// the row of a system already on the map without the panel hearing of it.
/// So a row that has changed is copied across, and a panel says what the map
/// holds rather than what it held.
fn refresh(
    mut panels: ResMut<Panels>,
    changed: Query<&System, Changed<System>>,
) {
    // Every star arrives changed, so without this the whole of a fetch is
    // walked for the sake of the panels nobody has open.
    if panels.open.is_empty() {
        return;
    }
    for system in &changed {
        for panel in &mut panels.open {
            // Only a panel about that one system. A filter's list came from
            // the database rather than the map, and a row that changed under
            // the map says nothing about whether the list is still the right
            // list.
            if let Subject::System(shown) = &mut panel.subject
                && shown.address == system.address
            {
                *shown = system.clone();
            }
        }
    }
}

/// Fetch the systems a filter admits, once a panel has been opened for it
///
/// From the database rather than from the map. The list is there to say where
/// a faction is, and the map holds only what the spyglass has dragged in,
/// which is mostly wherever the user has already been.
///
/// Asked for and waited on, as a search is: this answers something the user
/// just did, and a panel that filled itself in some frames later would look
/// broken when it opened.
///
/// It is the heaviest thing the map waits on. Measured at 44ms for the
/// largest faction on record, which stands in 314 systems, so opening one of
/// those panels drops a couple of frames. Worth moving onto a task if it comes
/// to be done often, and not worth the machinery while it is a click.
fn fill_filters(
    mut panels: ResMut<Panels>,
    populated: Res<Populated>,
    names: Res<Names>,
) {
    let unfilled: Vec<Filter> = panels
        .open
        .iter()
        .filter_map(|panel| match &panel.subject {
            Subject::Filter { filter, systems: None, .. } => {
                Some(filter.clone())
            }
            _ => None,
        })
        .collect();

    for filter in unfilled {
        let found = fetch(&populated, &names, &filter);
        for panel in &mut panels.open {
            if let Subject::Filter { filter: shown, systems, .. } =
                &mut panel.subject
                && *shown == filter
            {
                *systems = Some(found.clone());
            }
        }
    }
}

/// What the map can draw of everything `filter` admits
///
/// Put in the filter's own order where it has one, and by name where it has
/// not. What comes back is in no order at all, and which order a list holds
/// is the filter's to say.
///
/// Systems with no position on record are dropped. The map cannot draw one
/// and cannot fly to one, so a line naming it would answer nothing.
///
/// Which systems those are is the filter's own business. This is only what a
/// panel can do with them.
fn fetch(populated: &Populated, names: &Names, filter: &Filter) -> Vec<System> {
    // Already drawable, since the filter builds them from the resident tables
    // and drops any the names table cannot place.
    let mut found: Vec<System> = filter.systems(populated, names);

    // A filter with an order of its own has already answered in it: a route's
    // systems are the hops it is flown through, in the order they are flown.
    // Sorting them by where each falls in the route said the same thing until
    // a system stood in one twice — which is what a trip flown home to where
    // it set out from is — and then the last hop, being also the first, was
    // sorted to the front and the leg home was lost from the list and from
    // what the panel says the flying comes to.
    //
    // Where there is no order of its own, by name: what comes back is in no
    // order at all, and a list has to be in some order to hold still.
    if !filter.ordered() {
        found.sort_unstable_by(|one, other| one.name.cmp(&other.name));
    }
    found
}

/// What a click over a filter's panel asked of the filter
///
/// The panel reads as the filter's own row in the bar does, through the same
/// [`crate::ui::asked_of_row`]: a click says it is the one being worked with.
/// A panel offers none of the rest a row does -- no switch, no marks -- and
/// says so by passing `false`.
///
/// The double among them, which frames a row's systems, is one a window
/// cannot be given: egui rolls a panel up into its title bar on a double
/// click there, so a panel framed that way would fold shut on the way. The
/// button inside it asks instead.
///
/// Nothing until the button comes up. The press that reaches a panel is not
/// yet a gesture: what it lands on inside decides what it meant, and a leg's
/// name says the leg rather than the trip it is part of. Asked while the
/// button was still down, the panel answered first and was corrected on the
/// release, and the selection flickered through the wrong route on the way.
fn asked_of_panel(clicked: bool) -> Option<crate::ui::RowGesture> {
    crate::ui::asked_of_row(false, false, false, false, clicked)
}

/// Tell the user what is known about the systems they have opened
///
/// Written here rather than alongside the rest of the UI because a
/// [`System`]'s fields are the business of this module and its neighbours,
/// and this is the one place they are read out rather than drawn with.
#[allow(clippy::too_many_arguments)]
fn panels(
    mut contexts: EguiContexts,
    mut panels: ResMut<Panels>,
    names: Res<FactionNames>,
    mut selection: ResMut<Selection>,
    mut filters: ResMut<Filters>,
    mut selected: ResMut<crate::systems::route::SelectedFilter>,
    mut clock: ResMut<crate::systems::bodies::Clock>,
    orbit: Query<&OrbitCamera>,
    mut camera: MessageWriter<MoveCamera>,
    contents: Res<crate::systems::bodies::Contents>,
    // What is still being searched, so a trip's panel can say what is not
    // in it yet.
    searching: Res<crate::systems::route::frontier::Frontiers>,
    // Which systems can supercharge, which is the one thing about a star's
    // kind the index publishes for all of them.
    boosts: Res<crate::Boosts>,
    // And the real class of the stars a panel lists, looked up by address:
    // a list is finite where the galaxy is not. See [`StarClasses`].
    mut classes: ResMut<StarClasses>,
    transport: Res<crate::Transport>,
) -> Result {
    if panels.open.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // Where the camera is looking, which is the distance the spyglass and
    // the selection's own row are measured in.
    let center = orbit.single().map(|camera| camera.center).ok();
    // Where the eye stands, for a system's apparent magnitude — how bright it
    // looks from here, the figure the realistic view sizes a star by.
    let eye = orbit.single().map(|camera| camera.eye).ok();
    // The top right corner, clear of the settings pane and the bar, which
    // stand against the left edge and the top of it. The corner itself, since
    // a panel is placed by its own right hand top rather than by its left: a
    // window is as wide as its title bar needs, and a place worked out from a
    // width guessed for it is a place a wider panel overhangs the viewport
    // from, to be pushed back inside without the margin it was given.
    //
    // Only where a panel opens: the windows are movable, so where they end
    // up is the user's business.
    let room = ctx.content_rect();
    let corner = room.right_top() + egui::vec2(-MARGIN, MARGIN);

    // A panel and the gap under it, and how many of those the viewport holds
    // each way. Worked out afresh every frame, since the window it is all
    // measured against is the user's to resize.
    let width = panels.width.max(
        WIDTH + egui::Frame::window(&ctx.global_style()).total_margin().sum().x,
    );
    let step = egui::vec2(-(width + MARGIN), panels.height + MARGIN);
    let down = ((room.height() - MARGIN) / step.y).floor().max(1.) as usize;
    let across = ((room.width() - MARGIN) / -step.x).floor().max(1.) as usize;

    // What has landed since the last frame, and what the panels now list.
    // Asked here rather than where the rows are drawn: the drawing borrows
    // the panel, and one question a system is one question however many
    // panels list it.
    classes.poll();
    let listed: Vec<i64> = panels
        .open
        .iter()
        .filter_map(|panel| match &panel.subject {
            Subject::Filter { systems: Some(systems), .. } => Some(systems),
            _ => None,
        })
        .flatten()
        .map(|system| system.address)
        .collect();
    classes.ask(listed, &transport);

    let mut shut = Vec::new();
    let mut tallest: f32 = 0.;
    let mut widest: f32 = 0.;
    let mut moved = None;
    let mut picked = None;
    let mut opening = None;
    let mut wanted = None;
    // Whether the pointer was clicked over whichever panel it was over,
    // asked once for the whole pass: a click is one click however many
    // windows are drawn.
    //
    // Read on the click rather than on the press, as every row in the bar and
    // every line in these panels is read. A press writes while the button is
    // still down, and what a press means is not settled until it is let go
    // of: a press landing on a leg's name inside a trip's panel would say the
    // whole trip was being worked with, and the leg would only say otherwise
    // when the button came up.
    let clicked = ctx.input(|input| {
        input.pointer.button_clicked(egui::PointerButton::Primary)
    });
    let mut chosen = None;
    // Where a thing inside a system stands may rest on a place the map made
    // up rather than on one anybody scanned — see
    // [`crate::systems::bodies::Contents::guessed_under`] — and that is a
    // thing a panel has to say out loud, in the word for what kind of place
    // it was: a close pair's centre is the commonest, and an unscanned star
    // or body is stood up the same way.
    //
    // Only asked where a panel about something inside a system is open, and
    // only of the system whose rows are in hand: a panel holds a row and
    // outlives the camera leaving, so what is held may be about somewhere else
    // entirely by the time it is read.
    let orbits = panels
        .open
        .iter()
        .any(|panel| {
            matches!(panel.subject, Subject::Star(_) | Subject::Body(_))
        })
        .then(|| contents.orbits());
    let guessed = |address: i64, id: i16| {
        let held =
            orbits.as_ref().filter(|_| contents.of() == Some(address))?;
        contents.guessed_under(held, id)
    };
    let mut worked = None;
    // The stops a click on a leg's name inside a trip's panel asked to pick
    // out, and whether as well as instead. Handed out of the panels rather
    // than acted on inside them, as the system rows are.
    let mut picked_stops: Option<(Vec<System>, bool)> = None;
    // How much of a panel turned out to be its window, for the next frame to
    // take off the room; see [`Panels::frame`].
    let mut framing: f32 = 0.;
    let was_framing = panels.frame;
    for panel in &mut panels.open {
        let mut showing = true;
        let (row, column) = tile(panel.slot, down, across);
        let at =
            corner + egui::vec2(step.x * column as f32, step.y * row as f32);
        // Set before the panel is read from, so that the two borrows of it
        // do not overlap.
        let placed = std::mem::replace(&mut panel.placed, true);
        let id = panel.subject.id();
        let room = room_under(ctx, id, at, was_framing);
        let window = framed(
            ctx,
            panel.subject.title(),
            id,
            at,
            room,
            placed,
            &mut showing,
        );
        // A trip's legs land one at a time, so its joined route is rebuilt
        // from whatever the bar now holds rather than kept as it was when
        // the panel opened. Without this a panel opened mid-plot describes
        // a partial trip — its systems, its distance, its longest jump —
        // as though that were the whole of it, and never corrects itself.
        // The systems it fetched are for the route as it *was*, so they go
        // back to being unasked and `fill_filters` asks again.
        if let Subject::Filter { filter, legs, systems } = &mut panel.subject
            && let Some((joined, flown)) = crate::ui::trip_now(filter, &filters)
            && (*filter != joined || *legs != flown)
        {
            *filter = joined;
            *legs = flown;
            *systems = None;
        }

        // And how much of it is still being searched, for a trip: the
        // legs each carry the trip they belong to, so the searches under
        // way say how many of this one's are outstanding.
        let outstanding = match &panel.subject {
            Subject::Filter { filter, .. } => {
                filter.trip().map_or(0, |trip| searching.plotting(trip))
            }
            _ => 0,
        };

        // What the search that answered this cost, read off the row before
        // the panel is drawn: the drawing borrows the panel and the rows are
        // written further down the same pass.
        let timed = match &panel.subject {
            Subject::Filter { filter, legs, .. } => {
                took(filter, legs, &filters)
            }
            _ => None,
        };
        let mut held = 0.;
        let window = window.show(ctx, |ui| {
            held = inside(ui, id, room, |ui| match &panel.subject {
                Subject::System(system) => {
                    described(ui, system, &names, eye, &mut moved, &mut wanted)
                }
                Subject::Star(star) => mark_if_moved(&mut clock, |clock| {
                    star_described(
                        ui,
                        star,
                        clock,
                        guessed(star.system_address, star.id),
                    )
                }),
                Subject::Body(body) => mark_if_moved(&mut clock, |clock| {
                    body_described(
                        ui,
                        body,
                        clock,
                        guessed(body.system_address, body.id),
                    )
                }),
                Subject::Filter { filter, legs, systems } => admitted(
                    ui,
                    filter,
                    legs,
                    systems.as_deref(),
                    timed,
                    outstanding,
                    &boosts,
                    &classes,
                    center,
                    &mut picked,
                    &mut picked_stops,
                    &mut opening,
                    &mut moved,
                    &mut worked,
                ),
            })
        });

        // Only a panel that drew what it holds. A window rolled up into its
        // title bar stands a line high, which is no height to place the next
        // panel by. Its width is what it always was, the title bar being the
        // one part of it that is drawn either way, so that is taken from any
        // panel that was shown at all.
        if let Some(window) = window {
            widest = widest.max(window.response.rect.width());
            if window.inner.is_some() {
                tallest = tallest.max(window.response.rect.height());
                // What the window took over what it held, which is the title
                // bar and the margins. The most of any panel drawn, so the
                // room is measured against the hungriest of them.
                framing = framing.max(window.response.rect.height() - held);
            }
            // A panel about a filter reads as the filter's own row does: a
            // click says it is the one being worked with, a double says to
            // see the whole of what it admits. Through [`crate::ui::asked_of_row`]
            // so the order is the one written down there.
            //
            // Egui has already settled which window the pointer is over:
            // `contains_pointer` answers for the one on top, so a panel under
            // another does not take a click meant for it.
            if window.response.contains_pointer()
                && let Subject::Filter { filter, .. } = &panel.subject
                && asked_of_panel(clicked)
                    == Some(crate::ui::RowGesture::Select)
            {
                chosen = Some(filter.clone());
            }
        }
        if !showing {
            shut.push(panel.subject.id());
        }
    }

    if let Some(move_) = moved {
        camera.write(move_);
    }
    // Picking a system out of a list says which one is meant, as clicking a
    // star does. Where the camera goes is asked for separately, from the row
    // in the bar that names what is picked out.
    if let Some((system, gathering)) = picked {
        selection.pick(Picked::System(system), gathering);
    }
    // And a leg named in a trip's panel picks out what it was plotted
    // between, which is what a click on its row in the bar means. Through the
    // one call, so the two cannot come to two different things.
    if let Some((stops, gathering)) = picked_stops {
        selection.pick_out(stops.into_iter().map(Picked::System), gathering);
    }
    // Opened after the loop, since a panel asked for from inside one is a
    // panel pushed onto the list being walked.
    if let Some(system) = opening {
        panels.open_system(system);
    }
    // Already resolved, both halves of it having been read off a system the
    // map holds, so it goes straight in rather than round by `Lookup`.
    if let Some(filter) = wanted {
        filters.add(filter);
    }
    // Pressing a filter's panel is how the user says which of the ones on
    // screen they mean, as clicking its row in the bar is. Every kind can be
    // said; only a route reads that it was, `route::active` weighing what is
    // picked out against the routes being shown, so a faction picked out
    // leaves the routes as they were.
    // A leg named inside a trip's panel beats the press that reached the
    // panel at all: the press says which window is being worked with, and the
    // name inside it says which of the routes drawn there is meant.
    if let Some(filter) = worked.or(chosen) {
        selected.0 = vec![filter];
    }

    // Nothing while every panel is rolled up into its title bar, which is
    // not a height to place the next one by.
    if tallest > 0. {
        panels.height = tallest;
    }
    if framing > 0. {
        panels.frame = framing;
    }
    if widest > 0. {
        panels.width = widest;
    }
    if !shut.is_empty() {
        panels.open.retain(|panel| !shut.contains(&panel.subject.id()));
    }

    Ok(())
}

/// Everything the map knows about one system
fn described(
    ui: &mut Ui,
    system: &System,
    names: &FactionNames,
    eye: Option<DVec3>,
    moved: &mut Option<MoveCamera>,
    wanted: &mut Option<Filter>,
) {
    egui::Grid::new(("system-fields", system.address)).num_columns(2).show(
        ui,
        |ui| {
            let [x, y, z] = system.position;
            copied(ui, "Position", format!("{x:.2}, {y:.2}, {z:.2}"));
            // What the realistic view sizes a star by: the magnitude the index
            // build assigned, how bright it looks from where the camera stands,
            // and its tint bucket. Unknown for a system built from a name lookup
            // rather than a payload point.
            field(
                ui,
                "Abs. magnitude",
                match system.indexed_magnitude() {
                    Some(m) => format!("{m:.1}"),
                    None => UNKNOWN.into(),
                },
            );
            if let (Some(m), Some(eye)) = (system.indexed_magnitude(), eye) {
                let away = eye.distance(DVec3::from(system.position));
                field(
                    ui,
                    "App. magnitude",
                    format!(
                        "{:.1}",
                        Magnitude(m as f64)
                            .apparent(Distance::light_years(away))
                            .0
                    ),
                );
            }
            if let Some(t) = system.indexed_temperature() {
                field(ui, "Temperature", format!("{t:.0} K"));
            }
            // Two rows rather than one, because the two counts are reported
            // separately: the all-found tally and a nav beacon give the bodies
            // alone, and only the honk ever counts the belts and rings. Either
            // can stand known while the other is not.
            field(ui, "Bodies", named(&system.body_count));
            field(ui, "Belts and rings", named(&system.non_body_count));
            field(ui, "Population", crate::ui::thousands(system.population));
            field(ui, "Allegiance", named(&system.allegiance));
            field(ui, "Government", named(&system.government));
            field(ui, "Security", named(&system.security));
            economies(ui, &system.economies);
            // Unknown for the same systems the magnitude is: the moment rides
            // on the payload point, and one built off the names table has none.
            field(
                ui,
                "Updated",
                match system.updated_at {
                    Some(at) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
                    None => UNKNOWN.into(),
                },
            );
        },
    );

    factions(ui, &system.factions, names, wanted);

    // Its own system rather than whatever is selected, since several panels
    // stand open at once and each one is about the system named in its title
    // bar.
    ui.add_space(MARGIN);
    ui.horizontal(|ui| {
        if ui.button("Center Camera").clicked() {
            *moved = Some(MoveCamera {
                position: Some(DVec3::from(system.position)),
                framing: None,
            });
        }
        // The name is what the user carries out of here: into the game's own
        // map, a message to somebody, a spreadsheet. Nothing on the panel is
        // otherwise selectable, and a name retyped is a name misspelled.
        if ui.button("Copy Name").clicked() {
            ui.ctx().copy_text(system.name.to_string());
        }
    });
}

/// Metres in a solar radius
///
/// What a star's size is read in. Metres are what the database holds and what
/// the map draws in, and a star written out in them is a run of digits nobody
/// counts.
const SOLAR_RADIUS: f64 = 6.957e8;

/// Metres per second squared in a gravity
///
/// The journal records what a body pulls at in metres per second squared, and
/// what anybody wants to know is how that compares to standing on Earth.
const GRAVITY: f64 = 9.80665;

/// Pascals in an atmosphere
const ATMOSPHERE: f64 = 101_325.;

/// Seconds in a day
pub(crate) const DAY: f64 = 86_400.;

/// Days in an Earth year, for reading a span too long to say in days
const YEAR: f64 = 365.25;

/// Everything the map knows about one star
///
/// Read in the units a star is talked about in rather than the ones it is
/// stored in: suns for its size and its mass, days for its turn, and light
/// seconds for how far out it stands.
fn star_described(
    ui: &mut Ui,
    star: &DbStar,
    clock: &mut crate::systems::bodies::Clock,
    guessed: Option<&str>,
) {
    egui::Grid::new(("star-fields", star.system_address, star.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", format!("{}{}", star.star_class, star.subclass));
            field(ui, "Luminosity", star.luminosity.clone());
            field(
                ui,
                "Distance",
                format!("{:.1} Ls", star.distance_from_arrival_ls),
            );
            field(
                ui,
                "Radius",
                format!("{:.3} Sol", star.radius as f64 / SOLAR_RADIUS),
            );
            field(ui, "Mass", format!("{:.3} Sol", star.stellar_mass));
            field(ui, "Temperature", format!("{:.0} K", star.temperature));
            field(
                ui,
                "Age",
                format!(
                    "{} million years",
                    crate::ui::thousands(star.age_my.max(0) as u64)
                ),
            );
            field(ui, "Magnitude", format!("{:.2}", star.absolute_magnitude));
            turning(ui, &star.spin);
            circling(ui, star.orbit.as_ref(), clock, guessed);
            field(ui, "Mapped", yes_no(star.mapped));
            field(ui, "Discovered", dated(star.discovered_at));
            field(
                ui,
                "Updated",
                star.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// Everything the map knows about one body
fn body_described(
    ui: &mut Ui,
    body: &DbBody,
    clock: &mut crate::systems::bodies::Clock,
    guessed: Option<&str>,
) {
    egui::Grid::new(("body-fields", body.system_address, body.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", body.planet_class.clone());
            field(
                ui,
                "Distance",
                match body.distance_from_arrival {
                    Some(away) => format!("{away:.1} Ls"),
                    None => UNKNOWN.into(),
                },
            );
            field(
                ui,
                "Radius",
                format!(
                    "{} km",
                    crate::ui::thousands((body.radius / 1e3) as u64)
                ),
            );
            field(ui, "Mass", format!("{:.3} Earths", body.mass));
            field(
                ui,
                "Gravity",
                format!("{:.2} g", body.gravity as f64 / GRAVITY),
            );
            field(
                ui,
                "Temperature",
                match body.temperature {
                    Some(heat) => format!("{heat:.0} K"),
                    None => UNKNOWN.into(),
                },
            );
            standing(ui, &body.surface);
            field(ui, "Tidal lock", yes_no(body.tidal_lock));
            turning(ui, &body.spin);
            circling(ui, Some(&body.orbit), clock, guessed);
            field(ui, "Mapped", yes_no(body.mapped));
            field(ui, "Discovered", dated(body.discovered_at));
            field(
                ui,
                "Updated",
                body.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// What a body's surface is like, where it has one
///
/// A gas giant has none, and is one line saying so rather than a header over
/// five rows of nothing.
fn standing(ui: &mut Ui, surface: &Option<Surface>) {
    let Some(surface) = surface else {
        field(ui, "Surface", "None".into());
        return;
    };

    ui.label(egui::RichText::new("Surface").strong());
    ui.end_row();
    under(ui, "Landable", yes_no(surface.landable));
    under(ui, "Atmosphere", surface.atmosphere_type.to_string());
    under(
        ui,
        "Pressure",
        format!("{:.3} atm", surface.pressure as f64 / ATMOSPHERE),
    );
    under(ui, "Volcanism", named(&surface.volcanism));
    under(ui, "Terraforming", named(&surface.terraform_state));
    if let Some(crust) = &surface.composition {
        under(ui, "Composition", made_of(crust));
    }
    prospected(ui, &surface.materials);
}

/// What a crust is made of, as the scan divides it
///
/// Three fractions summing to one, said in hundredths and richest first,
/// which is the order the body is described in rather than the order the scan
/// happens to write. A part the scan found none of is left out: a rocky body
/// is rock and metal, and a line saying it is nought parts ice says nothing.
fn made_of(crust: &Composition) -> String {
    let mut parts =
        [("rock", crust.rock), ("metal", crust.metal), ("ice", crust.ice)];
    // `total_cmp` rather than `partial_cmp`: a fraction the scan left as NaN
    // still sorts somewhere rather than panicking the sort.
    parts.sort_by(|one, other| other.1.total_cmp(&one.1));
    let said: Vec<String> = parts
        .iter()
        .filter(|(_, share)| *share > 0.)
        .map(|(part, share)| format!("{:.0}% {part}", share * 100.))
        .collect();
    if said.is_empty() { UNKNOWN.into() } else { said.join(", ") }
}

/// The raw materials a surface can be prospected for, one to a row
///
/// Richest first, since what is asked of a body is what it has most of.
/// Nothing at all where the scan listed none: a header over no rows is a
/// header over nothing.
fn prospected(ui: &mut Ui, materials: &[Material]) {
    if materials.is_empty() {
        return;
    }

    ui.label(egui::RichText::new("Materials").strong());
    ui.end_row();
    for (name, share) in richest(materials) {
        under(ui, &name, format!("{share:.1}%"));
    }
}

/// The materials of a surface, named as they are read, richest first
///
/// Split from [`prospected`] because the order and the spelling are what
/// there is to get wrong, and neither needs a `Ui` to be asked about.
fn richest(materials: &[Material]) -> Vec<(String, f64)> {
    let mut sorted: Vec<(String, f64)> = materials
        .iter()
        .map(|material| (capitalised(&material.name), material.percent))
        .collect();
    sorted.sort_by(|one, other| other.1.total_cmp(&one.1));
    sorted
}

/// `word` with its first letter upper case
///
/// The journal spells a material `iron`, and a panel writes names as names.
/// A letter at a time, since a letter's upper case may be more than one
/// letter and the rest of the word is left exactly as it was read.
fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// How a thing turns on its own axis
fn turning(ui: &mut Ui, spin: &Spin) {
    ui.label(egui::RichText::new("Spin").strong());
    ui.end_row();
    under(ui, "Period", lasting(spin.period));
    under(ui, "Tilt", format!("{:.1}°", spin.tilt.to_degrees()));
}

/// The path a thing takes about whatever it goes round
///
/// One line saying None for the one that goes round nothing, which is the
/// star a system arrives at, as a body with no surface is one line saying the
/// same.
///
/// `guessed` names the kind of place this path is measured from where that
/// place is one the map made up: a close pair whose centre was never scanned,
/// or a star or body a chain names with no row behind it, is stood up at
/// the distance its riders report in a direction nobody measured, so
/// everything under it is drawn exactly as its own scan says about a point
/// that is a guess. The path is a reading either way, and where it puts the
/// thing is not, which is the one thing about it a panel could not otherwise
/// say.
fn circling(
    ui: &mut Ui,
    orbit: Option<&Orbit>,
    clock: &mut crate::systems::bodies::Clock,
    guessed: Option<&str>,
) {
    let Some(orbit) = orbit else {
        field(ui, "Orbit", "None".into());
        return;
    };

    ui.label(egui::RichText::new("Orbit").strong());
    ui.end_row();
    under(ui, "Radius", spanning(orbit.semi_major_axis));
    under(ui, "Period", lasting(orbit.orbital_period));
    under(ui, "Eccentricity", format!("{:.4}", orbit.eccentricity));
    under(ui, "Inclination", format!("{:.1}°", orbit.orbital_inclination));
    if let Some(kind) = guessed {
        under(ui, "Place", format!("Guessed ({kind} unscanned)"));
    }
    turned(ui, orbit.orbital_period as f64, clock);
}

/// Where round its orbit this thing stands, and a slider to move it by
///
/// Geared to this orbit alone, so the whole slider is one turn of this body
/// however long that is: a system has no span that suits all of it, the slowest
/// body of one taking a median 993 times as long to come round as its fastest.
///
/// It moves the map's own moment, which is one moment for the whole galaxy
/// and not an arrangement built body by body. So dragging this stirs
/// everything else by the same span of time, which for something far slower
/// is imperceptible and for something far faster is a blur -- and either is
/// the truth about what a year of this body does to its neighbours. A panel
/// outlives the camera leaving its system, so a slider dragged out there
/// moves the moment all the same; what it does not do any more is measure
/// that moment from a zero of its own.
///
/// Nothing to drag where the period is unrecorded, there being no turn to be a
/// fraction of.
///
/// The percentage is formatted rather than rounded. Egui clamps and rounds
/// the value it is handed before anything is drawn, and reports a slider
/// nobody touched as changed when the rounding moved it -- so a rail that
/// rounded to a tenth of a percent wrote that tenth back into the clock every
/// frame the phase was anything else. With another slider under the date
/// setting the same offset, the two fought over it frame by frame and the
/// date flickered between them; a phase that rounded up to a whole 100% took
/// the map on by one of this body's turns every frame, which is what ran the
/// reading off the end of what a date can hold.
fn turned(ui: &mut Ui, period: f64, clock: &mut crate::systems::bodies::Clock) {
    ui.horizontal(|ui| {
        ui.add_space(NEST);
        ui.label(egui::RichText::new("Phase").strong());
    });

    let mut through = clock.through(period) * 100.;
    let moved = ui
        .add_enabled_ui(period > 0., |ui| {
            ui.add(
                egui::Slider::new(&mut through, 0.0..=100.)
                    .custom_formatter(|through, _| format!("{through:.1}%")),
            )
        })
        .inner;
    // The turn the phase is measured in is the clock's own to keep, so
    // nothing here has to say when a drag begins or ends. See
    // [`crate::systems::bodies::Clock::offset_to`].
    if moved.changed() {
        clock.offset_to(period, through / 100.);
    }
    ui.end_row();
}

/// How long something takes, in the largest unit it fills
///
/// Days for anything that turns slowly, which is most of what is scanned, and
/// hours for the rest. A period in seconds is eight digits nobody reads.
///
/// Earth's, and said so for the two units a body has of its own. A day is a
/// body's turn about itself and a year is its turn about its star -- both of
/// them things the panel is otherwise reporting, so `1.2 days` beside a
/// rotation period is a real question about whose day is meant. An hour is
/// nobody's, so it goes unremarked, and neither is anything under one.
///
/// Down to seconds, which no orbit on record is but a span the map has been
/// run on by certainly can be: the status slider's near end is minutes, and a
/// span of them read as `0.1 hours` is a number in the wrong unit.
pub(crate) fn lasting(seconds: f32) -> String {
    if seconds <= 0. {
        return UNKNOWN.into();
    }
    let days = seconds as f64 / DAY;
    // Years for the long end, which a system's outermost bodies live at: the
    // slowest body of a system takes a median eighteen years to come round, and
    // six thousand days is a number nobody reads either.
    if days >= YEAR {
        format!("{:.1} Earth years", days / YEAR)
    } else if days >= 1. {
        format!("{days:.1} Earth days")
    } else if days * 24. >= 1. {
        format!("{:.1} hours", days * 24.)
    } else if days * 24. * 60. >= 1. {
        format!("{:.1} minutes", days * 24. * 60.)
    } else {
        format!("{:.0} seconds", seconds)
    }
}

/// How far something reaches, in the larger unit it fills
///
/// Light seconds where a light second is not most of the answer, and
/// kilometres where it is. A moon a few thousand kilometres out is a
/// hundredth of a light second, and a planet is millions of kilometres.
fn spanning(metres: f32) -> String {
    let metres = metres as f64;
    if metres >= crate::space::LIGHT_SECOND {
        format!("{:.2} Ls", metres / crate::space::LIGHT_SECOND)
    } else {
        format!("{} km", crate::ui::thousands((metres / 1e3) as u64))
    }
}

/// What the database says of a yes or no question
fn yes_no(answer: bool) -> String {
    if answer { "Yes".into() } else { "No".into() }
}

/// When something happened, where anything has said it did
///
/// Unknown for a discovery whose time nobody reported, which is most of them:
/// a scan finding a body already charted says somebody had been there without
/// saying when, and only a scan that found it unclaimed dates the finding.
fn dated(at: Option<DateTime<Utc>>) -> String {
    match at {
        Some(at) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
        None => UNKNOWN.into(),
    }
}

/// What a filter's panel says it is showing, above the list of it
///
/// How many systems, and for a route the whole of what it was plotted for: the
/// range, the drive, and how hard the search worked at it. A panel is titled
/// with what the filter is called, and a route is called after its two ends,
/// so two plots between the same pair come up under the same name. What tells
/// them apart is what was asked for — and all three of those are part of the
/// ask, each of them able to change which systems come back.
///
/// A range rather than a jump. It is how far the ship can go in one, which is
/// what the route was worked out against; how far it actually goes is the
/// distance on each line of the list below, and is usually less. Which is the
/// reason the drive belongs here beside it: a jump of nine hundred light years
/// on a line below, under a range of a hundred and fifty, is a jet cone and
/// not a mistake, and nothing else on screen said a jet cone was allowed.
///
/// Said in the panel rather than in the title, which is cut to the room a
/// window has and would lose it. The other filters are named for the whole of
/// what they are and have nothing to add here.
fn summary(
    filter: &Filter,
    count: usize,
    took: Option<Duration>,
    plotting: usize,
) -> String {
    let Some(range) = filter.range() else {
        return format!("{count} systems");
    };
    // Both are part of what was asked, and both are read off the route rather
    // than off the settings, which the user may have moved since.
    let boosted = filter.drive().and_then(|drive| drive.named());
    // What was asked for, in its own words: "fewest jumps", or "within 5%
    // of fewest", with ", shortest" where the ties were settled by
    // distance. It used to be one of three mode names and a trailing
    // "search", which said which button was pressed rather than what the
    // route is.
    let how = filter.how().map(|how| how.named()).unwrap_or_default();
    let mut said = match boosted {
        Some(boosted) => {
            format!("{count} systems, {range} Ly range, {boosted}, {how}")
        }
        None => format!("{count} systems, {range} Ly range, {how}"),
    };
    // What it cost to find, which is the other half of what the search mode
    // means: `quick` and `direct` differ by a jump or two and by minutes,
    // and the row that says which was asked for should say what it came to.
    // Nothing for a route this session did not plot — one restored, or one
    // whose row has been closed and re-added.
    if let Some(took) = took {
        said.push_str(&format!(", plotted in {}", crate::ui::waited(took)));
    }
    // And what is *not* in it yet. A trip is plotted a leg at a time, so a
    // panel read before the last of them lands describes a real route
    // through some of the stops — which is not the trip it is titled after,
    // and saying nothing about that is the panel lying by omission. The
    // systems, the distance and the longest jump below are all of them
    // about what has landed.
    if plotting > 0 {
        let legs = match plotting {
            1 => "1 leg".to_owned(),
            legs => format!("{legs} legs"),
        };
        said.push_str(&format!(" — {legs} still being plotted"));
    }
    said
}

/// The arrival star's class for the systems a panel lists
///
/// **A list is a finite thing, so it is looked up rather than guessed at.**
/// The map has no star class resident for every system — the payload
/// carries six temperature buckets and a route's stops are built from the
/// names table, which carries none — but a panel lists tens or hundreds of
/// systems, not two hundred million, and the index answers one address at a
/// time ([`galos_index::Source::bodies`]). So the classes are read for
/// exactly what is listed, off the task pool, once.
///
/// Held per address rather than per panel: two panels listing the same
/// system ask one question between them, and a class does not change while
/// the map is open.
///
/// [`None`] for a system nothing has scanned, which is most of the galaxy —
/// and said as nothing rather than as a guess.
#[derive(Resource, Default)]
pub struct StarClasses {
    known: HashMap<i64, Option<String>>,
    /// What has been asked and not yet answered, so a frame does not ask
    /// again while the pool is still reading.
    asked: HashSet<i64>,
    /// The reads under way, each answering for one address.
    reading: Vec<(i64, Task<Option<String>>)>,
}

impl StarClasses {
    /// The class where it is known, and [`None`] where it is not — whether
    /// because nothing is scanned or because the read has not landed.
    pub fn of(&self, address: i64) -> Option<&str> {
        self.known.get(&address)?.as_deref()
    }

    /// Ask about every address in `listed` that has not been asked about
    ///
    /// One read a system, off the pool: the index's own answer is a file per
    /// system, so there is nothing to batch.
    fn ask(
        &mut self,
        listed: impl IntoIterator<Item = i64>,
        transport: &crate::Transport,
    ) {
        for address in listed {
            if self.known.contains_key(&address) || !self.asked.insert(address)
            {
                continue;
            }
            let reading = transport.0.clone();
            self.reading.push((
                address,
                AsyncComputeTaskPool::get().spawn(async move {
                    let inside =
                        reading.bodies(address).await.unwrap_or_default();
                    arrival_class(&inside)
                }),
            ));
        }
    }

    /// Take in whatever has landed.
    fn poll(&mut self) {
        self.reading.retain_mut(|(address, task)| match bevy::tasks::block_on(
            bevy::tasks::futures_lite::future::poll_once(task),
        ) {
            Some(class) => {
                self.known.insert(*address, class);
                false
            }
            None => true,
        });
    }
}

/// The class of the star a ship drops in at
///
/// The index's own rule, not another one beside it
/// ([`galos_index::derive::arrival_class`]): the star nearest the arrival
/// point, ties broken by body id. It matters that this is the same rule the
/// published boost table was derived by — a panel that read the primary as
/// "the star that goes round nothing" would name a different star in a close
/// pair than the table saying whether that system can supercharge, and the
/// two readings would disagree about the same system on the same screen.
fn arrival_class(inside: &galos_index::meta::SystemBodies) -> Option<String> {
    galos_index::derive::arrival_class(inside).map(str::to_owned)
}

/// How a jump's fuel goes with its length, drive by drive
///
/// **What the map can say about fuel, and what it cannot.** The game's cost
/// of a jump is `multiplier x (distance x mass / optimal mass) ^ p`, which
/// wants the drive's class and rating, the hull, the cargo and the tank —
/// none of which the map is told. Dividing by the drive's own maximum
/// cancels nearly all of it, because a ship's *range* is by definition the
/// distance at which a jump costs that whole maximum:
///
/// ```text
/// fuel(d) / max fuel per jump = (d / range) ^ p
/// ```
///
/// The multiplier cancels, the laden mass cancels, the optimal mass
/// cancels. What is left is the jump against the range the route was
/// plotted at, and `p`, which is the drive's **class** and nothing else.
///
/// The exponent could be bounded — `p = 2.0` is the dearest any drive can
/// be, since a jump is no longer than the range — and a ceiling over every
/// drive in the game was what the panel said first. It read as a fact and
/// was not one: **nobody flies a bound over all drives, they fly a class 5
/// with a specific tank**, and a figure a third too high for their ship is
/// worse than no figure. So the panel states the distance, which is exact,
/// and this rule beside it, which is what turns the distance into fuel for
/// the ship the reader actually has.
///
/// The tank in these units is its capacity divided by the max fuel per
/// jump, both of which the outfitting screen states.
fn fuel_rule() -> String {
    let classes = POWERS
        .iter()
        .map(|(class, power)| format!("{class} → {power:.2}"))
        .collect::<Vec<String>>()
        .join(", ");

    format!(
        "A jump of d costs (d / range) ^ p of the drive's maximum fuel, \
         where p is its class: {classes}. The tank holds its capacity \
         divided by that maximum, so a 32 t tank at 0.90 t a jump is 35 \
         jumps' worth at full range — and far more at half of it, fuel \
         going as the square of the jump at least."
    )
}

/// The exponent of each frame shift drive class
///
/// The one ship fact the cancellation above leaves, and it depends on the
/// drive's size alone: a class 2 is the dearest per light year and a class
/// 8 the cheapest. Written out rather than interpolated, since it is a
/// table the game states and not a line anything derived.
const POWERS: [(u8, f64); 7] = [
    (2, 2.00),
    (3, 2.15),
    (4, 2.30),
    (5, 2.45),
    (6, 2.60),
    (7, 2.75),
    (8, 2.90),
];

/// What a route asks of a fuel tank, as far as the classes read say
///
/// The longest run of stops a ship crosses with **nothing to scoop**, and
/// which stop it sets out from. A fuel scoop takes hydrogen off the main
/// sequence and off nothing else (`galos_index::meta::scoopable`), so a
/// stretch of white dwarfs, brown dwarfs and black holes is a stretch the
/// ship crosses on the fuel it had — and where that stretch is longer than
/// the tank, the route is not a slower route, it is a stranded ship.
///
/// Said as the **distance** the stretch takes to cross, not as a count of
/// stops and not as a fuel figure. A count is the wrong reading on its own:
/// six short hops and two long jumps are the same count and nothing like
/// the same fuel, and what strands a ship is the fuel. A fuel figure is the
/// wrong reading too, because the map is not told the ship — see
/// [`fuel_rule`], which the line carries on hover so the reader can turn
/// the distance into their own drive's answer.
///
/// **A class nothing has read is not counted as unscoopable.** The run is
/// the stops *known* to have nothing to scoop, and the stops with no class
/// on record are counted separately and said separately — a route across
/// unexplored space is mostly unread, and reading that as a starving route
/// would condemn every galactic plot. So the reading is a floor: at least
/// this far, with this many unknown.
#[derive(Debug, Default, PartialEq)]
struct Scooping {
    /// The longest run of stops known to have nothing to scoop
    run: usize,
    /// Which stop that run sets out from
    from: Option<String>,
    /// How far crossing that run takes, in light years
    ///
    /// From the last star that could refuel the ship to the next one: the
    /// jumps out of the one up to and including the jump that lands on the
    /// other, a tank filled at the one having to reach the other. Nothing
    /// where the route states no distances.
    across: Option<f64>,
    /// How many of the route's stops have no class on record
    unread: usize,
}

impl Scooping {
    /// What a route's stops come to, walked in the order they are flown
    ///
    /// Each stop is its name, the class of the star waiting there, and how
    /// far the jump onto it was. A run ends where a scoopable star arrives,
    /// since the tank is full again there — and the jump that *landed* on
    /// that star was flown on the old tank, so it belongs to the run it
    /// ends. An unread stop ends a run as well rather than extending it:
    /// the run is what is known, and the unknowns are said beside it.
    fn of<'a>(
        stops: impl IntoIterator<Item = (&'a str, Option<&'a str>, Option<f64>)>,
    ) -> Scooping {
        let mut said = Scooping::default();
        let mut run = 0;
        let mut from: Option<&str> = None;
        // The jumps of the run standing, and the one that will land on the
        // next star able to refuel the ship.
        let mut jumps: Vec<f64> = Vec::new();

        // A finished run, against the longest one held. Weighed only where
        // a run *ends*, since the jump that lands on the star which refuels
        // the ship belongs to the run it ends and the run's length does not
        // grow to take it.
        let mut settle = |run: usize, from: Option<&str>, jumps: &[f64]| {
            if run > said.run {
                said.run = run;
                said.from = from.map(str::to_owned);
                said.across = match jumps.is_empty() {
                    true => None,
                    false => Some(jumps.iter().sum()),
                };
            }
        };

        for (name, class, jump) in stops {
            match class {
                Some(class) if galos_index::meta::scoopable(class) => {
                    // The jump that arrived here was flown before the tank
                    // was filled here, so it is the run's to pay for.
                    jumps.extend(jump);
                    settle(run, from, &jumps);
                    run = 0;
                    jumps.clear();
                }
                Some(_) => {
                    if run == 0 {
                        from = Some(name);
                        jumps.clear();
                    }
                    run += 1;
                    jumps.extend(jump);
                }
                // Nothing is known about refuelling here, so the run of
                // stops known to starve ends — without this jump, which
                // may well be paid for out of a tank filled here.
                None => {
                    said.unread += 1;
                    settle(run, from, &jumps);
                    run = 0;
                    jumps.clear();
                }
            }
        }
        // A route that ends mid-run: the ship still had to get there.
        settle(run, from, &jumps);
        said
    }

    /// What the panel says of it, or nothing where there is nothing to say
    ///
    /// Nothing for a route that can refuel at every stop it is known to
    /// pass, which is most routes through settled space: a line saying a
    /// route is fine is a line read every time to learn nothing.
    fn said(&self) -> Option<String> {
        let run = match self.run {
            0 => return None,
            1 => "1 stop".to_owned(),
            run => format!("{run} stops in a row"),
        };
        let where_from = match &self.from {
            Some(from) => format!(", from {from}"),
            None => String::new(),
        };
        // How far it is to cross, which is the figure a reader can turn
        // into their own drive's fuel. See [`fuel_rule`].
        let across = match self.across {
            Some(across) => format!(", {across:.1} Ly to cross"),
            None => String::new(),
        };
        let unread = match self.unread {
            0 => String::new(),
            1 => ", 1 stop unread".to_owned(),
            unread => format!(", {unread} stops unread"),
        };
        Some(format!("nothing to scoop at {run}{where_from}{across}{unread}"))
    }
}

/// How the plan behind a route was worked out, where there was one
///
/// The other half of what a route was asked for. The line above says the
/// range, the drive, the optimality and what the search cost; these are
/// the settings that decide how the coarse plan over the boost stars went
/// about it, and they are part of a route's own identity
/// ([`Filter::Route`]) precisely because they change the answer — so the
/// panel says what *this* route was planned with rather than what the
/// settings happen to hold now.
///
/// [`None`] for anything that was not planned: a proven route is searched
/// system by system, and an unaided one has no boost stars to plan over.
/// See [`Filter::tune`].
///
/// **The gap width is not said, because it is not one number.** The plan
/// climbs it — a chain that does not close on the goal is tried again a
/// jump wider ([`super::route::highway::Highway::plan`]) — so
/// [`Tuning::reach`] is where the climb started rather than what the plan
/// used, and a line that quoted it would be describing a rung the answer
/// may not have come from.
///
/// The plan's own percent is what was *asked* for, on the same footing:
/// an exact plan that spends its allowance is worked leaned instead and
/// this line cannot tell the two apart. The allowance is a method
/// safeguard rather than part of the answer's description, and what it is
/// worth is measured on [`Tuning::allowance`].
fn planned_with(filter: &Filter) -> Option<String> {
    let tune = filter.tune()?;
    let crossing = match tune.crossing {
        Crossing::Stepped => "stepped",
        Crossing::Searched => "searched",
    };
    let plan = match tune.planning {
        0 => "an exact plan".to_owned(),
        over => format!("a plan leaned {over}%"),
    };
    Some(format!("{plan}, gaps {crossing}"))
}

/// How long the search that answered a filter took, for its panel
///
/// A trip is not a row of its own — it is the route its legs come to, joined
/// by [`crate::ui::as_one`] — so its time is its legs', and the longest of
/// them rather than the sum: the legs are searched at once, so the wait is
/// the slowest of them.
fn took(
    filter: &Filter,
    legs: &[Filter],
    filters: &Filters,
) -> Option<Duration> {
    if let Some(took) = filters.took_of(filter) {
        return Some(took);
    }
    legs.iter().filter_map(|leg| filters.took_of(leg)).max()
}

/// The systems a filter admits, and the one the user picks out of them
///
/// Every system the database has for the filter, not only the ones the map
/// has fetched, since where a faction is, is most of what is being asked.
///
/// Answered the way the map itself is: one click says which system is meant
/// and a second says to go there, so a system reached through a list and a
/// system reached by its star are reached the same way.
///
/// Each line ends in a distance, and which distance it is follows from what
/// the list is: the jump that reaches the system where the filter is flown,
/// and how far off it is from the camera where it is not.
#[allow(clippy::too_many_arguments)]
fn admitted(
    ui: &mut Ui,
    filter: &Filter,
    legs: &[Filter],
    systems: Option<&[System]>,
    took: Option<Duration>,
    // How many of a trip's legs are still being searched; see `summary`.
    plotting: usize,
    // Which stops can supercharge, for the class each line says.
    boosts: &crate::Boosts,
    // And what the arrival star of each listed system is, where it has been
    // looked up; see [`StarClasses`].
    classes: &StarClasses,
    center: Option<DVec3>,
    picked: &mut Option<(System, bool)>,
    picked_stops: &mut Option<(Vec<System>, bool)>,
    described: &mut Option<System>,
    moved: &mut Option<MoveCamera>,
    worked: &mut Option<Filter>,
) {
    let Some(systems) = systems else {
        ui.label(egui::RichText::new("Looking...").weak());
        return;
    };

    if systems.is_empty() {
        ui.label(egui::RichText::new("No systems on record").weak());
        return;
    }

    ui.label(
        egui::RichText::new(summary(filter, systems.len(), took, plotting))
            .weak(),
    );
    // And how it was planned, on its own line: the first says what the
    // route had to be, this says how the plan went about finding it.
    if let Some(planned) = planned_with(filter) {
        ui.label(egui::RichText::new(planned).weak());
    }

    // What each line has to say about where its system is, which is not the
    // same question in the two kinds of list.
    //
    // A route is flown, so what is worth knowing about a system on one is the
    // jump that reaches it: how far it is from the system before, which is
    // what a ship has to be able to make. Measured off the list, which for a
    // route is already in the order it is travelled. The first system is
    // where the flying starts and no jump reaches it, so its line says
    // nothing.
    //
    // Everywhere else the systems are a set, in no order but the one this
    // list puts them in, and how far off they are from where the camera is
    // looking is both what orders them and what says why.
    let mut order: Vec<(&System, Option<f64>)> = if filter.ordered() {
        let mut legs = Vec::with_capacity(systems.len());
        let mut left = None;
        for system in systems {
            let at = DVec3::from(system.position);
            legs.push((system, left.map(|from: DVec3| from.distance(at))));
            left = Some(at);
        }
        legs
    } else {
        systems
            .iter()
            .map(|system| {
                (
                    system,
                    center.map(|at| at.distance(DVec3::from(system.position))),
                )
            })
            .collect()
    };

    // A filter with an order of its own is left in it. A route is travelled
    // from one end to the other, and a list of its systems put in any other
    // order is no longer a route, whatever it is sorted by.
    //
    // Where there is no such order, nearest first, from where the camera is
    // looking, which is the distance the whole map is measured in. Ordered
    // afresh each frame rather than once when the list arrived, so it goes on
    // answering which of these is near me as the user flies. That does mean
    // it reorders while the camera is moving; it holds still the moment it
    // stops, and the camera only moves when it is asked to.
    //
    // A stable sort, so that with no camera to measure from the order the
    // list arrived in is what is left.
    if !filter.ordered() {
        order.sort_by(|(_, one), (_, other)| match (one, other) {
            (Some(one), Some(other)) => one.total_cmp(other),
            _ => std::cmp::Ordering::Equal,
        });
    }

    // What flying it comes to, for a route. Under the summary, which says
    // what the ship was plotted at, this says what the plot asks of it: how
    // far it is all told, and the longest single jump, which is the one
    // deciding whether the ship as it stands can make the trip at all.
    //
    // Distances, and no fuel figure. Fuel is what a reader actually wants
    // to know and the map is not told the ship it would take to say it — so
    // the rule for working it out is on hover instead, exact for whatever
    // drive is fitted, where a figure of the map's own would have been a
    // third out for most of them. See [`fuel_rule`].
    if filter.ordered()
        && let Some((total, longest)) =
            flying(order.iter().filter_map(|(_, leg)| *leg))
    {
        ui.label(
            egui::RichText::new(format!(
                "{total:.1} Ly flown, longest jump {longest:.1} Ly"
            ))
            .weak(),
        )
        .on_hover_text(fuel_rule());
    }

    // And what it asks of the tank between refuellings. Under the distance,
    // because it is the other thing a plotted route can be impossible for: a
    // stretch with nothing to scoop is flown on the fuel the ship set out
    // with, and a stretch too far to cross on one tank strands it. Said only
    // where there is something to say. See [`Scooping`].
    if filter.ordered()
        && let Some(said) = Scooping::of(order.iter().map(|(system, leg)| {
            (system.name.as_str(), classes.of(system.address), *leg)
        }))
        .said()
    {
        ui.label(egui::RichText::new(said).weak()).on_hover_text(fuel_rule());
    }

    ui.add_space(MARGIN);

    // The list as text, for wherever it is wanted next. A route is flown in
    // the game with a hand on the keyboard, and a faction's holdings are read
    // against a spreadsheet, and neither is done off a window that cannot be
    // selected from. In the order the list is drawn in, and saying what each
    // line says, so what is copied is what is read.
    ui.horizontal(|ui| {
        if ui
            .button(if filter.ordered() { "Copy Route" } else { "Copy List" })
            .clicked()
        {
            ui.ctx().copy_text(as_text(&order, legs));
        }

        // Where the camera has to stand to see the whole of what the panel
        // lists, off the systems it is listing. Said as a button because the
        // gesture that asks it of a row cannot be asked of a window: a double
        // click on a panel's title rolls it up into its title bar, egui's
        // own reading of it, so a panel framed that way would fold shut on
        // the way.
        let framing =
            if filter.ordered() { "Frame Route" } else { "Frame List" };
        if ui.button(framing).clicked() {
            let places: Vec<DVec3> = order
                .iter()
                .map(|(system, _)| DVec3::from(system.position))
                .collect();
            if let Some((middle, extent)) =
                crate::systems::route::spawn::framing(&places)
                && extent > 0.
            {
                *moved = Some(MoveCamera {
                    position: Some(middle),
                    framing: Some(extent),
                });
            }
        }
    });
    ui.add_space(MARGIN);

    // The list is not scrolled here. The panel it stands in is one scroll
    // area of its own (see [`panels`]), and a list scrolled inside a scrolled
    // panel is two bars to reach one row with. It was capped at eight lines
    // for the tiling's sake — a panel that ran the height of the viewport
    // left nowhere to put the next one — and the panel scrolling is what
    // answers that instead, so a faction's whole holdings are listed and the
    // one bar carries them.
    {
        // One row, wherever in the list it stands. `at` keys it by place
        // rather than by which system stands there: the list is put in order
        // afresh every frame, so a row holds its rectangle while the system
        // in it changes as the camera moves, which is the one thing egui
        // reads as a widget taking another's state.
        // Answers rather than acts, so that what a row asked for is written
        // where the row is drawn. Acting here would hold the borrow of it for
        // as long as the list, and the name over each leg has answers of its
        // own to write.
        // What stands after one stop's name: how far the jump onto it was,
        // where it is what sorts the list — a list in an order nobody can
        // see reads as an order nobody chose — and what kind of star waits
        // there.
        //
        // The real class where the index has been asked and answered, and
        // the one thing published for every system until then: a stop that
        // can supercharge, which is why the route came this way. Nothing at
        // all for a system nothing has scanned and no cone on, rather than a
        // guess at a spectrum.
        let reading = |system: &System, away: Option<f64>| {
            let class =
                classes.of(system.address).map(str::to_owned).or_else(|| {
                    boosts
                        .get(system.address)
                        .map(|boost| boost.named().to_owned())
                });
            match (class, away) {
                (Some(class), Some(away)) => {
                    Some(format!("{class}, {away:.1} Ly"))
                }
                (Some(class), None) => Some(class),
                (None, away) => away.map(|away| format!("{away:.1} Ly")),
            }
        };

        // Where those readings go, settled for the whole route before a line
        // of it is drawn: a stop whose class and distance have to go under
        // its name in a list where the next stop's fit beside it reads as
        // two kinds of row. See [`crate::ui::Rows`]. Said once and kept, the
        // lines being drawn from the same strings that were measured.
        let said: Vec<Option<String>> =
            order.iter().map(|(system, away)| reading(system, *away)).collect();
        let rows = crate::ui::Rows::of(
            ui,
            // A trip's stops are drawn indented under their leg's name, so
            // they get that much less than the panel has. A route with no
            // legs is drawn flush and gets the whole of it.
            ui.available_width()
                - if legs.is_empty() { 0. } else { ui.spacing().indent },
            order.iter().zip(&said).map(|((system, _), reading)| {
                (system.name.as_str(), reading.as_deref())
            }),
        );

        // One row, and what a click on it asked for.
        let line_of = |ui: &mut Ui, at: usize, system: &System| {
            crate::ui::system_line(
                ui,
                &system.name,
                said[at].clone(),
                rows,
                ("admitted", at),
            )
        };

        // A trip is listed leg by leg. One run of forty systems says nothing
        // about which of them the user asked for, so each leg is named and
        // its stops drawn in under it -- the same reading the bar's rows
        // give, in the panel that is about the whole of it.
        //
        // The grouping is `by_leg`'s, which is also what the copying reads,
        // so what is drawn and what is taken away are the one list.
        let mut at = 0;
        for (leg, stops) in by_leg(&order, legs) {
            // A leg's name is the control for the leg, read the way its row
            // in the bar is read: a click says it is the one being worked
            // with, and a double says to see the whole of it. The stops under
            // it are systems and answer as the lines of any other list do, so
            // a trip's panel offers what the bar and the search list offer
            // rather than being the one place a route cannot be reached from.
            if let Some(leg) = leg {
                // Named, and how many jumps it is flown in beside the name:
                // the same two readings its row in the bar gives, said by
                // the same [`crate::ui::hops_said`] so a leg cannot read
                // one way in the bar and another in the panel about it.
                let heading = ui
                    .horizontal(|ui| {
                        let named = ui.add(
                            egui::Label::new(
                                egui::RichText::new(leg.name()).strong(),
                            )
                            .selectable(false)
                            .sense(egui::Sense::click()),
                        );
                        if let Some(hops) = leg.hops() {
                            ui.label(
                                egui::RichText::new(crate::ui::hops_said(hops))
                                    .weak(),
                            );
                        }
                        named
                    })
                    .inner;
                let settled = crate::ui::settled_click(
                    ui,
                    heading.id,
                    heading.clicked(),
                    heading.double_clicked(),
                );
                match crate::ui::asked_of_row(
                    false,
                    false,
                    false,
                    heading.double_clicked(),
                    settled.is_some(),
                ) {
                    // Every stop the leg runs through, taken off the whole
                    // list rather than off the rows drawn under the name.
                    // The two differ by one: a leg sets out from the system
                    // the leg before it landed on, which is drawn up there
                    // and is still where this one starts. Framed without it
                    // the camera stands over the leg's tail, and the bar's
                    // own row for the same leg would frame it differently.
                    Some(crate::ui::RowGesture::Frame) => {
                        let places: Vec<DVec3> = order
                            .iter()
                            .filter(|(system, _)| {
                                leg.place_of(system.address).is_some()
                            })
                            .map(|(system, _)| DVec3::from(system.position))
                            .collect();
                        if let Some((middle, extent)) =
                            crate::systems::route::spawn::framing(&places)
                            && extent > 0.
                        {
                            *moved = Some(MoveCamera {
                                position: Some(middle),
                                framing: Some(extent),
                            });
                        }
                    }
                    // The same two things a click on the leg's row in the
                    // bar means, and for the same reason: the leg is what is
                    // being worked with, and what it was plotted between is
                    // what the user is holding when they reach for its name.
                    // Both answers are handed back rather than acted on here,
                    // and the selection is picked out through
                    // [`crate::systems::selection::Selection::pick_out`], so
                    // a leg reached through the bar and the same leg reached
                    // through this panel cannot come to two different things.
                    Some(crate::ui::RowGesture::Select) => {
                        let ends = leg.stops();
                        *picked_stops = Some((
                            order
                                .iter()
                                .filter(|(system, _)| {
                                    ends.contains(&system.address)
                                })
                                .map(|(system, _)| (*system).clone())
                                .collect(),
                            crate::ui::gathering_with(
                                settled.unwrap_or_default(),
                            ),
                        ));
                        *worked = Some(leg.clone());
                    }
                    _ => {}
                }
                heading.on_hover_cursor(egui::CursorIcon::PointingHand);
            }
            let mut rows = |ui: &mut Ui| {
                for (step, (system, _)) in stops.iter().enumerate() {
                    match line_of(ui, at + step, system) {
                        Some(SystemAction::Select { gathering }) => {
                            *picked = Some(((*system).clone(), gathering))
                        }
                        Some(SystemAction::Travel) => {
                            *moved = Some(MoveCamera {
                                position: Some(DVec3::from(system.position)),
                                framing: None,
                            })
                        }
                        Some(SystemAction::Describe) => {
                            *described = Some((*system).clone())
                        }
                        None => {}
                    }
                }
            };
            match leg {
                Some(leg) => {
                    ui.indent(("leg", leg.name()), |ui| rows(ui));
                }
                None => rows(ui),
            }
            at += stops.len();
        }
    }
}

/// Every faction present in the system, one to a line
///
/// A list under a heading rather than rows in the grid above, because a
/// system holds as many factions as it holds and their names run long. Set
/// against the grid's two columns each would wrap into a paragraph, and the
/// column widths the rest of the panel is laid out in would be decided by
/// whichever faction happened to have the longest name.
///
/// Sorted by name, so that the order holds still. What comes back from the
/// database is in no order at all, and a fetch that replaced the row would
/// otherwise shuffle the list under whoever was reading it.
///
/// A faction whose name has not arrived yet is still one of the factions
/// here, so it keeps its line. Naming them is one query behind the panel
/// opening, and a list that grew a line a frame later would jump.
///
/// Clicking one asks the map for it: the faction becomes a filter, and
/// everything it is absent from goes dim. A system's panel is where the user
/// finds out who is here, and where else they are is the next question, so it
/// is asked from the answer rather than typed out again in the bar.
///
/// A faction still waiting on its name does not answer. What it is called is
/// half of a filter, being what its row in the bar says it is, and a row
/// naming a faction as punctuation would say nothing about what had gone dim.
fn factions(
    ui: &mut Ui,
    present: &[i32],
    names: &FactionNames,
    wanted: &mut Option<Filter>,
) {
    if present.is_empty() {
        return;
    }

    ui.add_space(MARGIN);
    ui.label(egui::RichText::new("Factions").strong());
    for (id, name) in listed(present, names) {
        let text = match name {
            Some(name) => egui::RichText::new(name),
            None => egui::RichText::new(UNNAMED).weak(),
        };
        let (_, answer) = crate::ui::line(ui, text, 0., name.is_some());

        if let Some(name) = name
            && answer.clicked()
        {
            *wanted = Some(Filter::Faction { id, name: name.to_owned() });
        }
    }
}

/// The factions of a system as their lines read, in order
///
/// Named first and among themselves by name, with whatever is still unnamed
/// held at the end. Sorting the placeholder in with the names would put it
/// above all of them, since it is punctuation, and the line would then jump
/// the length of the list the moment its name arrived.
///
/// The id rides along with the name because a line is a control: clicking one
/// asks for a filter, and a filter tests a system against the id.
fn listed<'a>(
    present: &[i32],
    names: &'a FactionNames,
) -> Vec<(i32, Option<&'a str>)> {
    let mut listed: Vec<(i32, Option<&str>)> =
        present.iter().map(|id| (*id, names.get(*id))).collect();
    listed.sort_unstable_by_key(|(id, name)| (name.is_none(), *name, *id));
    listed
}

/// A faction the map has yet to hear the name of
const UNNAMED: &str = "...";

/// One named thing the database knows about a system
///
/// The name is written as brightly as a header, and the answer beside it in
/// the ordinary text of the panel. A panel is read down its left hand column
/// until the line wanted is found, and it is the names that column is made
/// of.
fn field(ui: &mut Ui, name: &str, value: String) {
    ui.label(egui::RichText::new(name).strong());
    ui.label(value);
    ui.end_row();
}

/// One named thing worth taking away, copied by clicking on it
///
/// A position is typed into other tools to the last decimal, and one read off
/// a panel and retyped is a digit out somewhere. The value is the control
/// rather than a button beside it, so a panel of fields reads as it did and
/// the one row that answers a click says so when the pointer rests on it.
fn copied(ui: &mut Ui, name: &str, value: String) {
    ui.label(egui::RichText::new(name).strong());
    let shown = ui
        .add(egui::Label::new(value.clone()).sense(egui::Sense::click()))
        .on_hover_text("Click to copy")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    if shown.clicked() {
        ui.ctx().copy_text(value);
    }
    ui.end_row();
}

/// What a route comes to, flown: how far all told, and the longest jump in it
///
/// Nothing for a route with no leg to fly, there being nothing to add up and
/// no jump to be the longest. One pass, since both answers come off the same
/// legs and a route is walked to draw it anyway.
fn flying(legs: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    legs.fold(None, |so_far, leg| match so_far {
        None => Some((leg, leg)),
        Some((total, longest)) => Some((total + leg, longest.max(leg))),
    })
}

/// How a trip's list breaks into its legs
///
/// Each leg's name and the stops that fall under it, in the order flown. One
/// group named for nothing where there are no legs, which is every filter but
/// a trip: a list that is not a trip's is one list.
///
/// The stop a leg lands on is the stop the next sets out from and stands in
/// the joined list once, so every leg after the first begins on the one
/// before's last.
///
/// Here rather than in the drawing because the copying wants it too, and a
/// list drawn in one grouping and copied in another would be two answers to
/// the one question.
fn by_leg<'a, 'l>(
    order: &'a [(&'a System, Option<f64>)],
    legs: &'l [Filter],
) -> Vec<(Option<&'l Filter>, &'a [(&'a System, Option<f64>)])> {
    if legs.is_empty() {
        return vec![(None, order)];
    }

    let mut groups = Vec::with_capacity(legs.len());
    let mut at = 0;
    for leg in legs {
        let Filter::Route { systems: hops, .. } = leg else { continue };
        let takes = hops.len().saturating_sub(usize::from(at > 0));
        let Some(stops) = order.get(at..at + takes) else { break };
        groups.push((Some(leg), stops));
        at += takes;
    }
    groups
}

/// A list of systems as text, one to a line, as the panel draws them
///
/// Each line is the name and, where the list gives one, the distance the
/// panel's line ends in: the jump that reaches a system on a route, and how
/// far off it is from the camera otherwise. Set apart by a tab, so what is
/// pasted into a spreadsheet lands in two columns and what is pasted anywhere
/// else still reads as one line about one system.
fn as_text(order: &[(&System, Option<f64>)], legs: &[Filter]) -> String {
    let mut said: Vec<String> = Vec::new();
    for (named, stops) in by_leg(order, legs) {
        // A trip's legs are named and their stops drawn in under them, so the
        // text says the same. Two spaces rather than a tab, the tab already
        // standing between a name and the distance after it.
        let indent = if named.is_some() { "  " } else { "" };
        if let Some(leg) = named {
            said.push(leg.name().to_owned());
        }
        for (system, away) in stops {
            said.push(match away {
                Some(away) => {
                    format!("{indent}{}\t{away:.2} Ly", system.name)
                }
                None => format!("{indent}{}", system.name),
            });
        }
    }

    said.join("\n")
}

/// How far a field written under a header sits in from the ones above it
const NEST: f32 = 12.;

/// One named thing, written under the header it belongs to
fn under(ui: &mut Ui, name: &str, value: String) {
    ui.horizontal(|ui| {
        ui.add_space(NEST);
        ui.label(egui::RichText::new(name).strong());
    });
    ui.label(value);
    ui.end_row();
}

/// What a system trades in
///
/// Under a header, because the halves of an economy are named for their
/// standing against each other: "Secondary" among the flat fields says what
/// it is but not what it is secondary to.
///
/// A system with nothing on record is one line saying so. There is no primary
/// to head the pair with, and two nested lines both reading `UNKNOWN` say the
/// same thing at three times the length.
fn economies(ui: &mut Ui, economies: &Option<Economies>) {
    let Some(economies) = economies else {
        field(ui, "Economy", UNKNOWN.into());
        return;
    };

    ui.label(egui::RichText::new("Economy").strong());
    ui.end_row();
    under(ui, "Primary", economies.primary.to_string());
    under(ui, "Secondary", named(&economies.secondary));
}

/// What the database has yet to say about a system
const UNKNOWN: &str = "Unknown";

/// What the database says, or that it says nothing
///
/// Most of what is recorded about a system is optional, and a blank row
/// reads as a bug rather than as an answer.
fn named<T: Display>(value: &Option<T>) -> String {
    match value {
        Some(value) => value.to_string(),
        None => UNKNOWN.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::route::graph::{Crossing, Drive, Routing, Tuning};
    use crate::systems::tests::{system, tallied};
    use crate::tests::{context, painted, words};
    use chrono::DateTime;
    use elite_journal::Allegiance;
    use elite_journal::body::{
        AtmosphereType, Orbit as JournalOrbit, Spin as JournalSpin,
    };
    use elite_journal::system::Economy;

    /// A registry naming each of `known`
    fn known(known: &[(i32, &str)]) -> FactionNames {
        FactionNames(
            known.iter().map(|(id, name)| (*id, name.to_string())).collect(),
        )
    }

    /// The color each piece of text `contents` painted came out in
    ///
    /// Read back off the shapes for the reason [`crate::tests::words`] is:
    /// a panel is read by what its lines say and how brightly they say it,
    /// and the widget that wrote a line is gone by the time it is painted.
    ///
    /// A color asked for when the text was laid out is the one it keeps.
    /// Text laid out without one carries a placeholder instead, and what
    /// answers that is whatever the shape was painted with.
    fn written(
        mut contents: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Color32)> {
        let ctx = context();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

        fn text_of(
            shape: &egui::Shape,
            into: &mut Vec<(String, egui::Color32)>,
        ) {
            match shape {
                egui::Shape::Text(text) => {
                    let asked = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                        .filter(|color| *color != egui::Color32::PLACEHOLDER);
                    let color = text
                        .override_text_color
                        .or(asked)
                        .unwrap_or(text.fallback_color);
                    into.push((text.galley.text().into(), color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut painted = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut painted);
        }
        painted
    }

    /// What the economy of a system reads as, line by line
    ///
    /// In a grid, since that is where a panel writes its fields and what
    /// [`under`] indents against.
    fn economy(economies: Option<Economies>) -> Vec<String> {
        words(|ui| {
            egui::Grid::new("economies").num_columns(2).show(ui, |ui| {
                super::economies(ui, &economies);
            });
        })
    }

    /// What a system's own panel reads as, line by line
    fn panel(system: &System) -> Vec<String> {
        words(|ui| {
            described(ui, system, &known(&[]), None, &mut None, &mut None);
        })
    }

    /// What stands in the answer column beside `name`
    fn beside(painted: &[String], name: &str) -> String {
        let at = painted
            .iter()
            .position(|word| word == name)
            .unwrap_or_else(|| panic!("{name} was painted"));
        painted[at + 1].clone()
    }

    /// A tallied system says how much of itself there is to find
    #[test]
    fn a_tallied_system_says_what_it_holds() {
        let painted = panel(&tallied(1, Some(12), Some(3)));

        assert_eq!(beside(&painted, "Bodies"), "12");
        assert_eq!(beside(&painted, "Belts and rings"), "3");
    }

    /// A system tallied for its bodies alone still answers for its belts
    ///
    /// Only the honk counts the belts and rings, so a system counted by the
    /// all-found tally has the one and not the other. Dropping the row would
    /// read as a system with no belts rather than one nobody has honked.
    #[test]
    fn a_system_tallied_for_its_bodies_alone_says_so() {
        let painted = panel(&tallied(1, Some(12), None));

        assert_eq!(beside(&painted, "Bodies"), "12");
        assert_eq!(beside(&painted, "Belts and rings"), UNKNOWN);
    }

    /// Both halves are written out, under a header naming what they are
    #[test]
    fn an_economy_reads_as_its_two_halves() {
        let economies = Some(Economies {
            primary: Economy::Agriculture,
            secondary: Some(Economy::Extraction),
        });

        assert_eq!(
            economy(economies),
            vec![
                "Economy",
                "Primary",
                "Agriculture",
                "Secondary",
                "Extraction"
            ]
        );
    }

    /// A system with only a primary is still asked about its secondary
    ///
    /// The header promises two lines, and dropping the one that is unknown
    /// would read as a system whose secondary is something the panel is not
    /// saying.
    #[test]
    fn an_economy_missing_its_secondary_says_so() {
        let economies =
            Some(Economies { primary: Economy::Agriculture, secondary: None });

        assert_eq!(
            economy(economies),
            vec!["Economy", "Primary", "Agriculture", "Secondary", UNKNOWN]
        );
    }

    /// A system with nothing on record says it in one line
    #[test]
    fn a_system_with_no_economy_says_so_once() {
        assert_eq!(economy(None), vec!["Economy", UNKNOWN]);
    }

    /// Founders World, as the database holds it
    fn founders() -> DbBody {
        DbBody {
            system_address: 1,
            id: 14,
            parents: vec![],
            name: "Founders World".to_owned(),
            body_type: None,
            distance_from_arrival: Some(345.5563),
            updated_at: DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            planet_class: "Earthlike body".to_owned(),
            tidal_lock: false,
            mass: 0.69,
            radius: 5.485766e6,
            gravity: 9.13869,
            temperature: Some(298.70755),
            surface: None,
            orbit: JournalOrbit {
                semi_major_axis: 1.0023064e11,
                eccentricity: 0.0026,
                orbital_inclination: 1.5,
                periapsis: 0.,
                orbital_period: 8.6e7,
                ascending_node: Some(0.),
                mean_anomaly: Some(0.),
            },
            spin: JournalSpin { period: 3.6254802e6, tilt: 0.373026 },
            discovered_at: None,
            mapped: true,
        }
    }

    /// What a body's panel says
    fn body_said() -> Vec<String> {
        words(|ui| {
            body_described(
                ui,
                &founders(),
                &mut crate::systems::bodies::Clock::default(),
                None,
            )
        })
    }

    /// A place the map made up is owned as one, and says what was missing
    ///
    /// The body's own path is a reading and where it puts the body is not:
    /// what it is measured from was never scanned, so the map stood that up
    /// at the distance its riders report in a direction nobody measured. Not
    /// only ever a barycentre — an unscanned star or body is stood up the
    /// same way — so the word is part of the answer.
    #[test]
    fn a_guessed_place_says_what_was_never_scanned() {
        let said = |kind| {
            words(|ui| {
                body_described(
                    ui,
                    &founders(),
                    &mut crate::systems::bodies::Clock::default(),
                    kind,
                )
            })
        };

        let guessed = said(Some("star"));
        assert!(
            guessed.contains(&"Guessed (star unscanned)".to_owned()),
            "{guessed:?}"
        );
        assert!(
            said(Some("barycentre"))
                .contains(&"Guessed (barycentre unscanned)".to_owned())
        );
        // And nothing at all where nothing was made up, which is the
        // ordinary case: a row that says where it is says it.
        assert!(
            !said(None).iter().any(|word| word.starts_with("Guessed")),
            "a place nobody guessed at was called a guess"
        );
    }

    /// A phase slider nobody has touched leaves the clock exactly where it is
    ///
    /// Reported: holding the slider under the date left the reading flicking
    /// between two moments, and a while later the map crashed dating one --
    /// `DateTime + TimeDelta` overflowed. Egui clamps and rounds the value a
    /// slider is handed before it draws it, and reports a slider nobody
    /// touched as changed when the rounding moved it. So a rail that rounded
    /// to a tenth of a percent wrote that tenth back into the clock every
    /// frame the phase was anything else: it fought the slider that was
    /// actually being dragged, and where the phase rounded up to a whole
    /// 100% it took the map on by one of this body's turns per frame.
    #[test]
    fn an_untouched_phase_slider_leaves_the_clock_alone() {
        let ctx = context();
        let period = 400. * DAY;
        let mut clock = crate::systems::bodies::Clock::default();
        // A phase that is no round tenth of a percent of its own turn.
        clock.offset_to(period, 0.374_838_71);
        let was = clock.offset();

        for _ in 0..4 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::Grid::new("phase").show(ui, |ui| {
                    turned(ui, period, &mut clock);
                });
            });
        }

        assert_eq!(clock.offset(), was, "an untouched slider moved the clock");
    }

    /// One drag of the phase slider, from `from` to `to` along its own rail
    ///
    /// Where the rail stands is read off what the pass painted: the slider's
    /// rail is the widest thin rectangle in it. Four passes to a drag — the
    /// press, the move, the release, and an idle one, which is what a frame
    /// nobody touches is — since egui knows where a widget stands only once
    /// it has been drawn and reports a gesture on the pass it arrives.
    fn dragged(
        period: f64,
        clock: &mut crate::systems::bodies::Clock,
        drags: &[(f32, f32)],
    ) {
        let ctx = context();

        let pass =
            |input: egui::RawInput,
             clock: &mut crate::systems::bodies::Clock| {
                let mut widest = egui::Rect::NOTHING;
                let output = ctx.run_ui(input, |ui| {
                    ui.set_width(300.);
                    egui::Grid::new("phase").show(ui, |ui| {
                        turned(ui, period, clock);
                    });
                });
                for shape in &output.shapes {
                    if let egui::Shape::Rect(rect) = &shape.shape
                        && rect.rect.width() > widest.width()
                        && rect.rect.height() < 12.
                    {
                        widest = rect.rect;
                    }
                }
                widest
            };

        // Twice: an area is drawn from what it last came to, and the first
        // pass paints nothing to measure.
        pass(egui::RawInput::default(), clock);
        let rail = pass(egui::RawInput::default(), clock);

        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let at = |along: f32| {
            egui::pos2(rail.left() + rail.width() * along, rail.center().y)
        };
        let moved = |to: f32, events: Vec<egui::Event>| egui::RawInput {
            events: [vec![egui::Event::PointerMoved(at(to))], events].concat(),
            ..Default::default()
        };

        for (from, to) in drags {
            pass(moved(*from, vec![button(at(*from), true)]), clock);
            pass(moved(*to, Vec::new()), clock);
            pass(moved(*to, vec![button(at(*to), false)]), clock);
            pass(egui::RawInput::default(), clock);
        }
    }

    /// A phase dragged to the far end stays there, and drags back from it
    ///
    /// Reported as the slider being broken, and it was, in two ways that were
    /// the one fault: the handle jumped to the near end the moment a drag to
    /// the far end was let go of, and the drag after that ran the map forward
    /// when it was pulled back.
    ///
    /// The far end is a whole turn past where the body stood, which is the
    /// same place on its orbit and the beginning of the next turn. Folded
    /// back out of the offset, that reads as no phase at all -- so the
    /// handle was drawn at the near end while the map stood a turn on, and
    /// the next drag measured its fraction in the turn after the one the
    /// handle looked to be in.
    #[test]
    fn a_phase_dragged_to_its_far_end_stays_there_and_comes_back() {
        let period = 400. * DAY;
        let mut clock = crate::systems::bodies::Clock::default();

        dragged(period, &mut clock, &[(0., 1.)]);

        assert_eq!(
            clock.offset(),
            period,
            "a drag end to end did not run the map on by one turn"
        );
        assert_eq!(
            clock.through(period),
            1.,
            "the handle went back to the near end when it was let go of"
        );

        // And back: the handle stands at the far end, so a drag from there to
        // the middle is half a turn back rather than half a turn on.
        dragged(period, &mut clock, &[(1., 0.5)]);

        assert!(
            (clock.offset() - period / 2.).abs() < period / 100.,
            "dragging back from the far end left the map at {:.3} turns",
            clock.offset() / period
        );
    }

    /// A body's panel reads in the units a body is talked about in
    ///
    /// Which are not the ones it is stored in. The database holds metres and
    /// metres per second squared because that is what a scan records and what
    /// the map draws with, and nobody asks how many metres across a world is.
    #[test]
    fn a_body_is_described_in_the_units_it_is_read_in() {
        let said = body_said();

        assert!(said.contains(&"Earthlike body".to_owned()), "{said:?}");
        assert!(said.contains(&"345.6 Ls".to_owned()), "{said:?}");
        assert!(said.contains(&"5,485 km".to_owned()), "{said:?}");
        assert!(said.contains(&"0.690 Earths".to_owned()), "{said:?}");
        assert!(said.contains(&"0.93 g".to_owned()), "{said:?}");
        assert!(said.contains(&"299 K".to_owned()), "{said:?}");
    }

    /// A gas giant has no surface, and says so in one line
    ///
    /// Rather than a header over five rows of nothing, as a system with no
    /// economy on record does.
    #[test]
    fn a_body_with_no_surface_says_so_once() {
        let said = body_said();

        assert!(said.contains(&"Surface".to_owned()), "{said:?}");
        assert!(said.contains(&"None".to_owned()), "{said:?}");
        assert!(!said.contains(&"Landable".to_owned()), "{said:?}");
    }

    /// A material the scan found `percent` of
    fn material(name: &str, percent: f64) -> Material {
        Material { name: name.to_owned(), percent }
    }

    /// A crust is said by its parts, richest first, leaving out what is not
    /// there
    ///
    /// The scan reports three fractions summing to one, and a rocky body
    /// reports no ice at all. A line saying it is nought parts ice says
    /// nothing, and the order the scan writes them in is not the order a body
    /// is described in.
    #[test]
    fn a_crust_is_said_richest_first_without_what_is_not_there() {
        let rocky = Composition { ice: 0., rock: 0.911156, metal: 0.088844 };
        let icy = Composition { ice: 0.7, rock: 0.2, metal: 0.1 };

        assert_eq!(made_of(&rocky), "91% rock, 9% metal");
        assert_eq!(made_of(&icy), "70% ice, 20% rock, 10% metal");
    }

    /// Materials are listed richest first, named as names
    ///
    /// What is asked of a body is what it has most of, and the journal spells
    /// a material `iron` where a panel writes Iron.
    #[test]
    fn materials_are_listed_richest_first() {
        let found = [
            material("nickel", 15.2),
            material("iron", 20.1),
            material("carbon", 12.8),
        ];

        assert_eq!(
            richest(&found),
            vec![
                ("Iron".to_owned(), 20.1),
                ("Nickel".to_owned(), 15.2),
                ("Carbon".to_owned(), 12.8),
            ]
        );
    }

    /// A surface the scan listed no materials for has no Materials section
    ///
    /// A header over no rows is a header over nothing. The surface's own rows
    /// still draw, so this is the header being withheld rather than the
    /// section failing to reach the panel at all.
    #[test]
    fn a_surface_with_nothing_scanned_lists_no_materials() {
        let said = words(|ui| {
            egui::Grid::new("surface").num_columns(2).show(ui, |ui| {
                standing(ui, &Some(bare()));
            });
        });

        assert!(said.contains(&"Landable".to_owned()), "{said:?}");
        assert!(!said.contains(&"Materials".to_owned()), "{said:?}");
    }

    /// A surface a scan reached but found no materials on
    fn bare() -> Surface {
        Surface {
            atmosphere_type: AtmosphereType::None,
            pressure: 0.,
            composition: None,
            landable: true,
            atmosphere: None,
            volcanism: None,
            terraform_state: None,
            materials: vec![],
        }
    }

    /// A body's turn and its orbit are read in days
    ///
    /// A period is recorded in seconds, and eight digits of them is a number
    /// nobody reads.
    #[test]
    fn what_turns_slowly_is_read_in_days() {
        let said = body_said();

        assert!(said.contains(&"42.0 Earth days".to_owned()), "{said:?}");
        assert!(said.contains(&"21.4°".to_owned()), "{said:?}");
    }

    /// How far something reaches is read in whichever unit it fills
    #[test]
    fn a_reach_is_read_in_the_unit_it_fills() {
        assert_eq!(spanning(1.0023064e11), "334.33 Ls");
        assert_eq!(spanning(5.4946205e6), "5,494 km");
    }

    /// And how long it takes, likewise
    #[test]
    fn a_span_of_time_is_read_in_the_unit_it_fills() {
        assert_eq!(lasting(3.6254802e6), "42.0 Earth days");
        assert_eq!(lasting(3600.), "1.0 hours");
        // The short end, which only a span the map has been run on reaches.
        assert_eq!(lasting(225.), "3.8 minutes");
        assert_eq!(lasting(45.), "45 seconds");
        assert_eq!(lasting(0.), UNKNOWN);
        // The long end, which a system's outermost bodies live at. Six
        // thousand days is as unreadable as eight digits of seconds.
        assert_eq!(
            lasting((18.5 * 365.25 * DAY as f64) as f32),
            "18.5 Earth years",
        );
    }

    /// The names down the left of a panel are written as brightly as a header
    ///
    /// Nested or flat, a name is what the column is read down, and one in the
    /// same color as the answers beside it leaves the two to be told apart
    /// by which side of the panel they fell on.
    #[test]
    fn the_names_of_fields_read_as_brightly_as_a_header() {
        let painted = written(|ui| {
            egui::Grid::new("fields").num_columns(2).show(ui, |ui| {
                field(ui, "Security", "Low".into());
                economies(
                    ui,
                    &Some(Economies {
                        primary: Economy::Military,
                        secondary: None,
                    }),
                );
            });
        });

        let color = |wanted: &str| {
            painted
                .iter()
                .find(|(text, _)| text == wanted)
                .unwrap_or_else(|| panic!("{wanted} was painted"))
                .1
        };

        assert_eq!(color("Security"), color("Economy"));
        assert_eq!(color("Primary"), color("Economy"));
        assert_ne!(color("Low"), color("Economy"));
        assert_ne!(color("Military"), color("Economy"));
    }

    /// A trip flown home is listed in the order it is flown
    ///
    /// A route answers in its own order and a panel leaves it there. Sorted
    /// by where each system falls in the route, the stop a loop sets out
    /// from and comes back to — one system standing in the route twice —
    /// had its second standing sorted up beside its first, which lost the
    /// leg home from the list and from what the panel says the flying comes
    /// to.
    #[test]
    fn a_trip_flown_home_is_listed_in_the_order_flown() {
        let placed = |address: i64, at: f32| galos_index::meta::NameEntry {
            address,
            name: format!("STOP {address}").into(),
            position: [at, 0., 0.],
        };
        let names = Names::reaching(
            vec![placed(1, 0.), placed(2, 10.), placed(3, 20.)],
            Vec::new(),
        );
        // Out through the three and home again, which is the same system at
        // both ends.
        let ring = Filter::Route {
            label: "STOP 1 -> STOP 3 -> STOP 1".to_owned(),
            systems: vec![1, 2, 3, 1],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };

        let listed: Vec<i64> = fetch(&Populated::default(), &names, &ring)
            .iter()
            .map(|system| system.address)
            .collect();

        assert_eq!(listed, vec![1, 2, 3, 1]);
    }

    /// A system with no factions lists none
    #[test]
    fn a_system_with_no_factions_lists_nothing() {
        assert!(listed(&[], &known(&[])).is_empty());
    }

    /// The list reads in name order, whatever order the ids came in
    ///
    /// What the database returns is in no order at all, and a fetch that
    /// replaced the row would otherwise shuffle the list under whoever was
    /// reading it.
    #[test]
    fn factions_are_listed_by_name() {
        let names = known(&[(1, "Zargon Front"), (2, "Alliance of Sol")]);
        let wanted =
            vec![(2, Some("Alliance of Sol")), (1, Some("Zargon Front"))];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }

    /// A line comes out in a color something can draw
    ///
    /// A line is laid out from whatever it is handed, which is usually plain
    /// text carrying no color of its own. A placeholder answered by a
    /// placeholder reaches the tessellator, which panics, and takes every
    /// panel holding a list with it: the factions of a system, and the systems
    /// of a filter.
    #[test]
    fn a_line_paints_in_a_color() {
        painted(|ui| {
            crate::ui::line(
                ui,
                egui::RichText::new("Alliance of Sol"),
                0.,
                true,
            );
        });
    }

    /// So does one with room kept at its end
    #[test]
    fn a_line_with_room_reserved_paints_in_a_color() {
        painted(|ui| {
            crate::ui::line(ui, egui::RichText::new("Sol"), 20., true);
        });
    }

    /// And so does a whole faction list, marks and placeholders included
    #[test]
    fn a_faction_list_paints_in_a_color() {
        let names = known(&[(1, "Alliance of Sol")]);
        painted(|ui| {
            // One named and one still waiting, so both arms are drawn.
            factions(ui, &[1, 2], &names, &mut None);
        });
    }

    /// And a filter's list of systems, which carries a distance and a mark
    #[test]
    fn a_system_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        painted(|ui| {
            admitted(
                ui,
                &faction(7),
                &[],
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// So does one a route lists in the order it is travelled
    #[test]
    fn a_route_list_paints_in_a_color() {
        let systems = [system(1), system(2)];
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        painted(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
    }

    /// The rectangle a panel titled `title` came out in, opened at `at`
    ///
    /// The window as the panels draw it, `contents` being whatever the test
    /// wants to ask of the room inside it.
    fn shown(
        title: &str,
        at: egui::Pos2,
        contents: impl FnMut(&mut Ui),
    ) -> egui::Rect {
        let mut contents = contents;
        let ctx = crate::tests::context();
        let mut rect = egui::Rect::ZERO;

        // A screen to stand on. A panel is kept inside the viewport now, so
        // one opened on a context with no screen at all is pushed to wherever
        // egui's default rect leaves room for it.
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600., 600.),
            )),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let mut showing = true;
            let id = egui::Id::new("test-panel");
            let panel = framed(
                ui.ctx(),
                title,
                id,
                at,
                room_under(ui.ctx(), id, at, 0.),
                false,
                &mut showing,
            );
            if let Some(panel) = panel.show(ui.ctx(), &mut contents) {
                rect = panel.response.rect;
            }
        });

        rect
    }

    /// How tall a panel of `rows` lines comes out, on each screen in turn
    ///
    /// Drawn where the tiling opens one, against the top right of the screen,
    /// and through [`inside`] as a panel's contents always are — including the
    /// measuring of the window round them, which is what the room is worked
    /// out from. A few frames per screen, since that measurement is a frame
    /// old, and one context throughout, so a screen that shrinks under a panel
    /// already standing is a shrink rather than a fresh panel.
    fn stands(screens: &[f32], rows: usize) -> f32 {
        let ctx = crate::tests::context();
        let id = egui::Id::new("tall-panel");
        let at = egui::pos2(600. - MARGIN, MARGIN);
        let mut rect = egui::Rect::ZERO;
        let mut framing = 0.;
        for high in screens {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600., *high),
                )),
                ..Default::default()
            };
            for _ in 0..4 {
                let _ = ctx.run_ui(input.clone(), |ui| {
                    let mut showing = true;
                    let room = room_under(ui.ctx(), id, at, framing);
                    let panel = framed(
                        ui.ctx(),
                        "PANEL",
                        id,
                        at,
                        room,
                        false,
                        &mut showing,
                    );
                    let mut held = 0.;
                    let shown = panel.show(ui.ctx(), |ui| {
                        held = inside(ui, id, room, |ui| {
                            for row in 0..rows {
                                ui.label(format!("row {row}"));
                            }
                        });
                    });
                    if let Some(shown) = shown {
                        rect = shown.response.rect;
                        framing = rect.height() - held;
                    }
                });
            }
        }

        rect.height()
    }

    /// A panel is as tall as what it holds, up to the room there is
    ///
    /// A panel holds anything from four rows of an orbit to four hundred
    /// systems of a faction's holdings, and it ran off the bottom of the
    /// viewport with the rest of itself out of reach: the contents are
    /// scrolled now, so a long one stops at the room.
    ///
    /// It fills that room rather than egui's own default of four hundred
    /// pixels for a window with a scroll area in it, which left a long panel
    /// short with the screen empty under it. And no height is imposed to do
    /// either: the tiling steps the next panel by the tallest drawn, so a
    /// short panel held to the room would leave a column with one panel in it
    /// and a screen of nothing under it.
    #[test]
    fn a_panel_is_as_tall_as_what_it_holds_up_to_the_room() {
        let short = stands(&[600.], 4);
        let long = stands(&[600.], 400);

        assert!(short < 200., "four rows stood {short} tall");
        assert!(
            long <= 600.,
            "four hundred rows stood {long} tall, past a 600 screen"
        );
        assert!(
            long > 500.,
            "four hundred rows stood {long} tall, well short of the room"
        );
    }

    /// And comes back inside a viewport that shrinks under it
    ///
    /// The reported trouble: a panel standing the height of the screen and
    /// then the screen made shorter kept the height it had, so the end of it —
    /// the buttons under a system's factions — was off the bottom with nothing
    /// to reach it by. The room is measured afresh every frame, from where the
    /// panel actually stands, and caps the height as well as suggesting it.
    #[test]
    fn a_panel_comes_back_inside_a_shrinking_viewport() {
        let shrunk = stands(&[600., 300.], 400);

        assert!(
            shrunk <= 300.,
            "the panel stood {shrunk} tall on a screen of 300"
        );
    }

    /// How wide a panel titled `title` lays its contents out, and how much
    /// room it had
    ///
    /// What is being asked is what the contents make of the width the title
    /// left them.
    fn laid_out(title: &str) -> (f32, f32) {
        let mut taken = 0.;
        let rect = shown(title, egui::Pos2::ZERO, |ui| {
            spread(ui);
            taken = ui.available_width();
        });
        let margins =
            egui::Frame::window(&crate::tests::context().global_style())
                .total_margin()
                .sum()
                .x;

        (taken, rect.width() - margins)
    }

    /// A panel's contents are laid out across the whole of its window
    ///
    /// A window is at least as wide as its title bar, and a system is called
    /// what it is called: it is not cut down the way a route's two ends are,
    /// so a long enough name is a wider panel. Contents laid out to the width
    /// a panel asks for would leave a band of empty panel down the right hand
    /// side of one, which reads as a margin nobody chose.
    ///
    /// Named at a length no lettering would fit, rather than at one that fits
    /// today and not tomorrow.
    #[test]
    fn a_long_title_widens_what_stands_under_it() {
        let (taken, had) = laid_out(&"COL 285 SECTOR ".repeat(4));

        assert!(had > WIDTH, "{had} is no wider than the {WIDTH} asked for");
        assert_eq!(taken, had);
    }

    /// How wide one character of a panel's lettering stands
    fn character() -> f32 {
        let ctx = crate::tests::context();
        // Inside a pass, egui having no fonts to measure with before one.
        let mut one = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            one = crate::ui::one_character(ui.ctx(), egui::TextStyle::Body);
        });

        one
    }

    /// A title that fits leaves the panel the width it asked for
    ///
    /// To the character. A title bar holds a whole number of them and is
    /// filled out to its end, so a panel stands at the width asked for taken
    /// up to the next one, and never short of it.
    ///
    /// Which is also what holds [`TITLE_MARKS`] down. What the title bar
    /// spends on the fold arrow and the close mark is measured off egui
    /// rather than worked out, and a panel standing anywhere but within a
    /// character of the width asked for is that measurement having drifted.
    #[test]
    fn a_short_title_leaves_the_width_alone() {
        let (taken, had) = laid_out("SOL");

        assert_eq!(taken, had);
        assert!(had >= WIDTH, "{had} is narrower than the {WIDTH} asked for");
        assert!(
            had < WIDTH + character(),
            "{had} is over a character wider than the {WIDTH} asked for"
        );
    }

    /// How wide a panel comes out, folded away into its title bar or open
    ///
    /// Drawn over enough passes for egui to have finished animating the
    /// fold, a panel halfway into one being neither width.
    fn width(folded: bool) -> f32 {
        let ctx = crate::tests::context();
        let id = egui::Id::new("test-panel");
        let mut width = 0.;

        for pass in 0..20 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                if pass == 0 {
                    let mut fold =
                        egui::containers::collapsing_header::CollapsingState::
                            load_with_default_open(
                                ui.ctx(),
                                id.with("collapsing"),
                                true,
                            );
                    fold.set_open(!folded);
                    fold.store(ui.ctx());
                }

                let mut showing = true;
                let panel = framed(
                    ui.ctx(),
                    "SOL",
                    id,
                    egui::Pos2::ZERO,
                    room_under(ui.ctx(), id, egui::Pos2::ZERO, 0.),
                    pass > 0,
                    &mut showing,
                );
                if let Some(panel) = panel.show(ui.ctx(), |ui| {
                    spread(ui);
                    ui.label("5 systems");
                }) {
                    width = panel.response.rect.width();
                }
            });
        }

        width
    }

    /// Folding a panel away leaves its width alone
    ///
    /// A window is as wide as its title bar needs and as wide as whatever
    /// stands under it, and a folded panel is the title bar alone. A bar
    /// narrower than the contents is a panel that draws itself in the moment
    /// the fold finishes, and back out again when it is opened.
    #[test]
    fn folding_a_panel_leaves_its_width_alone() {
        assert_eq!(width(true), width(false));
    }

    /// A title fills the bar it stands in
    ///
    /// Which is what puts it at the left: egui centres a title between the
    /// marks either side of it, and one that fills the bar has nothing left
    /// to centre.
    #[test]
    fn a_title_fills_the_bar_it_stands_in() {
        let ctx = crate::tests::context();
        // Inside a pass, egui having no fonts to measure with before one.
        let (mut said, mut room) = (String::new(), 0);
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            said = titled(ui.ctx(), "SOL").text().to_owned();
            room = titling(ui.ctx());
        });

        assert!(said.starts_with("SOL"), "{said:?}");
        assert_eq!(said.chars().count(), room);
    }

    /// A route's panel is no wider than a panel, however it is named
    ///
    /// Both ends of a route run long, and a window is as wide as its title
    /// bar needs. Left whole, the title decides how wide the panel stands,
    /// which is a panel the tiling cannot step by and the user cannot read
    /// two of side by side.
    #[test]
    fn a_route_panel_is_no_wider_than_a_panel() {
        let (_, route) = laid_out("SIGMA DRACONIS -> MINISTRY");
        let (_, plain) = laid_out("SOL");

        assert_eq!(route, plain);
    }

    /// A panel opens with its right hand top corner where it was put
    ///
    /// Which is what standing off the edge of the viewport by the margin
    /// comes to. Placed by its left edge instead, a panel wider than the
    /// width it was given would reach past that edge, and egui would push it
    /// back inside with nothing between it and the corner.
    ///
    /// Both titles, since what a panel is called is what widens it and the
    /// corner is not to move with the words in the title bar.
    #[test]
    fn a_panel_opens_at_the_corner_it_is_given() {
        let at = egui::pos2(400., 30.);

        for title in ["SOL", "SIGMA DRACONIS -> MINISTRY"] {
            let rect = shown(title, at, |ui| {
                spread(ui);
                ui.label("5 systems");
            });

            assert_eq!(rect.right_top(), at, "{title}");
        }
    }

    /// A system at `place`, otherwise as bare as [`system`]
    fn placed(address: i64, place: [f64; 3]) -> System {
        let mut system = system(address);
        system.position = place;
        system
    }

    /// Every distance the list puts on the lines of a route through `places`
    ///
    /// Read off what was painted rather than asked of the row, a row laying
    /// its own text out having no label to be asked what it says.
    ///
    /// The camera is a hundred light years off, so a distance measured from
    /// it could not be read as a jump.
    fn flown(places: &[[f64; 3]]) -> Vec<String> {
        let systems: Vec<System> = places
            .iter()
            .enumerate()
            .map(|(place, at)| placed(place as i64 + 1, *at))
            .collect();
        let route = Filter::Route {
            label: "A -> B".to_owned(),
            systems: (1..=places.len() as i64).collect(),
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };

        crate::tests::words(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::new(100., 0., 0.)),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        })
        .into_iter()
        // A line that is a distance and nothing else. What the whole route
        // comes to is said in light years too, and is not a row's.
        .filter(|said| {
            said.strip_suffix(" Ly")
                .is_some_and(|figure| figure.parse::<f64>().is_ok())
        })
        .collect()
    }

    /// A route says how far the jump that reaches each system is
    ///
    /// It is flown, so what is worth knowing about a system on one is whether
    /// the ship can get to it from the one before. How far it is from the
    /// camera answers a question nobody asked of a route.
    ///
    /// Two distances over three systems, the first being where the flying
    /// starts: no jump reaches it, so its line has nothing to say.
    #[test]
    fn a_route_says_how_far_each_jump_is() {
        let said = flown(&[[0., 0., 0.], [3., 4., 0.], [3., 4., 12.]]);

        assert_eq!(said, vec!["5.0 Ly", "12.0 Ly"], "{said:?}");
    }

    /// A set of systems says how far off each one is instead
    ///
    /// Nothing about a faction's holdings is a sequence, so there is no jump
    /// to measure and the distance the whole map is read in is what is left.
    #[test]
    fn a_set_of_systems_says_how_far_off_each_is() {
        let systems = [placed(1, [3., 4., 0.]), placed(2, [0., 0., 12.])];

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &faction(7),
                &[],
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(said.contains(&"5.0 Ly".to_owned()), "{said:?}");
        assert!(said.contains(&"12.0 Ly".to_owned()), "{said:?}");
    }

    /// A route between `label`'s ends, plotted for a ship reaching `range`
    /// Proven rather than whatever the form opens at: the summary tests
    /// below are about what a route *says* it was plotted for, so they say
    /// what it was plotted for rather than reading the default and
    /// following it wherever it moves.
    fn plotted_for(label: &str, range: &str) -> Filter {
        plotted_with(label, range, Drive::Unaided, Routing::FEWEST)
    }

    /// A leg of a trip, named, through the stops it runs
    fn route_through(label: &str, stops: &[i64]) -> Filter {
        Filter::Route {
            label: label.to_owned(),
            systems: stops.to_vec(),
            range: "50".to_owned(),
            trip: Some("SOL -> LAVE -> DISO".to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        }
    }

    /// The same, for a named drive and search mode
    fn plotted_with(
        label: &str,
        range: &str,
        drive: Drive,
        how: Routing,
    ) -> Filter {
        Filter::Route {
            label: label.to_owned(),
            systems: vec![1, 2],
            range: range.to_owned(),
            trip: None,
            drive,
            how,
            tune: Tuning::default(),
        }
    }

    /// A route's panel says the whole of what it was plotted for
    ///
    /// A panel is titled with what its filter is called and a route is called
    /// after its two ends, so this line is the only thing on screen telling
    /// two plots between the same pair apart — and each of the three can
    /// change which systems came back.
    ///
    /// A range and not a jump. What the ship can cross in one is what the
    /// route was worked out against; what it actually crosses is on the lines
    /// below, and is usually less — unless a jet cone was allowed, which is
    /// why the drive is said beside it: a nine hundred light year jump under
    /// a hundred and fifty light year range is a supercharge and not a bug,
    /// and nothing else on screen says one was permitted.
    #[test]
    fn a_route_panel_says_what_it_was_plotted_for() {
        assert_eq!(
            summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0),
            "12 systems, 10 Ly range, optimal, fewest jumps"
        );
        assert_eq!(
            summary(
                &plotted_with(
                    "SOL -> COLONIA",
                    "150",
                    Drive::Optimised,
                    Routing::QUICK
                ),
                116,
                None,
                0
            ),
            "116 systems, 150 Ly range, SCO supercharged, 95% optimality, \
             fewest jumps, the nearest 512 expanded"
        );
    }

    /// And what the search cost, where this session is the one that paid it
    ///
    /// The other half of what a search mode means: `quick` and `direct`
    /// differ by a jump or two and by minutes, so the row that says which
    /// was asked should say what it came to. Nothing at all for a route
    /// this session did not plot, rather than a zero.
    #[test]
    fn a_route_panel_says_what_the_search_cost() {
        let said = summary(
            &plotted_for("SOL -> BARNARD", "10"),
            12,
            Some(Duration::from_millis(2230)),
            0,
        );
        assert!(said.ends_with("plotted in 2.2 s"), "{said}");

        let untimed =
            summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0);
        assert!(!untimed.contains("plotted"), "{untimed}");
    }

    /// A stop that can supercharge says so, beside its jump
    ///
    /// Which is why the route came that way, and the jump *out* of it is the
    /// long blue one on the line. The only class the index publishes for
    /// every system is what it can supercharge on, so a stop with no cone
    /// says nothing about its star rather than guessing at a spectrum.
    #[test]
    fn a_stop_that_can_supercharge_says_so() {
        use galos_index::SystemBoost;
        use galos_index::meta::Boost;

        let route = route_through("SOL -> LAVE", &[1, 2, 3]);
        let systems: Vec<System> = (1..=3).map(system).collect();
        let boosts = crate::Boosts::of(vec![SystemBoost {
            address: 2,
            boost: Boost::Neutron,
            position: [0., 0., 0.],
        }]);

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &route,
                &[],
                Some(&systems),
                None,
                0,
                &boosts,
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            )
        });

        assert!(
            said.iter().any(|line| line.contains("neutron star")),
            "the cone on the route was not named: {said:?}"
        );
        assert_eq!(
            said.iter().filter(|line| line.contains("neutron star")).count(),
            1,
            "a system with no cone was called one: {said:?}",
        );
    }

    /// A leg's heading says how many jumps it is, as its row does
    ///
    /// The panel about a trip is about the same legs the bar has rows for,
    /// so a leg had better not read one way in the one and another in the
    /// other. Said by [`crate::ui::hops_said`] in both places rather than
    /// formatted twice.
    #[test]
    fn a_legs_heading_says_its_hops() {
        let trip = plotted_for("3 Leg Route", "50");
        let legs = [
            route_through("SOL -> LAVE", &[1, 2, 3]),
            route_through("LAVE -> DISO", &[3, 4]),
        ];
        let systems: Vec<System> = (1..=4).map(system).collect();

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &trip,
                &legs,
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            )
        });

        // Two jumps in the first leg, one in the second, said the way the
        // bar says them.
        assert!(said.iter().any(|line| line == "2 hops"), "{said:?}");
        assert!(said.iter().any(|line| line == "1 hop"), "{said:?}");
        assert_eq!(
            crate::ui::hops_said(1),
            "1 hop",
            "the bar and the panel disagree about one jump",
        );
    }

    /// A trip still being plotted says so, rather than describing part of
    /// itself as the whole
    ///
    /// The reported trouble: a panel opened before the last leg lands reads
    /// as a finished trip. Everything under the summary — the systems, the
    /// distance flown, the longest jump — is about the legs that *have*
    /// landed, and a route through some of the stops is not the route the
    /// panel is titled after.
    #[test]
    fn a_trip_still_being_plotted_says_what_is_missing() {
        let trip = plotted_for("3 Leg Route", "50");

        let waiting = summary(&trip, 120, None, 1);
        assert!(
            waiting.ends_with("1 leg still being plotted"),
            "nothing said a leg was missing: {waiting}"
        );
        let two = summary(&trip, 120, None, 2);
        assert!(two.ends_with("2 legs still being plotted"), "{two}");

        // And once they are all in, it says only what it is.
        let whole = summary(&trip, 168, None, 0);
        assert!(
            !whole.contains("still being plotted"),
            "a finished trip said it was waiting: {whole}"
        );
    }

    /// And how it was planned, where it was planned at all
    ///
    /// What was *asked* is on the panel: how hard the plan was worked and
    /// how its gaps were crossed, both of them part of a route's identity
    /// because they change the answer. The gap width is not among them —
    /// the plan climbs it, so there is no one number to quote.
    ///
    /// Nothing for a route nothing planned — a proven route is searched
    /// system by system, and an unaided ship has no boost stars — where a
    /// line about the plan would be describing machinery that never ran.
    #[test]
    fn a_route_panel_says_how_it_was_planned() {
        let asked = |drive: Drive, how: Routing, tune: Tuning| Filter::Route {
            label: "SOL -> COLONIA".to_owned(),
            systems: vec![1, 2],
            range: "50".to_owned(),
            trip: None,
            drive,
            how,
            tune,
        };
        let wide = Tuning { reach: 450, ..Tuning::default() };

        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, wide))
                .as_deref(),
            Some("an exact plan, gaps stepped"),
        );

        // And the plan's own leaning, which is its own setting rather than
        // the route's percent: a reader who leaned the plan has a
        // different route and the panel is what says so.
        let leaned = Tuning { planning: 20, ..wide };
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, leaned))
                .as_deref(),
            Some("a plan leaned 20%, gaps stepped"),
        );

        // And the reach is not quoted, the plan having climbed it.
        assert!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, wide))
                .is_some_and(|said| !said.contains("450")),
            "the panel quoted a rung the answer may not have come from",
        );

        let searched = Tuning { crossing: Crossing::Searched, ..wide };
        assert!(
            planned_with(&asked(Drive::Standard, Routing::QUICK, searched))
                .is_some_and(|said| said.ends_with("searched")),
            "the crossing was not said",
        );

        // Nothing planned it: proven, and unaided.
        assert_eq!(
            planned_with(&asked(Drive::Standard, Routing::FEWEST, wide)),
            None,
            "a proven route was said to be planned",
        );
        assert_eq!(
            planned_with(&asked(Drive::Unaided, Routing::QUICK, wide)),
            None,
            "an unaided route was said to be planned",
        );
    }

    /// The longest stretch a route crosses with nothing to scoop
    ///
    /// The reading that says whether a plotted route is flyable at all
    /// rather than merely long: a scoop takes hydrogen off the main
    /// sequence, so a run of white dwarfs and brown dwarfs is crossed on
    /// the fuel the ship set out with.
    #[test]
    fn a_route_says_its_longest_stretch_with_nothing_to_scoop() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("ONE", Some("DA"), Some(50.)),
            ("TWO", Some("Y"), Some(50.)),
            ("THREE", Some("H"), Some(50.)),
            ("SCOOPABLE", Some("K"), Some(50.)),
            ("FOUR", Some("N"), Some(50.)),
        ]);

        assert_eq!(said.run, 3, "the run was miscounted: {said:?}");
        assert_eq!(said.from.as_deref(), Some("ONE"));
        assert_eq!(said.unread, 0);
        // Four jumps of fifty: the three onto the starving stops and the
        // one that lands on the star which can refuel the ship.
        assert_eq!(said.across, Some(200.));
        assert_eq!(
            said.said().as_deref(),
            Some(
                "nothing to scoop at 3 stops in a row, from ONE, \
                 200.0 Ly to cross"
            ),
        );
    }

    /// The stretch is measured in light years, not in stops
    ///
    /// Two stretches of the same count and nothing like the same crossing:
    /// what a tank answers is the distance, fuel going as the square of a
    /// jump at least, and the count alone says neither.
    #[test]
    fn a_stretch_is_said_in_light_years() {
        let stretch = |jump: f64| {
            Scooping::of([
                ("ONE", Some("DA"), Some(jump)),
                ("TWO", Some("DA"), Some(jump)),
                ("SCOOPABLE", Some("G"), Some(jump)),
            ])
        };

        assert_eq!(stretch(50.).run, stretch(25.).run, "the counts differ");
        assert_eq!(stretch(50.).across, Some(150.));
        assert_eq!(stretch(25.).across, Some(75.));
    }

    /// A route that states no distances says the stops and nothing else
    ///
    /// There is nothing to add up. A list drawn with no camera to measure
    /// from still names its stops, and saying nothing about the crossing is
    /// the honest half of that.
    #[test]
    fn a_route_with_no_distances_says_only_its_stops() {
        let said = Scooping::of([
            ("ONE", Some("DA"), None),
            ("TWO", Some("DA"), None),
        ]);

        assert_eq!(said.run, 2);
        assert_eq!(said.across, None);
        assert_eq!(
            said.said().as_deref(),
            Some("nothing to scoop at 2 stops in a row, from ONE"),
        );
    }

    /// The fuel rule names every drive class the game has
    ///
    /// What the panel says instead of a fuel figure of its own. A reader
    /// with a class 5 fitted needs the exponent for a class 5, and a
    /// ceiling over all of them read as a fact about their ship — so the
    /// rule is stated and the arithmetic left to the one person who knows
    /// what is fitted.
    #[test]
    fn the_fuel_rule_names_each_drive_class() {
        let said = fuel_rule();

        for (class, power) in POWERS {
            assert!(
                said.contains(&format!("{class} → {power:.2}")),
                "class {class} went unsaid: {said}",
            );
        }
        assert!(said.contains("(d / range) ^ p"), "{said}");
    }

    /// A route that can refuel everywhere says nothing at all
    ///
    /// A line saying a route is fine is a line read every time to learn
    /// nothing. Most routes through settled space are this.
    #[test]
    fn a_route_that_can_refuel_anywhere_is_not_remarked_on() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("BARNARD", Some("M"), Some(6.)),
        ]);

        assert_eq!(said.run, 0);
        assert_eq!(said.said(), None, "{said:?}");
    }

    /// A class nothing has read is not counted as a starving stop
    ///
    /// The honest half. A route across unexplored space is mostly unread,
    /// and reading unknown as unscoopable would condemn every galactic
    /// plot — so the run is what is *known* to have nothing to scoop, and
    /// the unknowns are counted beside it. The reading is a floor.
    #[test]
    fn an_unread_stop_is_said_rather_than_assumed() {
        let said = Scooping::of([
            ("SOL", Some("G"), None),
            ("ONE", Some("DA"), Some(50.)),
            ("UNREAD", None, Some(50.)),
            ("TWO", Some("DA"), Some(50.)),
        ]);

        assert_eq!(said.run, 1, "an unread stop extended the run: {said:?}");
        assert_eq!(said.unread, 1);
        assert_eq!(
            said.said().as_deref(),
            Some(
                "nothing to scoop at 1 stop, from ONE, 50.0 Ly to cross, \
                 1 stop unread"
            ),
        );
    }

    /// And a giant is the same star grown, so it still refuels a ship
    ///
    /// The case a first-letter rule gets wrong in the other direction: `MS`
    /// is an S-type star and not an `M` dwarf, while `M_RedGiant` is.
    #[test]
    fn a_giant_refuels_and_an_s_type_does_not() {
        let said = Scooping::of([
            ("GIANT", Some("M_RedGiant"), None),
            ("S TYPE", Some("MS"), Some(50.)),
            ("GIANT TOO", Some("K_OrangeGiant"), Some(50.)),
        ]);

        assert_eq!(said.run, 1, "{said:?}");
        assert_eq!(said.from.as_deref(), Some("S TYPE"));
    }

    /// Two plots between the same ends are told apart by it
    ///
    /// Which is the whole of why it is said. Both panels are titled the same,
    /// both list systems between the same two, and what the user asked for is
    /// the difference between them.
    #[test]
    fn two_routes_between_the_same_ends_read_apart() {
        let near = summary(&plotted_for("SOL -> BARNARD", "10"), 12, None, 0);
        let far = summary(&plotted_for("SOL -> BARNARD", "20"), 7, None, 0);

        assert_ne!(near, far);
        assert!(near.contains("10 Ly"), "{near}");
        assert!(far.contains("20 Ly"), "{far}");
    }

    /// A filter that was never plotted says how many and no more
    ///
    /// A faction and a hand-picked set were never asked a range, and a panel
    /// that answered one for them would be answering for the user.
    #[test]
    fn a_filter_that_was_not_plotted_says_only_how_many() {
        assert_eq!(summary(&faction(7), 12, None, 0), "12 systems");
        assert_eq!(
            summary(
                &Filter::Systems {
                    label: "3 systems".to_owned(),
                    systems: vec![1, 2, 3],
                },
                3,
                None,
                0
            ),
            "3 systems"
        );
    }

    /// And the panel draws whatever the summary came to
    ///
    /// Read off what was painted, the line being a label the panel lays out
    /// from what `summary` answered.
    #[test]
    fn the_panel_draws_the_summary() {
        let systems = [placed(1, [0., 0., 0.]), placed(2, [3., 4., 0.])];

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &plotted_for("SOL -> BARNARD", "10"),
                &[],
                Some(&systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });

        assert!(
            said.contains(
                &"2 systems, 10 Ly range, optimal, fewest jumps".to_owned()
            ),
            "{said:?}"
        );
    }

    /// A line carries the id its filter would be built from
    ///
    /// Clicking a faction asks for it, and what a filter tests against is the
    /// id, so the name alone would leave the line unable to say which faction
    /// it named.
    #[test]
    fn a_listed_faction_carries_its_id() {
        let names = known(&[(7, "Alliance of Sol")]);

        assert_eq!(listed(&[7], &names), vec![(7, Some("Alliance of Sol"))]);
    }

    /// A faction whose name has not arrived keeps its line, at the end
    ///
    /// Naming them is one query behind the panel opening, and a list that
    /// grew a line a frame later would jump under the reader. Held at the
    /// end rather than sorted in, since the placeholder is punctuation and
    /// would otherwise sit above every name and then jump the length of the
    /// list as soon as its own arrived.
    ///
    /// It answers to nothing while it waits. A faction is asked for by name
    /// as well as by id, and a filter row naming one as punctuation would say
    /// nothing about what had gone dim.
    #[test]
    fn a_faction_not_yet_named_takes_the_last_line() {
        let names = known(&[(1, "Alliance of Sol")]);
        let wanted = vec![(1, Some("Alliance of Sol")), (2, None)];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }

    /// Each panel takes the place after the last one opened
    #[test]
    fn panels_take_one_place_after_another() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 1]);
    }

    /// Shut a panel and its place goes to the next one opened
    ///
    /// Reading one system and then another is the common way to use these,
    /// and a place kept for a panel that is gone leaves the second one
    /// opening below a gap.
    #[test]
    fn a_shut_panel_hands_its_place_on() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        shut(&mut panels, system(1));
        panels.open_system(system(2));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0]);
    }

    /// The place handed on is the first free one, not the last one shut
    #[test]
    fn a_panel_fills_the_first_gap() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));
        panels.open_system(system(3));
        shut(&mut panels, system(2));
        panels.open_system(system(4));

        let slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        assert_eq!(slots, [0, 2, 1]);
    }

    /// No two panels stand in one place
    ///
    /// Whatever has been opened and shut in between. Two panels sharing a
    /// place is two windows on top of each other, with the lower one only
    /// findable by dragging the upper one off it.
    #[test]
    fn panels_do_not_share_a_place() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(2));
        panels.open_system(system(3));
        shut(&mut panels, system(1));
        shut(&mut panels, system(3));
        panels.open_system(system(4));
        // The one shut first, opened again alongside what took its place.
        panels.open_system(system(1));

        let mut slots: Vec<_> = panels.open.iter().map(|p| p.slot).collect();
        let held = slots.len();
        slots.sort_unstable();
        slots.dedup();
        assert_eq!(slots.len(), held);
    }

    /// A panel is put where the tiling says on the frame it opens
    ///
    /// Egui remembers where a window was for the whole session, shut windows
    /// included. A panel opened a second time takes whatever place is free
    /// then, and asking for that place as a default would leave egui
    /// answering with the place it had before, which something else may be
    /// standing in.
    #[test]
    fn a_panel_is_placed_when_it_opens() {
        let mut panels = Panels::default();
        panels.open_system(system(1));

        assert!(!panels.open[0].placed);
    }

    /// Shut the panel about `system`, as clicking its cross does
    fn shut(panels: &mut Panels, system: System) {
        let shut = Subject::System(system).id();
        panels.open.retain(|panel| panel.subject.id() != shut);
    }

    /// The first panel opens in the corner
    #[test]
    fn the_first_panel_takes_the_corner() {
        assert_eq!(tile(0, 3, 2), (0, 0));
    }

    /// The next opens below it rather than on it
    #[test]
    fn panels_tile_down_the_edge() {
        assert_eq!(tile(1, 3, 2), (1, 0));
        assert_eq!(tile(2, 3, 2), (2, 0));
    }

    /// A full column starts a fresh one to its left
    #[test]
    fn a_full_column_moves_across() {
        assert_eq!(tile(3, 3, 2), (0, 1));
        assert_eq!(tile(5, 3, 2), (2, 1));
    }

    /// A full viewport starts over in the corner
    ///
    /// The one place two panels are left on top of each other. There is
    /// nowhere else for the next one to go.
    #[test]
    fn a_full_viewport_starts_over() {
        assert_eq!(tile(6, 3, 2), (0, 0));
    }

    /// A viewport with room for nothing still answers
    ///
    /// The tiling is measured against a window the user can drag as small as
    /// they like, and dividing by what is left is how that would come back
    /// as a crash rather than as a cramped panel.
    #[test]
    fn no_room_is_still_a_place() {
        assert_eq!(tile(0, 0, 0), (0, 0));
        assert_eq!(tile(4, 0, 0), (0, 0));
    }

    /// A system already being read about is not opened twice
    #[test]
    fn one_system_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(1));

        assert_eq!(panels.open.len(), 1);
    }

    /// A faction filter, by id, called after it
    fn faction(id: i32) -> Filter {
        Filter::Faction { id, name: format!("Faction {id}") }
    }

    /// What a route comes to, flown, is the whole of it and its worst leg
    ///
    /// The longest jump is what says whether the ship can make the trip; the
    /// range it was plotted at was what was asked, not what is needed. A
    /// route with no leg to fly has neither answer.
    #[test]
    fn a_route_comes_to_its_whole_length_and_its_longest_jump() {
        assert_eq!(flying([5., 12., 3.].into_iter()), Some((20., 12.)));
        assert_eq!(flying([7.].into_iter()), Some((7., 7.)));
        assert_eq!(flying(std::iter::empty()), None);
    }

    /// And the panel says it for a route alone
    ///
    /// A faction's holdings are not flown in any order, so there is nothing
    /// about them to add up.
    ///
    /// Distances, and no fuel figure: the map is not told the ship a fuel
    /// figure would take, so the rule for working one out is on hover
    /// instead. See [`fuel_rule`].
    #[test]
    fn the_panel_says_what_a_route_comes_to_and_no_more() {
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [3., 4., 0.]),
            placed(3, [3., 4., 12.]),
        ];

        let route = listing(&plotted_for("SOL -> BARNARD", "10"), &held);
        assert!(
            route.contains(&"17.0 Ly flown, longest jump 12.0 Ly".to_owned()),
            "{route:?}"
        );

        let holdings = listing(&faction(7), &held);
        assert!(
            !holdings.iter().any(|said| said.contains("flown")),
            "{holdings:?}"
        );
    }

    /// A trip's panel lists it leg by leg, each leg's stops under its name
    ///
    /// One run of forty systems says nothing about which of them the user
    /// asked for. The figures above are still the whole trip's, and the seam
    /// stands in one leg only: the stop a leg lands on is where the next sets
    /// out from, and naming it twice would count a jump from a system to
    /// itself.
    #[test]
    fn a_trips_panel_lists_it_leg_by_leg() {
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
            placed(4, [20., 0., 0.]),
            placed(5, [26., 0., 0.]),
        ];
        let leg = |label: &str, systems: Vec<i64>| Filter::Route {
            label: label.to_owned(),
            systems,
            range: "12".to_owned(),
            trip: Some("A -> C -> E".to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![
            leg("FIRST LEG", vec![1, 2, 3]),
            leg("SECOND LEG", vec![3, 4, 5]),
        ];

        let said = crate::tests::words(|ui| {
            admitted(
                ui,
                &leg("2 Leg Route", vec![1, 2, 3, 4, 5]),
                &legs,
                Some(&held),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        });
        let at = |what: &str| {
            said.iter()
                .position(|line| line.starts_with(what))
                .unwrap_or_else(|| panic!("{what} was painted: {said:?}"))
        };

        // The whole trip's figures, above either leg. Matched on the start
        // of the line, the fuel beside the distance being
        // [`the_panel_says_what_a_route_comes_to_and_no_more`]'s business.
        assert!(at("26.0 Ly flown, longest jump 12.0 Ly") < at("FIRST LEG"));
        // Each leg's stops under its own name, in the order flown.
        assert!(at("FIRST LEG") < at("TEST 1"));
        assert!(at("TEST 3") < at("SECOND LEG"));
        assert!(at("SECOND LEG") < at("TEST 4"));
        // And the seam once: the first leg lands on Test 3, the second sets
        // out from it.
        assert_eq!(said.iter().filter(|line| *line == "TEST 3").count(), 1);
    }

    /// The list copied is the list drawn, a system to a line
    ///
    /// With the distance each line ends in, where it ends in one, set off by
    /// a tab so a paste lands in two columns. The first system of a route is
    /// reached by no jump, and its line says nothing after the name.
    #[test]
    fn the_list_is_copied_as_it_is_drawn() {
        let (sol, wolf) = (
            crate::systems::tests::named(1, "SOL"),
            crate::systems::tests::named(2, "WOLF 359"),
        );
        let order = [(&sol, None), (&wolf, Some(7.78))];

        assert_eq!(as_text(&order, &[]), "SOL\nWOLF 359\t7.78 Ly");
    }

    /// What a filter's panel offers to copy is named for what the list is
    ///
    /// A route is a way to fly and a faction's holdings are a set of places,
    /// and the button that hands either to the clipboard should say which it
    /// is handing over.
    #[test]
    fn a_route_offers_its_route_and_a_set_offers_its_list() {
        let held = [crate::systems::tests::named(1, "SOL")];

        let route = listing(&plotted_for("SOL -> BARNARD", "10"), &held);
        assert!(route.contains(&"Copy Route".to_owned()), "{route:?}");
        assert!(!route.contains(&"Copy List".to_owned()), "{route:?}");

        let holdings = listing(&faction(7), &held);
        assert!(holdings.contains(&"Copy List".to_owned()), "{holdings:?}");
        assert!(!holdings.contains(&"Copy Route".to_owned()), "{holdings:?}");
    }

    /// What a filter's list paints, line by line
    fn listing(filter: &Filter, systems: &[System]) -> Vec<String> {
        words(|ui| {
            admitted(
                ui,
                filter,
                &[],
                Some(systems),
                None,
                0,
                &crate::Boosts::absent(),
                &StarClasses::default(),
                None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
                &mut None,
            );
        })
    }

    /// A filter already being read about is not opened twice
    #[test]
    fn one_filter_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open_filter(faction(7));
        panels.open_filter(faction(7));

        assert_eq!(panels.open.len(), 1);
    }

    /// Two filters get a panel each
    #[test]
    fn each_filter_gets_its_own_panel() {
        let mut panels = Panels::default();
        panels.open_filter(faction(7));
        panels.open_filter(faction(9));

        assert_eq!(panels.open.len(), 2);
    }

    /// A filter and a system are never the same panel
    ///
    /// They share the tiling and the window, so nothing but the identity
    /// keeps a filter's panel from being taken for the panel of a system
    /// that happened to open at the same time.
    #[test]
    fn a_filter_and_a_system_are_different_panels() {
        let mut panels = Panels::default();
        panels.open_system(system(7));
        panels.open_filter(faction(7));

        assert_eq!(panels.open.len(), 2);
    }

    /// What the database does not say is said to be unknown
    ///
    /// Most of what is recorded about a system is optional, and a blank row
    /// reads as the panel having failed rather than as an answer.
    #[test]
    fn what_is_not_recorded_says_so() {
        assert_eq!(named(&Some(Allegiance::Empire)), "Empire");
        assert_eq!(named::<Allegiance>(&None), "Unknown");
    }
    /// Where each piece of text landed, and what it said
    fn placed_text(
        ctx: &egui::Context,
        input: egui::RawInput,
        contents: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Rect)> {
        let mut contents = contents;
        let output = ctx.run_ui(input, |ui| contents(ui));
        let mut found = Vec::new();
        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().to_owned(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }
        for shape in &output.shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// A panel is read on the click, not on the press that began it
    ///
    /// Measured off a real egui pass: with the button down `any_pressed` is
    /// true and `button_clicked` is false, and on the release they swap. Read
    /// off the press, a panel answered while the button was still down --
    /// saying the whole trip was being worked with -- and the leg's name
    /// corrected it when the button came up, so the selection flickered
    /// through whatever route was last plotted on the way.
    ///
    /// A double is still a click, and says the same thing: egui counts the
    /// second release as both, and what it does to a panel beyond that is
    /// roll it up into its title bar, which is egui's own reading of it.
    #[test]
    fn a_panel_is_read_on_the_click_not_the_press() {
        assert_eq!(asked_of_panel(false), None);
        assert_eq!(asked_of_panel(true), Some(crate::ui::RowGesture::Select));
    }

    /// A route's panel offers to frame it, since its title cannot be doubled
    ///
    /// The gesture that frames a filter's row is spoken for on a window:
    /// egui rolls a panel up into its title bar on a double click there. So
    /// the panel says it in words, beside the button that copies the same
    /// list, and asks the camera for the whole of what it lists.
    ///
    /// Driven at the position the button was painted at, the whole point
    /// being that there is something there to press.
    #[test]
    fn a_routes_panel_frames_it_by_a_button() {
        let route = Filter::Route {
            label: "SOL -> DISO".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [26., 0., 0.]),
        ];

        let ctx = context();
        let pass = |input: egui::RawInput, moved: &mut Option<MoveCamera>| {
            placed_text(&ctx, input, |ui| {
                admitted(
                    ui,
                    &route,
                    &[],
                    Some(&held),
                    None,
                    0,
                    &crate::Boosts::absent(),
                    &StarClasses::default(),
                    Some(DVec3::ZERO),
                    &mut None,
                    &mut None,
                    &mut None,
                    moved,
                    &mut None,
                );
            })
        };

        // Twice, the first pass being where the button is placed.
        pass(egui::RawInput::default(), &mut None);
        let text = pass(egui::RawInput::default(), &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "Frame Route")
            .expect("a button to frame the route")
            .1
            .center();

        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let mut moved = None;
        pass(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
                ..Default::default()
            },
            &mut moved,
        );

        // The middle of what it spans and the reach to the far end of it, as
        // the camera is stood back from a row's systems.
        let moved = moved.expect("the camera was asked to frame the route");
        assert_eq!(moved.position, Some(DVec3::new(13., 0., 0.)));
        assert_eq!(moved.framing, Some(13.));
    }

    /// A leg in a trip's panel is reached the way its row in the bar is
    ///
    /// A click on the name says the leg is the one being worked with, and a
    /// double says to see the whole of it. The stops under it are systems and
    /// answer as any list's lines do, so a route drawn as part of a trip can
    /// be got at from the panel about the trip rather than only from the bar.
    ///
    /// Driven at the position the name was actually painted at, since where
    /// the press lands is the whole of what is being claimed.
    #[test]
    fn a_leg_is_worked_with_by_a_click_and_framed_by_a_double() {
        let trip = Filter::Route {
            label: "SOL -> LAVE -> DISO".to_owned(),
            systems: vec![1, 2, 3, 4, 5],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![
            Filter::Route {
                label: "SOL -> LAVE".to_owned(),
                systems: vec![1, 2, 3],
                range: "10".to_owned(),
                trip: Some("SOL -> LAVE -> DISO".to_owned()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            },
            Filter::Route {
                label: "LAVE -> DISO".to_owned(),
                systems: vec![3, 4, 5],
                range: "10".to_owned(),
                trip: Some("SOL -> LAVE -> DISO".to_owned()),
                drive: Drive::Unaided,
                how: Routing::default(),
                tune: Tuning::default(),
            },
        ];
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
            placed(4, [20., 0., 0.]),
            placed(5, [26., 0., 0.]),
        ];

        let ctx = context();
        let pass = |input: egui::RawInput,
                    stops: &mut Option<(Vec<System>, bool)>,
                    moved: &mut Option<MoveCamera>,
                    worked: &mut Option<Filter>| {
            placed_text(&ctx, input, |ui| {
                admitted(
                    ui,
                    &trip,
                    &legs,
                    Some(&held),
                    None,
                    0,
                    &crate::Boosts::absent(),
                    &StarClasses::default(),
                    Some(DVec3::ZERO),
                    &mut None,
                    stops,
                    &mut None,
                    moved,
                    worked,
                );
            })
        };

        pass(egui::RawInput::default(), &mut None, &mut None, &mut None);
        let text =
            pass(egui::RawInput::default(), &mut None, &mut None, &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "LAVE -> DISO")
            .expect("the second leg's name")
            .1
            .center();

        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        // A click is answered once no double can still arrive
        // (`crate::ui::settled_click`), so the frames carry their own clock
        // and the gesture takes as many of them as a user's would.
        let frame = |time: f64, events| egui::RawInput {
            time: Some(time),
            events,
            ..Default::default()
        };
        // Longer than egui's `max_double_click_delay`, which is when a click
        // stops being half of anything.
        let settled = 0.4;

        let (mut stops, mut moved, mut worked) = (None, None, None);
        pass(
            frame(
                1.,
                vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
            ),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert!(worked.is_none(), "the click was answered before its window");
        pass(
            frame(1. + settled, Vec::new()),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert_eq!(worked.as_ref().map(|leg| leg.name()), Some("LAVE -> DISO"));
        assert!(moved.is_none(), "a click asked the camera for nothing");
        // And what the leg was plotted between, which is what the same click
        // on its row in the bar picks out: its two ends and none of the hops
        // it passes through on the way.
        let (picked, gathering) = stops.expect("the leg's stops");
        assert_eq!(
            picked.iter().map(|system| system.address).collect::<Vec<_>>(),
            vec![3, 5],
            "the hops came with it, or the ends did not"
        );
        assert!(!gathering, "a bare click was read as gathering");

        // The stops it runs through, which is one more than the rows drawn
        // under its name: it sets out from the system the leg before landed
        // on, and that row belongs to the leg before. Framed off the rows
        // alone this would read 23.0 and 3.0, standing the camera over the
        // leg's tail rather than over the leg.
        //
        // A press per frame, since that is what a double click is: egui
        // raises the click on the first release and the double on the second,
        // a frame apart, and the whole point is that the first does not act.
        let (mut stops, mut moved, mut worked) = (None, None, None);
        pass(
            frame(
                3.,
                vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
            ),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        pass(
            frame(3.05, vec![button(true), button(false)]),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        let framed =
            moved.as_ref().expect("a double asked the camera to frame it");
        assert_eq!(framed.position, Some(DVec3::new(21.5, 0., 0.)));
        assert_eq!(framed.framing, Some(4.5));
        assert!(worked.is_none(), "the double stood in for the click");
        assert!(stops.is_none(), "the double picked its stops out as well");

        // And nothing arrives afterwards: the double took the pending click
        // with it rather than leaving it to fire once the window passed.
        pass(
            frame(3. + settled, Vec::new()),
            &mut stops,
            &mut moved,
            &mut worked,
        );
        assert!(worked.is_none(), "the double's first click landed late");
        assert!(stops.is_none(), "the double picked its stops out late");
    }

    /// And the modifier reads there as it does everywhere else
    ///
    /// A click means these systems, a click with the modifier means these as
    /// well: the leg's stops joining what was already picked out rather than
    /// replacing it. Read off the same press, since the modifier is part of
    /// the press and not a setting.
    #[test]
    fn a_leg_clicked_with_the_modifier_gathers_its_stops() {
        let trip = Filter::Route {
            label: "SOL -> LAVE".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
            tune: Tuning::default(),
        };
        let legs = vec![trip.clone()];
        let held = [
            placed(1, [0., 0., 0.]),
            placed(2, [5., 0., 0.]),
            placed(3, [17., 0., 0.]),
        ];

        let ctx = context();
        let pass =
            |input: egui::RawInput, stops: &mut Option<(Vec<System>, bool)>| {
                placed_text(&ctx, input, |ui| {
                    admitted(
                        ui,
                        &trip,
                        &legs,
                        Some(&held),
                        None,
                        0,
                        &crate::Boosts::absent(),
                        &StarClasses::default(),
                        Some(DVec3::ZERO),
                        &mut None,
                        stops,
                        &mut None,
                        &mut None,
                        &mut None,
                    );
                })
            };

        pass(egui::RawInput::default(), &mut None);
        let text = pass(egui::RawInput::default(), &mut None);
        let at = text
            .iter()
            .find(|(said, _)| said == "SOL -> LAVE")
            .expect("the leg's name")
            .1
            .center();

        let mut keys = egui::Modifiers::default();
        keys.shift = true;
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: keys,
        };
        let mut stops = None;
        pass(
            egui::RawInput {
                time: Some(1.),
                events: vec![
                    egui::Event::PointerMoved(at),
                    button(true),
                    button(false),
                ],
                modifiers: keys,
                ..Default::default()
            },
            &mut stops,
        );
        // Answered once no double can still arrive; see
        // [`crate::ui::settled_click`]. The modifier is read off the press
        // rather than off this frame, which is the whole of what is asked
        // here — it is not held down any more.
        pass(
            egui::RawInput { time: Some(1.4), ..Default::default() },
            &mut stops,
        );

        let (_, gathering) = stops.expect("the leg's stops");
        assert!(gathering, "the modifier was not read off the press");
    }
}
