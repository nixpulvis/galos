//! The bar: the box in the corner the map is asked from, and the rows under
//! it saying what the map holds
//!
//! [`ask_bar`] puts one of [`AskMode`]'s three questions at a time, and each
//! is drawn by a module of its own under this one. [`state_bar`] holds what
//! outlives the asking: the filters being applied, what is picked out, and
//! how much of the sky is getting through.

use crate::map::bodies::Contents;
use crate::map::camera::MoveCamera;
use crate::map::filter::{Filter, Lookup, LookupNote};
use crate::map::route::frontier::Frontiers;
use crate::map::route::{RouteSettings, Router};
use crate::map::search::{Plot, Search, SearchNote, SearchResults};
use crate::map::selection::{Picked, Selection};
use crate::ui::bar::applied::{RowAsk, applied, reaching};
use crate::ui::bar::filter::{FilterBar, filter_body};
use crate::ui::bar::route::route_body;
use crate::ui::bar::search::{Picking, answer, cleared};
use crate::ui::bar::selection::selected;
use crate::ui::panels::Panels;
use crate::ui::text::typed;
use crate::ui::widgets::{FIELD_PADDING, entered, greyed, singleline};
use crate::ui::{
    BAR_WIDTH, CLOSE, Dropping, FIELD_GAP, MARGIN, PADDING, Pane, Standing,
    zone,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::{Context, Response, Ui};

pub(super) mod applied;
pub(super) mod filter;
mod route;
mod rows;
pub(super) mod search;
pub(super) mod selection;
mod tuning;

/// How far the selection's row stands from what is around it
///
/// The same above and below, so that the row sits balanced between the
/// input over it and whatever follows rather than hanging off one of them.
const ROW_MARGIN: f32 = 2.;

/// How far the selection's row holds its contents off its own edge
const ROW_PADDING: f32 = 3.;

// TODO: Form validation.

/// What the user has typed into the bar, and which question it is asking
///
/// The system searched for and the route's jump range, which are the fields
/// the bar itself owns. A faction is typed into the same box and kept in
/// [`FilterBar`], so that nothing about a filter is reachable from here.
///
/// A resource rather than a local, so that what is typed outlives any one
/// pass over the bar and can be read from outside the system that draws it.
#[derive(Resource, Default)]
pub(crate) struct BarFields {
    /// The system named in the box the bar leads with
    system: Option<String>,
    /// How far the ship a route is plotted for jumps
    ///
    /// The one thing about a route that is typed. Which systems it runs
    /// between is picked out on the map, and a range is a fact about a ship
    /// with nothing on the map to point at.
    route_range: Option<String>,
    /// Whether the map is to choose what order the stops are reached in
    ///
    /// Off, a trip is flown in the order the systems were picked, which is
    /// what a user who picked them in an order meant. On, the order is the
    /// map's to settle: a set gathered by looking around is a set of
    /// destinations rather than an itinerary, and the question is then which
    /// way round is cheapest.
    tour: bool,
    /// Whether the cheapest order may choose where the trip sets out from
    ///
    /// Held as the opting out rather than the option, so that the answer a
    /// fresh form gives is the one wanted: a trip sets out from the system
    /// picked first unless it is let go of. What the pane shows is the other
    /// way round, since what the user is choosing is to hold it.
    any_start: bool,
    /// Whether the trip comes home to the system it set out from
    ///
    /// A run flown out and back rather than a line that ends where the last
    /// stop is: the leg home is plotted, drawn and costed with the others,
    /// and the cheapest order weighs it. Only offered where there is a trip
    /// to close — see [`route_body`].
    looping: bool,
    /// Which of the three questions the box is asking, where it is out at all
    ///
    /// Nothing while the form is shut, and then the box is the search box:
    /// see [`AskMode`]. Turned on when the field takes focus, which
    /// [`chrome`](crate::ui::chrome) settles at the end of a frame from what it has just drawn.
    /// So this is one frame behind, which is as close as an immediate mode UI
    /// gets: a field cannot report that it has been clicked until it has been
    /// drawn, and whether to draw it is the question being asked.
    ///
    /// Off again only on an escape. A press on the map does not put the form
    /// away: what a route runs through is gathered by picking systems out,
    /// and a form that shut itself the moment the user reached for one of its
    /// own answers would be in the way of its own question.
    pub(crate) asking: Option<AskMode>,
    /// Whether the box has been asked for and not yet given the caret
    ///
    /// Set by a key or by a tab being chosen, and taken by the next pass over
    /// the bar, since only the pass that drew the box has a box to put the
    /// caret in.
    pub(crate) opening: bool,
    /// Whether the form has been asked to be put away
    ///
    /// The other half of [`BarFields::opening`], taken in the same place and
    /// for the same reason: only the pass that drew the fields can let go of
    /// the one holding the caret.
    pub(crate) shutting: bool,
}

impl BarFields {
    /// Ask for the caret to be put in the box, asking `mode`'s question
    ///
    /// What [`crate::map::keys`] does with a slash, and with the two shifted keys
    /// that reach the other two questions. The form drops out below the box as
    /// it does for a click into it, the focus being what opens it, and the
    /// mode is set here rather than waited for so that the pass which puts
    /// the caret in draws the field the caret belongs in.
    pub(crate) fn open(&mut self, mode: AskMode) {
        self.asking = Some(mode);
        self.opening = true;
    }
}

impl Pane for BarFields {
    fn showing(&self) -> bool {
        self.asking.is_some()
    }

    /// Ask for the form to be put away and the caret taken out of it
    ///
    /// What was typed is left standing, as it is when a press puts the form
    /// away: the form is shut rather than the question thrown out, and the
    /// mark inside the box is what takes the answer away.
    fn shut(&mut self) {
        self.shutting = true;
    }
}

/// Which of the three questions the bar's box is asking
///
/// The bar has three questions and one box. They have nothing to say to each
/// other — a system is searched for, a faction is filtered on, a route is
/// costed — and stacked out together they made a form three quarters of the
/// viewport tall, most of it about whatever the user was not doing. So one is
/// out at a time, and this is which.
///
/// A system by default, that being what the map is asked for most: the box at
/// rest is the search box, and it is the one mode reachable without knowing
/// the other two are there.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum AskMode {
    /// Find a system by name
    #[default]
    System,
    /// Dim the sky down to what a name or a span admits
    Filter,
    /// Cost a route between what is picked out
    Route,
}

impl AskMode {
    /// The three, in the order the strip stands them in
    ///
    /// Search first, being what the box is at rest. The filters next, since
    /// what they add shows up in the rows directly below. The route last: it
    /// is asked of systems those rows are already holding, so it reads as the
    /// end of that column rather than the start of it.
    const ALL: [AskMode; 3] =
        [AskMode::System, AskMode::Filter, AskMode::Route];

    /// What the tab is called
    ///
    /// The name and nothing else. The key that reaches it stood in the tab
    /// as well, which meant lettering a shift mark beside a capital in a face
    /// that draws every arrow it holds to three quarters of the cap height;
    /// nowhere else in the chrome says a binding in marks either.
    /// `BINDINGS` says all three at length, in the window a reader opens to
    /// learn the keyboard.
    fn said(self) -> &'static str {
        match self {
            AskMode::System => "System",
            AskMode::Filter => "Filter",
            AskMode::Route => "Route",
        }
    }

    /// What the box is asking for in this mode
    ///
    /// Standing in the field as its placeholder, so the box says which
    /// question it is putting. It keys the field as well — see [`singleline`]
    /// — so a mode with a question of its own keeps its own caret and its own
    /// text.
    ///
    /// The search and the route want the same thing of it, in the same words:
    /// both ask about a system by name, the route running between systems, so
    /// the two share the one field and what was typed into it survives the
    /// switch between them. What a click on the answer means is where they
    /// differ; see [`Picking`].
    fn wants(self) -> &'static str {
        match self {
            AskMode::System | AskMode::Route => "Search",
            AskMode::Filter => "Faction Name",
        }
    }

    /// What choosing it does, said on hover. See [`check`](crate::ui::widgets::check).
    fn hint(self) -> &'static str {
        match self {
            AskMode::System => "Find a system by name",
            AskMode::Filter => "Draw only what a faction or a span admits",
            AskMode::Route => "Plot a route through named or picked systems",
        }
    }
}

/// What the route's own field asks for
///
/// The range, in light years. It is one of the route's settings rather than
/// the question the bar is putting, so it stands in the form with the rest of
/// them rather than in the box: see [`route_body`]. It names the field as
/// well as standing in it, [`singleline`] keying a field on what it wants, so
/// it has to differ from every other placeholder in the form.
const RANGE_WANTED: &str = "Jump Range (Ly)";

/// What the pass over the ask bar came to
///
/// Everything about the bar that something drawn after it needs: the gear
/// stands level with the field, the state bar stands under what the bar drew,
/// and the press bookkeeping at the end of [`chrome`](crate::ui::chrome) weighs both against
/// where the pointer was.
pub(super) struct Asked {
    /// Where the field sits, which is the height the gear is hung at
    pub(super) middle: f32,
    /// The whole card, for weighing a press that landed off it
    pub(super) rect: egui::Rect,
    /// What the readings under the bar stand below
    ///
    /// The card's own foot while the form is out, so the rows stand clear of
    /// the frame around it. The field's foot while it is not: what the card
    /// comes to there is the field plus the padding of a frame drawn in
    /// nothing, and a line of text held off the box by a border that is not
    /// being painted reads as a reading that belongs to something else.
    pub(super) foot: f32,
    /// The fields the bar drew, which a press off the chrome lets go of
    ///
    /// The one box, and the route's stops field beside it where that mode is
    /// out; the same box twice where it is not, letting go of a field being
    /// the same gesture however often it is asked for.
    pub(super) boxes: [egui::Id; 2],
    /// Whether the field has just taken the caret, which is what opens the
    /// form
    pub(super) took_focus: bool,
}

/// The three questions the box can put, and which it is putting
///
/// Under the field rather than over it. The field is the one thing that
/// stands whether or not the form is out, so everything that comes and goes
/// comes and goes below it: a row appearing above would carry the box — and
/// the gear lined up with it — down the screen the moment it was clicked
/// into, out from under the pointer that had just clicked it.
///
/// Answers whether a tab was clicked, which is the caller's to act on: the
/// field a mode is chosen in order to type into is the next mode's field, and
/// only the next pass has one to put the caret in.
fn mode_strip(ui: &mut Ui, mode: &mut AskMode) -> bool {
    greyed(ui, |ui| {
        ui.horizontal(|ui| {
            let mut chosen = false;
            for offered in AskMode::ALL {
                chosen |= ui
                    .selectable_value(mode, offered, offered.said())
                    .on_hover_text(offered.hint())
                    .clicked();
            }
            chosen
        })
        .inner
    })
}

/// Ask the map for one thing, in the corner it is always asked from
///
/// One field at the top of the viewport, since the map is asked one question
/// over and over. Focusing it brings a pane up behind it, drops the tabs out
/// below it and the mode's own form under those, and a press landing off the
/// chrome puts the caret away again.
///
/// It keeps its box while the bar is at rest, so that what stands at the top
/// of the viewport reads as somewhere to type rather than as a word painted
/// on the map. The pane is that same box's frame drawn in nothing while the
/// bar is at rest, rather than a frame left out and put back: nothing shifts
/// as it comes up, because nothing about the layout has changed.
///
/// It stands beside the gear, in the room past `left`, and rides the settings
/// pane's edge as the gear does, so that the corner holds the whole of what
/// the map is asked. Down the middle it would stand over the sky the spyglass
/// fills, which is drawn about the middle of the viewport and is what the map
/// is for.
///
/// Only what is being asked is drawn here. What the map is holding — the
/// filters, the selection, how much of the sky is getting through — is
/// [`state_bar`], and the moment is [`time_strip`](crate::ui::clock::time_strip). The three were one column
/// and read as one thing that would not stop growing: a reading that is
/// always true sat between two forms that are usually not out, and the rows
/// saying what the map holds sat inside the frame of a form asking about
/// something else.
///
/// `asking` is whether the search box's answer is late enough to say so,
/// which the clock the question was put by settles: the bar draws during
/// egui's own pass and has no clock of its own.
#[allow(clippy::too_many_arguments)]
pub(super) fn ask_bar(
    ctx: &Context,
    left: f32,
    asking: bool,
    search: &mut BarFields,
    searched: &mut MessageWriter<Search>,
    note: &mut SearchNote,
    results: &mut SearchResults,
    selection: &mut Selection,
    center: Option<DVec3>,
    panels: &mut Panels,
    camera: &mut MessageWriter<MoveCamera>,
    plot: &mut Plot,
    route: &mut RouteSettings,
    router: &Router,
    searching: &Frontiers,
    filter: &mut FilterBar,
) -> Asked {
    // Whether the form is out, which is both what the card is framed by and
    // what the readings under it stand below.
    let out = search.asking.is_some();
    let bar = Dropping {
        id: "main-bar",
        standing: Standing::At(egui::pos2(left + MARGIN, MARGIN)),
        out,
        // Fixed, so that the bar keeps its width and its place as the form
        // drops out of it.
        width: BAR_WIDTH,
        holds_width: true,
    }
    .show(ctx, |ui| {
        let mut taken = false;
        // Which question is being put. A system while the form is
        // shut: the box at rest is the search box.
        let mut mode = search.asking.unwrap_or_default();
        // Whether whatever was last typed here is still being
        // looked up. Two of the three ask about a name, and both
        // wait on the same lookup.
        let waiting = match mode {
            AskMode::System | AskMode::Route => asking,
            AskMode::Filter => filter.pending.waiting(),
        };

        // The one box, holding whichever question is out. The
        // search and the route both ask it about a system's name,
        // and about the same one: what a route runs between is
        // systems, so the box is the same field in both and only
        // what a click on its answer means differs. See
        // [`Picking`]. What the mark takes with it differs by mode,
        // every question leaving something different standing as
        // its answer, so each says for itself what clearing means.
        let (response, emptied) = match mode {
            AskMode::System | AskMode::Route => ask_box(
                ui,
                &mut search.system,
                mode.wants(),
                !results.is_empty(),
                waiting,
            ),
            AskMode::Filter => ask_box(
                ui,
                &mut filter.input,
                mode.wants(),
                !filter.found.is_empty(),
                waiting,
            ),
        };
        taken |= response.gained_focus();
        // Asked for by a key, and answered here because this is
        // where the box is. Counted as the box having been taken,
        // rather than left to `gained_focus` to report a frame
        // later, so the form is out the moment it is asked for.
        if std::mem::take(&mut search.opening) {
            response.request_focus();
            taken = true;
        }
        // Carried out so a press landing off the chrome can let go
        // of it. See [`let_go_of`].
        let box_id = response.id;
        // And the range, where the route drew it: that mode has a second
        // thing to type, and a caret left in either field takes the keys the
        // map flies with.
        let mut range_box = None;
        // Where the gear stands, the two of them being one row.
        let middle = response.rect.center().y;

        // What each mode makes of its own field. Return and
        // nothing else asks the question: tab moves between the
        // fields of a form, and a form that went off and asked the
        // database something on the way past would be answering a
        // question nobody had finished asking.
        match mode {
            AskMode::System | AskMode::Route => {
                // Both answer a name, so neither is any answer at
                // all once that name is being typed over.
                if emptied {
                    cleared(&mut search.system, note, results);
                } else if response.changed() {
                    note.0 = None;
                    results.clear();
                }
                // The name as a name, since the room around one is
                // not part of it and a field holding nothing but
                // room is a field holding nothing. Both reach the
                // database as letters to match otherwise, and a
                // search for two spaces answers with every system
                // that has two.
                if entered(&response, ui)
                    && let Some(name) = typed(&search.system).map(str::to_owned)
                {
                    searched.write(Search::System { name });
                }
            }
            AskMode::Filter => {
                if emptied {
                    *filter.input = None;
                    *filter.note = LookupNote::Nothing;
                    filter.found.clear();
                } else if response.changed() {
                    *filter.note = LookupNote::Nothing;
                    filter.found.clear();
                }
                if entered(&response, ui)
                    && let Some(name) = typed(&filter.input).map(str::to_owned)
                {
                    filter.lookup.write(Lookup::Faction { name });
                }
            }
        }

        if search.asking.is_some() {
            // Off the field, which the tabs sat against: they are the form's
            // own first row and read as a strip hung on the bottom edge of
            // the box while they stood a bare item's spacing under it.
            ui.add_space(FIELD_GAP);
            if mode_strip(ui, &mut mode) {
                // The caret follows the tab, a mode being chosen
                // in order to type into it. Asked for rather than
                // taken, since the field it belongs in is the next
                // pass's to draw.
                search.opening = true;
            }
            search.asking = Some(mode);

            match mode {
                AskMode::System => answer(
                    ui,
                    note,
                    results,
                    center,
                    &filter.names,
                    Picking::Asked,
                    selection,
                    panels,
                    camera,
                ),
                AskMode::Filter => filter_body(ui, filter),
                AskMode::Route => {
                    // The same answer the search draws, under the same box,
                    // and a click on it adds a stop rather than standing in
                    // for what is picked out. The stops were the map's alone
                    // before: a route between two named systems meant going
                    // to the search, picking one out, searching again with a
                    // modifier held to keep the first, and coming back.
                    answer(
                        ui,
                        note,
                        results,
                        center,
                        &filter.names,
                        Picking::Gathers,
                        selection,
                        panels,
                        camera,
                    );
                    let range = route_body(
                        ui, search, selection, searched, plot, route, router,
                        searching,
                    );
                    taken |= range.gained_focus();
                    range_box = Some(range.id);
                }
            }
        }

        let boxes = [box_id, range_box.unwrap_or(box_id)];
        (taken, middle, boxes, response.rect.bottom())
    });

    let (took_focus, middle, boxes, field_foot) = bar.inner;
    let rect = bar.response.rect;
    // What the readings under the bar stand below; see [`Asked::foot`].
    let foot = if out { rect.bottom() } else { field_foot };
    Asked { middle, rect, foot, boxes, took_focus }
}

/// Say what the map is holding, under the bar that asked for it
///
/// The filters being applied, what is picked out, and how much of the sky is
/// getting through them. All of it outlives the asking: a filter changes what
/// the whole map looks like and a selection is what the next question will be
/// about, so none of it is put away with the form. A sky gone dim with
/// nothing to say why is a map that looks broken.
///
/// Its own zone for that reason, and drawn in no frame at all. A frame is
/// what the map draws around a form that is out — something transient, that a
/// press elsewhere puts away — and these are a readout. They stood inside the
/// bar's popup frame, which put what the map holds inside the box asking
/// about something else.
///
/// It stands at `top`, which is what the bar above it drew last — its frame
/// while the form is out, and the field itself while it is not, since the
/// padding of a frame drawn in nothing is not a gap the reader can see. See
/// [`Asked::foot`]. Handed in rather than measured here, the two being
/// separate areas: egui hands back where an area reached once it has been
/// drawn, and the bar is drawn first.
///
/// Answers whether a route between what is picked out was asked for, which is
/// `whole_selection`'s to say and the caller's to act on: what answers it is
/// a mode of the bar above.
#[allow(clippy::too_many_arguments)]
pub(super) fn state_bar(
    ctx: &Context,
    left: f32,
    top: f32,
    selection: &mut Selection,
    contents: &Contents,
    center: Option<DVec3>,
    panels: &mut Panels,
    camera: &mut MessageWriter<MoveCamera>,
    searched: &mut MessageWriter<Search>,
    filter: &mut FilterBar,
) -> (egui::Rect, bool) {
    // What the filter rows were asked, carried out of the closure they are
    // drawn in: acting on either inside it would want the bar's own state
    // while egui still holds it.
    let mut row_ask = RowAsk::default();
    // Whether a gesture is still under way anywhere, which is what says a
    // control held mid-drag is still being held. Read before the rows are
    // drawn, the control that took the hold standing well below them.
    let dragging = ctx.egui_is_using_pointer();

    let rows = zone("state-bar")
        .fixed_pos(egui::pos2(left + MARGIN, top))
        .show(ctx, |ui| {
            // The same margin the bar's frame keeps to the side, so a row
            // lines up under the field that asked for it rather than standing
            // out past it. Held off what it follows by a field's own gap
            // instead: this is a caption under a box, and a whole padding's
            // worth of air under the field read as a line that belonged to
            // neither the bar nor the map.
            egui::Frame::new()
                .inner_margin(egui::Margin {
                    top: FIELD_GAP as i8,
                    ..egui::Margin::same(PADDING)
                })
                .show(ui, |ui| {
                    ui.set_width(BAR_WIDTH);
                    // One count for the whole column rather than one per kind
                    // of row. The rows are the same height and stand one after
                    // another, so letting go of a filter row moves every
                    // selection row up into a rectangle a filter row was drawn
                    // in. Numbered apart, the two would put a fresh id at a
                    // rectangle that kept its place, which is what egui reads
                    // as a widget taking another's state.
                    let mut place = 0;
                    // The filters first, and the selection under them. Both
                    // stand in the one column, so whichever is on top decides
                    // which of them holds still: picking a system out or
                    // letting one go is a thing the user does over and over,
                    // and doing it must not walk the filter rows up and down
                    // under the pointer. A filter is asked for once and its
                    // row then stays, so the selection is what moves.
                    //
                    // From wherever the rows are being held while a control
                    // is held, so that asking for a filter does not move the
                    // control that asked out from under the pointer. Drawn
                    // from a copy, since a row let go of during a gesture that
                    // cannot reach it is nothing the user asked for.
                    //
                    // Asked of egui rather than taken from the control, which
                    // says when a drag begins and may never say that it ended.
                    filter.standstill.settle(dragging);
                    let mut standing;
                    let rows = match filter.standstill.rows(&filter.active) {
                        Some(held) => {
                            standing = held;
                            &mut standing
                        }
                        None => filter.active.bypass_change_detection(),
                    };
                    row_ask = applied(ui, rows, panels, &mut place);
                    let mut went = None;
                    let routing = selected(
                        ui,
                        selection,
                        contents,
                        center,
                        &mut went,
                        panels,
                        filter.active.bypass_change_detection(),
                        &mut place,
                    );
                    if let Some(went) = went {
                        camera.write(went);
                    }
                    // Two numbers only where there is a sky behind what is
                    // picked out and the user can see it: something has to be
                    // excluded, and what is excluded has to be drawn.
                    let dimming =
                        filter.active.any_enabled() && filter.dim.0 > 0.;
                    reaching(
                        ui,
                        &filter.in_reach,
                        dimming,
                        filter.spawning.queued() > 0,
                        filter.evicting.queued() > 0,
                    );

                    routing
                })
                .inner
        });

    // Clicking a route picks out what it was plotted between: a plot is an
    // answer to a question about two systems, and the question is what the
    // user is holding when they reach for the row. So the form comes back
    // filled in with the stops that made it, and the rings stand on them.
    //
    // A leg's row means its own two ends. A trip's row means every stop of the
    // trip, which is its legs' ends with the seams closed: the stop one leg
    // lands on is where the next sets out from, and it is one stop rather than
    // two. See [`Filter::stops`] and [`crate::map::filter::trip_stops`].
    //
    // The click and only the click. This hung off the panel the info button
    // opens to begin with, which meant asking what a trip was made of picked
    // its stops out as a side effect — a button that quietly did the other
    // button's job.
    if let Some((stops, gathering)) = row_ask.picked {
        selection.pick_out(
            stops.iter().filter_map(|address| {
                crate::map::galaxy::spawn::system_at(
                    *address,
                    &filter.populated,
                    &filter.names,
                )
                .map(Picked::System)
            }),
            gathering,
        );
    }

    // Which filters are being worked with. Every kind can be picked out, and
    // only a route reads that it was: `route::active` weighs what is picked
    // against the routes being shown, so a faction picked out leaves the
    // routes as they were rather than standing in front of them.
    if !row_ask.chosen.is_empty() {
        filter.chosen.0 = row_ask.chosen;
    }

    // And a route asked over: the row's own ask rather than the form's, which
    // may have moved off it since. One message a leg, a trip being several
    // searches; see [`crate::map::search::Search::Replot`].
    for route in row_ask.replot {
        searched.write(Search::Replot(route));
    }
    // A panel about the whole trip, which reads as a route's panel because
    // that is what a trip is: one line through every stop, in order.
    if let Some((whole, legs)) = row_ask.described {
        panels.open_trip(whole, legs);
    }
    // And where the camera goes to see one whole. Every system the filter
    // admits, not only the ones the map has dragged in, since where a faction
    // is, is most of what is being asked.
    //
    // Nothing where the filter admits nowhere: a span names no systems of its
    // own, and a faction with nothing on record is a frame over nothing,
    // which is a camera pulled in to a metre.
    let framing: Vec<Filter> =
        row_ask.framed.into_iter().chain(row_ask.framed_all).collect();
    if !framing.is_empty() {
        let places: Vec<DVec3> = framing
            .iter()
            .flat_map(|filter_| {
                filter_.systems(&filter.populated, &filter.names)
            })
            .map(|system| system.position())
            .collect();
        if let Some((middle, extent)) =
            crate::map::route::spawn::framing(&places)
            && extent > 0.
        {
            camera.write(MoveCamera {
                position: Some(middle),
                framing: Some(extent),
            });
        }
    }

    (rows.response.rect, rows.inner)
}

/// Let go of the box, the press that shut the form having not been a click
///
/// The form is shut by a press. Egui lets go of the focus on a click, which is
/// a press and a release it did not read as a drag, so the two part company
/// whenever the pointer moves between the two halves of a gesture. Over a map
/// dragged to turn it, that is most of them.
///
/// What that leaves is a box holding the focus with the form shut, and focus is
/// what opens the form. A box already holding it cannot take it again, so the
/// next click on it opens nothing and the user has to click away and back to
/// say what they had already said.
///
/// Named rather than surrendered outright, so that only this box lets go. A
/// press lands off the bar whenever it lands on the settings pane, and a value
/// being clicked into there is a field that has just taken the focus.
pub(super) fn let_go_of(ctx: &egui::Context, boxes: [egui::Id; 2]) {
    ctx.memory_mut(|memory| {
        for box_id in boxes {
            memory.surrender_focus(box_id);
        }
    });
}

/// The bar's one box, and the mark that empties it
///
/// One field asks all three of the bar's questions, whichever of them is out:
/// see [`AskMode`]. `wants` is what it is asking for, which names the field
/// as well as standing in it, and is what tells one mode's box from another's
/// — [`singleline`] keys the field on it, so each mode keeps its own caret
/// and what was typed into one is not typed into the next.
///
/// The mark stands inside the box at its right hand end, and only while there
/// is something to clear. `answered` is whether anything is standing under
/// the box as an answer to what is in it, since the mark takes that with it:
/// clearing the query and leaving the list standing under it would leave the
/// answer to a question that is no longer on screen.
///
/// What clearing means is the caller's, every mode leaving something
/// different behind. So this answers whether the mark was clicked and takes
/// nothing away itself — which is a change to what is typed there and reads
/// as one everywhere that watches for it.
pub(super) fn ask_box(
    ui: &mut Ui,
    value: &mut Option<String>,
    wants: &str,
    answered: bool,
    waiting: bool,
) -> (Response, bool) {
    // Laid out first, since the room it wants is room the field cannot have.
    // In nothing, so the color can be chosen once the pointer has been asked
    // about, which cannot happen until the field has been placed.
    let showing = typed(value).is_some() || answered;
    let mark = showing.then(|| {
        egui::WidgetText::from(
            egui::RichText::new(CLOSE).color(egui::Color32::PLACEHOLDER),
        )
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    });

    let gap = ui.spacing().item_spacing.x;
    let reserved = mark.as_ref().map_or(0., |mark| mark.size().x + gap);
    let response = singleline(ui, value, wants, reserved, waiting);

    let Some(mark) = mark else { return (response, false) };
    let rect = response.rect;
    let at = egui::Rect::from_min_max(
        egui::pos2(
            rect.right() - FIELD_PADDING.right as f32 - mark.size().x,
            rect.top(),
        ),
        egui::pos2(rect.right() - FIELD_PADDING.right as f32, rect.bottom()),
    );
    // Asked about after the field, so that it is the one answering where the
    // two overlap. Under it a click would land in the text and put the caret
    // somewhere instead.
    //
    // Named by what the field wants, as [`singleline`] names the field
    // itself: a form holding two of these holds two marks, and one name
    // between them is two controls sharing a state at two rectangles.
    let clearing = ui.interact(
        at,
        ui.id().with(("clear-box", wants)),
        egui::Sense::click(),
    );
    let lit = clearing.hovered() || clearing.has_focus();
    let size = mark.size();
    ui.painter().galley(
        egui::pos2(at.left(), rect.center().y - size.y / 2.),
        mark,
        if lit {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        },
    );
    let clearing = clearing.on_hover_cursor(egui::CursorIcon::PointingHand);

    if clearing.clicked() {
        return (response, true);
    }

    (response, false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::filter::Filters;

    use crate::testing::words;
    use crate::ui::bar::filter::faction_list;

    use crate::ui::bar::search::{found, system_list};
    use crate::ui::testing::{dragged, holding, results, results_of};
    use galos_index::records::Faction as DbFaction;

    // Only the debug-only passes below use it: egui compiles its
    // between-pass id check out of a release build. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    /// A faction the search found, by id, called after it
    fn faction_row(id: i32, name: &str) -> DbFaction {
        DbFaction { id, name: name.to_owned() }
    }

    /// One mode's list does not take the place of the last mode's
    ///
    /// The box puts three questions from one place, so what a search found
    /// and what a faction lookup found are drawn into the same rectangle one
    /// pass after the other. Egui reads a rect that kept its place while what
    /// stands in it changed identity as one widget taking another's state,
    /// says so in red across the bar, and these two lists are the pair most
    /// likely to do it: both hang directly under the field, and a tab is a
    /// click.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn one_modes_list_does_not_take_the_last_modes_place() {
        let systems = results(&["SOL", "SOLATI"], true);
        let factions = [faction_row(1, "The Dukes of Mikunn")];

        let said = crate::testing::between_passes(
            |ui| {
                let mut selection = Selection::default();
                let mut travelled = None;
                let mut described = None;
                found(
                    ui,
                    &systems,
                    None,
                    &crate::map::index::Names::default(),
                    Picking::Asked,
                    &mut selection,
                    &mut travelled,
                    &mut described,
                );
            },
            |ui| {
                faction_list(ui, factions.iter());
            },
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// A list with nothing in it takes no room at all
    ///
    /// A field that has yet to be asked has nothing to say under it, and an
    /// empty scroll area under every field is a form with gaps in it.
    #[test]
    fn an_empty_list_draws_nothing() {
        let nothing = SearchResults::default();

        let systems = words(|ui| {
            system_list(ui, nothing.iter(), None, "result");
        });
        let factions = words(|ui| {
            faction_list(ui, [].iter());
        });

        assert!(systems.is_empty(), "{systems:?}");
        assert!(factions.is_empty(), "{factions:?}");
    }

    /// The whole bar drawn at once, as a frame of the real thing
    ///
    /// The pieces are tested apart, and a clash between two of them shows up
    /// nowhere until they are drawn together. The selected system is one of
    /// the ones the search turned up, which is the ordinary case: a name is
    /// searched, something is picked out of what came back, and the list is
    /// still standing under the box.
    #[test]
    fn the_whole_bar_does_not_clash_with_itself() {
        let said = crate::testing::complaints(|ui| {
            let mut query = Some("SOL".to_owned());
            let offers = results(&["SOL", "SOLATI", "SOLLARO"], true);
            let mut selection = holding(&["SOL"]);
            let mut filters = Filters::default();
            filters.add(Filter::Systems {
                label: "2 systems".to_owned(),
                systems: vec![1, 2],
            });
            let mut panels = Panels::default();
            let mut travelled = None;
            let mut described = None;
            // One count for the whole column, as the bar keeps.
            let mut place = 0;

            ask_box(ui, &mut query, "Search", !offers.is_empty(), false);
            found(
                ui,
                &offers,
                None,
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                // Its own, `found` above taking a place where this takes a
                // whole move. Neither test reads what a row asked for.
                &mut None,
                &mut panels,
                &mut filters,
                &mut place,
            );
            applied(ui, &mut filters, &mut panels, &mut place);
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// The whole bar, drawn from what a search turned up and what is held
    ///
    /// The pieces come and go independently and share one column, so a row of
    /// one kind lands where a row of another kind was. Drawn together, as the
    /// real thing is, since that is the only place the clash could show.
    fn draw_bar<'a>(
        results: &'a [&'a str],
        selection: &'a [&'a str],
        filters: usize,
    ) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut query = Some("SOL".to_owned());
            let mut offers = SearchResults::default();
            if !results.is_empty() {
                offers = results_of(results);
            }
            let mut held = holding(selection);
            let mut applied_to = Filters::default();
            for id in 0..filters {
                applied_to.add(Filter::Faction {
                    id: id as i32,
                    name: format!("Faction {id}"),
                });
            }
            let mut panels = Panels::default();
            let mut travelled = None;
            let mut described = None;
            // One count for the whole column, as the bar keeps.
            let mut place = 0;

            ask_box(ui, &mut query, "Search", !offers.is_empty(), false);
            found(
                ui,
                &offers,
                None,
                &crate::map::index::Names::default(),
                Picking::Asked,
                &mut held,
                &mut travelled,
                &mut described,
            );
            // In the bar's own order: the filters, and the selection under
            // them. Which is the whole point of drawing them together here —
            // a harness that stacked them the other way round would clear
            // every clash the real column can have.
            applied(ui, &mut applied_to, &mut panels, &mut place);
            selected(
                ui,
                &mut held,
                &Contents::default(),
                None,
                // Its own, `found` above taking a place where this takes a
                // whole move. Neither test reads what a row asked for.
                &mut None,
                &mut panels,
                &mut applied_to,
                &mut place,
            );
        }
    }

    /// Putting the results away does not hand their places to the rows below
    ///
    /// The list stands between the search box and the rows, so clearing it
    /// moves every row up into a rectangle a result line was drawn in.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn clearing_the_results_does_not_change_the_row_ids() {
        let said = crate::testing::between_passes(
            draw_bar(&["SOL", "SOLATI", "SOLLARO"], &["SOL"], 1),
            draw_bar(&[], &["SOL"], 1),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// Nor does letting go of the selection hand its rows to anything
    ///
    /// The two kinds of row are drawn one after the other in the one column.
    /// The selection stands under the filters, so letting go of it takes rows
    /// off the bottom and the filter rows above do not move at all — which is
    /// the arrangement, and this is what holds it to it: numbered the other
    /// way round, every filter row would land in a rectangle a selection row
    /// was drawn in.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn letting_go_of_the_selection_does_not_change_the_filter_row_ids() {
        let said = crate::testing::between_passes(
            draw_bar(&[], &["SOL", "BARNARD"], 2),
            draw_bar(&[], &[], 2),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// A filter's row stands in the same place however much is picked out
    ///
    /// Which is why the selection is drawn under the filters and not over
    /// them. Picking a system out and letting one go is done over and over —
    /// a route is plotted by doing it several times — and every one of those
    /// clicks used to walk the filter rows down the column under the pointer,
    /// so the row a user was reaching for was somewhere else by the time they
    /// got there. A filter is asked for once and then stays, so it is the one
    /// that holds still and the selection is what moves.
    ///
    /// Read off where the text actually landed, since where the row is drawn
    /// is the whole of the claim. The id tests beside this one say no widget
    /// took another's state; this says the user's eye was not moved.
    #[test]
    fn a_filter_row_holds_its_place_as_the_selection_changes() {
        let ctx = crate::testing::context();
        let placed = |selection: &[&str]| {
            let mut draw = draw_bar(&[], selection, 2);
            let output = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui));
            let mut found = Vec::new();
            fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Pos2)>) {
                match shape {
                    egui::Shape::Text(text) => {
                        into.push((text.galley.text().to_owned(), text.pos))
                    }
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
        };
        let row_of = |placed: &[(String, egui::Pos2)], said: &str| {
            placed
                .iter()
                .find(|(text, _)| text == said)
                .unwrap_or_else(|| panic!("no row saying {said}"))
                .1
        };

        // Twice through, so nothing here is egui settling a first pass.
        placed(&["SOL"]);
        let one = placed(&["SOL"]);
        placed(&["SOL", "BARNARD", "WOLF 359"]);
        let three = placed(&["SOL", "BARNARD", "WOLF 359"]);

        for filter in ["Faction 0", "Faction 1"] {
            assert_eq!(
                row_of(&one, filter),
                row_of(&three, filter),
                "{filter} moved when two more systems were picked out"
            );
        }
    }

    /// Gathering past what the bar holds hands no place to anything
    ///
    /// The selection's rows scroll once there are more than [`SELECTED`] of
    /// them, so from there the column stops growing however many are picked
    /// out. Nothing under them moves, and a count that went on rising would
    /// put a fresh id at a rectangle that never moved.
    ///
    /// Both ways round it, since a system is let go of from a scrolling list
    /// as easily as it is added to one.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn gathering_past_the_bar_does_not_change_the_filter_row_ids() {
        let held = [
            "SOL",
            "BARNARD",
            "WOLF 359",
            "LALANDE 21185",
            "LUYTEN 726-8",
            "ROSS 154",
            "EPSILON ERIDANI",
        ];
        let fewer = &held[..held.len() - 1];

        let gathered = crate::testing::between_passes(
            draw_bar(&[], fewer, 2),
            draw_bar(&[], &held, 2),
        );
        let let_go = crate::testing::between_passes(
            draw_bar(&[], &held, 2),
            draw_bar(&[], fewer, 2),
        );

        assert!(gathered.is_empty(), "{gathered:?}");
        assert!(let_go.is_empty(), "{let_go:?}");
    }

    /// Letting go of one of several hands no row's place to another kind
    ///
    /// The rows of both kinds are the same height and stand in the one
    /// column, so dropping a selection row moves every row under it up by
    /// exactly one. The summary line stays put through this, more than one
    /// system being held either way, so nothing else takes up the slack.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn dropping_one_of_several_does_not_hand_its_place_to_a_filter() {
        let said = crate::testing::between_passes(
            draw_bar(&[], &["SOL", "BARNARD", "WOLF 359"], 2),
            draw_bar(&[], &["SOL", "BARNARD"], 2),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// A press landing off the form takes the focus off the box above it
    ///
    /// Egui lets the focus go on a click, so a gesture that drifts between
    /// the two parts them: the box is left holding the focus the form opens
    /// on, and the next click on it opens nothing. Over a map dragged to turn
    /// it, most clicks drift.
    ///
    /// The drag is what makes this worth a test. Driven as a click it passes
    /// against the unfixed code, egui having let go of the focus itself.
    #[test]
    fn a_press_off_the_form_lets_go_of_the_box() {
        let ctx = crate::testing::context();
        let mut value: Option<String> = None;

        let draw = |input, value: &mut Option<String>| {
            let mut field = (egui::Rect::NOTHING, egui::Id::NULL);
            let _ = ctx.run_ui(input, |ui| {
                let drawn = singleline(ui, value, "Search", 0., false);
                field = (drawn.rect, drawn.id);
            });
            field
        };

        // Two passes with nothing happening, to place it, then a click in it.
        draw(egui::RawInput::default(), &mut value);
        let (rect, box_id) = draw(egui::RawInput::default(), &mut value);
        for input in dragged(rect.center(), 0.) {
            draw(input, &mut value);
        }
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(box_id),
            "the click never put the caret in the box",
        );

        // And a gesture well away from it, drifting as one over the map does.
        let away = egui::pos2(rect.right() + 300., rect.bottom() + 300.);
        for input in dragged(away, 100.) {
            draw(input, &mut value);
        }
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            Some(box_id),
            "egui let the focus go by itself, so nothing here is needed",
        );

        // Which is the press the map answers, so it is where the box is let
        // go of: the caret would otherwise take the keys the map pans with.
        let_go_of(&ctx, [box_id, box_id]);
        draw(egui::RawInput::default(), &mut value);

        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            None,
            "the box kept the focus the form opens on",
        );
    }
}
