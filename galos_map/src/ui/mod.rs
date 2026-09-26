//! The chrome standing between the user and the map
//!
//! A gear in the top left corner, the bar beside it, and the settings pane
//! that gear slides out from the left edge. What is known about the system the
//! user picked out is drawn by `crate::map::selection`, which owns the
//! fields it reads.
//!
//! Three zones, one job each, because one column doing all three grew until
//! it was most of the viewport:
//!
//! - **Asking** is `ask_bar`, in the corner: one box, and one of `AskMode`'s
//!   three questions out under it at a time. A system is searched for, a
//!   faction is filtered on, a route is costed; the three have nothing to say
//!   to each other, so they are tabs rather than sections and the filters keep
//!   their own state in `FilterBar`, reached through that alone.
//! - **Holding** is `state_bar`, directly under it and in no frame: the
//!   filters being applied, what is picked out, how much of the sky is getting
//!   through. All of it outlives the asking, so none of it is put away with a
//!   form.
//! - **When** is `time_strip`, at the top of the viewport and in the middle of
//!   it. The moment is the galaxy's and is true whatever is being asked, so it
//!   is not filed inside a card that comes and goes.
//!
//! A frame is what the map draws around something transient. The bar takes one
//! while a form is out and the strip while the scrubber is; the rows never do,
//! being a readout.
//!
//! The pieces stand in the modules under this one: [`settings`] for the pane
//! and the gear, [`bar`] for the box and the rows under it, [`clock`] for the
//! strip, [`help`] for the bindings window, and [`panels`] for the windows a
//! row or a line opens. What they are built from is shared: [`widgets`],
//! [`list`] and [`text`].

use crate::input::{Keyboard, PointerOverUi, PressOwner};
use crate::map::bodies::{Contents, mark_if_moved};
use crate::map::camera::{MoveCamera, OrbitCamera};
use crate::map::schedule::{MapSet, PaintSet};
use crate::map::selection::{ClickedEmptySky, Selection};
use crate::ui::bar::filter::FilterBar;
use crate::ui::bar::search::SearchBar;
use crate::ui::bar::{AskMode, BarFields, ask_bar, let_go_of, state_bar};
use crate::ui::clock::{GearedTo, hidden, time_strip, turns_of};
use crate::ui::help::keys_window;
use crate::ui::panels::Panels;
use crate::ui::settings::{Settings, gear, settings_body, settings_pane};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::egui::{Context, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

mod bar;
mod clock;
mod help;
pub(crate) mod keys;
pub(crate) mod list;
pub(crate) mod loading;
pub(crate) mod panels;
mod settings;
#[cfg(test)]
mod testing;
mod text;
mod widgets;

pub fn plugin(app: &mut App) {
    app.add_plugins(keys::plugin);
    app.add_plugins(loading::plugin);
    app.add_plugins(panels::plugin);
    app.init_resource::<SettingsOpen>();
    app.init_resource::<ClockControl>();
    app.init_resource::<ShowClock>();
    app.init_resource::<KeysOpen>();
    app.init_resource::<BarFields>();
    app.add_systems(
        Update,
        shut_on_empty_sky
            .in_set(MapSet::Present)
            .after(crate::map::selection::nothing_clicked),
    );
    app.add_systems(
        EguiPrimaryContextPass,
        chrome
            .in_set(PaintSet::Ui)
            .run_if(in_state(crate::map::index::load::Opening::Drawn)),
    );
}

/// Whether the settings pane is out
///
/// A resource rather than a local because the pane is drawn before the gear
/// that toggles it, so that the gear knows how far in the pane has come and
/// can stand clear of it.
#[derive(Resource, Default)]
pub(crate) struct SettingsOpen(bool);

/// Whether the key bindings are being read
///
/// Opened and shut from the keyboard alone — see [`crate::map::keys`] —
/// since a reader wanting to know what a key does has a hand on the keys.
#[derive(Resource, Default)]
pub(crate) struct KeysOpen(pub(crate) bool);

/// Whether the pane's control over the clock is out
///
/// The date under the bar is what toggles it. The reading is where a user
/// meets the clock, so it is where they reach to change it, and the control
/// itself stands in the pane with the rest of what is set rather than in the
/// bar, which says what the map is doing and sets none of it.
///
/// A resource rather than a local for the same reason [`SettingsOpen`] is
/// one: the pane is drawn before the bar the date is in, so a click on the
/// date settles the next frame's pane rather than this one's.
#[derive(Resource, Default)]
pub(crate) struct ClockControl {
    /// Whether the pane is showing it
    ///
    /// Read outside this module by [`crate::map::keys`], which puts it away.
    pub(crate) out: bool,
    /// Which of the turns on offer the rail is asked to cover
    ///
    /// Held here rather than worked out from what is picked out, so that a
    /// reader who asked for the system's own span keeps it while they click
    /// about among its planets. See [`GearedTo`].
    to: GearedTo,
}

impl Pane for ClockControl {
    fn showing(&self) -> bool {
        self.out
    }

    /// Put the scrubber away, the map left wherever it put it
    ///
    /// The run-on is not let go of: `Now` is what does that, and the span
    /// stands in the reading either way, so a reader who shut the rail has
    /// not lost the moment they set with it.
    ///
    /// Written straight rather than asked for, unlike the bar's form: there
    /// is no field here to take the caret out of, so nothing has to wait for
    /// the pass that drew it.
    fn shut(&mut self) {
        self.out = false;
    }
}

/// Whether the moment is read out at all
///
/// On, the strip stands at the top of the viewport; off, nothing is drawn up
/// there. A reading is what the map says about itself rather than something
/// it draws, so it is a switch in the pane beside the names and the grid
/// rather than a key.
///
/// Turning it off puts the map back to the present — see [`hidden`]. The
/// reading is the only place the run-on is shown and the only way back from
/// it, so a hidden strip over a map standing three hours on is a map drawing
/// a moment nobody can see or undo.
#[derive(Resource)]
pub(crate) struct ShowClock(pub(crate) bool);

impl Default for ShowClock {
    fn default() -> ShowClock {
        ShowClock(true)
    }
}

/// How wide the bar stands, unfolded or not
///
/// Wide enough for the longest line it draws without a name in it, which is
/// the count of what the spyglass holds at millions of systems, and wide
/// enough past that to hold a system's name whole. Everything is lettered in
/// one width, so what a line wants is the number of characters in it and
/// nothing else.
const BAR_WIDTH: f32 = 325.;

/// How much room across the gear is given
///
/// Wider than the glyph, which leaves it a little air on either side. Said
/// rather than measured, since the bar stands beside the gear and the gear
/// stands level with the bar's search box: one of the two has to know where
/// it goes before the other has been drawn.
const GEAR_ROOM: f32 = 20.;

/// How far the chrome stands from the edges of the viewport, and from itself
///
/// Read by [`crate::ui::panels`] as well, so that the panels it opens
/// against the right edge stand off it by as much as the gear stands off the
/// left.
pub(crate) const MARGIN: f32 = 8.;

/// How far the bar's contents stand from the pane behind them
const PADDING: i8 = 6;

/// How far one field of a form stands from the next
const FIELD_GAP: f32 = 4.;

/// How wide the dot standing for the selection is drawn
const DOT: f32 = 7.;

/// How much of a line of text a spinner standing beside a label fills
///
/// A line is taller than the letters standing on it, and a spinner drawn to
/// the whole line towers over the word it is next to.
const SPINNER: f32 = 0.75;

/// The mark on the control that opens a panel about what a row names
///
/// Read by [`crate::ui::panels`] as well, so that a line in a list opens
/// what it names by the same mark a row in the bar does.
pub(crate) const INFO: &str = "ℹ";

/// The mark on the control that lets go of what a row names
const CLOSE: &str = "x";

/// The mark on the control that asks for a route again, from nothing
///
/// A turn, which is what it is: the same question put once more. It stands on
/// a route's row alone — a faction is not searched for and has nothing to ask
/// again — and it is what a leg that came back with no route, or one the
/// reader stopped, is tried again by. See [`crate::map::search::Search::Replot`].
const AGAIN: &str = "↻";

/// The mark on the control that stops what is running
///
/// A heavy multiplication x, U+2716, and the choice is not free: the map
/// letters its chrome in egui's own faces and nothing else, so a mark is
/// drawn only if one of them holds it. A plain U+2715 does not — it reached
/// the screen as an empty box, which is what a missing glyph looks like, and
/// was reported as a broken button. U+2716 is there, as is the 🗙 egui
/// letters its own window close with. [`a_lettered_mark_is_one_the_font_has`]
/// is what says so, rather than the next reader having to find out the way
/// this was found out.
const STOP: &str = "✖";

pub(crate) fn chrome(
    mut contexts: EguiContexts,
    mut settings: Settings,
    mut bar: SearchBar,
    mut over_ui: ResMut<PointerOverUi>,
    mut keyboard: ResMut<Keyboard>,
    mut open: ResMut<SettingsOpen>,
    mut keys: ResMut<KeysOpen>,
    mut search: ResMut<BarFields>,
    mut selection: ResMut<Selection>,
    contents: Res<Contents>,
    mut camera: MessageWriter<MoveCamera>,
    orbit: Query<&OrbitCamera>,
    buttons: Res<ButtonInput<MouseButton>>,
    mut press: ResMut<PressOwner>,
    mut panels: ResMut<Panels>,
    mut filter: FilterBar,
) -> Result {
    // Giving up here takes [`PressOwner::settle`] at the end with it, and
    // nothing
    // else settles a press. See what that says about the frame it is missed
    // on.
    let ctx = contexts.ctx_mut()?;

    // What the filters stood at before the bar was drawn. The bar is handed
    // them every frame and a `ResMut` reads as written for being handed out,
    // so they are drawn against without that counting and marked below only
    // where the user asked something of them. What reads the mark asks the
    // database again, once a poll at best and once a frame at worst.
    let asked_at = filter.active.revision();

    // The pane first, since where it has reached is where the gear stands.
    let edge = settings_pane(ctx, open.0, |ui| {
        settings_body(ui, &mut settings, &mut filter.dim);
    });

    // Its own window rather than a fold at the foot of the pane. It is a
    // reference and not a control: nothing in it is set, it is read while
    // doing something else, and the pane it was filed under is where the
    // things that *are* set live. Opened from the keyboard, which is what it
    // is about.
    keys_window(ctx, &mut keys.0);

    // Where the bar's own column stands: past the pane, past the gear, and
    // as wide as the bar. Read by the strip, which is centered on the
    // viewport and gives way to this.
    let left = edge + MARGIN + GEAR_ROOM;
    let chrome_right = left + MARGIN + BAR_WIDTH;

    // The moment first. It stands at the top of the viewport whatever the bar
    // is doing, so nothing about it waits on how tall the bar has grown.
    //
    // Marked as moved only where the scrubber moved it, so that reading the
    // clock out sixty times a second is not sixty frames of every orbit being
    // run again. Which is why the strip is skipped rather than drawn and
    // hidden: what is not drawn cannot be dragged, and nothing is written.
    let turns = turns_of(&selection, &contents);
    if settings.show_clock.0 {
        mark_if_moved(&mut settings.clock, |clock| {
            time_strip(
                ctx,
                chrome_right,
                clock,
                &mut settings.clock_control,
                turns,
            )
        });
    } else if settings.clock.offset() != 0. || settings.clock_control.out {
        // Once, on the frame the switch goes off: the offset is the map's to
        // draw and there would be nothing on screen to say it was standing
        // anywhere but now. Asked about first so that a clock already at the
        // present is not marked as moved every frame the strip is hidden.
        mark_if_moved(&mut settings.clock, |clock| {
            hidden(clock, &mut settings.clock_control)
        });
    }

    // Where distances in either column are measured from, and nothing where
    // the camera has yet to say.
    let center = orbit.single().map(|camera| camera.center()).ok();
    // The bar next, in the room the gear is not standing in. Then the rows
    // under where it reached, and the gear last of the three: it stands level
    // with the field, which is not known until the bar has drawn it.
    let asked = ask_bar(
        ctx,
        left,
        // Whether the search box's answer is late enough to say so. Settled
        // where the clock is, which is the system that put the question; the
        // bar draws during egui's own pass and has no clock of its own.
        bar.pending.waiting(),
        &mut search,
        &mut bar.search,
        &mut bar.note,
        &mut bar.results,
        &mut selection,
        center,
        &mut panels,
        &mut camera,
        &mut bar.plot,
        &mut bar.route,
        &bar.router,
        &bar.searching,
        &mut filter,
    );
    let (rows, routing) = state_bar(
        ctx,
        left,
        asked.foot,
        &mut selection,
        &contents,
        center,
        &mut panels,
        &mut camera,
        &mut bar.search,
        &mut filter,
    );
    gear(ctx, edge, asked.middle, &mut open.0);

    // A press that landed on neither of the bar's two zones. Never spent: the
    // map is free to answer every one of them, which is what lets a user pick
    // systems out with the form still open. What a route runs through is
    // gathered on the map, and the form is where the range is typed and where
    // the trip is asked for, so a press that answered one by closing the
    // other would be the form standing in the way of its own question.
    //
    // Both zones, since the rows are what a route is gathered from: a press
    // on one of them is a press on the chrome, and the caret stays where it
    // was.
    let over = ctx
        .pointer_latest_pos()
        .is_some_and(|at| asked.rect.contains(at) || rows.contains(at));
    let off_the_bar = !over && ctx.input(|i| i.pointer.any_pressed());
    // Two moments, and nothing else: the field takes focus, or an escape asks
    // for the form to be put away. Moments rather than states, so that
    // neither can undo the other. Asking whether the field holds focus would
    // open the form again the very next frame.
    //
    // Whichever question was last being asked, where one was: taking the
    // caret opens the form and says nothing about which mode it opens in. A
    // system where the form was shut, the box at rest being the search box.
    if asked.took_focus {
        let _ = search.asking.get_or_insert(AskMode::System);
    }
    // The caret goes even though the form stays. A press on the map means the
    // map, and a box left holding the caret takes the keys the map pans and
    // flies with.
    if off_the_bar {
        let_go_of(ctx, asked.boxes);
    }
    // An escape lets go of whichever field held the caret, where a press lets
    // go of the box alone. A press lands somewhere, and what it lands on is
    // entitled to the focus it has just taken; an escape lands on nothing and
    // means the form, whichever of its fields was being typed into.
    //
    // Egui lets go of a bare escape's focus itself, in the pass the key
    // arrives. Said here as well because it is what holds the two together:
    // the form must not be shut over a field still holding the caret, which is
    // the state [`let_go_of`] exists to keep the map out of.
    if std::mem::take(&mut search.shutting) {
        search.asking = None;
        ctx.memory_mut(|memory| memory.stop_text_input());
    }
    // Asking for a route out of the summary line opens the mode that costs
    // one, with the caret in the range: that is the one thing left to say, so
    // the gesture reads as one move rather than as a form appearing somewhere
    // to go and find. A frame later than the click, the rows being drawn
    // after the bar they open — which is as close as an immediate mode UI
    // gets, and the same lateness `opening` already carries.
    if routing {
        search.open(AskMode::Route);
    }

    // `egui_wants_pointer_input` covers a drag that began on a control and
    // has since been pulled off it, which being over one does not.
    over_ui.0 = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();
    // Both strengths, since a letter and a space are taken by different
    // things. Written together, the two being one reading of one context.
    keyboard.typing = ctx.text_edit_focused();
    keyboard.focused = ctx.egui_wants_keyboard_input();

    // Whose the press is, now that the UI has drawn and knows what it wanted
    // of it: whether it landed on the UI, and nothing else. A press off the
    // form is the map's even while the form is open, the form having no claim
    // on a gesture aimed past it.
    press.settle(&buttons, over_ui.0);

    if filter.active.revision() != asked_at {
        filter.active.set_changed();
    }

    Ok(())
}

/// A piece of chrome the map opens, and how it is put away
///
/// The bar's form and the clock's scrubber are one thing twice over. Both are
/// something always drawn — a field, a reading — with more of it out
/// underneath while the user is working with it. Both are framed while that
/// is out and drawn in nothing while it is not. And both are put away the
/// same way: whatever opened them again, the escape a reader looks for, and a
/// click on empty sky, which is the gesture that means nothing at all.
///
/// So neither says for itself what putting away means. [`Panes`] is what
/// asks, and [`crate::map::keys`] and [`crate::map::selection`] both go
/// through it, so a fourth thing that drops out of the chrome is put away by
/// the code that already puts away these two.
///
/// Not [`crate::ui::panels::Panels`], which is the windows the map opens
/// about what a row names. Those are moved and closed one at a time and
/// belong to nothing.
pub(crate) trait Pane {
    /// Whether it is out
    fn showing(&self) -> bool;

    /// Ask for it to be put away
    ///
    /// Asked rather than done, where a pane has a field in it: only the pass
    /// that drew the field can take the caret out of it. See
    /// [`BarFields::shut`].
    fn shut(&mut self);
}

/// Every pane the chrome opens
///
/// Both gestures that put chrome away mean all of it: an escape, which is
/// where a reader looks for the way out, and a click on empty sky, which is
/// the gesture that means nothing at all. Neither asks which pane the reader
/// had in mind — a form and a rail standing open together are two halves of
/// one arrangement, and putting one of them away and leaving the other is a
/// key that has to be pressed twice to do what it looks like it does.
#[derive(SystemParam)]
pub(crate) struct Panes<'w> {
    bar: ResMut<'w, BarFields>,
    clock: ResMut<'w, ClockControl>,
}

impl Panes<'_> {
    /// Put every one of them away
    ///
    /// Asked only of the ones that are out, since asking is not free: the
    /// bar's form is put away by a flag the next pass over the chrome reads,
    /// and one raised over a form that was never open shuts a field the user
    /// has only just clicked into.
    pub(crate) fn shut_all(&mut self) {
        for pane in [&mut *self.bar as &mut dyn Pane, &mut *self.clock] {
            if pane.showing() {
                pane.shut();
            }
        }
    }
}

/// Put every pane away on a click that landed on nothing
///
/// The gesture that means nothing is wanted: the map lets go of what it holds
/// and says so, and whatever was asking about it goes too — the bar's form and
/// the clock's rail alike, through the same [`Panes`] the escape key goes
/// through. A clock's rail left standing over a map that has just been cleared
/// is the one thing on screen still asking something.
fn shut_on_empty_sky(
    mut clicks: MessageReader<ClickedEmptySky>,
    mut panes: Panes,
) {
    if clicks.read().count() > 0 {
        panes.shut_all();
    }
}

/// Where a pane stands in the viewport
enum Standing {
    /// At a place of its own, as the bar stands in the corner
    At(egui::Pos2),
    /// In the middle of the top edge, held clear of `beside`
    ///
    /// Which is where the bar's own column ends. On a window wide enough the
    /// two never meet and the pane stands in the middle. Where centering the
    /// pane would put it into the bar, it stands where its head stood — where
    /// it is centered shut, which is the reading in the middle of the top
    /// edge — and grows out to the right instead. A reading that slid left
    /// into the field the user is typing into every time the rail came out
    /// under it is worse than a rail whose far end runs off the viewport.
    Middle { beside: f32 },
}

/// The area a zone of the chrome stands in
///
/// Every one of them — the pane and its gear, the bar, the readout under it
/// — stands in the map's own layer, and stands where it is put.
///
/// Above the map and its annotations, below the windows. `Middle` is the
/// order an area takes by itself, and is asked for here because the whole of
/// the map's chrome sits in it deliberately: the annotations are painted into
/// `Order::Background` and the panels are pushed up to `Order::Foreground`,
/// so the three read as a stack.
///
/// `Background` was the wrong end of that stack and cost two things.
/// Painting: a layer that is not an area — the annotations are one painter
/// list, not a window — is drained after every area of its own order, so a
/// selection ring and a name plate were drawn over the pane. And the pointer:
/// `is_pointer_over_egui` answers false for anything in `Background` that is
/// inside the root ui's available rect, so egui did not count the chrome as
/// its own, and a wheel turned over the pane zoomed the map behind it.
///
/// Never pulled back into the viewport, which is what egui does with an area
/// left to itself: one taller than the room under it is slid up the screen
/// until its foot is on the bottom edge, and the settings pane, which stands
/// off the left while it is shut, would be dragged back on.
///
/// The sliding is what the corner is drawn wrong by. The bar stands where it
/// is put and the readings stand under where it reached, so a column that
/// egui lifts off the bottom edge walks up over the field that asked for it
/// as the next filter is applied, and walks back down as one is let go of.
/// Both halves of the one corner overflow the same way instead: a column too
/// long for the viewport runs off the foot of it, as the form above it does.
pub(crate) fn zone(id: &str) -> egui::Area {
    egui::Area::new(egui::Id::new(id))
        .order(egui::Order::Middle)
        .constrain(false)
}

/// The chrome a pane is drawn in, framed while its body is out
///
/// What the bar and the strip share, which is everything about them but what
/// is written inside: the layer they stand in, where they stand, how wide,
/// and the frame that says whether anything is open. The body itself is the
/// caller's, `out` being the same flag it built this with.
///
/// The frame is the same frame either way, drawn in nothing while the pane is
/// shut rather than left out and put back: nothing shifts as it comes up,
/// because nothing about the layout has changed.
struct Dropping<'a> {
    /// What its area is spelled under
    id: &'a str,
    /// Where it stands
    standing: Standing,
    /// Whether its body is out
    out: bool,
    /// How wide it stands with the body out
    width: f32,
    /// Whether it keeps that width with the body away
    ///
    /// The bar does: what stands there at rest is a field, and one that
    /// changed width as the form came and went would be a box moving under
    /// the pointer. The strip does not: what stands there is a line of text
    /// over the sky, and an area laid out at the scrubber's width would claim
    /// a band of the map for the pointer with nothing drawn in it — a wheel
    /// turned up there would stop turning the map.
    holds_width: bool,
}

impl Dropping<'_> {
    /// Draw it, and answer where it stood and what its contents said
    fn show<R>(
        self,
        ctx: &Context,
        contents: impl FnOnce(&mut Ui) -> R,
    ) -> egui::InnerResponse<R> {
        let style = ctx.global_style();
        let mut frame = egui::Frame::popup(&style)
            .inner_margin(egui::Margin::same(PADDING));
        if !self.out {
            frame = frame
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(
                    frame.stroke.width,
                    egui::Color32::TRANSPARENT,
                ))
                .shadow(egui::Shadow::NONE);
        }

        // How wide it is about to come out, and how wide it comes out shut:
        // it is centered by the first and gives way by the second.
        //
        // Open, the width is not a guess at all — what is asked for inside
        // the frame, and the frame's own margins and stroke around it. Shut,
        // it is whatever the contents come to, so it is what the pane last
        // came out at *while shut*, kept under a key of its own rather than
        // read off the area's rect: the rect is last pass's width whatever
        // state that was, so the pass a pane opened on was placed by the
        // width it had just stopped being, and it appeared and then moved a
        // frame later.
        let open_across = self.width + frame.total_margin().sum().x;
        let shut_at = egui::Id::new((self.id, "shut-width"));
        let shut_across = ctx.data(|kept| kept.get_temp::<f32>(shut_at));
        let across = if self.out { Some(open_across) } else { shut_across };

        let area = zone(self.id);
        let area = match self.standing {
            Standing::At(at) => area.fixed_pos(at),
            // Not `constrain_to`, which anchors within whatever it is given:
            // handed the room beside the bar, `CENTER_TOP` centers the pane
            // in that room rather than on the viewport, so it sat well right
            // of the middle on every window wide enough for there to be no
            // question.
            Standing::Middle { beside } => match across {
                Some(across) => {
                    let room = ctx.content_rect();
                    let clear = beside + MARGIN;
                    let centered = (room.width() - across) / 2.;
                    // Centered on the viewport, until the body is wide enough
                    // that centering it would reach the bar. Then the pane
                    // stands where its head stood -- where it is centered
                    // shut, which is the reading in the middle of the top
                    // edge -- and the body grows out to the right, off the
                    // viewport if it must. What is given up is the far end of
                    // a rail; what is kept is the reading, which leads the
                    // pane, and the room between it and the field the user is
                    // typing into.
                    let left = if centered >= clear {
                        centered
                    } else {
                        shut_across
                            .map_or(clear, |shut| (room.width() - shut) / 2.)
                            .max(clear)
                    };
                    area.fixed_pos(egui::pos2(left, MARGIN))
                }
                // Nothing to go on, which is the first pass of the session.
                // Egui's own anchoring stands in, and is what centering means
                // before there is a width to center.
                None => area
                    .anchor(egui::Align2::CENTER_TOP, egui::vec2(0., MARGIN)),
            },
        };

        let width = self.width;
        let holds_width = self.holds_width;
        let out = self.out;
        let shown = area.show(ctx, |ui| {
            frame
                .show(ui, |ui| {
                    if out || holds_width {
                        ui.set_width(width);
                    }
                    contents(ui)
                })
                .inner
        });

        // What it came to while shut, for the next pass to place it by. Only
        // while shut: the open width is worked out rather than remembered,
        // and a pane that wrote its open width here would place the next shut
        // pass by it.
        if !out {
            let across = shown.response.rect.width();
            ctx.data_mut(|kept| kept.insert_temp(shut_at, across));
        }

        egui::InnerResponse::new(shown.inner, shown.response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::route::ARROW;

    use crate::testing::context;
    use crate::ui::clock::STRIP_WIDTH;

    use crate::ui::text::CUT;

    /// Every mark the chrome letters is one the face actually holds
    ///
    /// The map draws its chrome in egui's own faces and adds none of its
    /// own, so a character outside them is drawn as an empty box. That is
    /// how a stop button reached the screen as a hollow square and was
    /// reported as broken text: the mark was U+2715, which Hack does not
    /// carry and neither of the fallbacks does either.
    ///
    /// Asked of the two styles the chrome letters in, since a family is
    /// resolved per style and the fallbacks differ between them. Nothing
    /// here is about how a mark looks — only that there is something to
    /// look at, which is the part that can be checked without eyes.
    #[test]
    fn a_lettered_mark_is_one_the_font_has() {
        let ctx = context();
        // Fonts are built on the first pass, and asking before one panics.
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.label("first pass");
        });

        for style in [egui::TextStyle::Body, egui::TextStyle::Button] {
            let font = style.resolve(&ctx.global_style());
            for mark in [INFO, CLOSE, STOP, ARROW, CUT] {
                assert!(
                    ctx.fonts_mut(|fonts| fonts.has_glyphs(&font, mark)),
                    "{mark:?} is drawn as an empty box in {style:?}",
                );
            }
        }
    }

    /// How wide a pane came out, holding one short word
    ///
    /// Several passes, an area being placed at the size it last came out at.
    fn dropped(out: bool, holds_width: bool) -> egui::Rect {
        let ctx = crate::testing::context();
        let mut at = egui::Rect::NOTHING;
        for _ in 0..4 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                at = Dropping {
                    id: "a-pane",
                    standing: Standing::At(egui::pos2(MARGIN, MARGIN)),
                    out,
                    width: STRIP_WIDTH,
                    holds_width,
                }
                .show(ui.ctx(), |ui| ui.label("now"))
                .response
                .rect;
            });
        }

        at
    }

    /// A pane keeps the width it is asked to keep, and no other
    ///
    /// Which is the whole of what the bar and the strip differ by in
    /// [`Dropping`]. The bar keeps its width shut, so that what stands there
    /// at rest is a field that does not change shape as the form comes and
    /// goes. The strip does not: what stands there is a line of text over the
    /// sky, and an area laid out at the scrubber's width would claim a band of
    /// the map for the pointer with nothing drawn in it.
    #[test]
    fn a_pane_keeps_the_width_it_is_asked_to_keep() {
        let holding = dropped(false, true);
        assert!(
            holding.width() >= STRIP_WIDTH,
            "a pane holding its width came to {} of {STRIP_WIDTH}",
            holding.width()
        );

        let letting_go = dropped(false, false);
        assert!(
            letting_go.width() < STRIP_WIDTH / 2.,
            "a shut pane took {} of {STRIP_WIDTH} for one word",
            letting_go.width()
        );

        // And takes the width up again the moment its body is out, whichever
        // it was asked for.
        assert!(dropped(true, false).width() >= STRIP_WIDTH);
    }

    /// A zone too long for the viewport runs off the foot of it
    ///
    /// Reported as the readings under the bar going off screen differently
    /// from the bar itself. Every zone of the chrome stands where it is put;
    /// egui does the opposite with an area left to itself, lifting one taller
    /// than the room under it until its foot is on the bottom edge. The bar
    /// does stand still, so what that lifting did was walk the column of
    /// readings up over the field that asked for them as the next filter was
    /// applied, and back down as one was let go of.
    ///
    /// Drawn where the state bar stands — a fixed place, well down a short
    /// viewport — with more rows than there is room for under it.
    #[test]
    fn a_long_zone_stands_where_it_is_put() {
        let ctx = crate::testing::context();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280., 800.),
            )),
            ..Default::default()
        };
        let top = 500.;
        let mut at = egui::Rect::NOTHING;
        // Several passes: an area is placed and sized by what it last came
        // out at, so the pass that would lift it is the pass after the first.
        for _ in 0..3 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                at = zone("a-column")
                    .fixed_pos(egui::pos2(MARGIN, top))
                    .show(ui.ctx(), |ui| {
                        ui.set_width(BAR_WIDTH);
                        for row in 0..30 {
                            ui.label(format!("row {row}"));
                        }
                    })
                    .response
                    .rect;
            });
        }

        assert!(
            (at.top() - top).abs() < 1.,
            "the column was put at {top} and stood at {}",
            at.top()
        );
        // And ran off the bottom edge rather than being packed into the room
        // above it, which is the other half of standing still.
        assert!(
            at.bottom() > 800.,
            "thirty rows came to {} of a 800 point viewport",
            at.bottom()
        );
    }

    /// The instrument itself sees a clash when there is one
    ///
    /// Two widgets given one id at two rects is the fault `complaints` is
    /// there to catch. Without this, a run that finds nothing says only that
    /// nothing was heard, which is not the same as nothing being said.
    #[test]
    fn complaints_hears_a_real_clash() {
        let said = crate::testing::complaints(|ui| {
            let id = egui::Id::new("the-same-id");
            let one = egui::Rect::from_min_size(
                egui::pos2(0., 0.),
                egui::vec2(50., 20.),
            );
            let two = egui::Rect::from_min_size(
                egui::pos2(0., 200.),
                egui::vec2(50., 20.),
            );
            ui.interact(one, id, egui::Sense::click());
            ui.interact(two, id, egui::Sense::click());
        });

        assert!(!said.is_empty(), "complaints heard nothing about a clash");
    }

    /// The instrument itself hears an id change when there is one
    ///
    /// Two ids at one rectangle across two passes is the fault
    /// [`crate::testing::between_passes`] is there to catch. Without this, a run
    /// that finds nothing says only that nothing was heard, which is not the
    /// same as nothing being said: the listener is installed once per process
    /// and quietly does nothing if something else got there first.
    // Debug only: egui compiles `warn_if_rect_changes_id` out of a
    // release build, so there is nothing to hear. See
    // [`crate::testing::between_passes`].
    #[cfg(debug_assertions)]
    #[test]
    fn between_passes_hears_a_real_id_change() {
        let at =
            egui::Rect::from_min_size(egui::pos2(0., 0.), egui::vec2(50., 20.));

        let said = crate::testing::between_passes(
            |ui| {
                ui.interact(at, egui::Id::new("one"), egui::Sense::click());
            },
            |ui| {
                ui.interact(at, egui::Id::new("two"), egui::Sense::click());
            },
        );

        assert!(!said.is_empty(), "heard nothing about a real id change");
    }
}
