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
//!
//! [`window`] is where a panel stands and how big it is; [`system`] and
//! [`fields`] what one says about a system, a star or a body; [`filter`] and
//! [`fuel`] what one says about a filter or a route.

use crate::map::bodies::mark_if_moved;
use crate::map::camera::{MoveCamera, OrbitCamera};
use crate::map::filter::{Filter, Filters};
use crate::map::galaxy::System;
use crate::map::index::{Factions, Names, Populated};
use crate::map::schedule::{MapSet, PaintSet};
use crate::map::selection::{Picked, Selection};
use crate::ui::MARGIN;
use crate::ui::panels::filter::{admitted, took};
use crate::ui::panels::fuel::StarClasses;
use crate::ui::panels::system::{body_described, described, star_described};
use crate::ui::panels::window::{WIDTH, framed, inside, room_under, tile};
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use galos_index::meta::{Body as DbBody, Star as DbStar};

mod fields;
mod filter;
mod fuel;
mod system;
mod window;

pub fn plugin(app: &mut App) {
    app.init_resource::<Panels>();
    app.init_resource::<StarClasses>();
    app.add_systems(Update, refresh.in_set(MapSet::Present));
    app.add_systems(Update, fill_filters.in_set(MapSet::Present));
    // `ui::chrome` concludes at its end whether the pointer is busy with the
    // UI, from every window drawn in the pass so far. Drawn before it, these
    // are counted in the same frame they are shown rather than the next.
    app.add_systems(
        EguiPrimaryContextPass,
        panels.in_set(PaintSet::Ui).before(crate::ui::chrome),
    );
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
            // whatever the bar now holds ([`crate::ui::bar::applied::trip_now`]). Keyed
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
/// [`crate::ui::list::asked_of_row`]: a click says it is the one being worked with.
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
fn asked_of_panel(clicked: bool) -> Option<crate::ui::list::RowGesture> {
    crate::ui::list::asked_of_row(false, false, false, false, false, clicked)
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
    names: Res<Factions>,
    mut selection: ResMut<Selection>,
    mut filters: ResMut<Filters>,
    mut selected: ResMut<crate::map::route::SelectedFilter>,
    mut clock: ResMut<crate::map::bodies::Clock>,
    orbit: Query<&OrbitCamera>,
    mut camera: MessageWriter<MoveCamera>,
    contents: Res<crate::map::bodies::Contents>,
    // What is still being searched, so a trip's panel can say what is not
    // in it yet.
    searching: Res<crate::map::route::frontier::Frontiers>,
    // Which systems can supercharge, which is the one thing about a star's
    // kind the index publishes for all of them.
    boosts: Res<crate::map::index::Boosts>,
    // And the real class of the stars a panel lists, looked up by address:
    // a list is finite where the galaxy is not. See [`StarClasses`].
    mut classes: ResMut<StarClasses>,
    transport: Res<crate::map::index::Transport>,
) -> Result {
    if panels.open.is_empty() {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    // Where the camera is looking, which is the distance the spyglass and
    // the selection's own row are measured in.
    let center = orbit.single().map(|camera| camera.center()).ok();
    // Where the eye stands, for a system's apparent magnitude — how bright it
    // looks from here, the figure the realistic view sizes a star by.
    let eye = orbit.single().map(|camera| camera.eye()).ok();
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
    // [`crate::map::bodies::Contents::guessed_under`] — and that is a
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
            && let Some((joined, flown)) =
                crate::ui::bar::applied::trip_now(filter, &filters)
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
        // And what became of its own search, for a leg whose row stands
        // before its answer. Nothing for a trip's joined route, which is
        // built out of the rows rather than standing in them: what is
        // outstanding there is `outstanding`, counted above.
        let state = match &panel.subject {
            Subject::Filter { filter, .. } => filters.plotted_of(filter),
            _ => None,
        };
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
                    state,
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
            // see the whole of what it admits. Through [`crate::ui::list::asked_of_row`]
            // so the order is the one written down there.
            //
            // Egui has already settled which window the pointer is over:
            // `contains_pointer` answers for the one on top, so a panel under
            // another does not take a click meant for it.
            if window.response.contains_pointer()
                && let Subject::Filter { filter, .. } = &panel.subject
                && asked_of_panel(clicked)
                    == Some(crate::ui::list::RowGesture::Select)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::galaxy::tests::system;
    use crate::map::route::graph::{Drive, Routing, Tuning};
    use crate::ui::testing::faction;

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

    /// A system already being read about is not opened twice
    #[test]
    fn one_system_gets_one_panel() {
        let mut panels = Panels::default();
        panels.open_system(system(1));
        panels.open_system(system(1));

        assert_eq!(panels.open.len(), 1);
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
        assert_eq!(
            asked_of_panel(true),
            Some(crate::ui::list::RowGesture::Select)
        );
    }
}
