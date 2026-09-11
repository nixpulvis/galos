//! The chrome standing between the user and the map
//!
//! A gear in the top left corner, the bar beside it, and the settings pane
//! that gear slides out from the left edge. What is known about the system the
//! user picked out is drawn by `crate::systems::selection`, which owns the
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

use crate::camera::{MoveCamera, OrbitCamera};
use crate::grid::{Bright, RulerUnit, ShowGrid, ShowMiddle, ShowPicked};
use crate::search::{Plot, Search, SearchNote, SearchResults, Searching};
use crate::systems::bodies::spawn::ShowOrbits;
use crate::systems::bodies::{Clock, Contents, mark_if_moved};
use crate::systems::fetch::Poll;
use crate::systems::filter::{
    DimTo, FactionResults, Filter, Filters, Lookup, LookupNote, Resolving,
    SPANS, Standstill, Watch,
};
use crate::systems::info::Panels;
use crate::systems::labels::ShowBodyNames;
use crate::systems::labels::{NameLimit, NameRadius};
use crate::systems::pointing::PRIMARY;
use crate::systems::route::SelectedFilter;
use crate::systems::route::frontier::Frontiers;
use crate::systems::route::graph::{Drive, Routing};
use crate::systems::route::tour::Shape;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::selection::{Picked, SELECTION, Selection};
use crate::systems::spawn::{
    ColorBy, PendingSpawns, ShowNames, StarExposure, StarProfile,
};
use crate::systems::{InReach, PendingEvictions, Spyglass};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui::{Context, Response, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use chrono::Datelike;
use galos_index::meta::{Faction as DbFaction, NameEntry};
use galos_photometry::psf::ProfileKind;

pub fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.init_resource::<Keyboard>();
    app.init_resource::<SettingsOpen>();
    app.init_resource::<ClockControl>();
    app.init_resource::<ShowClock>();
    app.init_resource::<KeysOpen>();
    app.init_resource::<PressOwner>();
    app.init_resource::<BarFields>();
    // The lettering leads, being what everything after it is drawn in. It is
    // drawn while the index is still being read, since the loading screen is
    // lettered the same way; the bar is not, holding tables that read does
    // not deliver until it lands. See [`crate::loading`].
    app.add_systems(
        EguiPrimaryContextPass,
        (lettering, chrome.run_if(in_state(crate::loading::Opening::Drawn)))
            .chain(),
    );
}

/// Set every style the chrome is drawn in
///
/// Once. A style set on the context is the style it keeps, and a font asked
/// for every frame is a font asked for sixty times a second to no end.
pub(crate) fn lettering(
    mut contexts: EguiContexts,
    mut set: Local<bool>,
) -> Result {
    if *set {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    ctx.all_styles_mut(styled);
    *set = true;

    Ok(())
}

/// Set the chrome's lettering and the marks that stand in it
///
/// The map is read in names and numbers standing in columns: how far off each
/// system is, how long each jump of a route is, how much of the sky is getting
/// through the filters. Set proportionally those columns are ragged, digits
/// being narrower than the letters beside them.
///
/// And what a route is called is one system, an arrow, and another. The hyphen
/// and the angle of an ASCII arrow are drawn to a width apiece in a monospaced
/// face and meet as an arrow; set proportionally the hyphen is short and low
/// and the two read as punctuation that happened to land side by side.
///
/// A point smaller than egui letters them, each of them, so that what stands
/// over what is unchanged. A monospaced face is wider than the proportional
/// one it stands in for, and the chrome is read at a glance off the top of a
/// map rather than paragraph by paragraph.
///
/// The marks egui draws for itself are sized here as well: the fold arrow on a
/// panel's title bar, the mark that shuts it, and the boxes in the settings
/// pane. They are set for lettering a size larger than this, and a mark drawn
/// to one scale beside words drawn to another reads as two pieces of chrome
/// that came from different maps.
pub(crate) fn styled(style: &mut egui::Style) {
    use egui::FontFamily::Monospace;
    use egui::{FontId, TextStyle};

    style.text_styles = [
        (TextStyle::Small, FontId::new(8., Monospace)),
        (TextStyle::Body, FontId::new(11.5, Monospace)),
        (TextStyle::Button, FontId::new(11.5, Monospace)),
        (TextStyle::Monospace, FontId::new(11., Monospace)),
        (TextStyle::Heading, FontId::new(17., Monospace)),
    ]
    .into();

    style.spacing.icon_width = 12.;
    style.spacing.icon_width_inner = 7.;
}

/// Whether the pointer is busy with the UI
///
/// Only the UI knows which of a window's pixels are its own, so it answers
/// here rather than the map guessing from rectangles it would have to be told
/// about.
///
/// Where the pointer is now, which is the question a wheel asks: a scroll
/// belongs to no press and so has no owner to be asked about. What a press
/// belongs to is [`PressOwner`], and everything weighing a click or a drag asks
/// that instead.
///
/// Egui lays out during its own pass, so this is what the last frame's layout
/// concluded. A wheel turned over a pane that was not there last frame turns
/// the map as well, which is a pane the user has only just opened.
#[derive(Resource, Default)]
pub(crate) struct PointerOverUi(pub(crate) bool);

/// Whether the settings pane is out
///
/// A resource rather than a local because the pane is drawn before the gear
/// that toggles it, so that the gear knows how far in the pane has come and
/// can stand clear of it.
#[derive(Resource, Default)]
pub(crate) struct SettingsOpen(bool);

/// Whether the key bindings are being read
///
/// Opened and shut from the keyboard alone — see [`crate::keys`] —
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
    /// Read outside this module by [`crate::keys`], which puts it away.
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

/// Put the map back to the present, the reading of it having gone
///
/// What hiding the clock comes to besides drawing nothing: the offset is let
/// go of and the scrubber shut, so that turning the strip off is the map at
/// `now` and turning it back on is a strip that says so. Left standing, the
/// offset would go on being drawn into every orbit with nothing on screen to
/// say why the planets are where they are.
fn hidden(clock: &mut Clock, control: &mut ClockControl) {
    clock.reset();
    control.out = false;
}

/// What the chrome has taken of the keyboard
///
/// The map is driven by bare keys and so is most of what a field wants, so the
/// two have to be told apart. See [`crate::keys`], which is the whole of what
/// reads this.
///
/// Two questions rather than one, because the chrome takes the keyboard at two
/// strengths. A field being typed into takes every letter. Anything holding the
/// focus takes only space and enter, which egui reads as a click on whatever
/// holds it.
///
/// Settled at the end of the chrome's own pass and read by the next frame's
/// [`crate::schedule::MapSet::Search`], as [`PointerOverUi`] is.
#[derive(Resource, Default)]
pub(crate) struct Keyboard {
    /// Whether a field is being typed into
    ///
    /// A text field alone. Egui goes on holding a focus wherever tab last
    /// reached, a checkbox on the settings pane among them, and a checkbox does
    /// nothing with a letter. Reading [`Keyboard::focused`] instead would leave
    /// every letter the map is driven by dead until the focus was let go of.
    pub(crate) typing: bool,
    /// Whether anything in the chrome holds the focus
    ///
    /// What a binding on space or enter has to read instead. Egui reads either
    /// of those as a click on whatever holds the focus, so the chrome answers
    /// them before the map does, and a control tabbed onto and left holding it
    /// would be clicked again by every press of the key that flies the camera.
    ///
    /// Wider than [`Keyboard::typing`] only while the user is stepping the
    /// chrome by keyboard: a click grants no focus, so nothing else puts it on
    /// a control that is not a field.
    pub(crate) focused: bool,
}

/// Whose a press is
///
/// Decided once, when the button goes down, and held until it comes up. The
/// pointer is doing one thing at a time and the thing it is doing belongs to
/// somebody: a drag that began on a slider is the slider's for as long as it
/// lasts, wherever the pointer wanders, and a press that shut the bar's form
/// is the form's even though it landed on the sky.
/// Never named outside this module. What the rest of the map asks is whose a
/// press is, and every answer to that is a `bool` on [`PressOwner`] or
/// [`Gesture`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Owner {
    /// The pointer was over a control, or the press was spent on one
    Ui,
    /// The map's to answer
    Map,
}

/// Who the press under way belongs to
///
/// Egui draws from `PostUpdate`, after every system that answers a click, so
/// what the UI made of a press is a frame behind whoever asks. Settling it
/// once at the press rather than asking afresh at the release is what makes
/// the lateness harmless: a release is a frame after its own press at worst,
/// by which time this has been written.
///
/// Reached through [`Gesture`] rather than read directly, that being where the
/// one case this cannot answer straight away is handled.
#[derive(Resource, Default)]
pub(crate) struct PressOwner {
    /// Whose the press under way is, while a button is down
    owner: Option<Owner>,
    /// Whose a press was that came up in the same frame it went down
    ///
    /// A frame long enough to hold a whole click puts the map's reading of it
    /// before the UI's, so whose it was is news that has to keep until the
    /// next frame. Standing for one frame and no longer.
    ///
    /// A second whole click in that next frame takes its place and the first
    /// goes unanswered. Two entire clicks inside two frames is a frame rate
    /// with troubles this cannot help with.
    carried_over: Option<Owner>,
}

impl PressOwner {
    /// Every button the map answers to
    ///
    /// One owner for the pointer rather than one per button. The pointer is
    /// doing one thing, and a press landing while another is already down is
    /// part of whatever that was.
    const BUTTONS: [MouseButton; 3] =
        [MouseButton::Left, MouseButton::Right, MouseButton::Middle];

    /// Settle who the pointer belongs to, the UI having now spoken
    ///
    /// `wanted` is whether the UI took this press: the pointer was over a
    /// control, or the press was spent shutting something. Called at the end
    /// of the UI's own pass, that being the first moment either is known.
    ///
    /// Wants reaching every frame. Nothing else clears an owner, so a frame
    /// that draws no UI at all leaves the last press held, and a press held
    /// after the button came up reads as a drag of the map that never ends
    /// and as a click on every release after it. [`crate::ui::chrome`] gives
    /// up before here when there is no egui context to draw into, which is a
    /// map with no window rather than a map with a stuck pointer, so this is
    /// left as the simpler arrangement of the two. Should it ever be seen,
    /// the fix is to settle from a system of its own, reading what the UI
    /// wanted out of a resource rather than off the end of drawing.
    pub(crate) fn settle(
        &mut self,
        buttons: &ButtonInput<MouseButton>,
        wanted: bool,
    ) {
        // Last frame's, which has now been read by everything that reads it.
        self.carried_over = None;

        let began = buttons.any_just_pressed(Self::BUTTONS);
        if began && self.owner.is_none() {
            self.owner = Some(if wanted { Owner::Ui } else { Owner::Map });
        }

        if !buttons.any_pressed(Self::BUTTONS) {
            if began && buttons.just_released(PRIMARY) {
                self.carried_over = self.owner;
            }
            self.owner = None;
        }
    }

    /// Whether the press under way is the UI's
    ///
    /// The question for whoever cannot wait to be told. A press nobody owns
    /// yet answers no: picking reports a click before the UI has settled
    /// whose the press was, and a star that cannot be picked out on a slow
    /// map would be a worse answer than one picked out during a gesture the
    /// UI turned out to want.
    pub(crate) fn taken_by_ui(&self) -> bool {
        self.owner == Some(Owner::Ui)
    }
}

/// What the pointer has just done, and whether it was the map's to answer
///
/// The one question every system weighing a click asks, so that none of them
/// works out an answer of its own from the button and where the pointer was.
/// Both halves are needed together: the button says what happened this frame
/// and [`PressOwner`] says whose it was.
#[derive(SystemParam)]
pub(crate) struct Gesture<'w> {
    buttons: Res<'w, ButtonInput<MouseButton>>,
    press: Res<'w, PressOwner>,
}

impl Gesture<'_> {
    /// Whether the map is being dragged
    ///
    /// False for the first frame of a drag, the UI not having said whose it
    /// is until the end of that frame. A frame of a map that has not started
    /// turning yet, against a frame of one that turns under a press meant for
    /// a slider.
    pub(crate) fn dragging_map(&self) -> bool {
        self.press.owner == Some(Owner::Map)
    }

    /// Whether `button` is down
    ///
    /// Which of them is being dragged with, once [`Self::dragging_map`] has
    /// said the drag is the map's at all. Offered here so that asking takes
    /// one thing rather than a system holding its own copy of the input
    /// beside this, which would be two readings of the same buttons sitting
    /// where they could be told apart.
    pub(crate) fn pressed(&self, button: MouseButton) -> bool {
        self.buttons.pressed(button)
    }

    /// Whether a click the map owns has just finished
    ///
    /// On the release, where the press landed in an earlier frame and the
    /// owner is already standing. A frame holding the whole click answers a
    /// frame later, through [`PressOwner::carried_over`], which is the one
    /// place that
    /// wait is spelled out.
    pub(crate) fn on_map(&self) -> bool {
        if self.buttons.just_released(PRIMARY) {
            return self.press.owner == Some(Owner::Map);
        }
        self.press.carried_over == Some(Owner::Map)
    }
}

// TODO: Form validation.

/// How wide the settings pane stands when it is out
const PANE_WIDTH: f32 = 240.;

/// How wide the bar stands, unfolded or not
///
/// Wide enough for the longest line it draws without a name in it, which is
/// the count of what the spyglass holds at millions of systems, and wide
/// enough past that to hold a system's name whole. Everything is lettered in
/// one width, so what a line wants is the number of characters in it and
/// nothing else.
const BAR_WIDTH: f32 = 325.;

/// How tall the gear is drawn
const GEAR_SIZE: f32 = 18.;

/// How much room across the gear is given
///
/// Wider than the glyph, which leaves it a little air on either side. Said
/// rather than measured, since the bar stands beside the gear and the gear
/// stands level with the bar's search box: one of the two has to know where
/// it goes before the other has been drawn.
const GEAR_ROOM: f32 = 20.;

/// How far the chrome stands from the edges of the viewport, and from itself
///
/// Read by [`crate::systems::info`] as well, so that the panels it opens
/// against the right edge stand off it by as much as the gear stands off the
/// left.
pub(crate) const MARGIN: f32 = 8.;

/// How far the bar's contents stand from the pane behind them
const PADDING: i8 = 6;

/// How far one field of a form stands from the next
const FIELD_GAP: f32 = 4.;

/// How much of a slider's row the number beside it is given
///
/// The rail takes everything but this, so the boxes line up down the pane and
/// the last of them ends where the pane does. Enough for the widest number a
/// slider here reaches: the spyglass runs to 110,000 light years.
const VALUE_WIDTH: f32 = 56.;

/// The name the time control's own `Ui` is spelled out under
///
/// Global, so that what is inside it is numbered from this name alone and not
/// from how many widgets stand above it in the bar.
const WATCH: &str = "watch-control";

/// How much of the watch slider's row the span beside it is given
///
/// Wider than [`VALUE_WIDTH`], the reading being a name rather than a number.
/// Enough for "15 minutes", the longest of [`SPANS`], which comes to 69.25 in
/// the face the map letters its controls in.
const SPAN_WIDTH: f32 = 70.;

/// Hold a slider's value in a box of its own
///
/// A slider left to draw its own value sizes that box to the number in it, so
/// a column of them comes out ragged and none reaches the edge of the pane.
/// Drawn separately, every box is the same width and they line up.
///
/// The caller builds the box, since what clamps and how fast it drags is the
/// slider's own business: the three the spyglass is offered at share one value
/// and none of them may clamp it, where the one the filters are dimmed by is
/// a percentage and clamps to it.
fn value_box(ui: &mut Ui, value: egui::DragValue<'_>) -> Response {
    ui.add_sized(egui::vec2(VALUE_WIDTH, ui.spacing().interact_size.y), value)
}

/// Size the next slider to the room it stands in
///
/// Egui draws a rail at [`egui::style::Spacing::slider_width`] and not at
/// whatever room it has, so a slider left alone is an island of the same
/// hundred pixels wherever it is put. Asked here rather than once for the
/// whole pane, so that a slider indented under a checkbox fills what is left
/// of its line rather than running out past it.
///
/// What the slider leaves is `beside`, for whatever stands at the end of the row
/// reading it out, and the gap between the two, so the row ends flush with
/// whatever it is standing in.
fn fill_width(ui: &mut Ui, beside: f32) {
    let gap = ui.spacing().item_spacing.x;
    ui.spacing_mut().slider_width =
        (ui.available_width() - beside - gap).max(0.);
}

/// How wide the dot standing for the selection is drawn
const DOT: f32 = 7.;

/// How much of a line of text a spinner standing beside a label fills
///
/// A line is taller than the letters standing on it, and a spinner drawn to
/// the whole line towers over the word it is next to.
const SPINNER: f32 = 0.75;

/// The mark on the control that opens a panel about what a row names
///
/// Read by [`crate::systems::info`] as well, so that a line in a list opens
/// what it names by the same mark a row in the bar does.
pub(crate) const INFO: &str = "ℹ";

/// The mark on the control that lets go of what a row names
const CLOSE: &str = "x";

/// How far the selection's row stands from what is around it
///
/// The same above and below, so that the row sits balanced between the
/// input over it and whatever follows rather than hanging off one of them.
const ROW_MARGIN: f32 = 2.;

/// How far the selection's row holds its contents off its own edge
const ROW_PADDING: f32 = 3.;

/// The color that dot is drawn in
///
/// [`SELECTION`] in egui's terms, so that the status line under the search
/// box and the ring out on the map are one mark in two places rather than
/// two colors to be matched up.
const SELECTION_DOT: egui::Color32 = egui::Color32::from_rgb(
    (SELECTION.red * 255.) as u8,
    (SELECTION.green * 255.) as u8,
    (SELECTION.blue * 255.) as u8,
);

/// How much of the value a drag on the box beside a radius is worth
///
/// A fraction of the number itself rather than a distance, since a radius runs
/// over four decades and a drag has one speed: half a percent per pixel moves
/// 5 to 6 as readily as 50,000 to 100,000, where a fixed speed does one or the
/// other and not both.
///
/// Far finer than the rail beside it, which is the point of having both. A
/// logarithmic rail spends those four decades over its own width, which comes
/// to about seven percent of the value for every pixel of it, so the rail
/// reaches anywhere and settles on nothing. This is the instrument for
/// settling, and it is worth roughly a tenth of what the rail is.
const RADIUS_DRAG: f32 = 0.005;

/// A radius in light years, on a log slider with a box beside it
///
/// One rail, logarithmic, since the range runs over five orders of magnitude
/// and a linear one would spend nearly all of itself between ten thousand
/// light years and a hundred thousand, which is a distance nobody sets, while
/// leaving no purchase at all down where a single light year is a real
/// distance. A logarithmic rail gives every decade the same room.
///
/// Exactness is the box's business, not the rail's. A pixel near the top of
/// the rail is worth hundreds of light years however it is scaled, so a number
/// that has to be exact is typed rather than dragged to.
///
/// `ceiling` bounds what can be asked for, and only that. A value already
/// above it is shown held down to it and left alone underneath, so a ceiling
/// that moves cannot quietly rewrite a setting: names asked for out to twenty
/// light years stay asked for out to twenty when the spyglass is drawn in to
/// five, and are back at twenty when it opens again. Writing the held figure
/// back instead loses the asking the first frame the ceiling drops under it,
/// with nothing to say it happened and no way to get it back but to ask again.
fn radius_slider(ui: &mut Ui, radius: &mut f32, ceiling: f32) -> Response {
    let ceiling = ceiling.clamp(Spyglass::FLOOR, Spyglass::CEILING);
    // The galaxy's own outer bound does not move, so a radius past it is out
    // of range rather than merely out of reach, and is corrected once and
    // kept.
    //
    // There is no such bound underneath. The reach follows the camera all the
    // way in (see [`crate::systems::reach_with_camera`]), so a radius under
    // the rail's own least is a real setting and not a mistake to be
    // corrected: writing the rail's least back would hold the sky within a
    // thousandth of a light year open every frame this pane is drawn, and put
    // the neighbours of a system flown into back on the map. Shown held to the
    // rail and left alone underneath, as a radius over the ceiling is.
    *radius = radius.min(Spyglass::CEILING);
    // Read before the rail borrows it, and the reason it is read at all.
    let speed = (*radius * RADIUS_DRAG).max(f32::EPSILON) as f64;
    fill_width(ui, VALUE_WIDTH);

    // What the widgets work on. They hold whatever they are given inside the
    // range, so this is the copy that gets held rather than the setting.
    let mut asked = radius.clamp(Spyglass::FLOOR, ceiling);

    let response = ui
        .horizontal(|ui| {
            let rail = ui.add(
                egui::Slider::new(&mut asked, Spyglass::FLOOR..=ceiling)
                    .logarithmic(true)
                    .show_value(false),
            );
            let box_ = value_box(
                ui,
                egui::DragValue::new(&mut asked)
                    .range(Spyglass::FLOOR..=ceiling)
                    .speed(speed),
            );
            rail.union(box_)
        })
        .inner;

    // Only where the figure was actually asked for. Anything else is the
    // ceiling having moved, which is not an answer to the question.
    if response.changed() {
        *radius = asked;
    }

    response
}

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
    /// [`chrome`] settles at the end of a frame from what it has just drawn.
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
    /// What [`crate::keys`] does with a slash, and with the two shifted keys
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

/// Everything the settings pane sets
///
/// One parameter rather than nine. A system may take only sixteen, and these
/// are all the same thing: what the pane is a pane of.
#[derive(SystemParam)]
pub(crate) struct Settings<'w> {
    spyglass: ResMut<'w, Spyglass>,
    view: ResMut<'w, View>,
    color_by: ResMut<'w, ColorBy>,
    population_scale: ResMut<'w, ScalePopulation>,
    star_exposure: ResMut<'w, StarExposure>,
    star_profile: ResMut<'w, StarProfile>,
    show_names: ResMut<'w, ShowNames>,
    poll: ResMut<'w, Poll>,
    name_radius: ResMut<'w, NameRadius>,
    name_limit: ResMut<'w, NameLimit>,
    show_orbits: ResMut<'w, ShowOrbits>,
    clock: ResMut<'w, Clock>,
    clock_control: ResMut<'w, ClockControl>,
    show_clock: ResMut<'w, ShowClock>,
    show_body_names: ResMut<'w, ShowBodyNames>,
    show_grid: ResMut<'w, ShowGrid>,
    unit: ResMut<'w, RulerUnit>,
    show_middle: ResMut<'w, ShowMiddle>,
    show_picked: ResMut<'w, ShowPicked>,
    bright: ResMut<'w, Bright>,
    bounded: ResMut<'w, crate::systems::bounded::LodFetch>,
    /// The commander's own journal, where the map was pointed at one
    ///
    /// The only optional thing in here, and the only one this pane reads
    /// rather than owns: the layer exists or it does not, decided by the
    /// environment before the window opened, and where it does not there is
    /// nothing for a control to be about. See [`crate::journal`].
    journal: Option<Res<'w, crate::journal::Journal>>,
}

/// The whole of the bar's filter section
///
/// One parameter for the same reason [`Settings`] is one: a system may take
/// only
/// sixteen. Grouped by what it is about rather than by where it is drawn,
/// since the bar's three sections have little to say to each other and this
/// way none of them can reach into another's state by accident.
///
/// The count comes from [`InReach`] rather than being taken over the systems
/// here, since what the bar has to say is how much of the sky in front of the
/// user is getting through, and only [`crate::systems::visibility`] knows
/// which systems those are.
#[derive(SystemParam)]
pub(crate) struct FilterBar<'w, 's> {
    /// The filters themselves, which the rows are drawn from and changed in
    ///
    /// Named for what it holds rather than for its type, since this is
    /// reached through a parameter that is already about filters and
    /// `filter.filters` says the word twice and the thing once.
    active: ResMut<'w, Filters>,
    /// How much of the sky is getting through them
    in_reach: Res<'w, InReach>,
    /// Whether systems are still being turned into stars, for the count's
    /// spinner
    spawning: Res<'w, PendingSpawns>,
    /// Whether systems are being dropped off the map, for the count's arrow
    evicting: Res<'w, PendingEvictions>,
    /// What is typed into the field that asks for one
    ///
    /// Here rather than among the bar's other fields, so that nothing about a
    /// filter is reachable through the search's state or the route's.
    input: Local<'s, Option<String>>,
    /// Where a filter the user has typed is sent to be looked up
    lookup: MessageWriter<'w, Lookup>,
    /// What became of the last one asked for
    note: ResMut<'w, LookupNote>,
    /// The factions the last name typed might have meant
    found: ResMut<'w, FactionResults>,
    /// Whether the name typed into it is still being looked up
    pending: Res<'w, Resolving>,
    /// How faintly what they exclude is drawn
    dim: ResMut<'w, DimTo>,
    /// Where the control over time stands
    watch: ResMut<'w, Watch>,
    standstill: ResMut<'w, Standstill>,
    /// Which filter the user is working with, which a click on a row says
    ///
    /// Only a route does anything with it today; the rest are picked out and
    /// nothing yet reads that they were.
    chosen: ResMut<'w, SelectedFilter>,
    /// What a filter's systems are, for framing them
    ///
    /// The two tables `Filter::systems` answers from. Read here rather than
    /// worked out in the bar, a row asking to see a filter whole being a
    /// question about where its systems are and not about the row.
    populated: Res<'w, crate::Populated>,
    names: Res<'w, crate::Names>,
}

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
        heading(ui, "Spyglass", false);
        // The spyglass is now a bound on the LoD walk rather than a source of
        // its own: enabled, the walk is clamped to the reach and only the near
        // sky draws; disabled, the whole sky draws, thinned by the level of
        // detail alone. What is under it only settles where that bound falls,
        // so it nests under the toggle the way the other sections nest theirs.
        // "Enable" is the spyglass's `clear`: to bound the view is to clear
        // away what the reach does not hold.
        check(
            ui,
            &mut settings.spyglass.clear,
            "Enable",
            "Only draw systems near the camera",
        );
        if settings.spyglass.clear {
            ui.indent("spyglass", |ui| {
                check(
                    ui,
                    &mut settings.spyglass.follow_camera,
                    "Follow Camera",
                    "Set the radius from the camera's zoom",
                );
                ui.add_space(FIELD_GAP);
                titled(
                    ui,
                    "Radius (Ly)",
                    "How far from the camera to draw systems",
                );
                // Greyed while the camera sets it: dragging it would be
                // overwritten on the next frame, and a control that springs
                // back is worse than one that says it is not yours to move.
                ui.add_enabled_ui(!settings.spyglass.follow_camera, |ui| {
                    radius_slider(
                        ui,
                        &mut settings.spyglass.radius,
                        Spyglass::CEILING,
                    );
                });
                // The camera cannot both be told where to stand and be asked
                // where it is standing, so the one that reads the camera hides
                // the one that writes it.
                if !settings.spyglass.follow_camera {
                    ui.add_space(FIELD_GAP);
                    check(
                        ui,
                        &mut settings.spyglass.lock_camera,
                        "Lock Camera",
                        "Stop the camera leaving the radius",
                    );
                }
            });
        }

        // The source and the map-wide actions apply whether or not the
        // spyglass bounds the view, so they stand outside it. LoD Fetch is the
        // source the spyglass now bounds; the two buttons are debug escapes.
        ui.add_space(FIELD_GAP);
        // The map's source: draw only what the walk marks, off the cell
        // payloads, in place of the spyglass region. On by default — it is what
        // ends the far-view entity explosion — and off falls back to the old
        // region fetch. Switching it clears the map and rebuilds from nothing.
        check(
            ui,
            &mut settings.bounded.0,
            "LoD Fetch",
            "Load systems by detail, not by whole regions",
        );
        // How often the map goes back for what it already holds. Out here
        // rather than under the spyglass because it is not the spyglass's:
        // `bodies::fetch` asks the inside of a system on it, and
        // `filter::mark` re-cuts the time filter on it, and neither has
        // anything to do with a region. Under the spyglass it was reachable
        // only while the bound was on and the region fetch with it, which hid
        // the one control that governs what the map does whichever source is
        // running.
        ui.horizontal(|ui| poll_value(ui, &mut settings.poll.0));

        // What belongs to both views, which is what this section is for and
        // what it is named for. The galaxy drawn as a map or as a sky, and a
        // system seen from inside it, are two views with their own sections
        // below; a switch that governs both filed under either would read as
        // turning off only that half of it.
        //
        // The labels are that: a name is drawn over a system out among the
        // stars and over a body within one, and one key turns both off (see
        // [`crate::keys`]). They stood in the two view sections, which is
        // where a reader who wanted the names off had to find them twice. The
        // ruling is the same argument — one ruled plane carries the map from
        // light years down to light seconds.
        heading(ui, "General", true);
        // Named apart, where the two sections named both of them "Show
        // Labels" and left the heading over each to say which was meant.
        // Together they have to say it themselves.
        check(
            ui,
            &mut settings.show_names.0,
            "System Names",
            "Show system names on the map",
        );
        if settings.show_names.0 {
            // Indented under what turns them on, since neither means anything
            // without it. The rule egui draws down the side of an indent says
            // as much, and says it without a heading standing over nothing
            // whenever the box is unchecked.
            ui.indent("names", |ui| {
                // The reach controls answer a map-view question — how far about
                // the center to name — and say nothing in the realistic view,
                // where a star earns its name by being bright enough to draw
                // rather than by standing near the center. See
                // [`crate::systems::labels::worth_placing`].
                if *settings.view == View::Map {
                    check(
                        ui,
                        &mut settings.name_radius.follow_spyglass,
                        "Names Follow Spyglass",
                        "Name systems out to the spyglass radius",
                    );
                    if !settings.name_radius.follow_spyglass {
                        // A name can only be drawn for a system that is drawn,
                        // and the spyglass decides that. One that is not
                        // clearing draws everything loaded, and then names may
                        // be asked for beyond its reach.
                        let ceiling = if settings.spyglass.clear {
                            settings.spyglass.radius
                        } else {
                            Spyglass::CEILING
                        };
                        titled(
                            ui,
                            "Name Radius (Ly)",
                            "How far from the center to show names",
                        );
                        radius_slider(
                            ui,
                            &mut settings.name_radius.radius,
                            ceiling,
                        );
                    }
                } else {
                    // The realistic view has no reach to hold names to, so it
                    // holds them to a brightness instead: named brightest
                    // first, down to this limiting magnitude. Turning it down
                    // names fewer of them, the way Name Radius names fewer in
                    // the map view. A star past the exposure's floor is not
                    // drawn and so cannot be named whatever this says.
                    titled(
                        ui,
                        "Name Limit (mag)",
                        "Only name stars brighter than this",
                    );
                    let mut mag = settings.name_limit.0;
                    fill_width(ui, VALUE_WIDTH);
                    let slider = ui
                        .horizontal(|ui| {
                            let rail = ui.add(
                                egui::Slider::new(&mut mag, -2.0..=12.0)
                                    .step_by(0.5)
                                    .show_value(false),
                            );
                            let typed = value_box(
                                ui,
                                egui::DragValue::new(&mut mag)
                                    .range(-2.0..=12.0)
                                    .speed(0.1)
                                    .suffix(" mag"),
                            );
                            rail | typed
                        })
                        .inner;
                    // Only when it lands somewhere new, as the exposure slider
                    // is, so a still slider does not mark the resource changed.
                    if slider.changed() && settings.name_limit.0 != mag {
                        settings.name_limit.0 = mag;
                    }
                }
            });
        }

        check(
            ui,
            &mut settings.show_body_names.0,
            "Body Names",
            "Show body names inside a system",
        );
        // The one reading among the switches, and here because it is drawn
        // over both views as the names and the ruling are: what the map is
        // standing at is as true inside a system as out among the stars.
        //
        // Turning it off is the map at the present, which the hint says
        // because the reading is the only place a run-on is shown and the
        // only way back from one.
        check(
            ui,
            &mut settings.show_clock.0,
            "Clock",
            "Show the date, and draw the map at the present",
        );
        ui.add_space(FIELD_GAP);
        check(ui, &mut settings.show_grid.0, "Grid", "Show the measuring grid");
        if settings.show_grid.0 {
            // Indented under what turns them on, the same as the names are,
            // since a unit for a ruler that is not drawn is a choice about
            // nothing. Left to the map by default, which turns the ruler over
            // as it descends into a system; pinned either way for reading a
            // system's distances in light years or a neighbourhood's in light
            // seconds.
            ui.indent("said", |ui| {
                check(
                    ui,
                    &mut settings.show_middle.0,
                    "Show Center Position",
                    "Show coordinates of the view center",
                );
                check(
                    ui,
                    &mut settings.show_picked.0,
                    "Show Selected Positions",
                    "Show coordinates of selected systems",
                );
                ui.add_space(FIELD_GAP);
                // How loudly the whole ruling is drawn, lines and numbers
                // together. Past a hundred for a ruler that has to be read off
                // a bright field, under it for one that should stay out of the
                // way of a busy sky.
                titled(ui, "Brightness (%)", "How bright the grid is drawn");
                let mut bright = settings.bright.0 * 100.;
                fill_width(ui, VALUE_WIDTH);
                let slider = ui
                    .horizontal(|ui| {
                        let rail = ui.add(
                            egui::Slider::new(&mut bright, 0.0..=100.)
                                .step_by(5.)
                                .show_value(false),
                        );
                        let typed = value_box(
                            ui,
                            egui::DragValue::new(&mut bright)
                                .range(0.0..=100.)
                                .suffix("%"),
                        );
                        rail | typed
                    })
                    .inner;
                // Only on a change. Written every frame it would mark the
                // resource changed every frame, and the planes are rebuilt
                // from it.
                if slider.changed() {
                    settings.bright.0 = bright / 100.;
                }
                ui.add_space(FIELD_GAP);
                titled(ui, "Units", "What the grid is measured in");
                choose(
                    ui,
                    &mut *settings.unit,
                    RulerUnit::Automatic,
                    "Automatic",
                    "Light years in space, light seconds in a system",
                );
                choose(
                    ui,
                    &mut *settings.unit,
                    RulerUnit::LightYears,
                    "Light Years",
                    "Always light years",
                );
                choose(
                    ui,
                    &mut *settings.unit,
                    RulerUnit::LightSeconds,
                    "Light Seconds",
                    "Always light seconds",
                );
            });
        }

        // The commander's own journal, where there is one to draw. Under
        // General because it is about the whole sky rather than about either
        // view of it, and drawn only where the map is reading a journal at
        // all: a switch for a layer that does not exist is a switch that
        // says the map could be showing something it has no way to show.
        //
        // Read through the toggle rather than a resource of its own, the
        // transport being what acts on it and an atomic being what the
        // transport reads. Written only on a change, as the brightness above
        // is: the toggle is a republish of everything the map holds.
        if let Some(journal) = &settings.journal {
            ui.add_space(FIELD_GAP);
            let mut on = journal.on.on();
            if check(
                ui,
                &mut on,
                "Your Own Journal",
                "Draw the systems from your own journal files",
            )
            .changed()
            {
                journal.on.set(on);
            }
        }

        // Which of the two ways the sky itself is drawn, and what each of
        // them offers. What is named over it went up to General, a name being
        // drawn either way.
        heading(ui, "Galaxy View", true);
        choose(
            ui,
            &mut *settings.view,
            View::Map,
            "Map",
            "Flat colored dots, one per system",
        );
        choose(
            ui,
            &mut *settings.view,
            View::Realistic,
            "Realistic",
            "Stars at their real color and brightness",
        );
        if *settings.view == View::Map {
            ui.add_space(FIELD_GAP);
            titled(ui, "Color By", "What a system's color means");
            choose(
                ui,
                &mut *settings.color_by,
                ColorBy::Allegiance,
                "Allegiance",
                "Color by controlling power",
            );
            choose(
                ui,
                &mut *settings.color_by,
                ColorBy::Government,
                "Government",
                "Color by government type",
            );
            choose(
                ui,
                &mut *settings.color_by,
                ColorBy::Security,
                "Security",
                "Color by security level",
            );
            ui.add_space(FIELD_GAP);
            check(
                ui,
                &mut settings.population_scale.0,
                "Scale w/ Population",
                "Size systems by population; hide empty ones",
            );
        }
        if *settings.view == View::Realistic {
            ui.add_space(FIELD_GAP);
            // The point-spread profile the stars wear: a Moffat with its wings
            // or a tighter Gaussian. Read into a local and written back only on
            // a change, so drawing the radios does not mark the resource changed
            // every frame and cut the texture again; see
            // [`crate::systems::spawn::reprofile`].
            titled(ui, "Point spread", "How a star's light blurs");
            let mut profile = settings.star_profile.0;
            for choice in ProfileKind::ALL {
                choose(
                    ui,
                    &mut profile,
                    choice,
                    choice.name(),
                    spread_hint(choice),
                );
            }
            if profile != settings.star_profile.0 {
                settings.star_profile.0 = profile;
            }
            ui.add_space(FIELD_GAP);
            // How many stops the star field is lifted to the display. From a
            // sky bright enough for only the most luminous stars, up through
            // the dark-adapted field at zero to several stops past it, where
            // the faint sky fills in.
            titled(ui, "Exposure (EV)", "How brightly the stars are exposed");
            let mut ev = settings.star_exposure.0;
            fill_width(ui, VALUE_WIDTH);
            let slider = ui
                .horizontal(|ui| {
                    let rail = ui.add(
                        egui::Slider::new(&mut ev, -12.0..=8.0)
                            .step_by(0.5)
                            .show_value(false),
                    );
                    let typed = value_box(
                        ui,
                        egui::DragValue::new(&mut ev)
                            .range(-12.0..=8.0)
                            .speed(0.1)
                            .suffix(" EV"),
                    );
                    rail | typed
                })
                .inner;
            // Only when it lands somewhere new, so a slider reporting the same
            // value frame after frame does not mark `StarExposure` changed and
            // trip every reader of it needlessly.
            if slider.changed() && settings.star_exposure.0 != ev {
                settings.star_exposure.0 = ev;
            }
        }

        // What is drawn once the camera is inside a system, rather than what
        // the galaxy is drawn as. Its own section for that reason, and not
        // under the view above it: which of the two ways the sky is drawn says
        // nothing about what a system looks like from within. The body names
        // went up to General with the system names, one key turning both off
        // and a reader wanting them off having had to find them twice.
        heading(ui, "System View", true);
        check(
            ui,
            &mut settings.show_orbits.0,
            "Orbit Lines",
            "Show the orbit each body follows",
        );

        // How the filters answer, rather than which they are: the filters
        // themselves are asked for in the bar, and this is the one thing
        // about them that is set once and left alone.
        heading(ui, "Filters", true);
        titled(
            ui,
            "Filtered Opacity (%)",
            "How faintly unmatched systems are drawn",
        );
        let mut showing = filter.dim.0 * 100.;
        fill_width(ui, VALUE_WIDTH);
        let slider = ui
            .horizontal(|ui| {
                let rail = ui.add(
                    egui::Slider::new(&mut showing, 0.0..=100.)
                        .step_by(5.)
                        .show_value(false),
                );
                let typed = value_box(
                    ui,
                    egui::DragValue::new(&mut showing)
                        .range(0.0..=100.)
                        .suffix("%"),
                );
                rail | typed
            })
            .inner;
        // Only when it lands on a value the resource does not already hold. The
        // step and the f32 round-trip can have the slider report a change frame
        // after frame at a value it is already at — 0.30 is not exactly
        // representable, so `dim.0 * 100` snapped back to `/ 100` never settles
        // — and writing that every frame marks the resource changed every
        // frame, which repaints every dimmed star and (through
        // `refetch_on_filter_change`) clears the surveys and refetches without
        // end.
        if slider.changed() {
            let set = showing / 100.;
            if filter.dim.0 != set {
                filter.dim.0 = set;
            }
        }
        // Which is a filter in the plainer sense: this kind of system and
        // none of the rest. At zero the excluded are not dimmed but dropped —
        // never loaded, and evicted if already on the map — so the sign is that
        // they are not there rather than that they are faint.
        if filter.dim.0 == 0. {
            ui.label(egui::RichText::new("Not loaded").weak());
        }
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
    let center = orbit.single().map(|camera| camera.center).ok();
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
        &mut bar.how,
        &mut bar.drive,
        &bar.boosts,
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

/// Slide the settings pane in from the left, and draw `contents` in it
///
/// Answers how far its right edge has reached, which is where the gear
/// stands. Zero while the pane is shut, so the gear sits in the corner and
/// rides the pane's edge as it comes out.
///
/// An [`egui::Area`] rather than a panel, so that the pane travels in from
/// off the viewport rather than growing in place, and because every top-level
/// `Panel::show` is deprecated with nothing at the top level to replace it.
fn settings_pane(
    ctx: &Context,
    open: bool,
    contents: impl FnOnce(&mut Ui),
) -> f32 {
    // Asked for every frame, shown or not, since this is what advances the
    // slide and answers when it has finished.
    let out = ctx.animate_bool(egui::Id::new("settings-pane"), open);
    if out == 0. {
        return 0.;
    }

    let height = ctx.content_rect().height();
    let style = ctx.global_style();
    // Square, since three of its four sides are off the viewport, and edged
    // so that the one that is not reads against the map behind it.
    let frame = egui::Frame::side_top_panel(&style)
        .stroke(style.visuals.window_stroke())
        .shadow(style.visuals.window_shadow);
    let margins = frame.total_margin().sum();

    zone("settings-pane")
        .fixed_pos(egui::pos2((out - 1.) * PANE_WIDTH, 0.))
        .show(ctx, |ui| {
            frame.show(ui, |ui| {
                ui.set_width(PANE_WIDTH - margins.x);
                ui.set_height(height - margins.y);
                // The bar stands beside what it scrolls rather than over it,
                // so that the width asked for below is the width there is.
                // Floated, it would be drawn across the right hand end of
                // every slider in the pane.
                ui.spacing_mut().scroll.floating = false;
                egui::ScrollArea::vertical().show(ui, contents);
            });
        })
        .response
        .rect
        .right()
}

/// The handle on the settings pane, alone in the corner it opens from
///
/// Bare, so that what stands in the corner is a gear rather than a box with a
/// gear in it. It rides the pane's edge at `left`, since a handle the pane
/// slides over is a handle the user cannot reach.
///
/// `middle` is where the bar's search box sits, and the gear is hung about it
/// rather than dropped from the top of the viewport as the bar is. The two
/// stand side by side, so what lines them up is the field the user is looking
/// at rather than the top edge of a box the field is padded inside.
///
/// It is given [`GEAR_ROOM`] across, that being what the bar leaves for it.
/// Measured instead, the room would not be known until the gear had been
/// drawn, and the gear cannot be drawn until the bar has said where its search
/// box is.
fn gear(ctx: &Context, left: f32, middle: f32, open: &mut bool) {
    let style = ctx.global_style();
    let clicked = zone("settings-gear")
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(egui::pos2(left + MARGIN, middle))
        .show(ctx, |ui| {
            let mut gear = egui::RichText::new("⚙").size(GEAR_SIZE);
            if *open {
                gear = gear.color(style.visuals.strong_text_color());
            }
            ui.add_sized(
                egui::vec2(GEAR_ROOM, 0.),
                egui::Button::new(gear).frame(false),
            )
            .clicked()
        })
        .inner;

    if clicked {
        *open = !*open;
    }
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
/// asks, and [`crate::keys`] and [`crate::systems::selection`] both go
/// through it, so a fourth thing that drops out of the chrome is put away by
/// the code that already puts away these two.
///
/// Not [`crate::systems::info::Panels`], which is the windows the map opens
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
    /// [`BINDINGS`] says all three at length, in the window a reader opens to
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

    /// What choosing it does, said on hover. See [`check`].
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
/// and the press bookkeeping at the end of [`chrome`] weighs both against
/// where the pointer was.
struct Asked {
    /// Where the field sits, which is the height the gear is hung at
    middle: f32,
    /// The whole card, for weighing a press that landed off it
    rect: egui::Rect,
    /// What the readings under the bar stand below
    ///
    /// The card's own foot while the form is out, so the rows stand clear of
    /// the frame around it. The field's foot while it is not: what the card
    /// comes to there is the field plus the padding of a frame drawn in
    /// nothing, and a line of text held off the box by a border that is not
    /// being painted reads as a reading that belongs to something else.
    foot: f32,
    /// The fields the bar drew, which a press off the chrome lets go of
    ///
    /// The one box, and the route's stops field beside it where that mode is
    /// out; the same box twice where it is not, letting go of a field being
    /// the same gesture however often it is asked for.
    boxes: [egui::Id; 2],
    /// Whether the field has just taken the caret, which is what opens the
    /// form
    took_focus: bool,
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

/// Draw `contents` with what is chosen in it marked in grey
///
/// A tab, and the switch under the clock's reading, are not selected systems.
/// Egui marks a chosen one in the color it marks a selection in, and in this
/// chrome that color means picked out on the map: it is the ring around a
/// star and the dot on a row, and nothing else. So a chosen one is filled in
/// the grey the rest of the chrome is drawn in and says which it is by being
/// filled at all.
///
/// Scoped, since a style set on a `Ui` is set on the rest of that `Ui`: the
/// same color is what egui highlights selected text with, and a field typed
/// into below would lose its own.
fn greyed<R>(ui: &mut Ui, contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.scope(|ui| {
        let strong = ui.visuals().strong_text_color();
        let filled = ui.visuals().widgets.active.weak_bg_fill;
        let visuals = ui.visuals_mut();
        visuals.selection.bg_fill = filled;
        visuals.selection.stroke.color = strong;

        contents(ui)
    })
    .inner
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
/// [`state_bar`], and the moment is [`time_strip`]. The three were one column
/// and read as one thing that would not stop growing: a reading that is
/// always true sat between two forms that are usually not out, and the rows
/// saying what the map holds sat inside the frame of a form asking about
/// something else.
///
/// `asking` is whether the search box's answer is late enough to say so,
/// which the clock the question was put by settles: the bar draws during
/// egui's own pass and has no clock of its own.
#[allow(clippy::too_many_arguments)]
fn ask_bar(
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
    how: &mut Routing,
    drive: &mut Drive,
    boosts: &crate::Boosts,
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
                        Picking::Gathers,
                        selection,
                        panels,
                        camera,
                    );
                    let range = route_body(
                        ui, search, selection, searched, plot, how, drive,
                        boosts, searching,
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
/// [`whole_selection`]'s to say and the caller's to act on: what answers it is
/// a mode of the bar above.
#[allow(clippy::too_many_arguments)]
fn state_bar(
    ctx: &Context,
    left: f32,
    top: f32,
    selection: &mut Selection,
    contents: &Contents,
    center: Option<DVec3>,
    panels: &mut Panels,
    camera: &mut MessageWriter<MoveCamera>,
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
    // two. See [`Filter::stops`] and [`crate::systems::filter::trip_stops`].
    //
    // The click and only the click. This hung off the panel the info button
    // opens to begin with, which meant asking what a trip was made of picked
    // its stops out as a side effect — a button that quietly did the other
    // button's job.
    if let Some((stops, gathering)) = row_ask.picked {
        selection.pick_out(
            stops.iter().filter_map(|address| {
                crate::systems::spawn::system_at(
                    *address,
                    &filter.populated,
                    &filter.names,
                )
                .map(Picked::System)
            }),
            gathering,
        );
    }

    // Which filter is being worked with. Every kind can be picked out, and
    // only a route reads that it was: `route::active` weighs what is picked
    // against the routes being shown, so a faction picked out leaves the
    // routes as they were rather than standing in front of them.
    if let Some(chosen) = row_ask.chosen {
        filter.chosen.0 = Some(chosen);
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
            crate::systems::route::spawn::framing(&places)
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

/// How wide the strip stands while the scrubber is out
///
/// A little wider than the reading it is opened from, which comes to some
/// 250 points with a span and a `Now` beside the date, and wide enough for
/// the spans [`marks`] writes under the rail without them running together.
/// It was drawn at 620 to begin with — nearly twice the bar — on the
/// argument that a logarithmic rail spends its decades over its own width, so
/// every pixel taken off it is a coarser instrument. True, and beside the
/// point: at that width the reading sat in a third of a box and the rail was
/// a bare grey bar across the top of the map. Exactness on the rail is not
/// what the far end of it is for, and the marks are what make a coarse rail
/// readable.
///
/// A number rather than what the reading leaves over. The rail is scaled by
/// the room it is in, and room measured off a line that grows with the value
/// the rail last set is a control whose scale is a function of its own value.
const STRIP_WIDTH: f32 = 420.;

/// How wide the scrubber's rail runs
///
/// The strip's own width, which is what [`time_strip`] sets the row the rail
/// stands in to: the frame's margins are outside that, so the rail fills the
/// row rather than stopping short of it. A number for the reason
/// [`clock_control`] gives at length — a rail scaled by the room a growing
/// reading leaves is a rail scaled by its own value — so it is written down
/// beside the width it comes from rather than measured where it is used.
const RAIL_WIDTH: f32 = STRIP_WIDTH;

/// When the map is standing, at the top of the viewport
///
/// Its own zone, in the middle of the top edge. The moment is the galaxy's:
/// it is true whatever the bar is being asked and whether or not the camera is
/// inside a system, so it is read where a reading of that kind belongs rather
/// than filed inside the corner card that comes and goes. In the bar it moved
/// down the screen every time a form dropped out above it.
///
/// Bare while the map stands at the present, which is how it opens: a weak
/// line of text over the sky and nothing else. Clicking it drops the scrubber
/// out under it, and then it takes the frame — the same [`Dropping`] the bar
/// is drawn in, and put away by the same gestures: see [`Pane`].
///
/// Answers where it stood, which is what
/// `the_strip_gives_way_to_the_bar_on_a_narrow_window` reads. Change
/// detection is the caller's: what the scrubber writes is a moment, and
/// whether the clock moved has nothing to do with where the reading of it is
/// drawn.
fn time_strip(
    ctx: &Context,
    chrome_right: f32,
    clock: &mut Clock,
    control: &mut ClockControl,
    turns: Turns,
) -> egui::Rect {
    Dropping {
        id: "time-strip",
        standing: Standing::Middle { beside: chrome_right },
        out: control.out,
        width: STRIP_WIDTH,
        holds_width: false,
    }
    .show(ctx, |ui| dated(ui, clock, turns, control))
    .response
    .rect
}

/// What the scrubber may be geared to
///
/// Both turns rather than the better of them, since which is wanted is the
/// reader's to say: a planet's own year is the span that says something about
/// the planet, and the system's widest orbit is the span in which the whole
/// arrangement has been through every shape it takes. See [`GearedTo`], which
/// is what the strip asks and this answers.
///
/// Either may be missing. There is no body's turn without a body picked out
/// in the system the map is holding, and no system's turn where no orbit in
/// it has a period on record.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub(crate) struct Turns {
    /// One turn of the body picked out
    body: Option<f64>,
    /// One turn of the widest orbit the system has on record
    system: Option<f64>,
}

impl Turns {
    /// What the rail covers, given which of the two is asked for
    ///
    /// The other where the one asked for is not on record, so that a rail is
    /// offered wherever there is any turn to cover: the choice is which of
    /// two spans to read, and it is nothing to do with whether the map can be
    /// run on at all.
    fn geared(self, to: GearedTo) -> Option<Geared> {
        let body = self.body.map(Geared::Body);
        let system = self.system.map(Geared::System);
        match to {
            GearedTo::Body => body.or(system),
            GearedTo::System => system.or(body),
        }
    }

    /// Whether there is a choice to offer
    ///
    /// Both, or there is nothing to choose between and a switch that reads as
    /// two ways of asking for the same rail.
    fn choice(self) -> bool {
        self.body.is_some() && self.system.is_some()
    }
}

/// Which turn the scrubber's rail is asked to cover
///
/// The body picked out by default, that being the narrower of the two and the
/// one the reader said something about by picking it: a rail over the whole
/// system's widest orbit moves a planet by whole years at a nudge.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(crate) enum GearedTo {
    /// One turn of the body picked out, laid evenly
    #[default]
    Body,
    /// One turn of the system's widest orbit, laid by decades
    System,
}

impl GearedTo {
    /// The two, in the order the switch stands them in
    ///
    /// The narrower first, as the strip reads left to right and as the rails
    /// themselves run.
    const ALL: [GearedTo; 2] = [GearedTo::Body, GearedTo::System];

    /// What the switch calls it
    fn said(self) -> &'static str {
        match self {
            GearedTo::Body => "Body",
            GearedTo::System => "System",
        }
    }

    /// What choosing it does, said on hover. See [`check`].
    fn hint(self) -> &'static str {
        match self {
            GearedTo::Body => "Cover one turn of the body picked out",
            GearedTo::System => "Cover one turn of the system's widest orbit",
        }
    }
}

/// The turns the map has to offer, where the camera is standing
///
/// A body's turn is only a body's turn while the map is holding the system it
/// is in: what is picked out survives a flight and its period does not follow
/// it out of the system it was scanned in.
fn turns_of(selection: &Selection, contents: &Contents) -> Turns {
    Turns {
        body: selection
            .newest_body()
            .filter(|(address, _)| contents.of() == Some(*address))
            .and_then(|(_, id)| contents.turn_of(id)),
        system: contents.slowest_turn(),
    }
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
fn let_go_of(ctx: &egui::Context, boxes: [egui::Id; 2]) {
    ctx.memory_mut(|memory| {
        for box_id in boxes {
            memory.surrender_focus(box_id);
        }
    });
}

/// The whole of the bar's searching
///
/// What was asked, what came back, and whether an answer is late. Gathered as
/// the filters are: the bar is drawn by one system, a system may take sixteen
/// things, and the bar asks about more than sixteen.
#[derive(SystemParam)]
pub(crate) struct SearchBar<'w> {
    /// Where a name typed into a field is sent to be looked up
    search: MessageWriter<'w, Search>,
    /// What to say about a name that found nothing
    note: ResMut<'w, SearchNote>,
    /// The systems the search box found
    results: ResMut<'w, SearchResults>,
    /// What the search box has out
    pending: Res<'w, Searching>,
    /// How the route last asked for is getting on
    plot: ResMut<'w, Plot>,
    /// Which of the fewest-jumps routes to ask for
    how: ResMut<'w, Routing>,
    drive: ResMut<'w, Drive>,
    /// Whether the index publishes a supercharge table at all, which is what
    /// a route for a supercharging drive needs before it can be asked for
    boosts: Res<'w, crate::Boosts>,
    searching: Res<'w, Frontiers>,
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
fn ask_box(
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

/// Take away everything standing as an answer to the name in the box
///
/// The query, the note about a name that resolved to nothing, and the list of
/// what it might have meant. One gesture takes all three because they are one
/// answer: a list left standing under an empty box answers a question that is
/// no longer on screen to be read.
fn cleared(
    value: &mut Option<String>,
    note: &mut SearchNote,
    results: &mut SearchResults,
) {
    *value = None;
    note.0 = None;
    results.clear();
}

/// How many of what a search found the bar shows before the list scrolls
///
/// A screenful of the bar rather than of the viewport. The list hangs under
/// the input with the map behind it, and one long enough to reach the bottom
/// of the screen would answer which system did you mean by covering over the
/// sky the answer is about.
const OFFERED: usize = 5;

/// What the map can be asked to do with one system
///
/// The same ones the map itself answers, wherever the line was drawn: one
/// click says which system is meant, a modifier held with it says as well as
/// the rest, and a second click says to go there. Shared by every list of
/// systems the map draws, so that reaching one through a search and reaching
/// one through a filter are the same gesture rather than two to be learned.
pub(crate) enum SystemAction {
    /// Pick the system out, as clicking a star does
    ///
    /// `gathering` holds the modifier rather than a variant of its own,
    /// because holding it does not ask for something else. It is the one
    /// gesture either way, saying which system is meant; all the modifier
    /// says is whether the rest are meant along with it.
    Select { gathering: bool },
    /// Send the camera to it, as double clicking a star does
    Travel,
    /// Say what is known about it, and leave the selection alone
    Describe,
}

/// Whether a key is down asking for as well as rather than instead
///
/// The same gesture the sky answers, so that a line in the list and the star
/// it names are picked out the same way. Command covers control where the
/// user came from Windows or Linux and the cloverleaf where they came from a
/// Mac, and shift stands beside them as the one no platform reads as asking
/// for something else.
///
/// The same three [`crate::systems::spawn`] asks the keyboard for directly.
pub(crate) fn gathering(ui: &Ui) -> bool {
    ui.input(|input| {
        let keys = input.modifiers;
        keys.command || keys.ctrl || keys.shift
    })
}

/// Act on what a line was asked for
///
/// Apart from the drawing, since a list draws every line before any of them
/// is acted on: picking one out changes what the lines are drawn from. Which
/// makes it the piece worth asking about on its own.
fn act_on(
    action: SystemAction,
    system: &NameEntry,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::systems::System>,
) {
    // The names table holds only placed systems, so a listed one always has
    // somewhere to be. Its political columns fill in when a fetch draws it.
    let placed = crate::systems::System::from(system);
    match action {
        SystemAction::Select { gathering } => {
            selection.pick(Picked::System(placed), gathering);
        }
        SystemAction::Travel => {
            *travelled = Some(crate::systems::system_to_vec(system))
        }
        SystemAction::Describe => *described = Some(placed),
    }
}

/// The systems the last search found, for the user to choose between
///
/// Every search is answered here, whether the user typed part of a name or the
/// whole of one. A name spelled out in full leads the list rather than being
/// picked out on its own: the search says which systems are on record under
/// that name and the click says which of them is meant, and a search that
/// picked something out would let go of whatever had been gathered before it.
/// This stands where the note would, the two never appearing together, since a
/// search either found systems to list or found nothing and says so.
///
/// The list is left standing once something is picked out of it. Choosing is
/// most of what it is for, and a list that puts itself away as soon as it is
/// touched makes trying the second candidate a matter of typing the whole
/// query again.
///
/// A line answers the gestures a star answers. A plain click picks that system
/// out in place of the rest — or gathers it, where `picking` says the list is
/// being read for stops; see [`Picking`] — a click with ctrl, command or
/// shift held gathers it up alongside them and lets go of one already held,
/// and a double click sends the camera there. Gathering reaches across
/// searches: the list goes when the next name is typed and what was picked
/// out of it stays, so a set can be built a name at a time.
///
/// Every system listed can be picked. The resident table is names and
/// positions in one, so an entry is a placed system by construction and there
/// is no line here the camera cannot be sent to.
///
/// Each line carries the info mark the rows in the bar carry, opening what is
/// known about that system without picking it out. That is how a list of
/// candidates is read: several are opened and compared while the selection
/// stays wherever the user left it, which is the whole point of being handed
/// several rather than one.
///
/// `travelled` is where a line asked the camera to go and `described` is what
/// a line asked to be written out, both of which the caller acts on rather
/// than this: a message writer cannot be had outside a system, and a list that
/// reports what it was asked for can be drawn in a test.
///
/// Drawn only while the box is asking about a name, which is [`ask_bar`]'s to
/// say: the search asks one, and so does the route, which runs between the
/// systems a name is asked about. One left standing under a shut form — or
/// under a box that has since been turned to asking for a faction — is an
/// answer to a question that is no longer on screen. What was found is kept,
/// so opening the form again is where they left off rather than a search to
/// do a second time.
///
/// Unlike the rows in the state bar, which stand whether or not the form is
/// out. A selection and a filter outlive the asking and go on saying what the
/// map is doing; a list of candidates is the asking itself.
fn found(
    ui: &mut Ui,
    results: &SearchResults,
    center: Option<DVec3>,
    picking: Picking,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::systems::System>,
) {
    if results.is_empty() {
        return;
    }

    let Some((system, action)) =
        system_list(ui, results.iter(), center, "result")
    else {
        return;
    };
    act_on(picking.of(action), system, selection, travelled, described);
}

/// What a plain click on a line the search found means
///
/// The gestures are the same either way — a modifier gathers, a double click
/// flies there, the mark opens what is known — and what differs is what the
/// plain click on its own does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Picking {
    /// Stand in for whatever was picked out, unless a modifier says otherwise
    ///
    /// Which is how a star on the map answers a click, and what a search is:
    /// one name asked about, and the system meant picked out.
    Asked,
    /// Gather it up alongside them, modifier or not
    ///
    /// What the route asks. A trip is two or more systems, so a click that
    /// let go of the last stop to take the next could never build one, and
    /// holding a modifier down to add the thing the form exists to add is a
    /// form that argues with itself.
    Gathers,
}

impl Picking {
    /// What a line's gesture comes to under this reading of a plain click
    fn of(self, action: SystemAction) -> SystemAction {
        match (self, action) {
            (Picking::Gathers, SystemAction::Select { .. }) => {
                SystemAction::Select { gathering: true }
            }
            (_, action) => action,
        }
    }
}

/// What came back of the name in the box: the systems found, or that none was
///
/// The two never stand together — a search either found systems to list or
/// found nothing and says so — and neither is drawn under a box asking about
/// something else. Both modes that ask about a name draw this: the search,
/// where a click picks a system out, and the route, where it adds a stop.
/// See [`Picking`].
///
/// Where a line asked the camera to go and what it asked to be written out
/// are acted on here rather than handed back, both of them being messages
/// this has the writers for.
#[allow(clippy::too_many_arguments)]
fn answer(
    ui: &mut Ui,
    note: &SearchNote,
    results: &SearchResults,
    center: Option<DVec3>,
    picking: Picking,
    selection: &mut Selection,
    panels: &mut Panels,
    camera: &mut MessageWriter<MoveCamera>,
) {
    if let Some(note) = &note.0 {
        ui.colored_label(egui::Color32::LIGHT_RED, note);
    }

    let mut travelled = None;
    let mut described = None;
    found(
        ui,
        results,
        center,
        picking,
        selection,
        &mut travelled,
        &mut described,
    );
    if let Some(position) = travelled {
        camera.write(MoveCamera { position: Some(position), framing: None });
    }
    if let Some(system) = described {
        panels.open_system(system);
    }
}

/// A list of systems, and what a click asked of one of them
///
/// Every list of systems the map offers to be chosen from is this, the search
/// results among them. What a click means is the caller's, since that is the
/// only part that differs between one list and another, and the lines
/// themselves are [`system_line`] so that a system is read the same way
/// wherever it is listed.
///
/// Scrolls past [`OFFERED`], which is a screenful of the bar rather than of
/// the viewport: the list hangs over the map and one long enough to reach the
/// bottom of the viewport answers a question by covering up what it is about.
///
/// `center` is where distances are measured from, and nothing where the camera
/// has yet to say. With nothing to measure from a line carries no distance
/// rather than one measured from somewhere else.
///
/// `salt` keys one list's lines apart from another's. Within a list they are
/// keyed by place rather than by which system a line is about, as the rows in
/// the bar are keyed and for the reason given there: a fresh search leaves the
/// lines where they were and makes every one of them about something else.
pub(crate) fn system_list<'a>(
    ui: &mut Ui,
    systems: impl Iterator<Item = &'a NameEntry>,
    center: Option<DVec3>,
    salt: &str,
) -> Option<(&'a NameEntry, SystemAction)> {
    // Nothing found is nothing drawn, rather than an empty list taking a
    // line's worth of room under the field that has yet to be asked.
    let mut systems = systems.peekable();
    systems.peek()?;

    let height = ui.text_style_height(&egui::TextStyle::Body)
        + LINE_PADDING * 2.
        + ui.spacing().item_spacing.y;

    // Settled after the list is drawn, since what a click asks for is usually
    // a change to what the lines are being drawn from.
    let mut chose = None;

    scrolling(ui, height * OFFERED as f32, salt, |ui| {
        for (index, system) in systems.enumerate() {
            let at = crate::systems::system_to_vec(system);
            // How far off it is, where there is anywhere to measure from.
            let trailing =
                center.map(|center| format!("{:.1} Ly", center.distance(at)));
            let asked = system_line(ui, &system.name, trailing, (salt, index));
            if let Some(asked) = asked {
                chose = Some((system, asked));
            }
        }
    });

    chose
}

/// Say what is picked out, and how far off it is
///
/// The status of the selection, which is not what the search box holds. The
/// box is a query, and a query answers with however many systems match it,
/// so it can never stand for the one system picked out. This says which that
/// is, in its own words, and goes on being right when a star is clicked on
/// the map and the box still holds whatever was last typed into it.
///
/// It is also what says a search worked. A search that resolves picks its
/// system out, and that shows up here.
///
/// The line is also the control that sends the camera to what is picked out:
/// double clicking the answer to go to what it names beats a button saying so
/// in words, and the dot in the ring's own color says which mark out on the
/// map it is about. The same press flies to a star on the map and frames a
/// filter's systems, so a row reads the way the thing it stands for does.
///
/// Measured from where the camera is looking rather than from the camera
/// itself, since that is the distance the spyglass and the fetch are
/// measured in: a system nearer than the spyglass radius is one that is
/// drawn.
/// Several picked out are several rows, each about one of them, so that no
/// row has to answer which system it means. Five of them and then scrolling,
/// as the results list is, and for the same reason: the bar hangs over the
/// map and a list long enough to reach the bottom of the viewport answers a
/// question by covering up what it is about.
///
/// The summary line above them is drawn only while more than one is picked
/// out, and carries [`whole_selection`]'s controls. One system picked out is
/// the case
/// the rows already read well, and a line saying "1 system" over a row naming
/// it says the same thing twice.
/// `travelled` is where a row asked the camera to go, which the caller writes
/// rather than this, as [`found`] does and for the same reason. A whole move
/// rather than a place: a row says where alone, and the summary line's control
/// to frame the set says how much to take in as well.
///
/// Answers whether a route between what is picked out was asked for, which is
/// [`whole_selection`]'s to say and the caller's to act on: the form that
/// answers it
/// is drawn further down the bar.
#[allow(clippy::too_many_arguments)]
fn selected(
    ui: &mut Ui,
    selection: &mut Selection,
    contents: &Contents,
    center: Option<DVec3>,
    travelled: &mut Option<MoveCamera>,
    panels: &mut Panels,
    filters: &mut Filters,
    place: &mut usize,
) -> bool {
    if selection.is_empty() {
        return false;
    }

    let gap = ui.spacing().item_spacing.x;
    // Settled after the rows, since each is drawn from the same selection it
    // asks to change.
    let mut chose = None;
    // Where the column had reached, which is where what these rows spend of
    // it is counted from. See the end of this function.
    let from = *place;

    let routing = selection.len() > 1
        && whole_selection(ui, selection, filters, travelled);

    let height = ui.text_style_height(&egui::TextStyle::Body).max(DOT)
        + (ROW_PADDING + ROW_MARGIN) * 2.
        + ui.spacing().item_spacing.y;
    let mut rows = |ui: &mut Ui| {
        for index in 0..selection.len() {
            let Some(held) = selection.get(index) else { continue };

            // How far off it is, measured from the focus for both kinds and
            // said in whatever unit suits the range. A body inside a system
            // stands light seconds away where a system stands light years, and
            // either given in the other's unit is a number with too many
            // digits to read at a glance.
            //
            // Nothing else about a body stands on its row. What kind of thing
            // it is, and everything else on record, is the panel's to say.
            let beside =
                selection.position(index).zip(center).map(|(at, focus)| {
                    let away = focus.distance(at);
                    match held {
                        Picked::System(_) => format!("{away:.1} Ly"),
                        Picked::Body(_) => format!(
                            "{:.1} Ls",
                            crate::space::light_seconds(away)
                        ),
                    }
                });

            // Laid out and painted rather than assembled from labels. A label
            // is a widget in its own right, and two of them under one
            // clickable row leave three widgets bidding for the pointer: the
            // row answers over the gaps and the labels answer over the words,
            // so it flickers between being a control and not as the pointer
            // crosses them.
            let away = beside.map(|line| {
                egui::WidgetText::from(egui::RichText::new(line).weak())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Extend),
                        f32::INFINITY,
                        egui::TextStyle::Body,
                    )
            });

            let buttons = lay_out_buttons(ui);
            let name = held.name();

            // Whatever the dot, the distance and the marks leave the name.
            // System names run to "Col 285 Sector XY-Z b12-34", and one laid
            // out against no bound at all is painted straight out past the
            // edge of the bar.
            let room = ui.available_width()
                - ROW_PADDING * 2.
                - DOT
                - gap
                - buttons_width(&buttons, gap)
                - away.as_ref().map_or(0., |away| away.size().x + gap);
            let name =
                egui::WidgetText::from(egui::RichText::new(name).strong())
                    .into_galley(
                        ui,
                        Some(egui::TextWrapMode::Truncate),
                        room.max(0.),
                        egui::TextStyle::Body,
                    );
            // Keyed on where the row sits in the bar rather than on what it
            // holds, and counted on from whatever came before it rather than
            // from this list's own first row. See [`row_of`].
            let of = ("bar-row", *place);
            *place += 1;
            let (outer, row) = row_of(
                ui,
                // The width the rest of the form is laid out in, so that the
                // row lines up with the fields above and below it rather than
                // being measured against anything of its own.
                name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
                of,
            );
            let rect = outer.shrink2(egui::vec2(0., ROW_MARGIN));

            if row.hovered() || row.has_focus() {
                ui.painter().rect_filled(
                    rect,
                    ui.visuals().widgets.hovered.corner_radius,
                    ui.visuals().widgets.hovered.weak_bg_fill,
                );
            }
            let middle = rect.center().y;
            let mut x = rect.left() + ROW_PADDING;
            ui.painter().circle_filled(
                egui::pos2(x + DOT / 2., middle),
                DOT / 2.,
                SELECTION_DOT,
            );
            x += DOT + gap;
            for galley in [Some(name), away].into_iter().flatten() {
                let size = galley.size();
                // The galleys carry the colors they were laid out in, so
                // there is nothing for a fallback to answer for.
                ui.painter().galley(
                    egui::pos2(x, middle - size.y / 2.),
                    galley,
                    egui::Color32::PLACEHOLDER,
                );
                x += size.x + gap;
            }

            let Buttons { info, close } = place_buttons(ui, rect, buttons, of);

            let asked = asked_of_selection(
                close.clicked(),
                info.is_some_and(|info| info.clicked()),
                row.double_clicked(),
                row.clicked(),
            );
            if let Some(asked) = asked {
                chose = Some((index, asked));
            }
            row.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    };

    // Only once there are more than the bar holds. A scroll area around three
    // rows is a scroll area that never scrolls and takes a little room off
    // the end of every one of them for a bar that is not there.
    if selection.len() > SELECTED {
        scrolling(ui, height * SELECTED as f32, "selection", &mut rows);
    } else {
        rows(ui);
    }

    // What the column spent on them, which is what the rows below it are
    // numbered from, and which is not how many rows were drawn: past
    // [`SELECTED`] they are drawn inside a scroll area of a fixed height, so
    // the seventh system picked out moves nothing below it.
    //
    // The count has to move with the rows below rather than with the rows
    // here, that being the whole of what it is for. Gathering another system
    // while the list scrolls would otherwise put a fresh id at a filter row
    // that kept its rectangle, which is what egui reads as one widget taking
    // another's state: it says so out loud and paints the row red.
    //
    // The rows drawn inside the scroll area are numbered on past this and
    // come to no harm by it. They are keyed within the scroll area's own
    // `Ui`, so what they are numbered has nothing to say about anything
    // drawn outside it.
    *place = from + selection.len().min(SELECTED);

    if let Some((index, action)) = chose {
        match action {
            SelectionAction::Travel => {
                *travelled = selection.position(index).map(|position| {
                    MoveCamera { position: Some(position), framing: None }
                })
            }
            // Whatever the row is about. A system is described from the row
            // the bar holds; what is inside one is described from the rows the
            // map is holding, which it has for as long as the thing is drawn,
            // and a row for one is only held for that long either.
            SelectionAction::Describe => match selection.get(index) {
                Some(Picked::System(system)) => {
                    panels.open_system(system.clone())
                }
                Some(Picked::Body(body)) => {
                    if let Some(star) = contents.star(body.id()) {
                        panels.open_star(star.clone());
                    } else if let Some(row) = contents.body(body.id()) {
                        panels.open_body(row.clone());
                    }
                }
                None => {}
            },
            SelectionAction::LetGo => selection.remove(index),
        }
    }

    routing
}

/// What the bar can be asked to do with one selected system
///
/// Said by index, several rows standing at once and each being about one of
/// them.
#[derive(Clone, Copy, Debug, PartialEq)]
enum SelectionAction {
    /// Send the camera to it, as double clicking its star does
    Travel,
    /// Open the panel describing it
    Describe,
    /// Let go of this one, and hold the rest
    LetGo,
}

/// What a press on one selected system's row asked of it
///
/// The same reading [`asked_of_row`] gives a filter's row, which is where the
/// order is written down: the mark, then the button, then the double, then
/// the click. A row here has no switch inside it -- what it stands for is
/// picked out by standing there at all -- so it is the one place a row can be
/// pressed that this leaves out.
///
/// The camera is what a double asks for, as a double on the star itself asks,
/// rather than what a single click asks as it used to. A row about one system
/// has the whole of that system in view already, so the gesture that frames a
/// filter is a flight to this one.
///
/// A single click asks for nothing. On a filter's row it says which of several
/// is the one being worked with; here there is nothing for it to say, the row
/// standing for something already picked out. Left as a gesture with no
/// answer rather than given the camera back, so that the same press means the
/// same thing wherever in the bar it lands.
fn asked_of_selection(
    close: bool,
    info: bool,
    double: bool,
    click: bool,
) -> Option<SelectionAction> {
    match asked_of_row(close, info, false, double, click) {
        Some(RowGesture::LetGo) => Some(SelectionAction::LetGo),
        Some(RowGesture::Describe) => Some(SelectionAction::Describe),
        Some(RowGesture::Frame) => Some(SelectionAction::Travel),
        Some(RowGesture::Toggle | RowGesture::Select) | None => None,
    }
}

/// How many selected systems the bar shows before the rows start scrolling
const SELECTED: usize = 5;

/// One row standing for everything picked out, and what it offers
///
/// Says how many there are and offers to bring the map to bear on them.
/// [`whole_set`] is the same shape over the filter rows.
///
/// The set is left alone once it has been filtered on. The filter took a copy
/// of the addresses, so letting go of the rings and the rows afterwards
/// leaves those systems picked out, which is most of what the filter is for.
///
/// Answers whether a route through them was asked for. A route wants two of
/// them at the least, so the control appears the moment the second is picked
/// and stays as more are gathered. That is what there is to find: a set
/// gathered out on the map says here what can be done with it, rather than
/// leaving the user to guess that the form dropping out of the search box has
/// a section about the systems they have already picked.
///
/// It reaches the form rather than plotting, since a route still wants a jump
/// range and there is nowhere here to say one.
///
/// And offers to frame the lot: to stand the camera back over the middle of
/// what is picked out, far enough to take all of it in. A space walks the set
/// one at a time and says nothing about how far out to stand, and a set
/// gathered across the galaxy is one the user wants to see whole before
/// walking it. Systems and bodies alike, a body being somewhere as much as a
/// system is. Not offered where they all stand in one place, there being
/// nothing to stand back from and a frame over nothing being a camera pulled
/// in to a metre.
fn whole_selection(
    ui: &mut Ui,
    selection: &Selection,
    filters: &mut Filters,
    travelled: &mut Option<MoveCamera>,
) -> bool {
    // The systems alone, [`Filter`] naming systems by address and testing a
    // [`System`]. A body is counted among what is picked out, and there is as
    // yet no filter for it to build.
    let picked = selection.systems().count();
    let mut routing = false;
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(format!("{picked} systems")).weak());
        // Offered only where there is a system to filter on. A set of bodies
        // builds a filter over no addresses, which admits nothing, and the map
        // fetches by the same answer it dims by: the sky goes black under a
        // row saying none was picked. Widen this when a filter can name a
        // body, rather than dropping it.
        if picked > 0 && ui.button("Filter").clicked() {
            filters.add(Filter::Systems {
                label: format!("{picked} systems"),
                systems: selection.addresses(),
            });
        }
        if stops_of(selection).is_ok() {
            routing = ui.button("Route").clicked();
        }
        if let Some((middle, extent)) = spanned(selection)
            && ui.button("Frame").clicked()
        {
            *travelled = Some(MoveCamera {
                position: Some(middle),
                framing: Some(extent),
            });
        }
    });
    routing
}

/// The middle of everything picked out, and how far it reaches from there
///
/// Nothing where it reaches nowhere: one thing alone, or several standing in
/// the one place, which is a place to fly to rather than a span to take in.
///
/// The same [`crate::systems::route::spawn::framing`] a plotted route is
/// framed by, so a set and the route through it are stood back from the same
/// way.
fn spanned(selection: &Selection) -> Option<(DVec3, f32)> {
    let places: Vec<DVec3> = (0..selection.len())
        .filter_map(|index| selection.position(index))
        .collect();
    let (middle, extent) = crate::systems::route::spawn::framing(&places)?;

    (extent > 0.).then_some((middle, extent))
}

/// A count with its digits grouped in threes
///
/// A population runs to eleven digits and a count of the sky to six, and
/// either is a length rather than a number until it is broken up.
pub(crate) fn thousands(count: u64) -> String {
    let digits = count.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (place, digit) in digits.char_indices() {
        if place > 0 && (digits.len() - place).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

/// How a route is written: the two systems it runs between, in order
pub(crate) const ARROW: &str = " -> ";

/// What stands where a name was cut short
///
/// Two stops rather than an ellipsis, an ellipsis being one character that
/// reads as three and a name being cut to make room in the first place.
const CUT: &str = "..";

/// Say a route in `room` characters, keeping both of its ends
///
/// A route is named for where it starts and where it ends, and either name is
/// long on its own: `SIGMA DRACONIS -> MINISTRY` is twenty six characters.
/// Cut from the right, as a widget cuts a line too long for it, what goes is
/// the end the route was plotted to reach, and every route out of one system
/// is then called the same thing.
///
/// So the room left over by the arrow is halved between them, and an end that
/// does not want its half leaves the rest to the other. What is not a route
/// is handed back whole, there being no second end to keep and whoever draws
/// it having its own way of cutting a line that does not fit.
///
/// An odd character over goes to the name that leads, that being the one read
/// first, and the two ends are otherwise given exactly as much as each other.
///
/// Counted in characters, which is a width now that everything is lettered in
/// one.
pub(crate) fn shortened(label: &str, room: usize) -> String {
    let Some((start, end)) = label.split_once(ARROW) else {
        return label.to_owned();
    };
    if label.chars().count() <= room {
        return label.to_owned();
    }

    let names = room.saturating_sub(ARROW.chars().count());
    // An end cut below a character and the mark saying it was cut is an end
    // that says nothing, and two of those either side of an arrow say only
    // that a route runs between two systems. Where it comes to that, what
    // room there is goes to the name that leads.
    if names < (CUT.chars().count() + 1) * 2 {
        return clipped(label, room);
    }

    let (start_wants, end_wants) = (start.chars().count(), end.chars().count());
    let half = names / 2;
    let (start_gets, end_gets) = if start_wants <= half {
        (start_wants, names - start_wants)
    } else if end_wants <= half {
        (names - end_wants, end_wants)
    } else {
        (names - half, half)
    };

    format!("{}{ARROW}{}", clipped(start, start_gets), clipped(end, end_gets))
}

/// Say `name` in `room` characters
///
/// Every character there is room for, cut wherever the room runs out. Backing
/// up to the word before it would read better and say less: system names are
/// told apart by their tails, `COL 285 SECTOR SC-K B22-2` from `COL 285 SECTOR
/// XY-Z A1-0`, so a name cut back to `COL 285..` is a name that no longer says
/// which one it is. A trailing space goes, being a character that says
/// nothing.
///
/// What is left ends in [`CUT`], so a name that was cut says as much. Room
/// enough for nothing but that mark is answered with as much of it as there
/// is room for: a column of them is at least a column.
fn clipped(name: &str, room: usize) -> String {
    if name.chars().count() <= room {
        return name.to_owned();
    }
    if room <= CUT.chars().count() {
        return CUT.chars().take(room).collect();
    }

    let cut = room - CUT.chars().count();
    let kept: String = name.chars().take(cut).collect();

    format!("{}{CUT}", kept.trim_end())
}

/// How wide one character of `kind` stands
///
/// Everything is lettered in one width, so a character is a measure of room
/// as much as a pixel is.
pub(crate) fn one_character(ctx: &Context, kind: egui::TextStyle) -> f32 {
    let font = kind.resolve(&ctx.global_style());
    ctx.fonts_mut(|fonts| fonts.glyph_width(&font, 'M'))
}

/// How many characters of `kind` fit in `room` pixels
///
/// Exact, and exactly what [`shortened`] is measured in, everything being
/// lettered in one width.
pub(crate) fn characters(
    ctx: &Context,
    kind: egui::TextStyle,
    room: f32,
) -> usize {
    let one = one_character(ctx, kind);
    if one <= 0. {
        return 0;
    }
    (room / one).floor().max(0.) as usize
}

/// How many characters of `kind` it takes to cover `room` pixels
///
/// [`characters`] the other way about. As many as fit stops short of the
/// room whenever the room is not a whole number of characters, which is for
/// whoever is filling a space rather than reading what is in it.
pub(crate) fn covering(
    ctx: &Context,
    kind: egui::TextStyle,
    room: f32,
) -> usize {
    let one = one_character(ctx, kind);
    if one <= 0. {
        return 0;
    }
    (room / one).ceil().max(0.) as usize
}

/// What a field holds, if it holds anything
///
/// A field clicked into and not yet typed in holds an empty string, which is
/// not something the user has said. Neither is a line of spaces.
fn typed(field: &Option<String>) -> Option<&str> {
    field.as_deref().map(str::trim).filter(|text| !text.is_empty())
}

/// The jump range asked for, or what is wrong with what was asked
fn jump_range(asked: &str) -> Result<f64, &'static str> {
    match asked.trim().parse::<f64>() {
        Ok(range) if range > 0. => Ok(range),
        Ok(_) => Err("Jump range must be more than nothing"),
        Err(_) => Err("Jump range must be a number of light years"),
    }
}

/// What the map needs before it can plot at all, or what is missing
///
/// Two preconditions, and the second is why this stands beside
/// [`jump_range`] rather than inside it. A range is what the user typed; a
/// supercharge table is what the index published, and a route for a drive
/// that can take a jet cone is a question the map cannot answer without one.
///
/// It used to answer anyway. The router read an absent table as a galaxy
/// where nobody has a jet cone and handed back the unaided route — under the
/// supercharged drive's name, since a route carries what it was plotted with
/// and its panel says so. So the map claimed to have plotted something it had
/// not, and the only tell was a jump count that looked high. Said instead,
/// with the command that fixes it.
///
/// An empty table is not this. A published table with nothing in it is an
/// answer — there is nowhere to supercharge — and the unaided route is the
/// right one.
fn plotting(
    asked: &str,
    drive: Drive,
    boosts: &crate::Boosts,
) -> Result<f64, &'static str> {
    let range = jump_range(asked)?;
    if drive.named().is_some() && !boosts.published() {
        return Err("No supercharge table in the index. Rebuild it with \
             `galos-sync db --only boosts`, or let the sync that writes \
             it publish once more, or plot unaided.");
    }
    Ok(range)
}

/// The systems a route runs through, or why it has nowhere to run
///
/// What is picked out on the map, in the order it was picked, rather than
/// names typed into fields of the form. Picking a system out is already how
/// the user says which one they mean, to the panel that describes it and to
/// the filter built from it, so a route with fields of its own would be
/// asking twice about systems the map is holding for them.
///
/// Two of them at the least. A longer set is a route through every system in
/// it, leg by leg in the order it was picked: the first is where the flying
/// starts, and each after it is where the next leg is plotted to.
fn stops_of(selection: &Selection) -> Result<Vec<&str>, &'static str> {
    // The systems alone. A route runs between places, and a body picked out
    // beside them is a thing inside one rather than a stop to plot to.
    let stops: Vec<&str> =
        selection.systems().map(|system| system.name()).collect();
    match stops.len() {
        0 => Err("Pick two or more systems"),
        1 => Err("Pick one more system"),
        _ => Ok(stops),
    }
}

/// How wide the systems picked out stand, in light years
///
/// The distance between the two that stand furthest apart, which is the
/// widest the set is by any reading: no two are further, and no one system
/// lies outside a sphere that wide.
///
/// Not [`spanned`]'s reach, which is what the camera stands back by and what
/// a system's shell is drawn to over the orbits inside it. That is measured
/// from the middle of the *box* the systems fill, and the box's middle moves
/// when the set changes -- so dropping a system can leave the rest further
/// from the new middle than any of them were from the old one, and the figure
/// would climb as the set shrank. Measured over pairs it cannot: taking a
/// system away takes its pairs with it and leaves the rest as they were.
///
/// Every pair, which is one comparison for each. A hand-picked set runs to
/// tens rather than thousands, and the alternative is a hull.
///
/// This rather than the legs added up. A route has not been asked for yet, so
/// how far one would run is not knowable: it turns on the order the stops are
/// reached in, and for the cheapest order that turns on a jump range which
/// may not have been typed. How wide the set stands needs none of it, and is
/// the question a set of destinations raises.
///
/// Nothing where fewer than two are picked out: no pair, nothing to be wide.
fn across(selection: &Selection) -> Option<f64> {
    let places: Vec<DVec3> =
        selection.systems().map(|system| system.position()).collect();

    places
        .iter()
        .enumerate()
        .flat_map(|(index, from)| {
            places[index + 1..].iter().map(move |to| from.distance(*to))
        })
        .max_by(f64::total_cmp)
}

/// The order the trip will be flown in, as indices into what is picked out
///
/// The order they were picked, or the cheapest order to reach them all in
/// where that was asked for and a range is in hand to cost a leg with —
/// which is what `cheapest` carries. The ordering itself is
/// [`crate::systems::route::tour`]'s, and `shape` is what it is told about
/// the trip: which end is held, and whether the leg home is costed with the
/// rest.
///
/// One place decides it, so what the form says the trip comes to and what the
/// trip is actually asked for cannot disagree.
fn flown_order(
    selection: &Selection,
    shape: Shape,
    cheapest: Option<f64>,
) -> Vec<usize> {
    let places: Vec<DVec3> =
        selection.systems().map(|system| system.position()).collect();
    match cheapest {
        Some(range) => {
            crate::systems::route::tour::ordered(&places, range, shape)
        }
        None => (0..places.len()).collect(),
    }
}

/// The stops as the trip is to be flown, named
///
/// The order they were picked, or the cheapest order to reach them all in
/// where that was asked for and a range is in hand to cost a leg with. The
/// ordering is [`crate::systems::route::tour`]'s; this is only the naming.
///
/// A ring names its first stop twice, at both ends. The stops are what the
/// legs are cut from — a leg to each name from the one before it — so the
/// leg home is a leg like any other: asked for, walked, drawn and costed.
/// Nothing else about a trip has to know a ring from a line.
///
/// Handed back as names rather than as an order, since names are what a trip
/// is asked for with and what the legs are keyed by.
fn asked_in_order(
    stops: &[&str],
    selection: &Selection,
    shape: Shape,
    cheapest: Option<f64>,
) -> Vec<String> {
    // The systems `stops_of` named, in the same order, so an index into one
    // is an index into the other.
    let mut named: Vec<String> = flown_order(selection, shape, cheapest)
        .into_iter()
        .filter_map(|at| stops.get(at))
        .map(|stop| stop.to_string())
        .collect();

    if shape.loops()
        && let Some(home) = named.first().cloned()
    {
        named.push(home);
    }
    named
}

/// How many legs a trip through `stops` is flown in
///
/// The gaps between the stops, which is one fewer than there are of them —
/// and one apiece for a ring, which has the leg home as well.
fn legs_flown(stops: usize, shape: Shape) -> usize {
    match shape.loops() {
        true => stops,
        false => stops.saturating_sub(1),
    }
}

/// What shape a trip through `stops` is asked for in
///
/// The form holds two flags and they come to one shape, which is what
/// [`crate::systems::route::tour`] is told and what says how many legs the
/// trip is. One reading of them, so the count the form says, the order the
/// stops go out in, and the legs actually plotted cannot disagree.
///
/// A ring only where there is a trip to close. Two stops flown out and back
/// is the one leg twice over, drawn on top of itself, so [`route_body`] does
/// not offer the control — and a flag left standing from a wider set is not
/// obeyed here either, a control nobody can see being no way to let go of
/// one.
///
/// A ring holds its start whatever the other flag says: every way round one
/// costs the same, so a free start is not a choice about cost, and the form
/// puts that control away while a loop is asked for. See [`Shape`].
fn shape_of(fields: &BarFields, stops: usize) -> Shape {
    match (fields.looping && stops > 2, fields.any_start) {
        (true, _) => Shape::Loop,
        (false, true) => Shape::Anywhere,
        (false, false) => Shape::FromFirst,
    }
}

/// What the form says of the systems picked out, before a route is asked for
///
/// A pair is so far apart, which is the whole of what there is to say about
/// two: a route between them runs from the one to the other however it gets
/// there.
///
/// More than two is said as how many legs it will be and how wide they
/// stand — [`legs_flown`], so a ring counts the leg home among them. Not how
/// far the route will run: that turns on the order they are reached in, and
/// the order may turn on a range not yet typed. How much sky they cover is
/// knowable before anything is walked, and is what a set of destinations
/// raises.
fn apart_said(away: f64, legs: usize) -> String {
    match legs {
        0 | 1 => format!("{away:.1} Ly apart"),
        legs => format!("{legs} legs, {away:.1} Ly across"),
    }
}

/// What the route asks, under the box that names its stops
///
/// The bar's one box is the search box in this mode as in that one — a route
/// runs between systems, and systems are found by name — so what it found
/// stands above this and a click on a line adds a stop. See [`ask_bar`] and
/// [`Picking`].
///
/// Which leaves the range, which is asked for here: it is one of the route's
/// settings rather than the question the bar is putting, and it stands with
/// them, above the two that say how the route is to be worked out. It led the
/// form to begin with, in the bar's own box, which cost the mode the box:
/// there was nowhere left to name a system, and the stops were whatever the
/// map already had picked out.
///
/// Answers the range's field, which the caller needs for the same reason it
/// needs the box's: a caret left in either takes the keys the map flies with.
/// See [`Asked::boxes`].
///
/// Which systems the route runs through is [`stops_of`]'s to settle, off what
/// is picked out — by a click on the map, on a row, or on a name the box
/// found.
///
/// How it is getting on is said between the settings and the button, where
/// what it is about is on either side of it.
#[allow(clippy::too_many_arguments)]
fn route_body(
    ui: &mut Ui,
    search: &mut BarFields,
    selection: &Selection,
    searched: &mut MessageWriter<Search>,
    plot: &mut Plot,
    how: &mut Routing,
    drive: &mut Drive,
    boosts: &crate::Boosts,
    searching: &Frontiers,
) -> Response {
    // Which systems it runs through is not said here. They are the rows in
    // the state bar below, named there and in that order, and a form that
    // spelled them out again would say the same thing twice -- at six stops,
    // in a line of names longer than the bar is wide. What is missing is
    // said, though: a route wants two, and the box above this is where a
    // second one is found.
    let stops = stops_of(selection);
    if let Err(why) = &stops {
        // Weakly. Nothing has gone wrong: the user is part way through
        // asking, and a form in red before it has been filled in is a form
        // scolding whoever fills it in.
        ui.label(egui::RichText::new(*why).weak());
    }
    // What shape the trip is flown in: which end is held, and whether it
    // comes home. Read here rather than beside the controls that set it, so
    // that what the form says the trip comes to, what the ordering is asked
    // for, and what is actually plotted are the one answer.
    let shape = shape_of(search, stops.as_ref().map_or(0, Vec::len));

    // What the map can say about them before a route is asked for: how many
    // legs it will be, and how wide they stand. Nothing about the order they
    // will be reached in, which is what the range settles and what nothing
    // here waits on.
    if let (Ok(stops), Some(away)) = (&stops, across(selection)) {
        let legs = legs_flown(stops.len(), shape);
        ui.label(egui::RichText::new(apart_said(away, legs)).weak());
    }
    ui.add_space(FIELD_GAP);

    // How far the ship jumps unaided, which is the one number a route cannot
    // be worked out without. Nothing stands under it as an answer, so
    // clearing it takes the range and nothing else.
    let (box_, emptied) =
        ask_box(ui, &mut search.route_range, RANGE_WANTED, false, false);
    if emptied {
        search.route_range = None;
    }
    ui.add_space(FIELD_GAP);

    // How hard the map should work at it. Two of the three are the fewest
    // jumps and differ in whether the map may spend the time proving the
    // shortest of them; the third declines to prove anything and comes back
    // in a hundredth of the time. What each gives up is said on hover rather
    // than in the label, a route being something the reader either has an
    // opinion about or does not. See `Routing` for what they measured out at.
    egui::ComboBox::from_label("Search")
        .selected_text(match *how {
            Routing::Quick => "Quick",
            Routing::Direct => "Direct",
            Routing::Shortest => "Shortest",
        })
        .show_ui(ui, |ui| {
            // One line each, as the pane's hints are: what taking it gets
            // you, in the words the rows use. What each of them gives up in
            // exchange is [`Routing`]'s to say at length.
            for (mode, said, hint) in [
                (
                    Routing::Quick,
                    "Quick",
                    "Fastest to find, up to 5% more jumps",
                ),
                (Routing::Direct, "Direct", "Fewest jumps, straightest path"),
                (
                    Routing::Shortest,
                    "Shortest",
                    "Fewest jumps, shortest distance, slowest to find",
                ),
            ] {
                ui.selectable_value(&mut *how, mode, said).on_hover_text(hint);
            }
        });
    // Whether a jet cone counts, and what it is worth. A neutron star
    // supercharges a drive for one jump — four times the range, six off the
    // drive built for it — so a route that may use one runs through the
    // neutron stars on the way rather than in the ship's own reach. The range
    // typed above stays what the ship does unaided; this is what a boost
    // multiplies it by. See `Drive`.
    egui::ComboBox::from_label("Supercharging")
        .selected_text(match *drive {
            Drive::Unaided => "None",
            Drive::Standard => "Standard (x4 / x1.5)",
            Drive::Optimised => "SCO Mk II (x6 / x3)",
        })
        .show_ui(ui, |ui| {
            // The multiples are in the names, so a hint says what they are
            // multiples of rather than saying them twice.
            for (fitted, said, hint) in [
                (
                    Drive::Unaided,
                    "None",
                    "No boosts: every jump is the ship's range",
                ),
                (
                    Drive::Standard,
                    "Standard (x4 / x1.5)",
                    "Boost off neutron stars and white dwarfs",
                ),
                (
                    Drive::Optimised,
                    "SCO Mk II (x6 / x3)",
                    "Bigger boosts off the same stars",
                ),
            ] {
                ui.selectable_value(&mut *drive, fitted, said)
                    .on_hover_text(hint);
            }
        });

    // Return in the range asks for the route, as pressing the button does. It
    // is the last thing a route waits on, and a form with one thing left to
    // do should not have to be reached for.
    let submitted = entered(&box_, ui);
    // What came back of the last route asked for answers the range as it was
    // then, so it goes as soon as it is not. Work still under way is not an
    // answer to anything yet, and stays.
    if box_.changed() && matches!(*plot, Plot::Failed(_)) {
        *plot = Plot::Nothing;
    }

    // How the last route asked for is getting on. Only ever a route that
    // was asked for: a field being typed into is not an attempt at
    // anything, and a form that answers back before it has been submitted
    // is a form scolding whoever fills it in.
    if let Plot::Failed(trouble) = &*plot {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, trouble);
    }

    // What shape the trip takes. Only where there is a trip to shape: two
    // stops have one order and one leg between them either way round, and
    // three or more picked out are as likely to be a set of destinations as
    // an itinerary.
    if stops.as_ref().is_ok_and(|stops| stops.len() > 2) {
        check(
            ui,
            &mut search.looping,
            "Loop",
            "Fly home to the first stop at the end",
        );
        check(
            ui,
            &mut search.tour,
            "Cheapest order",
            "Reorder the stops to fly the least",
        );
        // Only under the box it qualifies, and not under a ring: where the
        // order is the user's own there is nothing to hold the start
        // against, and a ring has no free end to hold — every way round one
        // costs the same, so where it is entered is not a choice about cost.
        if search.tour && !search.looping {
            ui.indent("start", |ui| {
                let mut from_first = !search.any_start;
                if check(
                    ui,
                    &mut from_first,
                    "Start w/ First Selected System",
                    "Keep the first stop as the start",
                )
                .changed()
                {
                    search.any_start = !from_first;
                }
            });
        }
    }

    ui.add_space(FIELD_GAP);
    // The two things a route is made of: which systems it runs through, and
    // what it may be flown in. The button is dead until both are in hand,
    // since a plot missing one of them is nothing to ask the router about.
    let asked = stops.ok().zip(typed(&search.route_range));
    // Egui lays a button's contents out as atoms, and a custom atom is a
    // slot of a given size that hands its rect back to be painted into. So
    // the spinner takes a place in the row beside the label rather than
    // being painted over the top of it, and asks for no room at all on a
    // button that has nothing to say.
    let slot = ui.id().with("plotting");
    let mut atoms = egui::Atoms::new("Plot Route");
    if *plot == Plot::Working {
        let turning = ui.text_style_height(&egui::TextStyle::Button) * SPINNER;
        atoms.push_left(egui::Atom::custom(slot, egui::Vec2::splat(turning)));
    }
    let button = ui
        .add_enabled_ui(asked.is_some(), |ui| {
            egui::Button::new(atoms).atom_ui(ui)
        })
        .inner;
    // A route is worked out against a database that takes as long as it
    // takes, and a button that has gone quiet says nothing about whether it
    // heard.
    if let Some(turning) = button.rect(slot) {
        egui::Spinner::new().paint_at(ui, turning);
    }
    // How far the search has got. A route across the galaxy expands hundreds
    // of thousands of systems over several seconds, and a spinner says the map
    // is working without saying whether it is getting anywhere. The map draws
    // the same progress out on the sky; this is the number beside the button.
    let expanded = searching.expanded();
    if *plot == Plot::Working && expanded > 0 {
        // How much has been looked at, and how close it has got. The second is
        // the one that answers the question a wait asks: the map draws the
        // chain to that system out on the sky, and this is how far it still
        // has to go.
        let said = match searching.closest() {
            Some(away) => format!(
                "{} systems searched, {} Ly to go",
                crate::ui::thousands(expanded),
                crate::ui::thousands(away.round() as u64),
            ),
            None => {
                format!("{} systems searched", crate::ui::thousands(expanded))
            }
        };
        ui.label(egui::RichText::new(said).weak());
    }

    if (button.response.clicked() || submitted)
        && let Some((stops, range)) = asked
    {
        *plot = match plotting(range, *drive, boosts) {
            Ok(range) => {
                searched.write(Search::Route {
                    how: *how,
                    stops: asked_in_order(
                        &stops,
                        selection,
                        shape,
                        search.tour.then_some(range),
                    ),
                    // Back to text, since a route is fetched under a key
                    // made of what was asked for and a float is no kind of
                    // key.
                    range: range.to_string(),
                    drive: *drive,
                });
                Plot::Working
            }
            Err(trouble) => Plot::Failed(trouble.to_owned()),
        };
    }

    box_
}

/// Say which filters are being applied, and how much is getting through
///
/// Drawn whether or not the form is out. A filter changes what the whole map
/// looks like and outlives the asking, so it has to be readable from the
/// closed bar: a sky gone dim with nothing to say why is a map that looks
/// broken.
///
/// Each row is the control that turns its own filter off, so one can be
/// lifted to see what it was hiding and put back without being typed again.
/// The mark at the end takes it away for good. Over two or more, [`whole_set`]
/// stands above them and says both of those things about all of them at once.
///
/// Drawn in sections, one to a [`Section`], each with its own count standing
/// over it. A route is a line across the map and a faction is a way of reading
/// the sky, and a column that ran the two together said "3 filters" over a
/// heap of both and gave the user nowhere to turn all of one kind off.
///
/// Laid out and painted rather than assembled from widgets, as the selection
/// row is and for the same reason. A checkbox and a button under one row are
/// three things bidding for the pointer, and it flickers between being a
/// control and not as the pointer crosses them.
///
/// How much of the sky is getting through them is said by [`reaching`], which
/// stands under these and is drawn whether or not any of them is.
fn applied(
    ui: &mut Ui,
    filters: &mut Filters,
    panels: &mut Panels,
    place: &mut usize,
) -> RowAsk {
    let mut ask = RowAsk::default();
    if filters.is_empty() {
        return ask;
    }

    // Settled after the sections are drawn, since the rows are drawn from the
    // same filters they change.
    let mut toggling = None;
    let mut removing = None;
    let mut opening = None;
    let mut whole: Option<(FilterAction, Section, Vec<usize>)> = None;

    for (place_of, section) in Section::all(filters).into_iter().enumerate() {
        let rows = section.rows(filters);
        if rows.is_empty() {
            continue;
        }

        // Above the rows it stands for, where a heading stands, and over two
        // or more of them: one row already says everything a count of one
        // could, and the control over it would do what that row's own does.
        //
        // A trip keeps its row whatever it holds. It is named rather than
        // counted, so the row says something no leg of it says, and a trip
        // whose legs have not all landed yet would otherwise appear as a
        // heap of routes and then gather itself up.
        if (rows.len() > 1 || matches!(section, Section::Trip(_)))
            && let Some(asked) = whole_set(
                ui,
                &section.said(rows.len()),
                section.on(filters),
                matches!(section, Section::Trip(_)),
                place,
            )
        {
            whole = Some((asked, section.clone(), rows.clone()));
        }

        // A trip's legs are drawn in from its row, so the block reads as one
        // trip with its legs under it rather than as a row and then some
        // routes. The other sections are counts of things that stand on their
        // own and are not drawn in from anything.
        let mut rows_of = |ui: &mut Ui| {
            section_rows(
                ui,
                filters,
                &rows,
                place,
                &mut toggling,
                &mut removing,
                &mut opening,
                &mut ask,
            );
        };
        match section {
            Section::Trip(_) => {
                ui.indent(("trip-legs", place_of), |ui| rows_of(ui));
            }
            _ => rows_of(ui),
        }
    }

    if let Some(index) = toggling {
        filters.toggle(index);
    }
    if let Some(index) = removing {
        filters.remove(index);
    }
    if let Some(filter) = opening {
        panels.open_filter(filter);
    }
    match whole {
        // Every stop of the set, in the order they are flown, each once. A
        // trip is legs and a leg is two ends, so the stop one leg lands on is
        // where the next sets out from and is one stop rather than two.
        Some((FilterAction::Select(gathering), _, rows)) => {
            let legs: Vec<Filter> = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
            ask.picked =
                Some((crate::systems::filter::trip_stops(&legs), gathering));
        }
        Some((FilterAction::Toggle, _, rows)) => filters.toggle_all(&rows),
        // Every filter of the section at once, so what the camera stands back
        // to take in is all of them together rather than each in turn.
        Some((FilterAction::Frame, _, rows)) => {
            ask.framed_all = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
        }
        // The trip as one route, which is what a panel about it is about. Its
        // legs are what it is made of and each has a panel of its own.
        Some((FilterAction::Describe, Section::Trip(trip), rows)) => {
            let legs: Vec<Filter> = rows
                .iter()
                .filter_map(|index| filters.get(*index))
                .map(|active| active.filter.clone())
                .collect();
            ask.described =
                as_one(&trip, &rows, filters).map(|whole| (whole, legs));
        }
        Some((FilterAction::Describe, ..)) => {}
        Some((FilterAction::LetGo, _, rows)) => filters.clear(&rows),
        None => {}
    }

    ask
}

/// What one press on a filter's row meant
///
/// Five things can be pressed in the space of a row, and a press lands on
/// exactly one of them. Kept apart from the drawing because the order is the
/// whole of it: [`asked_of_row`] is where that order is written down and the
/// only place it can be got wrong.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum RowGesture {
    /// Take the filter away for good
    LetGo,
    /// Open the panel describing it
    Describe,
    /// Turn it off, or back on
    Toggle,
    /// Send the camera to see the whole of what it admits
    Frame,
    /// Say it is the one being worked with
    Select,
}

/// Which of them a press on a row was
///
/// The mark first, then the switch, then the double. Egui answers the first
/// click of a pair as a click and the second as a double, so a row double
/// clicked has already been asked about as a click by the time this is
/// reached: the double has to beat the click or framing a filter would pick
/// it out on the way, which is the same order the selection rows are read in.
///
/// The switch beats the row for the plainer reason that it stands inside it.
/// A press on the dot is a press on the row as well, and it means the dot.
///
/// Read here by everything a press can land on that stands for one filter or
/// one system: the bar's rows, the sections over them, and the legs a trip's
/// panel lists. A row that offers fewer of the five says so by passing
/// `false`, rather than keeping an order of its own.
pub(crate) fn asked_of_row(
    close: bool,
    info: bool,
    switch: bool,
    double: bool,
    click: bool,
) -> Option<RowGesture> {
    if close {
        Some(RowGesture::LetGo)
    } else if info {
        Some(RowGesture::Describe)
    } else if switch {
        Some(RowGesture::Toggle)
    } else if double {
        Some(RowGesture::Frame)
    } else if click {
        Some(RowGesture::Select)
    } else {
        None
    }
}

/// Whether a click on `row` is a click, and not the first half of a double
///
/// egui raises `clicked()` on the first release of a double click, a frame
/// before anything reports the double at all — measured: frame one
/// `clicked` alone, frame two `clicked` and `double_clicked` together. So
/// [`asked_of_row`]'s priority, which arbitrates the flags of one pass and
/// gets frame two right, has already been handed frame one and acted on it.
///
/// Which matters wherever [`RowGesture::Select`] does something a double
/// click is not supposed to do. It replaces what is picked out, so
/// double clicking a route's row to fly to it first wiped whatever the user
/// was holding and put the route's own ends there instead — and the double
/// then framed it, over a selection it had no business changing.
///
/// So the click is held for the window a double may still arrive in, and
/// answered only once it has passed. A double inside it takes the pending
/// click away with it. The wait is egui's own `max_double_click_delay`,
/// three tenths of a second, and it is the wait a double click already costs
/// the gesture it is not.
///
/// What comes back is the modifiers of the *press*, since that is what the
/// gesture meant and the hand is off the key by the time the window has
/// passed. See [`gathering_with`].
///
/// Kept in egui's own per-id store rather than in a resource: it is one
/// press per row, it belongs to the row, and it goes when the row does.
/// A row must be drawn to be answered — a pending click on a row that stops
/// being listed is never returned, which is the same as the row's press
/// never having been read.
pub(crate) fn settled_click(
    ui: &Ui,
    row: egui::Id,
    click: bool,
    double: bool,
) -> Option<egui::Modifiers> {
    let pending = row.with("click awaiting a double");
    let now = ui.input(|input| input.time);

    if double {
        ui.data_mut(|data| data.remove::<(f64, egui::Modifiers)>(pending));
        return None;
    }
    if click {
        let keys = ui.input(|input| input.modifiers);
        ui.data_mut(|data| data.insert_temp(pending, (now, keys)));
        return None;
    }
    let held: Option<(f64, egui::Modifiers)> =
        ui.data(|data| data.get_temp(pending));
    let waited = ui.ctx().options(|it| it.input_options.max_double_click_delay);
    match held {
        Some((since, keys)) if now - since >= waited => {
            ui.data_mut(|data| data.remove::<(f64, egui::Modifiers)>(pending));
            Some(keys)
        }
        _ => None,
    }
}

/// Whether a press meant "and these as well", read off the press itself
///
/// [`gathering`] asks the frame it is called in, which is right for a gesture
/// acted on where it lands. A click held to see whether a double follows is
/// not: by the time it is answered the modifier has been let go of, and the
/// press would read as a bare one. So the keys travel with the pending click
/// and are asked here.
pub(crate) fn gathering_with(keys: egui::Modifiers) -> bool {
    keys.command || keys.ctrl || keys.shift
}

/// What a press on a filter's row asked of it, beyond what the row settles
///
/// The two that reach past the filters themselves: which one the user means,
/// and where they want the camera. Handed back rather than acted on here, as
/// the selection rows hand back what they were asked, since neither is the
/// row's own business to carry out.
#[derive(Default)]
struct RowAsk {
    /// The filter a click picked out as the one being worked with
    chosen: Option<Filter>,
    /// The filter a double click asked to see the whole of
    framed: Option<Filter>,
    /// The trip a section's row asked for a panel about
    ///
    /// The route it is flown as, and the legs it is made of: the first is
    /// what the panel is about and the second is how its list is broken up.
    described: Option<(Filter, Vec<Filter>)>,
    /// Every filter of a section, where its own row asked to see them all
    ///
    /// Apart from [`Self::framed`] because it is a set rather than one of
    /// them: the camera stands back to take in all of them together, which is
    /// not where it would stand for any one.
    framed_all: Vec<Filter>,
    /// The systems a click asked to pick out, and whether as well as instead
    ///
    /// What a route or a whole trip was plotted between, in the order it is
    /// flown. The flag is the modifier: held, the stops are picked out
    /// alongside whatever was already, which is a union and not a toggle —
    /// see [`crate::systems::selection::Selection::gather`].
    picked: Option<(Vec<i64>, bool)>,
}

/// A trip's legs joined back into the one route they are flown as
///
/// Every system it passes through, in the order it passes through them, with
/// the seams closed: the stop a leg lands on is the stop the next sets out
/// from and stands in the list once. So a panel about it lists the trip as it
/// would list a route, and the distance each line ends in is the jump that
/// reaches that system whichever leg it fell in.
///
/// Nothing until a leg has landed. A trip whose legs are all still being
/// walked has no systems to describe, and the row already says how far along
/// they are.
///
/// The range comes off the legs, they having all been plotted for the one
/// ship. The trip it names is its own, so a panel about a trip is one panel
/// however often the row is pressed.
fn as_one(trip: &str, rows: &[usize], filters: &Filters) -> Option<Filter> {
    let legs: Vec<&Filter> = rows
        .iter()
        .filter_map(|index| filters.get(*index))
        .map(|active| &active.filter)
        .collect();
    let range = legs.first()?.range()?.to_owned();
    // As the range is, and for the same reason: the legs were all plotted for
    // the one ship, so the trip they come to was plotted for it too. The
    // search mode with them, all the legs having been asked the one way.
    let drive = legs.first()?.drive()?;
    let how = legs.first()?.how()?;

    let mut systems: Vec<i64> = Vec::new();
    for leg in legs {
        let Filter::Route { systems: hops, .. } = leg else { continue };
        let seam = usize::from(!systems.is_empty());
        systems.extend(hops.iter().skip(seam));
    }
    if systems.len() < 2 {
        return None;
    }

    Some(Filter::Route {
        label: Section::Trip(trip.to_owned()).said(rows.len()),
        systems,
        range,
        trip: Some(trip.to_owned()),
        drive,
        how,
    })
}

/// Which group of the bar's filter rows a filter stands in
///
/// Routes apart from the rest. A route is a line drawn across the map between
/// two systems the user named, and a faction or a hand-picked set is a way of
/// reading the sky it is drawn over. They are worth different questions: how
/// many routes am I comparing, and how much of the sky am I picking out.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Section {
    /// Factions and hand-picked sets, which pick the sky out
    Filters,
    /// The legs of one trip, which are flown as one thing
    ///
    /// A trip through five systems is four routes, and they are four rows.
    /// Grouped so the four read as the one trip they were asked for: the row
    /// over them says what the whole of it comes to, framing takes the legs
    /// together, and turning it off takes the whole line off the map.
    Trip(String),
    /// Routes asked for on their own, which are drawn over the sky
    Routes,
}

impl Section {
    /// Every section the bar has rows for, in the order it draws them
    ///
    /// The sky first, then what is drawn over it, which is the order the map
    /// is built up in and the order the two read in. The trips in the order
    /// they were plotted, each ahead of the loose routes: a trip is several
    /// rows and reads as a block, and one wedged between single routes would
    /// not.
    ///
    /// Only sections with a row in them. A section standing over nothing is a
    /// count of nothing.
    fn all(filters: &Filters) -> Vec<Section> {
        let mut sections = vec![Section::Filters];
        for active in filters.iter() {
            let Some(trip) = active.filter.trip() else { continue };
            let trip = Section::Trip(trip.to_owned());
            if !sections.contains(&trip) {
                sections.push(trip);
            }
        }
        sections.push(Section::Routes);
        sections
    }

    /// Whether this section holds `filter`
    fn holds(&self, filter: &Filter) -> bool {
        match self {
            Section::Filters => !filter.is_route(),
            Section::Trip(trip) => filter.trip() == Some(trip.as_str()),
            // A route belonging to a trip is that trip's row, not a loose one.
            Section::Routes => filter.is_route() && filter.trip().is_none(),
        }
    }

    /// Which places in `filters` this section's rows stand at
    ///
    /// Places rather than the filters themselves, since what the row over them
    /// asks for is a change to those filters and an index is what says which.
    fn rows(&self, filters: &Filters) -> Vec<usize> {
        filters
            .iter()
            .enumerate()
            .filter(|(_, active)| self.holds(&active.filter))
            .map(|(index, _)| index)
            .collect()
    }

    /// Whether any filter in this section is turned on
    fn on(&self, filters: &Filters) -> bool {
        self.rows(filters)
            .iter()
            .filter_map(|index| filters.get(*index))
            .any(|active| active.enabled)
    }

    /// How many legs the trip called `trip` has
    ///
    /// Its name is its stops joined by [`ARROW`], so the legs are the gaps
    /// between them: one fewer than the stops, and one for every arrow.
    fn legs(trip: &str) -> usize {
        trip.matches(ARROW).count()
    }

    /// What a row standing over `count` of them says
    ///
    /// A trip says how many legs it is rather than naming its stops. The
    /// stops are the rows under it, named there and in that order, and a
    /// trip through six of them spelled out runs longer than the bar is
    /// wide. What it comes to is added on separately, by whoever has the legs
    /// to add up.
    fn said(&self, count: usize) -> String {
        match self {
            Section::Filters => {
                if count == 1 {
                    "1 filter".to_owned()
                } else {
                    format!("{count} filters")
                }
            }
            Section::Trip(trip) => {
                let legs = Section::legs(trip);
                if legs == 1 {
                    "1 Leg Route".to_owned()
                } else {
                    format!("{legs} Leg Route")
                }
            }
            Section::Routes => {
                if count == 1 {
                    "1 route".to_owned()
                } else {
                    format!("{count} routes")
                }
            }
        }
    }
}

/// Draw a row for each filter at `rows`, and say what a click asked of one
///
/// Split out from [`applied`] because the sections draw the same row and only
/// the count above them differs.
#[allow(clippy::too_many_arguments)]
fn section_rows(
    ui: &mut Ui,
    filters: &Filters,
    rows: &[usize],
    place: &mut usize,
    toggling: &mut Option<usize>,
    removing: &mut Option<usize>,
    opening: &mut Option<Filter>,
    ask: &mut RowAsk,
) {
    let gap = ui.spacing().item_spacing.x;

    for index in rows.iter().copied() {
        let Some(active) = filters.get(index) else { continue };
        // What a route says at its end, where a selection row says how far
        // off its system is. Nothing on the others: a faction's name says all
        // there is to say, and a set says how many it holds in its own name.
        let hops = active.filter.hops().map(|hops| {
            let said = if hops == 1 {
                "1 hop".to_owned()
            } else {
                format!("{hops} hops")
            };
            egui::WidgetText::from(egui::RichText::new(said).weak())
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
        });

        // No info button where nothing could be said: a span admits the
        // galaxy over and what it admits is on the map already.
        let buttons = if active.filter.worth_describing() {
            lay_out_buttons(ui)
        } else {
            lay_out_close(ui)
        };

        // Whatever the dot, the hops and the marks leave. Faction names run
        // long, and one laid out against no bound is painted out past the
        // edge of the bar.
        let room = ui.available_width()
            - ROW_PADDING * 2.
            - DOT
            - gap
            - buttons_width(&buttons, gap)
            - hops.as_ref().map_or(0., |hops| hops.size().x + gap);
        // Cut here rather than left to the layout below, which cuts from the
        // right hand end and would take the far end of a route with it.
        // Egui's own truncation still stands behind this, for the names it
        // has nothing to say about.
        let text = egui::RichText::new(shortened(
            active.filter.name(),
            characters(ui.ctx(), egui::TextStyle::Body, room),
        ));
        let name = egui::WidgetText::from(if active.enabled {
            text.strong()
        } else {
            text.weak()
        })
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room.max(0.),
            egui::TextStyle::Body,
        );

        // By place in the bar rather than by which filter it names, counted
        // on from the selection's rows above. See [`row_of`].
        let of = ("bar-row", *place);
        *place += 1;
        let (outer, row) = row_of(
            ui,
            name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
            of,
        );
        let rect = outer.shrink2(egui::vec2(0., ROW_MARGIN));

        if row.hovered() || row.has_focus() {
            ui.painter().rect_filled(
                rect,
                ui.visuals().widgets.hovered.corner_radius,
                ui.visuals().widgets.hovered.weak_bg_fill,
            );
        }

        // Filled while the filter is being asked and hollow while it is not,
        // so that a filter turned off still reads as one that is there.
        let middle = rect.center().y;
        let mut x = rect.left() + ROW_PADDING;
        let dot = egui::pos2(x + DOT / 2., middle);
        if active.enabled {
            ui.painter().circle_filled(
                dot,
                DOT / 2.,
                ui.visuals().strong_text_color(),
            );
        } else {
            ui.painter().circle_stroke(
                dot,
                DOT / 2.,
                egui::Stroke::new(1_f32, ui.visuals().weak_text_color()),
            );
        }
        x += DOT + gap;
        for galley in [Some(name), hops].into_iter().flatten() {
            let size = galley.size();
            // The galleys carry the colors they were laid out in, so there
            // is nothing for a fallback to answer for.
            ui.painter().galley(
                egui::pos2(x, middle - size.y / 2.),
                galley,
                egui::Color32::PLACEHOLDER,
            );
            x += size.x + gap;
        }

        let Buttons { info, close } = place_buttons(ui, rect, buttons, of);

        // The dot is the switch. It is already the answer to whether the
        // filter is being asked -- filled while it is, hollow while it is not
        // -- so the thing that says so is the thing that changes it, and the
        // row is left for what the row is about.
        //
        // Interacted after the row was laid out, so it stands in front and
        // takes the press the row would otherwise have.
        let switch = ui.interact(
            egui::Rect::from_center_size(dot, egui::Vec2::splat(DOT * 2.)),
            ui.id().with((of, "dot")),
            egui::Sense::click(),
        );

        let settled =
            settled_click(ui, row.id, row.clicked(), row.double_clicked());
        match asked_of_row(
            close.clicked(),
            info.is_some_and(|info| info.clicked()),
            switch.clicked(),
            row.double_clicked(),
            settled.is_some(),
        ) {
            Some(RowGesture::LetGo) => *removing = Some(index),
            Some(RowGesture::Describe) => {
                *opening = Some(active.filter.clone())
            }
            Some(RowGesture::Toggle) => *toggling = Some(index),
            Some(RowGesture::Frame) => ask.framed = Some(active.filter.clone()),
            Some(RowGesture::Select) => {
                let keys = settled.unwrap_or_default();
                ask.picked =
                    Some((active.filter.stops(), gathering_with(keys)));
                ask.chosen = Some(active.filter.clone());
            }
            None => {}
        }
        switch.on_hover_cursor(egui::CursorIcon::PointingHand);
        row.on_hover_cursor(egui::CursorIcon::PointingHand);
    }
}

/// Say how much of the sky is in reach, and how much of it is getting through
///
/// Under the filter rows, and drawn whether or not there are any. What the
/// spyglass reaches is worth knowing before anything has been asked of it: it
/// is the one number that says whether the map is showing a handful of systems
/// or a hundred thousand, and the control that decides it is the one to reach
/// for either way. Said in the spyglass's own name for that reason.
///
/// Counted from [`InReach`], which is tallied where visibility is settled for
/// every system at once, so what is said is the answer the map acted on. Within
/// the spyglass rather than loaded: what it has dragged in from wherever the
/// camera has been is not what the user is looking at.
///
/// The leading number is always what can be seen. `dimming` says whether there
/// is more sky behind it that can also be seen, faintly, and only then is the
/// larger number worth putting beside it:
///
/// - Nothing asked of the map, so the two are the same number: `324 in
///   spyglass`
/// - Filters, and what they exclude drawn faintly behind: `8 of 324 in
///   spyglass`
/// - Filters, and what they exclude drawn not at all: `8 in spyglass`, the
///   rest being neither on screen nor fetched
///
/// Nothing at all for a sky of one system or none. A count of one is not a
/// reading anybody wants: it says less than the system's own name beside it
/// does, and it is what the line stands at for the whole of a descent, where
/// the reach holds the system the camera is inside and nothing else.
///
/// Said in as few words as it can be. The bar is [`BAR_WIDTH`] wide and the
/// numbers are what grow: the sky runs to millions of systems, and a line
/// that has to wrap to hold two of them is a line that moves the rows under
/// it about as the user flies.
fn reaching(
    ui: &mut Ui,
    in_reach: &InReach,
    dimming: bool,
    spawning: bool,
    evicting: bool,
) {
    let InReach { admitted, total } = *in_reach;
    // Nothing to count where the sky in reach is one system or none. A count
    // of one says less than the system's own name does, and it is what the
    // line reads as for the whole of a descent: the camera inside a system
    // with the reach drawn in behind it has that system and nothing else.
    if total <= 1 {
        return;
    }

    let said = if dimming {
        format!(
            "{} of {} in spyglass",
            thousands(admitted as u64),
            thousands(total as u64)
        )
    } else {
        format!("{} in spyglass", thousands(admitted as u64))
    };
    // To the right of the count: a green dot while systems are still being
    // turned into stars, a red one while they are being taken back off. Drawn
    // only for the frames the queue is not empty, so they blink on and off
    // with the work rather than easing.
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(said).weak());
        let radius = ui.text_style_height(&egui::TextStyle::Body) * 0.3;
        if spawning {
            dot(ui, radius, egui::Color32::from_rgb(80, 200, 120));
        }
        if evicting {
            dot(ui, radius, egui::Color32::from_rgb(220, 80, 80));
        }
    });
}

/// A filled circle of `radius` in `color`, laid inline in a row
///
/// Painted rather than lettered so it is a shape whatever the font holds, and
/// held to no animation: it is there while the work is and gone the frame it
/// stops.
fn dot(ui: &mut Ui, radius: f32, color: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(
        egui::Vec2::splat(radius * 2.),
        egui::Sense::hover(),
    );
    ui.painter().circle_filled(rect.center(), radius, color);
}

/// What the bar can be asked to do with the filters as a set
#[derive(Clone, Copy, Debug, PartialEq)]
enum FilterAction {
    /// Pick out the systems every one of them was plotted between
    ///
    /// What a click on the row means. A section of filters has no one filter
    /// to be the one being worked with, which is what a click on a filter's
    /// own row settles, so the row is free to mean the thing a set can answer
    /// and a single row cannot: every stop of the trip at once.
    ///
    /// Carries whether the press meant "and these as well", read off the
    /// press rather than off the frame it is acted on — the click waits out
    /// the window a double could arrive in, and the modifier is let go of
    /// inside it. See [`settled_click`].
    Select(bool),
    /// Turn every filter off, or every one back on
    Toggle,
    /// Open a panel describing the whole of it
    Describe,
    /// Send the camera to see the whole of what all of them admit
    Frame,
    /// Take them all away
    LetGo,
}

/// One row standing for every filter under it, and what a click on it asked
///
/// The two gestures a row gives, said of all of them at once: the row turns
/// them off and back on, and the mark at its end takes them away. Both are
/// what a set wants and what a list of rows answers slowest, a filter at a
/// time being the only way to reach them otherwise.
///
/// Drawn as a row rather than as a pair of buttons, so there is nothing new
/// to read: the dot and the mark say for all of them what each row's own say
/// for one, and they stand in the same two places.
///
/// The dot is filled while `on`, which says whether any filter under it is
/// being asked, since that is what the row undoes. The same click that put the
/// rest of the sky away brings it back, so the state it shows is the state its
/// own gesture is about.
///
/// `said` is what it is standing over, which the section works out: the bar
/// draws its filters in groups and each has one of these of its own, so this
/// is handed what to say rather than counting a whole set it is not about.
fn whole_set(
    ui: &mut Ui,
    said: &str,
    on: bool,
    describes: bool,
    place: &mut usize,
) -> Option<FilterAction> {
    let gap = ui.spacing().item_spacing.x;
    // A trip is one thing to be described, as each of its legs is: how far
    // the whole of it runs, and every system it passes through in the order
    // it passes through them. A count of filters is not, there being nothing
    // to say about a heap of them that their own rows do not say.
    let buttons =
        if describes { lay_out_buttons(ui) } else { lay_out_close(ui) };

    let room = ui.available_width()
        - ROW_PADDING * 2.
        - DOT
        - gap
        - buttons_width(&buttons, gap);
    let text = egui::RichText::new(said);
    let name =
        egui::WidgetText::from(if on { text.strong() } else { text.weak() })
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                room.max(0.),
                egui::TextStyle::Body,
            );

    // By place in the bar, as the rows below it are keyed. See [`row_of`].
    let of = ("bar-row", *place);
    *place += 1;
    let (outer, row) = row_of(
        ui,
        name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
        of,
    );
    let rect = outer.shrink2(egui::vec2(0., ROW_MARGIN));

    if row.hovered() || row.has_focus() {
        ui.painter().rect_filled(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }

    let middle = rect.center().y;
    let mut x = rect.left() + ROW_PADDING;
    let dot = egui::pos2(x + DOT / 2., middle);
    if on {
        ui.painter().circle_filled(
            dot,
            DOT / 2.,
            ui.visuals().strong_text_color(),
        );
    } else {
        ui.painter().circle_stroke(
            dot,
            DOT / 2.,
            egui::Stroke::new(1_f32, ui.visuals().weak_text_color()),
        );
    }
    x += DOT + gap;
    // The galley carries the color it was laid out in, so there is nothing
    // for a fallback to answer for.
    ui.painter().galley(
        egui::pos2(x, middle - name.size().y / 2.),
        name,
        egui::Color32::PLACEHOLDER,
    );

    let Buttons { info, close } = place_buttons(ui, rect, buttons, of);

    // The switch, as the rows below have. Said of all of them at once, which
    // is what this row is for.
    let switch = ui.interact(
        egui::Rect::from_center_size(dot, egui::Vec2::splat(DOT * 2.)),
        ui.id().with((of, "dot")),
        egui::Sense::click(),
    );

    // Read in the same order a row below is, by the same rule: see
    // [`asked_of_row`]. A click on the name means the systems the set was
    // plotted between — a trip's every stop, which is the one thing this row
    // can say that none of the rows under it can.
    let settled =
        settled_click(ui, row.id, row.clicked(), row.double_clicked());
    let asked = match asked_of_row(
        close.clicked(),
        info.is_some_and(|info| info.clicked()),
        switch.clicked(),
        row.double_clicked(),
        settled.is_some(),
    ) {
        Some(RowGesture::LetGo) => Some(FilterAction::LetGo),
        Some(RowGesture::Describe) => Some(FilterAction::Describe),
        Some(RowGesture::Toggle) => Some(FilterAction::Toggle),
        Some(RowGesture::Frame) => Some(FilterAction::Frame),
        Some(RowGesture::Select) => Some(FilterAction::Select(gathering_with(
            settled.unwrap_or_default(),
        ))),
        None => None,
    };
    row.on_hover_cursor(egui::CursorIcon::PointingHand);
    switch.on_hover_cursor(egui::CursorIcon::PointingHand);
    asked
}

/// What the box asks in [`AskMode::Filter`], under the field
///
/// The field itself is the bar's one box, which is asking for a faction while
/// this mode is out: see [`ask_bar`]. What is left is what a name cannot say
/// — which of the factions holding it was meant, and how lately a system
/// must have been heard from — so this is the answer to the name and the one
/// filter that is not a name at all.
///
/// The field empties once a faction has been asked for. What was typed is a
/// row by then, and the field's next job is the next filter.
///
/// What went wrong is said here rather than beside the box, so that a name
/// that resolved to nothing is read under the name that did it, and the
/// search's own note cannot be mistaken for it: one box asks all three
/// questions, and only one of them is being asked at a time.
fn filter_body(ui: &mut Ui, filter: &mut FilterBar) {
    if let LookupNote::Failed(why) = &*filter.note {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, why);
    }

    // A click chooses, as it does in every other list the map draws. The
    // search has already asked what the lookup would have asked, so the line
    // carries the id a filter tests against and there is nothing left to look
    // up: the faction goes straight into a row of its own.
    //
    // The field and the list go with it. What was typed is a row by now, and
    // the field's next job is the next faction.
    if let Some(faction) = faction_list(ui, filter.found.iter()) {
        filter.active.bypass_change_detection().add(Filter::Faction {
            id: faction.id,
            name: faction.name.clone(),
        });
        *filter.input = None;
        filter.found.clear();
    }

    watch_control(
        ui,
        &mut filter.watch,
        filter.active.bypass_change_detection(),
        &mut filter.standstill,
    );
}

/// Say what moment the map is standing at
///
/// Drawn in [`time_strip`], at the top of the viewport and in the middle of
/// it. What the map is doing, said where a reader is looking rather than kept
/// behind the gear, and its own zone rather than a line in the bar: the
/// moment is the galaxy's and holds whatever the bar is being asked, so a
/// reading filed under the asking moved down the screen every time a form
/// dropped out above it. It answers the one question the arrangement on
/// screen raises — when is this — and it is the only place the map ever says
/// what day the game is on.
///
/// The date alone while nothing has run the map on, which is how it opens. A
/// slider puts the map some span past the present, and then the span is named
/// beside the date and can be let go of: the sliders each cover one turn of
/// something, so none of them can reach back to nothing on its own.
///
/// Said whether or not a system is held. The moment is the galaxy's and not a
/// system's -- see [`Clock`] -- so there is a date to read out on the way
/// between two of them, and `Now` is reachable from wherever a drag was left.
/// It was drawn off the held system's newest scan to begin with, which meant
/// no line at all out in the sky and an offset that could be set from a
/// panel with no way to let go of it.
///
/// Clicking the reading opens the slider that sets it, in the line below.
/// What a reader wants to change is the thing they are reading, so the way to
/// it is the reading itself rather than a control filed away in the pane
/// where nobody would find it. The span past the present answers a click as
/// the date does, the two being halves of one moment. Clicked again the
/// slider goes, the map left wherever it put it -- `Now` is what lets go of
/// that.
fn dated(
    ui: &mut Ui,
    clock: &mut Clock,
    turns: Turns,
    control: &mut ClockControl,
) {
    let out = control.out;
    let running_on = clock.offset() != 0.;
    let clicked = ui
        // To the height the bar's box comes to, and its contents laid
        // level in it, so that the reading stands on the same line as the
        // field and the gear hung on the field's own middle. Both panes are
        // at the top of the viewport behind the same padding, so the one
        // thing that had them out of true was that a field is padded inside
        // and a line of text is not: the reading sat a few points high of
        // the box beside it. See [`field_height`].
        .allocate_ui_with_layout(
            egui::vec2(ui.available_width(), field_height(ui)),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                let mut asked = reading(ui, drawn_at(clock));
                if running_on {
                    // In the marks' own words rather than
                    // [`crate::systems::info::lasting`]'s. The two stand in one
                    // line with the switch and `Now` at the end of it, and
                    // `+14989.7 Earth years` -- which is what the far end of a
                    // wide pair's rail comes to -- ran clean through them.
                    asked |= reading(
                        ui,
                        format!("+{}", briefly(clock.offset(), true)),
                    );
                }

                // The controls stand at the far end of the strip while the
                // scrubber is out, which is what fills a line the reading only
                // half covers, and is a place they keep: read beside the span
                // they are about, they walk along the line as it grows a digit.
                //
                // Beside the span while the strip is shut, there being no width
                // to stand at the end of: the strip is then only as wide as what
                // is written in it, and there is no rail to gear.
                let mut let_go = false;
                if out {
                    ui.with_layout(
                        egui::Layout::right_to_left(egui::Align::Center),
                        |ui| {
                            if running_on {
                                let_go = ui.small_button("Now").clicked();
                            }
                            // Beside `Now` at the far end rather than beside the
                            // reading: the span in the reading grows and shrinks
                            // as the rail is dragged, and a switch that slid
                            // along above the rail with it would be a control
                            // moving under the hand using it.
                            if turns.choice() {
                                gearing(ui, &mut control.to);
                            }
                        },
                    );
                } else if running_on {
                    let_go = ui.small_button("Now").clicked();
                }
                if let_go {
                    clock.reset();
                }

                asked
            },
        )
        .inner;
    if clicked {
        control.out = !control.out;
    }

    // From the state the pass began in, and not from what the click has just
    // asked for. The pane around this is framed and sized before anything in
    // it is drawn -- see [`Dropping`] -- so a rail drawn on the pass the
    // click arrived is a rail drawn in a strip still the width of the
    // reading, with no frame under it and no fill behind it: for one frame
    // the slider hung outside its own panel. What a click asks for is the
    // next pass's to draw, which is where the frame and the width will be
    // waiting for it.
    if out {
        clock_control(ui, clock, turns.geared(control.to));
    }
}

/// Which of the turns on offer the rail covers
///
/// Two words at the far end of the reading, with the rail they are about
/// directly underneath. Only where there are two turns to choose between —
/// see [`Turns::choice`] — a switch between one thing and the same thing
/// being a control that does nothing.
///
/// Not in the settings pane, which is where what the map is drawn like is
/// set. This is about the control beneath it and nothing else: it changes
/// what one drag is worth and says so by changing the marks under the rail.
///
/// Drawn backwards, because the row it stands in runs from the right: `Body`
/// last is `Body` leftmost, so the pair reads narrower first, in the order
/// [`GearedTo::ALL`] holds and the rails themselves run.
fn gearing(ui: &mut Ui, to: &mut GearedTo) {
    greyed(ui, |ui| {
        for offered in GearedTo::ALL.into_iter().rev() {
            ui.selectable_value(to, offered, offered.said())
                .on_hover_text(offered.hint());
        }
    });
}

/// One weak word of the status line, and whether it was clicked
///
/// Every part of the reading answers a click, the date and the span past the
/// present alike: they are two halves of the one moment, and a reader
/// reaching for the number they mean to change should not have to know which
/// half of it the control hangs off. `Now` is the exception, being a control
/// already and one about the same thing.
fn reading(ui: &mut Ui, said: String) -> bool {
    ui.add(
        egui::Label::new(egui::RichText::new(said).weak())
            .sense(egui::Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
    .on_hover_text("Set what moment the system is drawn at")
    .clicked()
}

/// The smallest span the status slider runs the map on by, in seconds
///
/// The near end of a logarithmic rail has to stand at some span rather than
/// at none, since no run of decades reaches zero. A minute: the clock is
/// stepped in whole seconds, and the fastest thing the journal records comes
/// round in hours, so a minute is a hundredth of the quickest turn there is
/// and nothing slower stirs enough to see.
///
/// Zero itself is still the far near end of the rail, egui putting the value
/// exactly at the range's start where the handle is run all the way down.
const SPAN_FLOOR: f64 = 60.;

/// What the status rail is geared to
///
/// One turn of something either way, and which thing settles both how long
/// the rail is and how it is laid out.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) enum Geared {
    /// One turn of the body picked out
    ///
    /// Laid evenly, as that body's own phase slider is: the rail is one orbit
    /// of the one thing being watched, every stretch of it is the same span,
    /// and halfway along is half a turn. Nothing about the span asked for is
    /// lopsided, so nothing about the rail should be.
    Body(f64),
    /// The widest orbit the system has on record
    ///
    /// Laid by decades, because with nothing picked out the spans worth
    /// asking for are not evenly spread: the widest orbit of a system takes a
    /// median eighteen years to come round and its fastest body a few hours,
    /// so an even rail over the whole of that spends its length on spans that
    /// blur every inner body and cannot be nudged by an hour anywhere.
    System(f64),
}

impl Geared {
    /// How long the rail runs, in seconds
    fn turn(self) -> f64 {
        match self {
            Geared::Body(turn) | Geared::System(turn) => turn,
        }
    }

    /// Whether every stretch of the rail is the same span
    fn even(self) -> bool {
        matches!(self, Geared::Body(_))
    }
}

/// A slider over one turn, under the reading
///
/// What the date opens. Geared to the body picked out where there is one, and
/// to the widest orbit the system has otherwise, so its far end is that thing
/// one turn on: watching a planet, the rail is that planet's year, and with
/// nothing picked out it is the span in which the whole system has been
/// through every arrangement it takes. The sliders under the bodies do the
/// same for a body whose panel is open, and this is the one that needs no
/// panel, which is the whole reason it is here.
///
/// No numbers of its own. What it comes to is a moment, and the moment is the
/// line above it -- along with the span it stands past the present, which is
/// what a number on the rail would have said and says it in the units a
/// reader thinks in.
///
/// The span outright rather than a phase, which is the difference between a
/// rail and a body's slider. Dragged to the far end it stands at a turn from
/// now and stays there; dragged back it comes back. Read as a phase it wrapped
/// instead -- a whole turn reads as none of one -- so the far end put the
/// handle back at the near end with the map a turn out, and the next drag
/// measured from the turn after that. Where the turn was the ceiling itself
/// there was nowhere further to go and the reading stuck until `Now`.
///
/// One range, and its far end is [`Clock::CEILING`] where a turn runs past
/// that. The rail's end and the furthest the clock will go have to be the
/// same number or the last stretch of the rail asks for spans the clock
/// answers with the one it stopped at: the handle stands where the pointer
/// put it, the reading stands at the ceiling, and the two disagree for as
/// long as the drag lasts.
///
/// Nothing to drag where no orbit in the system has a period recorded: there
/// is no turn to cover, and a slider over nothing would move the map by
/// nothing however far it was dragged.
///
/// Its width is the strip's, taken as a number rather than as what the line
/// above it left over. That line grows with the reading -- a fifth digit in
/// the year, a longer span beside it -- and egui widens a `Ui` to hold a row
/// too wide for it, so once the line is wider than the strip the room left
/// under it moves with what the rail last set. Which is a control whose scale
/// is a function of its own value: at a span of some ten thousand years the
/// two settled into a cycle, one width setting the span that asks for the
/// other, and the reading flicked between two moments a log step apart under
/// a hand holding still. Measured pixel by pixel along the rail:
/// `11090.83y then 7922.02y`, over and over. Held to
/// `the_reading_holds_still_all_along_the_rail`.
fn clock_control(ui: &mut Ui, clock: &mut Clock, geared: Option<Geared>) {
    let turn = geared.map_or(0., Geared::turn).min(Clock::CEILING);
    // Where the map already stands, as much of it as this rail covers. A
    // body's own slider can have run the offset past a turn of the widest
    // orbit; the rail then reads at its far end rather than wrapping round.
    let mut past = clock.offset().min(turn);
    ui.spacing_mut().slider_width = RAIL_WIDTH - ui.spacing().item_spacing.x;
    // Thinner than the sliders in the pane, and shorter in its row. Those are
    // read one to a line down a column of controls; this is one rail across
    // the top of the map, and egui's own proportions drew it as a grey bar
    // over the sky with a lozenge in it.
    ui.spacing_mut().slider_rail_height = RAIL_HEIGHT;
    ui.spacing_mut().interact_size.y = RAIL_ROOM;
    let moved = ui
        .add_enabled_ui(turn > 0., |ui| {
            let mut rail = egui::Slider::new(&mut past, 0.0..=turn)
                .show_value(false)
                .handle_shape(egui::style::HandleShape::Circle);
            if !geared.is_some_and(Geared::even) {
                rail = rail.logarithmic(true).smallest_positive(SPAN_FLOOR);
            }
            ui.add(rail)
        })
        .inner;
    if moved.changed() {
        clock.offset_at(past);
    }

    if let Some(geared) = geared {
        marks(ui, moved.rect, geared);
    }
}

/// How thick the rail is drawn
const RAIL_HEIGHT: f32 = 4.;

/// How tall a row the rail is given
///
/// The handle is sized off it — egui draws one at a fifth of the row either
/// side of the rail — so this is what settles how big the thing under the
/// pointer is. Enough to hit and no more.
const RAIL_ROOM: f32 = 14.;

/// How far apart the handle's circle stands from the rail's own ends
///
/// Egui shrinks the range the handle travels in by its own radius at either
/// end, so a value's place along the rail is measured in what is left rather
/// than in the whole of it. The same fifth of the row it draws the handle at.
fn handle_radius(rail: egui::Rect) -> f32 {
    rail.height() / 2.5
}

/// A minute, and the units built on it, in seconds
const MINUTE: f64 = 60.;
const HOUR: f64 = 60. * MINUTE;
const DAY: f64 = 24. * HOUR;
const YEAR: f64 = 365.25 * DAY;

/// The spans a logarithmic rail is marked at
///
/// One or two to a decade, at the spans a reader thinks in rather than at
/// round numbers of seconds: an hour, a day, a year. Which of them are drawn
/// is what the rail covers and what fits — see [`marks`] — so this is every
/// mark the map might make, from the rail's own near end at [`SPAN_FLOOR`] up
/// past the ten thousand years a wide pair takes to come round.
const MARKED: [f64; 12] = [
    MINUTE,
    10. * MINUTE,
    HOUR,
    6. * HOUR,
    DAY,
    7. * DAY,
    30. * DAY,
    YEAR,
    10. * YEAR,
    100. * YEAR,
    1_000. * YEAR,
    10_000. * YEAR,
];

/// Say how far along the rail `span` falls, as a fraction of its length
///
/// The same arithmetic egui lays the handle out by, so a mark stands under
/// the place the handle stops at rather than near it: linear over a body's
/// own turn, and over the decades from [`SPAN_FLOOR`] otherwise. Anything at
/// or under the near end is the near end, which is where nothing and a minute
/// both stand on a rail that runs to years.
fn along(span: f64, turn: f64, even: bool) -> f32 {
    if turn <= 0. {
        return 0.;
    }
    if even {
        return (span / turn).clamp(0., 1.) as f32;
    }
    if span <= SPAN_FLOOR {
        return 0.;
    }
    let floor = SPAN_FLOOR.log10();
    let ceiling = turn.log10();
    if ceiling <= floor {
        return 0.;
    }
    (((span.log10() - floor) / (ceiling - floor)) as f32).clamp(0., 1.)
}

/// Write what the rail's places come to, under it
///
/// A rail over one turn of a system runs from a minute to millennia and says
/// nothing about where along it a day is. Named marks are what make a
/// logarithmic rail readable: the handle stands over a word rather than a
/// third of the way along nothing.
///
/// The near end is `now`, that being where the rail's own floor and no span
/// at all both stand, and the far end is however long the turn is. Between
/// them, the spans of [`MARKED`] the turn covers — or the quarters of it,
/// where the rail is a body's own turn laid evenly and decades would mark one
/// end of it.
///
/// Whatever will not fit is left out, left to right: a mark is drawn only
/// where it stands clear of the last one written. The far end is written
/// first for that reason, being the one a reader needs — it says what the
/// rail covers — so a mark that would run into it is the one that goes.
fn marks(ui: &mut Ui, rail: egui::Rect, geared: Geared) {
    let turn = geared.turn().min(Clock::CEILING);
    if turn <= 0. {
        return;
    }

    let even = geared.even();
    let mut wanted = vec![(0_f64, "now".to_owned())];
    if even {
        for quarter in 1..4 {
            let span = turn * quarter as f64 / 4.;
            wanted.push((span, briefly(span, false)));
        }
    } else {
        for span in MARKED.into_iter().filter(|span| *span < turn) {
            wanted.push((span, briefly(span, false)));
        }
    }
    wanted.push((turn, briefly(turn, false)));

    let gap = ui.spacing().item_spacing.x;
    let inset = handle_radius(rail);
    let ends = egui::Rangef::new(rail.left() + inset, rail.right() - inset);
    let written: Vec<(f32, std::sync::Arc<egui::Galley>)> = wanted
        .into_iter()
        .map(|(span, said)| {
            let at = egui::lerp(ends, along(span, turn, even));
            let galley = egui::WidgetText::from(
                egui::RichText::new(said).weak().small(),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Small,
            );
            (at, galley)
        })
        .collect();

    let row = ui
        .allocate_exact_size(
            egui::vec2(
                rail.width(),
                written.first().map_or(0., |(_, said)| said.size().y),
            ),
            egui::Sense::hover(),
        )
        .0;

    // The far end first, then the rest from the near end up, so that what is
    // dropped where the two meet is a mark in the middle rather than the one
    // saying how far the rail goes.
    let mut taken: Vec<egui::Rangef> = Vec::with_capacity(written.len());
    let order = written.len().saturating_sub(1);
    for index in std::iter::once(order).chain(0..order) {
        let Some((at, said)) = written.get(index) else { continue };
        let across = said.size().x;
        // Centered on the mark, and held inside the row at either end: the
        // near end's word would otherwise hang off the strip by half of
        // itself.
        let left = (at - across / 2.).clamp(row.left(), row.right() - across);
        let stands = egui::Rangef::new(left - gap, left + across + gap);
        if taken.iter().any(|held| held.intersects(stands)) {
            continue;
        }
        taken.push(stands);
        ui.painter().galley(
            egui::pos2(left, row.top()),
            said.clone(),
            egui::Color32::PLACEHOLDER,
        );
    }
}

/// A span in the largest unit it fills, in as few characters as say it
///
/// For the strip, where the reading, the switch and `Now` share one line and
/// a dozen marks share the one under it:
/// [`crate::systems::info::lasting`] writes `18.0 Earth years`, which is the
/// right answer in a panel and four marks' worth of room here — and at the
/// ceiling, `+14989.7 Earth years`, which ran clean through the switch.
///
/// `fine` asks for a tenth of the unit, which is what the reading wants and
/// the marks do not: a mark stands at a span chosen to be a whole one, where
/// a reading has to move as the rail is dragged. `+3 h` held for every drag
/// across a stretch of the rail is a reading that looks stuck.
///
/// Years are whole and grouped either way. A tenth of a year is not
/// something a reader is asking about out there, and `14989.7 y` is a length
/// rather than a number; the date beside it is where the moment itself is
/// read.
fn briefly(span: f64, fine: bool) -> String {
    let tenths = usize::from(fine);
    if span < HOUR {
        format!("{:.*} min", tenths, span / MINUTE)
    } else if span < DAY {
        format!("{:.*} h", tenths, span / HOUR)
    } else if span < 60. * DAY {
        format!("{:.*} d", tenths, span / DAY)
    } else if span < 330. * DAY {
        // Months only where a month is the largest unit filled. A year read
        // as `12 mo` is the right number in the wrong unit, and the mark
        // beside it says `10 y`.
        format!("{:.*} mo", tenths, span / (30. * DAY))
    } else {
        format!("{} y", thousands((span / YEAR).round() as u64))
    }
}

/// How far ahead of ours the game's own calendar runs, in years
///
/// The two run together otherwise: an hour out there is an hour here, and the
/// journal stamps its scans in our own time. Which is why the span the clock
/// holds needs no converting at all and only the year it lands in does.
const AHEAD_BY: i32 = 1286;

/// The moment the map is standing at, by the game's calendar
///
/// [`Clock::moment`] in our own, turned once here: the two calendars run
/// together and only the year is 1286 apart.
///
/// In the game's own notation: the day before the month, the month named
/// rather than numbered, the time to the second, and the whole of it in
/// capitals. A reader comparing the map against the panel in front of them is
/// comparing two of the same thing, and a named month cannot be read the
/// American way round by mistake. The date leads, the line being read as a
/// date that carries a time rather than as a clock.
///
/// The year is written out here rather than by `%Y`, which puts a `+` in
/// front of anything past four digits: that is ISO 8601 saying the year is an
/// expanded one, and it reads on the map as a span rather than as a date. Five
/// digits is reachable and not even far-fetched -- the calendar already stands
/// 1286 years on, and a system whose widest orbit is a wide pair's takes
/// millennia to come round, so running the slider to the end of one lands
/// there.
fn drawn_at(clock: &Clock) -> String {
    let drawn = clock.moment();
    let year = drawn.year() + AHEAD_BY;
    let dated = drawn
        .with_year(year)
        // The one day of ours a game year may not hold: a leap day landing
        // 1286 years on in a year without one. Read as the last day of that
        // February, which is the nearest date there is to it.
        .or_else(|| drawn.with_day(28).and_then(|day| day.with_year(year)))
        .unwrap_or(drawn);

    format!("{} {year} {}", dated.format("%d %b"), dated.format("%H:%M:%S"))
        .to_uppercase()
}

/// Ask for a filter by how lately a system was updated
///
/// A slider over named spans rather than a field to type a time into. What is
/// being asked is roughly how fresh, and the spans are the answers anybody
/// wants: the far end of a typed time is a database going back years and the
/// near end is the last minute.
///
/// The label names what is being asked about and the span says how far back,
/// so the two read as "Last Updated" over "6 hours".
///
/// Only on a change, as the opacity beside it is. Asking is what marks the
/// filters as changed, and what reads that mark puts a fresh question to the
/// database.
///
/// Answers the slider, so that a caller can say whether it is being dragged.
fn watch_control(
    ui: &mut Ui,
    watch: &mut Watch,
    active: &mut Filters,
    standstill: &mut Standstill,
) -> Response {
    ui.add_space(FIELD_GAP);
    ui.label("Last Updated");

    let mut standing = Watch(watch.0);
    fill_width(ui, SPAN_WIDTH);
    // Spelled out, and global so that the name is the whole of it. Egui
    // otherwise numbers a widget by how many were drawn before it, and this
    // one is drawn under the rows the filters make: asking for a filter here
    // adds a row above, every widget below it is renumbered, and egui follows
    // a drag by id. The slider the user pressed stops existing mid-gesture and
    // the drag is dropped.
    //
    // `push_id` does not answer it. A child `Ui` given a name still takes the
    // count it was made at into the ids of what it holds, so the widgets
    // inside are renumbered along with everything else.
    let slider = ui
        .scope_builder(
            egui::UiBuilder::new().id_salt(WATCH).global_scope(true),
            |ui| {
                ui.horizontal(|ui| {
                    let slider = ui.add(
                        egui::Slider::new(&mut standing.0, 0..=SPANS.len() - 1)
                            .show_value(false),
                    );
                    // Beside the slider rather than in the box egui draws its
                    // own reading in. That box is a field to type the value
                    // into, and what it would take is one of nine places along
                    // the slider, where what the user is reading is a span. So
                    // the box is turned off and the span said here.
                    //
                    // Read off the slider rather than off `watch`, which is
                    // not written until the drag is over: taken from there the
                    // name would say the span the slider set out from all the
                    // way through a drag.
                    ui.label(standing.name());
                    slider
                })
                .inner
            },
        )
        .inner;

    // Taken before the drag is allowed to ask for anything, so what is held
    // is how the rows stood when the press landed and not how they stood after
    // the first step of the drag had already added one.
    if slider.drag_started() {
        standstill.hold(active);
    }

    if slider.changed() {
        watch.0 = standing.0;
        match watch.span() {
            // Worked out here and not where it is asked. `Utc::now` is the one
            // thing in this that cannot be tested, so it is read at the one
            // place that turns a span into a moment.
            Some(span) => active.ask_within(watch.name(), span),
            // Its row stands above this control, so letting go of it mid-drag
            // would take the control up a row and out from under the pointer.
            // It stops asking where it stands instead, and says so.
            None if slider.dragged() => active.turn_time_off(watch.name()),
            None => active.ask_nothing_of_time(),
        }
    }

    // Let go of at the far end, so what stopped asking during the drag is let
    // go of now that moving the rows is nothing to the gesture.
    if slider.drag_stopped() {
        standstill.release();
        if watch.span().is_none() {
            active.ask_nothing_of_time();
        }
    }

    slider
}

/// The factions a search found, and which of them was clicked
///
/// Names alone. A faction is a name and an id, the id is what a filter tests
/// against rather than anything to read, and there is nothing else on record
/// about one worth a column.
///
/// Not [`system_list`], which draws systems: a faction has nowhere to be, so
/// there is no distance to say, nothing to fly to and no panel of its own to
/// open. What the two share is the line they are drawn with.
fn faction_list<'a>(
    ui: &mut Ui,
    factions: impl Iterator<Item = &'a DbFaction>,
) -> Option<&'a DbFaction> {
    // Nothing found is nothing drawn, as a list of systems is.
    let mut factions = factions.peekable();
    factions.peek()?;

    let height = ui.text_style_height(&egui::TextStyle::Body)
        + LINE_PADDING * 2.
        + ui.spacing().item_spacing.y;
    let mut chose = None;

    scrolling(ui, height * OFFERED as f32, "factions", |ui| {
        for faction in factions {
            // Keyed by where it sits, which is what `line` allocates itself
            // and what the lines of every other list are keyed by: a fresh
            // search leaves them where they were and makes each about
            // something else.
            let (_, answer) =
                line(ui, egui::RichText::new(faction.name.as_str()), 0., true);
            if answer.clicked() {
                chose = Some(faction);
            }
            answer.on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    });

    chose
}

/// Every key the map answers, and what each does
///
/// The same table the README carries, kept here so that the map says it too:
/// a binding nobody can find from inside the map is a binding for whoever
/// wrote it. Held to the README's table by
/// `the_pane_and_the_readme_list_the_same_keys`, so the two cannot drift.
///
/// Each is a key struck on its own, but for the four that want shift — the
/// three that put a question in the bar's box and the `?` that opens this
/// window — which is what [`crate::keys`] promises and the README says.
const BINDINGS: [(&str, &str); 17] = [
    ("W A S D", "Pan along the ruled plane"),
    ("Q E", "Pan down and up through it"),
    ("Z X", "Swing the camera round what it looks at"),
    ("C V", "Lower and raise it over the plane"),
    ("F R", "Zoom in and out"),
    ("Space", "Fly to what is picked out, one at a time"),
    ("H", "Go home: Sol, from where the map opened"),
    ("L", "Show or hide the labels"),
    ("O", "Show or hide the orbit lines"),
    ("G", "Show or hide the grid"),
    ("J", "Show or hide your own journal's systems"),
    ("/ or Shift-S", "Search the box for a system"),
    ("Shift-F", "Ask the box for a faction to filter on"),
    ("Shift-R", "Ask the box for systems to route between"),
    ("Esc", "Put away the bindings, or everything the chrome has open"),
    ("F1 or ?", "Show or hide these bindings"),
    ("F3", "Show or hide the diagnostics window"),
];

/// The window the bindings are read in
///
/// A window rather than a panel: it is read against whatever the reader was
/// doing when they wanted it, and it is moved out of the way rather than
/// closed. Its own close mark as well as the key that opened it, a window
/// being the one thing on screen a reader already knows how to shut.
///
/// Not resizable and not collapsible. There is one thing in it, it is as wide
/// as the widest binding, and a reference rolled up into its title bar is a
/// reference nobody can read.
fn keys_window(ctx: &Context, open: &mut bool) {
    egui::Window::new("Keys")
        .open(open)
        .resizable(false)
        .collapsible(false)
        .show(ctx, keys_reference);
}

/// Say what the keys do
///
/// Two columns, the key set as a heading is and what it does in the ordinary
/// text, so the column of keys is what the eye runs down.
fn keys_reference(ui: &mut Ui) {
    egui::Grid::new("keys-reference").num_columns(2).show(ui, |ui| {
        for (key, does) in BINDINGS {
            ui.label(egui::RichText::new(key).strong());
            ui.label(egui::RichText::new(does).weak());
            ui.end_row();
        }
    });
}

/// A control and what it does, said on hover
///
/// One line, plainly: the ordinary word for the thing that happens when the
/// control is used. "Fetch fresh data periodically", not "go back for what the
/// map holds" — the second is the voice this crate's doc comments are written
/// in, and in a tooltip it is a sentence a reader has to decode to learn that
/// a checkbox fetches anything. The prose belongs in the comments; a hint is
/// for someone who wants to know what a switch does and get on with it.
///
/// So: say the verb. Fetch, show, draw, color, name, hide. Say the noun the
/// user would use for what it acts on — systems, names, the grid — and not the
/// name the code gives it. Leave out why it is there, how it works and what it
/// costs; those are what a doc comment is for, and [`Routing`] is an example
/// of one carrying what would not fit here.
///
/// No full stop. It is a label rather than prose, as the control's own name
/// is, and every one of them ends the same way for the same reason.
///
/// Answered through these three rather than by each control reaching for
/// `on_hover_text` itself, so that a control added without a hint reads as
/// odd at the callsite instead of quietly having none.
fn check(ui: &mut Ui, on: &mut bool, said: &str, hint: &str) -> Response {
    ui.checkbox(on, said).on_hover_text(hint)
}

/// What each point-spread profile does to a star, in a line
///
/// Its own function rather than a method on the kind, the kind belonging to
/// [`galos_photometry`] and this being what the pane says about it rather than
/// what it is.
fn spread_hint(kind: ProfileKind) -> &'static str {
    match kind {
        ProfileKind::Moffat => "Soft halo, like a telescope",
        ProfileKind::Gaussian => "Tight dot, no halo",
    }
}

/// One of several, and what choosing it does. See [`check`].
fn choose<T: PartialEq>(
    ui: &mut Ui,
    held: &mut T,
    value: T,
    said: &str,
    hint: &str,
) -> Response {
    ui.radio_value(held, value, said).on_hover_text(hint)
}

/// The name over a slider or a value, and what it sets. See [`check`].
fn titled(ui: &mut Ui, said: &str, hint: &str) -> Response {
    ui.label(said).on_hover_text(hint)
}

/// Open a section, in the form or in the settings pane
///
/// The rule is the break between one section and the next, and needs no
/// run-up of its own: the row above it keeps as much room under itself as it
/// keeps over, and a section gap on top of that would sit the row nearer the
/// input than the section and read as belonging to neither.
///
/// `ruled` is how the first section of the pane goes without one. The top of
/// the pane is already an edge, and a rule drawn against it reads as a
/// section with nothing in it. Every section of the form is ruled, having the
/// input and the selection's row above it.
fn heading(ui: &mut Ui, name: &str, ruled: bool) {
    if ruled {
        ui.separator();
    }
    ui.label(egui::RichText::new(name).strong());
    ui.add_space(FIELD_GAP);
}

/// Take a row's worth of the bar, and answer for it under an id of its own
///
/// Every id in these rows is spelled out rather than taken from the order the
/// widgets happened to be drawn in. Egui hands out an unspelled id from that
/// order, and the rows of the bar do not keep their places within it: the note
/// about a name that resolved to nothing comes and goes above them, and the
/// selection's rows come and go with the selection. Either shifts the count
/// every row below is numbered by.
///
/// What is spelled out is where the row sits in the bar's one column, counted
/// across every kind of row in it, and not which system or filter it is about.
/// A row here is clicked and hovered and nothing else, and that is all egui
/// keeps against an id, so a place is the honest key: the pointer is over the
/// third row, whichever row now stands there.
///
/// Across the kinds and not within each, because the kinds share the column
/// and are drawn to the same height. Letting go of one selected system moves
/// every filter row up by exactly one row, so each lands in a rectangle a
/// selection row was drawn in. Numbered apart, the two would put a fresh id
/// at a rectangle that kept its place, which is the very thing this is for.
///
/// Keying a row on what it holds paints a red rectangle across the bar.
/// Between one pass and the next egui looks for a rect that kept its place
/// while everything in it changed identity, which is what a replaced selection
/// and a dropped filter both are, and it cannot tell that apart from one
/// widget taking another's state. It warns and paints the rect in red.
///
/// `push_id` does not answer this either, and makes it worse: a child `Ui`
/// registers a rect of its own, so a parent named for the row's system adds a
/// second widget at the row's rect that changes identity along with it.
///
/// The space is taken without a widget of its own, since the row is what
/// answers for it and two things at one rect is what the ordering was
/// complaining of in the first place.
fn row_of(
    ui: &mut Ui,
    height: f32,
    of: impl std::hash::Hash,
) -> (egui::Rect, Response) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), height));
    let row = ui.interact(rect, ui.id().with(of), egui::Sense::click());

    (rect, row)
}

/// The two buttons a row in the bar ends with
///
/// Info opens a panel about whatever the row names, and close lets go of it.
/// Close stands outermost, where a window's own close button stands, so that
/// the gesture is in the same place wherever it is offered.
struct Buttons {
    /// Nothing where the row names nothing a panel could describe
    info: Option<Response>,
    close: Response,
}

/// The glyphs those buttons are drawn with, outermost first
const GLYPHS: [&str; 2] = [CLOSE, INFO];

/// Lay the buttons out without placing them
///
/// A row needs their width before it can be allocated, since what is left is
/// the room its name has, and it cannot be painted into before it exists. So
/// they are measured here and placed by [`place_buttons`] once there is a row
/// to place them in.
fn lay_out_buttons(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    lay_out(ui, &GLYPHS)
}

/// The close button alone, for a row with nothing to describe
///
/// Close is outermost, so a row that ends here ends where every other row
/// ends and the column of buttons reads straight down.
fn lay_out_close(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    lay_out(ui, &GLYPHS[..1])
}

fn lay_out(ui: &Ui, glyphs: &[&str]) -> Vec<std::sync::Arc<egui::Galley>> {
    glyphs
        .iter()
        .map(|glyph| {
            // Laid out in nothing, so that the color can be chosen once the
            // pointer has been asked about, which cannot happen until the row
            // has been placed.
            egui::WidgetText::from(
                egui::RichText::new(*glyph).color(egui::Color32::PLACEHOLDER),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Extend),
                f32::INFINITY,
                egui::TextStyle::Body,
            )
        })
        .collect()
}

/// How much room the buttons take at the end of a row, gaps included
fn buttons_width(buttons: &[std::sync::Arc<egui::Galley>], gap: f32) -> f32 {
    buttons.iter().map(|button| button.size().x + gap).sum()
}

/// Paint the buttons into the right hand end of `rect` and answer for each
///
/// Asked about after the row they sit in, so that they are the ones answering
/// where they overlap it. Under it the row would have to work out what it was
/// not being clicked on.
fn place_buttons(
    ui: &mut Ui,
    rect: egui::Rect,
    buttons: Vec<std::sync::Arc<egui::Galley>>,
    of: impl std::hash::Hash,
) -> Buttons {
    let middle = rect.center().y;
    let gap = ui.spacing().item_spacing.x;
    let mut right = rect.right() - ROW_PADDING;
    let mut answers = Vec::new();

    for (which, galley) in buttons.into_iter().enumerate() {
        let width = galley.size().x;
        let at = egui::Rect::from_min_max(
            egui::pos2(right - width, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let response = ui.interact(
            at,
            ui.id().with((&of, "row-button", which)),
            egui::Sense::click(),
        );
        // Lit for the pointer resting on it and for the keyboard reaching it
        // alike. A stop that shows nothing when it is reached reads as the
        // focus having gone missing.
        //
        // The glyph brightens and nothing is painted behind it. The row it
        // sits in lights up under the pointer already, and a second rectangle
        // inside that one reads as a button dropped into a row rather than as
        // part of it.
        let lit = response.hovered() || response.has_focus();
        let height = galley.size().y;
        ui.painter().galley(
            egui::pos2(at.left(), middle - height / 2.),
            galley,
            if lit {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            },
        );

        right = at.left() - gap;
        answers.push(response.on_hover_cursor(egui::CursorIcon::PointingHand));
    }

    let mut answers = answers.into_iter();
    // In `GLYPHS` order, which is close first. A row laid out with the close
    // button alone ends there.
    let close = answers.next().expect("a close button");
    let info = answers.next();
    Buttons { info, close }
}

/// How far a line in a list holds its text off its own edge
pub(crate) const LINE_PADDING: f32 = 3.;

/// One full width line of a list, and the pointer's answer to it
///
/// The whole line answers rather than the letters on it, so that a short name
/// is as easy to hit as a long one and a list reads as a column of controls
/// rather than as text that happens to be clickable. Laid out and painted for
/// the reason the rows in the bar are: a label is a widget in its own right,
/// and one inside a row that also answers leaves the two bidding for the
/// pointer.
///
/// `reserved` is room kept clear at the right hand end, which the caller
/// paints into itself. The rect handed back is the whole line, so it knows
/// where that room ended up.
///
/// A line that is not a `control` is laid out the same and answers to nothing:
/// it neither lights under the pointer nor takes the hand cursor, so a list
/// holding one keeps its shape without offering something that cannot be had.
pub(crate) fn line(
    ui: &mut Ui,
    text: egui::RichText,
    reserved: f32,
    control: bool,
) -> (egui::Rect, Response) {
    let room = ui.available_width() - LINE_PADDING * 2. - reserved;
    let text = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        room.max(0.),
        egui::TextStyle::Body,
    );

    let height = text.size().y;
    let (rect, answer) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height + LINE_PADDING * 2.),
        if control { egui::Sense::click() } else { egui::Sense::empty() },
    );

    if control && (answer.hovered() || answer.has_focus()) {
        ui.painter().rect_filled(
            rect,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    ui.painter().galley(
        egui::pos2(rect.left() + LINE_PADDING, rect.center().y - height / 2.),
        text,
        // A real color, since a line is laid out from whatever the caller
        // hands over and that is usually plain text. Plain text carries no
        // color of its own, so it comes out of layout as a placeholder for
        // this to answer, and a placeholder answered by a placeholder reaches
        // the tessellator, which panics rather than guess.
        ui.visuals().text_color(),
    );

    if control {
        (rect, answer.on_hover_cursor(egui::CursorIcon::PointingHand))
    } else {
        (rect, answer)
    }
}

/// One system's line in a list, and what a click on it asked for
///
/// Every list of systems the map draws is this line: the ones a search found
/// and the ones a filter admits, so far. They are the same thing in two
/// places, and a change to how a system is picked out of a list belongs in one
/// of them rather than in each.
///
/// `trailing` is what stands at the right hand end, before the mark. Usually
/// how far off the system is, and in the same slot whatever it says, so the
/// column reads down.
///
/// `salt` keys the mark apart from the marks on the lines around it. The
/// caller chooses it, knowing what its own list does between one pass and the
/// next.
pub(crate) fn system_line(
    ui: &mut Ui,
    name: &str,
    trailing: Option<String>,
    salt: impl std::hash::Hash,
) -> Option<SystemAction> {
    let gap = ui.spacing().item_spacing.x;
    let trailing = trailing.map(|text| {
        egui::WidgetText::from(egui::RichText::new(text).weak()).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    });

    // Every list the map draws is of systems it can place, so every line
    // carries the mark that opens a panel on one.
    let mark = {
        // Laid out in nothing, so the color can be chosen once the pointer
        // has been asked about, which cannot happen until the line has been
        // placed.
        egui::WidgetText::from(
            egui::RichText::new(INFO).color(egui::Color32::PLACEHOLDER),
        )
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    };

    let reserved = mark.size().x
        + gap
        + trailing.as_ref().map_or(0., |text| text.size().x + gap);
    let (rect, answer) = line(ui, egui::RichText::new(name), reserved, true);
    let middle = rect.center().y;

    // Asked about after the line, so that it is the one answering where the
    // two overlap. Under it the line would have to work out what it was not
    // being clicked on.
    let describing = {
        let at = egui::Rect::from_min_max(
            egui::pos2(rect.right() - LINE_PADDING - mark.size().x, rect.top()),
            egui::pos2(rect.right() - LINE_PADDING, rect.bottom()),
        );
        let answer = ui.interact(
            at,
            ui.id().with(("describe", salt)),
            egui::Sense::click(),
        );
        // Brightened alone, as the marks in the bar are. The line beneath it
        // already lights up under the pointer, and a rectangle inside that
        // one reads as a button dropped into the line.
        let lit = answer.hovered() || answer.has_focus();
        let height = mark.size().y;
        ui.painter().galley(
            egui::pos2(at.left(), middle - height / 2.),
            mark,
            if lit {
                ui.visuals().strong_text_color()
            } else {
                ui.visuals().weak_text_color()
            },
        );
        (at, answer.on_hover_cursor(egui::CursorIcon::PointingHand))
    };

    // Between the name and the mark, right against the mark, so the distances
    // line up down the list rather than following the names.
    if let Some(text) = trailing {
        let size = text.size();
        let right = describing.0.left() - gap;
        ui.painter().galley(
            egui::pos2(right - size.x, middle - size.y / 2.),
            text,
            egui::Color32::PLACEHOLDER,
        );
    }

    // The mark first, then the double. Egui answers the first click of a pair
    // as a click and the second as a double, so a line double clicked has
    // already been picked out by the time this is asked, which is what the
    // first click of the pair was for.
    if describing.1.clicked() {
        Some(SystemAction::Describe)
    } else if answer.double_clicked() {
        Some(SystemAction::Travel)
    } else if answer.clicked() {
        Some(SystemAction::Select { gathering: gathering(ui) })
    } else {
        None
    }
}

/// A scrolling list whose bar stands beside its contents rather than over them
///
/// Egui floats a scroll bar over the top right corner of what it is
/// scrolling. That reads well over a paragraph and not at all over a list
/// whose lines carry a control at that end: the bar and the mark are drawn on
/// the same few pixels, and whichever the pointer lands on is a coin toss.
///
/// A bar that is laid out is taken out of the room its contents are given, so
/// a line ends where the bar begins and the two never meet. It costs the
/// width of the bar, and only while there is more to scroll to: egui shows
/// one when it is needed and takes no room when it is not.
///
/// Grows to `height` and no further, and no taller than what is in it, so a
/// list of three lines is three lines rather than one with room going spare.
///
/// The room is asked for rather than read off the `Ui`. A scroll area holds
/// itself to whatever height it is offered, and inside an [`egui::Area`] what
/// is on offer is the height the area came out at *last* frame: egui lays an
/// area's contents out in the rectangle the last pass left behind. So a list
/// that came back under a bar which had been shut was offered the shut bar's
/// height — three lines of the five — and it stayed three, the area coming
/// out that tall again on the strength of it and offering no more the frame
/// after. Asked for, the room is the room whatever the area last was; the
/// rectangle is allocated at the height wanted and the space actually taken
/// is what the caller is charged, so a short list is still short.
pub(crate) fn scrolling<R>(
    ui: &mut Ui,
    height: f32,
    salt: impl std::hash::Hash,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    // In a ui of its own, since a style set on a `Ui` is set on the rest of
    // that `Ui`, and this is asked for by the list rather than by whatever
    // follows it.
    ui.allocate_ui(egui::vec2(ui.available_width(), height), |ui| {
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical()
            // Named by the caller, since the bar holds several of these at
            // once and a scroll area left to work its own id out from where it
            // sits gets the same one as the next: egui says so out loud, in
            // red, over the map. Each list is its own place to have scrolled
            // to anyway, and where one has been scrolled to says nothing about
            // the others.
            .id_salt(salt)
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, contents)
            .inner
    })
    .inner
}

/// Whether the user has just finished with a field by pressing return
fn entered(response: &Response, ui: &Ui) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// How far a text field's text stands from its own edge
///
/// Egui's own is tighter above and below than it is at the sides, which
/// leaves a field looking squeezed against the one under it.
const FIELD_PADDING: egui::Margin =
    egui::Margin { left: 4, right: 4, top: 4, bottom: 4 };

/// How tall a row holding one text field comes to
///
/// What [`singleline`] takes: one row of the face the chrome is lettered in,
/// which is what a single line `TextEdit` asks for, and [`FIELD_PADDING`]
/// above and below it. No floor at `interact_size`: egui puts one under a
/// button and not under a field.
///
/// Read by [`dated`] as well, whose first row is a line of text rather than a
/// field and which stands level with the bar's box. Worked out rather than
/// measured off a drawn field, the pane that wants it being a different pane
/// and drawn before the field is. Held to the field itself by
/// `the_reading_stands_level_with_the_box`.
fn field_height(ui: &Ui) -> f32 {
    ui.text_style_height(&egui::TextStyle::Body)
        + (FIELD_PADDING.top + FIELD_PADDING.bottom) as f32
}

/// The border a text field keeps while nothing is happening to it
///
/// Egui draws a field at rest with no fill and no border, leaving nothing on
/// screen but the text in it. That reads well enough inside a pane with an
/// edge of its own, and not at all against the map.
const FIELD_BORDER: egui::Stroke =
    egui::Stroke { width: 1., color: egui::Color32::from_gray(90) };

/// One text field, showing what it wants when it holds nothing
///
/// `reserved` is room kept clear inside the right hand end, which the caller
/// paints into itself. Kept by widening the field's own margin rather than by
/// narrowing the field, so that the box goes on reaching the full width and a
/// name long enough runs up to what is standing in there rather than under
/// it.
///
/// `placeholer` names the field as well as standing in it, so two fields in
/// one form want two of them. It is the only name a field has, so it is drawn
/// whenever the field is empty, whether or not the caret is in it: a field
/// typed into and emptied again, or one the form put the caret in without
/// being asked, would otherwise be a blank box with nothing on screen to say
/// what belongs in it.
fn singleline(
    ui: &mut Ui,
    value: &mut Option<String>,
    placeholer: &str,
    reserved: f32,
    waiting: bool,
) -> Response {
    // Named rather than left to the running count, so that whether the field
    // is being typed into can be asked before it is drawn. What it draws
    // depends on the answer.
    let id = ui.id().with(("field", placeholer));
    let editing = ui.memory(|memory| memory.has_focus(id));

    // Whether what it wants stands in the field as its contents. It does
    // while the caret is elsewhere, so a field holding nothing is a field
    // holding those words. Under the caret they are the hint instead, since
    // contents there are about to be typed into.
    //
    // Read off what the field holds and who is typing, rather than kept up as
    // the focus comes and goes. A field is drawn every frame and the focus
    // moves between two of them in one, so a placeholder put back by the
    // moment of losing it is a placeholder that stays away whenever that
    // moment is not seen. Nothing typed is nothing typed, and an empty string
    // is nothing typed however the field came to hold one.
    let wanting = typed(value).is_none() && !editing;
    let mut text = if wanting {
        placeholer.to_owned()
    } else {
        value.clone().unwrap_or_default()
    };
    // Room for the spinner, inside whatever the caller has already kept for
    // itself, so the two stand side by side rather than one over the other.
    let turning = if waiting {
        ui.text_style_height(&egui::TextStyle::Body) * SPINNER
    } else {
        0.
    };
    let gap = ui.spacing().item_spacing.x;
    let kept = reserved + if waiting { turning + gap } else { 0. };
    let margin = egui::Margin {
        right: FIELD_PADDING.right + kept as i8,
        ..FIELD_PADDING
    };

    // In a scope of its own, since a style set on a `Ui` is set on the rest
    // of that `Ui`: the grey a field wants for what it is holding would go
    // on to grey the headings under it.
    let response = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = FIELD_BORDER;
            if wanting {
                ui.visuals_mut().override_text_color =
                    Some(egui::Color32::GRAY);
            }
            ui.add_sized(
                egui::vec2(ui.available_width(), 0.),
                // The hint answers the field being empty with the caret in
                // it, which is the one empty state `wanting` does not cover:
                // the words cannot stand in the field as its contents there,
                // since the caret is about to be typed into them.
                egui::TextEdit::singleline(&mut text)
                    .id(id)
                    .margin(margin)
                    .hint_text(placeholer),
            )
        })
        .inner;

    // The words the field was standing there wanting are not words anybody
    // typed, so a field showing them holds nothing whatever is in the box.
    if !wanting {
        *value = Some(text);
    }

    // Inside the field's own right hand end, where the room was kept, and
    // clear of whatever the caller keeps room for out beyond it. A question
    // is answered under the field it was typed into, so where it is coming
    // from is said in the field itself rather than off in a corner.
    if waiting {
        let rect = response.rect;
        let at = egui::Rect::from_center_size(
            egui::pos2(
                rect.right() - FIELD_PADDING.right as f32 - reserved + gap / 2.
                    - turning / 2.,
                rect.center().y,
            ),
            egui::Vec2::splat(turning),
        );
        egui::Spinner::new().paint_at(ui, at);
    }
    response
}

fn poll_value(ui: &mut Ui, opt: &mut Option<f64>) {
    let mut enabled = opt.is_some();
    if check(ui, &mut enabled, "Poll", "Fetch fresh data periodically")
        .changed()
    {
        if enabled {
            // Turned back on at what it opened at, the wait it was left at
            // having gone when it was turned off.
            *opt = Some(10.);
        } else {
            *opt = None
        }
    }

    // The unit stands in the box with the number, so the row is the name of
    // the thing and the value of it and nothing between them.
    if let Some(val) = opt {
        ui.add(
            egui::DragValue::new(val).range(0.0..=60.).speed(0.01).suffix(" s"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::tests::row;
    use crate::systems::filter::Filter;
    use crate::systems::selection::PickedBody;
    use crate::tests::{painted, words};
    use chrono::{DateTime, Utc};

    /// A moment out in the galaxy, ours
    fn ours(text: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(text)
            .expect("the fixture is a moment")
            .with_timezone(&Utc)
    }

    /// A clock standing at `text`, in our own time
    fn standing(text: &str) -> Clock {
        let mut clock = Clock::default();
        clock.follows(ours(text));
        clock
    }

    /// The game's calendar runs 1286 years ahead of ours and otherwise with it
    ///
    /// The journal stamps its scans in our own time, so the span the clock
    /// holds needs no converting and only the year it lands in does.
    #[test]
    fn the_games_calendar_runs_1286_years_ahead() {
        let clock = standing("2014-12-16T13:45:00Z");

        assert_eq!(drawn_at(&clock), "16 DEC 3300 13:45:00");
    }

    /// And the run-on is carried into it
    ///
    /// The moment on screen is the present the map is standing at plus
    /// however far a slider has run it on.
    #[test]
    fn the_moment_shown_carries_how_far_the_map_has_run_on() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_to(365. * 86_400., 1.);

        assert_eq!(
            drawn_at(&clock),
            "01 JAN 3302 00:00:00",
            "a year on from a new year is the next one"
        );
    }

    /// A leap day reads as the last day of its own February
    ///
    /// 1286 years on from one of ours is not always a year with a 29th in it,
    /// and there is no such date to show. The 28th is the nearest there is.
    #[test]
    fn a_leap_day_reads_as_the_last_of_its_february() {
        let clock = standing("2024-02-29T09:00:00Z");

        assert_eq!(drawn_at(&clock), "28 FEB 3310 09:00:00");
    }

    /// A year past four digits is said as a year, without a sign
    ///
    /// Reported from the map: `18 MAR +10284 17:06:50`. `%Y` marks a year
    /// outside the four-digit range as an expanded one the ISO way, and a `+`
    /// in the middle of a date reads as a span. Reachable without trying: the
    /// calendar already stands 1286 years on, and the slider covers one turn
    /// of the system's widest orbit, which for a wide pair is millennia.
    #[test]
    fn a_year_past_four_digits_is_said_without_a_sign() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_to(9000. * 365.25 * 86_400., 1.);

        assert_eq!(drawn_at(&clock), "10 MAR 12301 00:00:00");
    }

    /// The date is said with no system held at all
    ///
    /// The moment is the galaxy's rather than a system's, so there is one to
    /// read out between systems as much as inside one -- and `Now`, the only
    /// way to let go of a run-on, rides that line. Drawn off the held
    /// system's newest scan, the line went out on the way between two systems
    /// and took the way back with it.
    #[test]
    fn the_date_is_said_without_a_system_held() {
        let mut clock = standing("2015-01-01T00:00:00Z");
        let said = words(|ui| {
            dated(
                ui,
                &mut clock,
                Turns::default(),
                &mut ClockControl::default(),
            );
        });

        assert!(said.contains(&"01 JAN 3301 00:00:00".to_owned()), "{said:?}");
    }

    /// Clicking the date opens the slider under it, and clicking it again
    /// puts it away
    ///
    /// The reading is where a user meets the clock, so a reader who wants
    /// another moment reaches for the moment on screen rather than hunting the
    /// pane for a control they have never seen. Drawing the line opens
    /// nothing on its own.
    #[test]
    fn clicking_the_date_opens_the_slider_under_it() {
        let ctx = crate::tests::context();
        let turn = 400. * 86_400.;
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |input, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                dated(ui, &mut clock, system_turn(turn), control);
                at = ui.min_rect();
            });
            at
        };

        // Two passes with nothing happening, to place the line.
        let _ = line(egui::RawInput::default(), &mut control);
        let at = line(egui::RawInput::default(), &mut control);
        assert!(!control.out, "the line opened the slider unbidden");

        let date = at.left_center() + egui::vec2(4., 0.);
        line(clicking(date), &mut control);
        assert!(control.out, "a click on the date opened nothing");

        line(clicking(date), &mut control);
        assert!(!control.out, "a second click left the slider out");
    }

    /// Every word painted in `output`
    ///
    /// [`words`] runs a pass of its own, which is no use where what is wanted
    /// is what one pass of several said.
    fn spoken(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => {
                    into.push(text.galley.text().to_owned())
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// The pass that asks for the rail does not draw it
    ///
    /// Reported: opening the strip, the slider looked to be outside its own
    /// panel for a moment. The pane is framed and sized before anything in it
    /// is drawn — see [`Dropping`] — so a rail drawn on the pass the click
    /// arrived is a rail drawn in a strip still the width of the reading,
    /// with no frame under it and no fill behind it.
    ///
    /// The marks are how it shows: they are the only words the rail paints,
    /// so a pass with `now` in it is a pass that drew a rail.
    #[test]
    fn the_pass_that_opens_the_rail_does_not_draw_it() {
        let ctx = crate::tests::context();
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |input, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let output = ctx.run_ui(input, |ui| {
                ui.set_width(STRIP_WIDTH);
                dated(ui, &mut clock, system_turn(400. * DAY), control);
                at = ui.min_rect();
            });
            (at, spoken(&output))
        };

        // Two passes with nothing happening, to place the line.
        let _ = line(egui::RawInput::default(), &mut control);
        let (at, said) = line(egui::RawInput::default(), &mut control);
        assert!(!said.contains(&"now".to_owned()), "a rail unbidden: {said:?}");

        let date = at.left_center() + egui::vec2(4., 0.);
        let (_, asking) = line(clicking(date), &mut control);
        assert!(control.out, "a click on the date opened nothing");
        assert!(
            !asking.contains(&"now".to_owned()),
            "the rail was drawn on the pass that asked for it: {asking:?}"
        );

        // And the next pass has the frame and the width waiting for it.
        let (_, drawn) = line(egui::RawInput::default(), &mut control);
        assert!(drawn.contains(&"now".to_owned()), "{drawn:?}");
    }

    /// And so does clicking the span past the present
    ///
    /// The date and the span are halves of the one moment. A reader who has
    /// run the map on is reading the span, and that is the number they reach
    /// for to change it.
    #[test]
    fn clicking_the_span_opens_the_slider_too() {
        let ctx = crate::tests::context();
        let turn = 400. * 86_400.;
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut line = |offset: f64, input, control: &mut ClockControl| {
            let mut clock_ = Clock::default();
            clock_.offset_at(offset);
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                dated(ui, &mut clock_, system_turn(turn), control);
                at = ui.min_rect();
            });
            clock = clock_;
            at
        };

        // The date alone, to measure how far along the row the span starts.
        let _ = line(0., egui::RawInput::default(), &mut control);
        let dateless = line(0., egui::RawInput::default(), &mut control);

        // And again with the map run on, so the span stands beside it.
        let span = 200. * 86_400.;
        let _ = line(span, egui::RawInput::default(), &mut control);
        let whole = line(span, egui::RawInput::default(), &mut control);
        assert!(
            whole.width() > dateless.width(),
            "the span was not drawn beside the date"
        );

        let on = egui::pos2(dateless.right() + 8., dateless.center().y);
        line(span, clicking(on), &mut control);

        assert!(control.out, "a click on the span opened nothing");
        assert_eq!(clock.offset(), span, "the click moved the map");
    }

    /// A rail geared past the ceiling stops where a date runs out
    ///
    /// Reported as a crash: `DateTime + TimeDelta` overflowed. The ceiling is
    /// the room left between now and the end of chrono's calendar once the
    /// game's 1286 years are added, so the far end of even an absurd rail is
    /// a moment that can still be written. Only a turn longer than that
    /// reaches it, which is a wide pair's and nothing a body has.
    #[test]
    fn a_rail_past_the_ceiling_stops_at_it() {
        let year = 365.25 * 86_400.;
        let ran = slid(Geared::System(400_000. * year), &[(0., 2.)]);

        assert_eq!(ran.offset(), Clock::CEILING);

        let mut clock = standing("2015-01-01T00:00:00Z");
        clock.offset_at(ran.offset());
        assert_eq!(drawn_at(&clock), "19 FEB 253306 00:00:00");
    }

    /// The status slider runs the system on by one turn of its widest orbit
    ///
    /// Which is the whole point of gearing it to that one: a control over a
    /// whole system has to reach every arrangement the system passes through,
    /// and nothing past them. Run end to end it comes to exactly that turn,
    /// however far the drag is carried on past the rail.
    #[test]
    fn the_status_slider_runs_the_system_on_by_its_widest_turn() {
        let turn = 400. * 86_400.;
        let clock = slid(Geared::System(turn), &[(0., 2.)]);

        assert_eq!(
            clock.offset(),
            turn,
            "the slider ran the map on by something other than a turn"
        );
    }

    /// And its near end is decades of span rather than a even share of one
    ///
    /// The spans worth asking for are not evenly spread: a system's widest
    /// orbit takes a median eighteen years and its fastest body a few hours,
    /// so half the rail spent on half of eighteen years is a control that
    /// cannot be nudged by an hour at all. Halfway along a logarithmic rail
    /// stands at the geometric middle instead, which is hours rather than
    /// years.
    #[test]
    fn the_status_slider_is_finer_near_the_present() {
        let turn = 400. * 86_400.;
        let middle = slid(Geared::System(turn), &[(0., 0.5)]).offset();

        assert!(middle > 0., "halfway along the rail moved nothing");
        assert!(
            middle < turn / 100.,
            "halfway along the rail stood at {middle} of {turn}, \
             which is an even share of the turn"
        );
    }

    /// And the far end is a place to come back from
    ///
    /// Reported: dragged to the right end, the rail broke and the reading
    /// stuck at the ceiling until `Now`. It set a phase, and a whole turn
    /// reads as none of one, so the far end put the handle back at the near
    /// end with the map a turn out and the next drag measured from the turn
    /// after that -- which, where the turn was the ceiling, had nowhere to
    /// go. The rail sets the span outright, so a second drag means what it
    /// says wherever the first one left the map.
    #[test]
    fn the_status_slider_comes_back_from_its_far_end() {
        let turn = 400. * 86_400.;
        let back =
            slid(Geared::System(turn), &[(0., 2.), (0.98, 0.5)]).offset();

        assert!(back > 0., "the map came back further than it was dragged");
        assert!(
            back < turn / 100.,
            "a drag back to the middle of the rail left the map at {back} \
             of {turn}"
        );
    }

    /// And nowhere along it does the reading move under a still hand
    ///
    /// Reported: dragging out past some ten thousand years, the time flicked
    /// between two numbers until the drag carried on past it, and again on
    /// the way back down. The rail took its width from what the line above it
    /// left over, and that line grows with the reading -- a fifth digit in
    /// the year, a longer span beside it -- so the width the rail was
    /// measured at moved with the span the rail had last set. One width asked
    /// for the span that asked for the other, frame after frame.
    ///
    /// Walked pixel by pixel, every one of them held for two frames, because
    /// the cycle only appears where the line's width crosses the bar's and no
    /// single place along the rail would have found it.
    #[test]
    fn the_reading_holds_still_all_along_the_rail() {
        let ctx = crate::tests::context();
        let year = 365.25 * 86_400.;
        let mut clock = Clock::default();
        let control = |input, clock: &mut Clock| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                ui.set_width(BAR_WIDTH);
                dated(
                    ui,
                    clock,
                    system_turn(Clock::CEILING),
                    &mut ClockControl { out: true, ..Default::default() },
                );
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default(), &mut clock);
        let at = control(egui::RawInput::default(), &mut clock);
        let y = at.bottom() - 8.;
        let start = egui::pos2(at.left() + 4., y);
        control(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(start),
                    egui::Event::PointerButton {
                        pos: start,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::default(),
                    },
                ],
                ..Default::default()
            },
            &mut clock,
        );

        let mut moved = Vec::new();
        for step in 0..(at.width() as i32) {
            let on = egui::pos2(start.x + step as f32, y);
            let mut held = Vec::new();
            for _ in 0..2 {
                control(
                    egui::RawInput {
                        events: vec![egui::Event::PointerMoved(on)],
                        ..Default::default()
                    },
                    &mut clock,
                );
                held.push(clock.offset() / year);
            }
            if held[0] != held[1] {
                moved.push(format!(
                    "{step}px: {:.2}y then {:.2}y",
                    held[0], held[1]
                ));
            }
        }

        assert!(moved.is_empty(), "the reading moved on its own: {moved:?}");
    }

    /// Where the strip stood, drawn over a viewport `across` wide
    ///
    /// Several frames, since a strip that has just been opened is measured
    /// from the pass before it: an area is placed at the size it last came
    /// out at.
    fn stripped(across: f32, chrome_right: f32, out: bool) -> egui::Rect {
        let ctx = crate::tests::context();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let mut clock = Clock::default();
        let mut control = ClockControl { out, ..Default::default() };
        let mut at = egui::Rect::NOTHING;
        for _ in 0..4 {
            let _ = ctx.run_ui(input.clone(), |ui| {
                at = time_strip(
                    ui.ctx(),
                    chrome_right,
                    &mut clock,
                    &mut control,
                    system_turn(400. * DAY),
                );
            });
        }

        at
    }

    /// The reading stands in the middle of the top edge
    #[test]
    fn the_strip_stands_in_the_middle_of_the_top() {
        let wide = stripped(1600., 400., false);

        assert!(
            (wide.center().x - 800.).abs() < 2.,
            "the reading stood at {}, not the middle of 1600",
            wide.center().x
        );
        assert!(wide.top() >= MARGIN, "{wide:?} stood against the top edge");
    }

    /// Shut, it claims no more of the sky than the words take
    ///
    /// It is drawn over the map, and an area is the pointer's wherever it
    /// reaches. Laid out at the width the scrubber wants, a shut strip would
    /// take a band across the top of the viewport for a line of text, and a
    /// wheel turned up there would stop turning the map.
    #[test]
    fn a_shut_strip_takes_only_the_room_the_reading_wants() {
        let shut = stripped(1600., 400., false);

        assert!(
            shut.width() < STRIP_WIDTH / 2.,
            "a reading took {} of {STRIP_WIDTH}",
            shut.width()
        );
    }

    /// How wide a pane came out, holding one short word
    ///
    /// Several passes, an area being placed at the size it last came out at.
    fn dropped(out: bool, holds_width: bool) -> egui::Rect {
        let ctx = crate::tests::context();
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

    /// A pane in the middle stands where it belongs the first pass it is out
    ///
    /// Reported as opening badly: it appeared, then moved and grew, all
    /// inside a frame or two. A pane is placed before it is drawn, so it has
    /// to be placed by a width it does not have yet; taken off the area's own
    /// rect that is last pass's width, which on the pass a pane opens on is
    /// the width of the reading it has just stopped being — a strip half the
    /// width, centered as though it were still shut, and then a jump.
    ///
    /// So the width the body is about to take is worked out rather than
    /// remembered, and this is what holds it to that: shut for a few passes,
    /// then one pass out, and the strip is where it will still be on the
    /// next.
    #[test]
    fn a_pane_opens_where_it_will_stand() {
        let across = 1600.;
        let ctx = crate::tests::context();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let mut clock = Clock::default();
        let mut control = ClockControl::default();
        let mut stood = |ctx: &Context, control: &mut ClockControl| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input.clone(), |ui| {
                at = time_strip(
                    ui.ctx(),
                    400.,
                    &mut clock,
                    control,
                    system_turn(400. * DAY),
                );
            });
            at
        };

        for _ in 0..4 {
            stood(&ctx, &mut control);
        }

        // The pass it opens on, and the pass after it, which is where the
        // strip settles.
        control.out = true;
        let opened = stood(&ctx, &mut control);
        let settled = stood(&ctx, &mut control);

        assert!(
            (opened.center().x - settled.center().x).abs() < 1.,
            "it opened at {} and settled at {}",
            opened.center().x,
            settled.center().x
        );
        assert!(
            (opened.width() - settled.width()).abs() < 1.,
            "it opened {} wide and settled at {}",
            opened.width(),
            settled.width()
        );
        assert!(
            (settled.center().x - across / 2.).abs() < 2.,
            "and settled off center, at {}",
            settled.center().x
        );

        // And the same on the way back, which is the other half of the same
        // report: the pass it shuts on is placed by what it came to the last
        // time it stood shut rather than by the open width it has just
        // stopped being.
        control.out = false;
        let shut = stood(&ctx, &mut control);
        let resting = stood(&ctx, &mut control);

        assert!(
            (shut.center().x - resting.center().x).abs() < 1.,
            "it shut at {} and came to rest at {}",
            shut.center().x,
            resting.center().x
        );
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

    /// Out, it is at least as wide as the rail it holds
    ///
    /// The rail is scaled to [`RAIL_WIDTH`] whatever room it is given — see
    /// [`clock_control`] for why it is a number — so a strip narrower than
    /// that is a rail painted out past its own frame.
    #[test]
    fn an_open_strip_holds_its_rail() {
        let out = stripped(1600., 400., true);

        assert!(
            out.width() >= RAIL_WIDTH,
            "{} against a rail of {RAIL_WIDTH}",
            out.width()
        );
        // And is the wider of the two states by some way, the reading alone
        // being a fraction of it.
        assert!(out.width() > stripped(1600., 400., false).width() * 2.);
    }

    /// Where the reading stood, drawn over a viewport `across` wide
    ///
    /// Several passes, since an area paints nothing at all on the first of
    /// them and is placed by what it last came to.
    fn read_at(
        ctx: &Context,
        across: f32,
        chrome_right: f32,
        clock: &mut Clock,
        control: &mut ClockControl,
    ) -> egui::Rect {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(across, 800.),
            )),
            ..Default::default()
        };
        let said = drawn_at(clock);
        let mut found = None;
        for _ in 0..3 {
            let output = ctx.run_ui(input.clone(), |ui| {
                time_strip(
                    ui.ctx(),
                    chrome_right,
                    clock,
                    control,
                    system_turn(400. * DAY),
                );
            });
            found = spoken_at(&output, &said);
        }

        found.unwrap_or_else(|| panic!("the date was not drawn: {said}"))
    }

    /// Up against the bar, the reading holds still rather than sliding into it
    ///
    /// The pane is centered on the viewport, so a body wider than the head
    /// grows to both sides of it and the reading slides left as the rail comes
    /// out. There is room for that on a wide window and none on a narrow one,
    /// where what it slides into is the bar: the rail arrived with its near
    /// end against the search box and the date it was opened from had moved.
    ///
    /// So a pane that cannot be centered clear of the bar stands where its
    /// head stood instead, and grows out to the right.
    #[test]
    fn a_strip_against_the_bar_does_not_slide_into_it() {
        let chrome_right = MARGIN + GEAR_ROOM + MARGIN + BAR_WIDTH;
        // Room for the reading in the middle, and none for the rail there.
        let across = chrome_right * 2. + STRIP_WIDTH / 2.;
        let ctx = crate::tests::context();
        let mut clock = Clock::default();
        let mut control = ClockControl::default();

        let shut =
            read_at(&ctx, across, chrome_right, &mut clock, &mut control);
        assert!(
            (shut.center().x - across / 2.).abs() < 4.,
            "the reading stood at {} of {across} shut",
            shut.center().x
        );

        control.out = true;
        let out = read_at(&ctx, across, chrome_right, &mut clock, &mut control);

        assert!(
            (out.left() - shut.left()).abs() < 1.,
            "the reading was at {} and opening took it to {}",
            shut.left(),
            out.left()
        );
        assert!(
            out.left() > chrome_right + MARGIN,
            "and it stood against the bar, at {}",
            out.left()
        );
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
        let ctx = crate::tests::context();
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

    /// Every word the pass painted, and where it was painted
    ///
    /// [`words`] answers what was said and not where, and where is the whole
    /// question when two things laid out from opposite ends share a line.
    fn placed(contents: impl FnOnce(&mut Ui)) -> Vec<(String, egui::Rect)> {
        let ctx = crate::tests::context();
        let mut contents = Some(contents);
        let output = ctx.run_ui(egui::RawInput::default(), |ui| {
            if let Some(contents) = contents.take() {
                contents(ui);
            }
        });

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

        let mut found = Vec::new();
        for shape in &output.shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// The reading stands level with the search box
    ///
    /// The three things across the top of the map are one row: the gear, the
    /// bar's box and the clock's reading. The gear is hung on the box's own
    /// middle, so it follows wherever the box goes; the reading is in a pane
    /// of its own and had to be put level by hand. It sat a few points high,
    /// a field being padded inside where a line of text is not.
    ///
    /// Drawn as [`chrome`] draws them: two panes at the same top, in the same
    /// frame, each holding its own first row.
    #[test]
    fn the_reading_stands_level_with_the_box() {
        let ctx = crate::tests::context();
        let mut clock = Clock::default();
        let mut typed = None;
        let mut box_at = egui::Rect::NOTHING;
        let mut painted = None;
        // Several passes: an area is placed at the size it last came out at,
        // and the first of them paints nothing at all.
        for _ in 0..4 {
            painted = Some(ctx.run_ui(egui::RawInput::default(), |ui| {
                let ctx = ui.ctx().clone();
                Dropping {
                    id: "a-bar",
                    standing: Standing::At(egui::pos2(MARGIN, MARGIN)),
                    out: false,
                    width: BAR_WIDTH,
                    holds_width: true,
                }
                .show(&ctx, |ui| {
                    box_at =
                        ask_box(ui, &mut typed, "Search", false, false).0.rect;
                });
                Dropping {
                    id: "a-strip",
                    standing: Standing::At(egui::pos2(600., MARGIN)),
                    out: false,
                    width: STRIP_WIDTH,
                    holds_width: false,
                }
                .show(&ctx, |ui| {
                    dated(
                        ui,
                        &mut clock,
                        Turns::default(),
                        &mut ClockControl::default(),
                    );
                });
            }));
        }

        let output = painted.expect("a pass was drawn");
        let said = drawn_at(&clock);
        let reading = spoken_at(&output, &said)
            .unwrap_or_else(|| panic!("the date was not drawn: {said}"));

        assert!(
            (reading.center().y - box_at.center().y).abs() < 1.,
            "the reading stood at {} and the box at {}",
            reading.center().y,
            box_at.center().y
        );
    }

    /// Where `word` was painted in `output`, if it was
    fn spoken_at(output: &egui::FullOutput, word: &str) -> Option<egui::Rect> {
        fn walk(
            shape: &egui::Shape,
            word: &str,
            found: &mut Option<egui::Rect>,
        ) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == word => {
                    *found = Some(egui::Rect::from_min_size(
                        text.pos,
                        text.galley.size(),
                    ));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, word, found);
                    }
                }
                _ => {}
            }
        }

        let mut found = None;
        for shape in &output.shapes {
            walk(&shape.shape, word, &mut found);
        }
        found
    }

    /// The widest line the strip can hold does not run into itself
    ///
    /// Reported: run all the way out, the reading ran clean through the
    /// switch. The line is a reading laid out from the left and controls laid
    /// out from the right, and egui does not stop the two meeting in the
    /// middle -- it widens the `Ui` and the words overlap.
    ///
    /// The widest of everything at once: the clock at its ceiling, which is
    /// the longest date and the longest span the map can stand at, and both
    /// turns on offer so that the switch is drawn beside `Now`.
    #[test]
    fn the_widest_reading_does_not_run_into_the_switch() {
        let mut clock = Clock::default();
        clock.offset_at(Clock::CEILING);
        let mut control = ClockControl { out: true, to: GearedTo::System };
        let said = placed(|ui| {
            ui.set_width(STRIP_WIDTH);
            dated(
                ui,
                &mut clock,
                both_turns(12. * YEAR, Clock::CEILING),
                &mut control,
            );
        });

        let at = |word: &str| {
            said.iter()
                .find(|(text, _)| text == word)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{word} was not drawn: {said:?}"))
        };
        // The span past the present is the last of the reading, and the
        // switch is the first of the controls at the other end.
        let span = at(&format!("+{}", briefly(Clock::CEILING, true)));
        let switch = at("Body");

        assert!(
            span.right() < switch.left(),
            "the reading reached {} and the switch began at {}",
            span.right(),
            switch.left()
        );
    }

    /// And on a narrow window it gives way to the bar
    ///
    /// The two are drawn from opposite rules — the bar from the left edge,
    /// the strip from the middle — so on a window narrow enough they meet.
    /// A reading standing over the field being typed into is worse than a
    /// reading standing off center.
    #[test]
    fn the_strip_gives_way_to_the_bar_on_a_narrow_window() {
        let chrome_right = MARGIN + GEAR_ROOM + MARGIN + BAR_WIDTH;
        let narrow = stripped(chrome_right + STRIP_WIDTH, chrome_right, true);

        assert!(
            narrow.left() >= chrome_right + MARGIN,
            "the strip stood at {} over a bar reaching {chrome_right}",
            narrow.left()
        );
    }

    /// The spans written under a rail geared to `turn`
    ///
    /// The whole of what the control letters: the rail itself shows no value,
    /// so every word painted here is a mark.
    fn marked(turn: f64) -> Vec<String> {
        let mut clock = Clock::default();
        words(|ui| {
            ui.set_width(STRIP_WIDTH);
            clock_control(ui, &mut clock, Some(Geared::System(turn)));
        })
    }

    /// The rail says where both of its ends are
    ///
    /// Which is what a logarithmic rail cannot say for itself: it runs from a
    /// minute to millennia and reads as a bare grey bar. The near end is the
    /// present and the far end is what the rail covers, and a reader wanting
    /// to know how far a drag will carry the map is asking about the second.
    #[test]
    fn the_rail_says_where_its_ends_are() {
        let said = marked(18. * YEAR);

        assert!(said.contains(&"now".to_owned()), "{said:?}");
        assert!(said.contains(&"18 y".to_owned()), "{said:?}");
        assert!(said.len() > 3, "nothing between the ends: {said:?}");
    }

    /// A mark stands where the handle stops, not near it
    ///
    /// The marks are laid out by [`along`] and the handle by egui, off the
    /// same range and the same logarithm. Said twice, so it is worth holding
    /// the two together: a rail whose words sit a tenth of the way off
    /// wherever the handle lands is worse than one with no words at all.
    #[test]
    fn a_mark_stands_where_the_handle_stops() {
        let turn = 400. * DAY;
        for asked in [0.25_f32, 0.5, 0.75] {
            let stood = slid(Geared::System(turn), &[(0., asked)]);
            let mark = along(stood.offset(), turn, false);

            assert!(
                (mark - asked).abs() < 0.03,
                "a drag to {asked} of the rail marked at {mark}"
            );
        }
    }

    /// A crowded rail drops marks rather than stacking them
    ///
    /// Every span [`MARKED`] holds falls inside ten thousand years, which is
    /// a dozen words in the width of the strip. What goes is a mark in the
    /// middle; the two ends stay, being what the rail is read by.
    #[test]
    fn a_crowded_rail_drops_marks_rather_than_stacking_them() {
        let said = marked(10_000. * YEAR);

        assert!(said.len() < MARKED.len(), "{said:?}");
        assert!(said.contains(&"now".to_owned()), "{said:?}");
        assert!(said.contains(&"10,000 y".to_owned()), "{said:?}");
        let mut once = said.clone();
        once.sort();
        once.dedup();
        assert_eq!(
            once.len(),
            said.len(),
            "a mark was written twice: {said:?}"
        );
    }

    /// Hiding the clock puts the map back to the present
    ///
    /// The reading is the only place a run-on is shown and the only way back
    /// from one, so a hidden strip over a map standing three hours on is a map
    /// drawing a moment nobody can see or undo.
    #[test]
    fn hiding_the_clock_puts_the_map_back_to_the_present() {
        let mut clock = Clock::default();
        clock.offset_at(3. * HOUR);
        let mut control = ClockControl { out: true, ..Default::default() };

        hidden(&mut clock, &mut control);

        assert_eq!(clock.offset(), 0.);
        assert!(!control.out, "the scrubber was left out over nothing");
    }

    /// A system's turn on offer and no body's
    fn system_turn(turn: f64) -> Turns {
        Turns { body: None, system: Some(turn) }
    }

    /// Both turns on offer, the body's and the system's
    fn both_turns(body: f64, system: f64) -> Turns {
        Turns { body: Some(body), system: Some(system) }
    }

    /// The strip asks which turn the rail is to cover, and answers it
    ///
    /// A body's own year and the span its whole system takes to come round
    /// are two questions, and which one a reader wants is not something the
    /// map can work out from what they clicked: the body picked out says they
    /// are looking at it, not what span they mean to drag over.
    #[test]
    fn the_rail_covers_whichever_turn_was_asked_for() {
        let turns = both_turns(11.9 * YEAR, 14_990. * YEAR);

        assert_eq!(
            turns.geared(GearedTo::Body),
            Some(Geared::Body(11.9 * YEAR))
        );
        assert_eq!(
            turns.geared(GearedTo::System),
            Some(Geared::System(14_990. * YEAR))
        );
    }

    /// And falls back to whichever turn there is
    ///
    /// The choice is between two spans to read. Whether the map can be run on
    /// at all is a different question, and a reader who last asked for a
    /// body's turn should not lose the rail by flying out of the system.
    #[test]
    fn a_turn_that_is_not_on_record_gives_way_to_the_one_that_is() {
        let system = system_turn(400. * DAY);

        assert_eq!(
            system.geared(GearedTo::Body),
            Some(Geared::System(400. * DAY)),
            "a body's turn was asked for where there is none"
        );
        assert_eq!(Turns::default().geared(GearedTo::System), None);
    }

    /// The switch stands only where there are two turns to choose between
    #[test]
    fn the_switch_is_offered_over_two_turns_and_not_one() {
        assert!(both_turns(11.9 * YEAR, 14_990. * YEAR).choice());
        assert!(!system_turn(400. * DAY).choice());
        assert!(!Turns::default().choice());
    }

    /// What the strip says, geared to `turns` and asking `to`
    fn strip_said(turns: Turns, to: GearedTo) -> Vec<String> {
        let mut clock = Clock::default();
        let mut control = ClockControl { out: true, to };
        words(|ui| {
            ui.set_width(STRIP_WIDTH);
            dated(ui, &mut clock, turns, &mut control);
        })
    }

    /// Both turns are named in the strip, and the marks follow the choice
    ///
    /// Which is what says the switch did anything: the rail is a grey bar
    /// either way, and what changes under it is how far one drag carries the
    /// map. Geared to a body of a dozen years the far mark is that dozen;
    /// geared to the system it is the thousands the widest orbit takes.
    #[test]
    fn the_marks_follow_whichever_turn_was_asked_for() {
        let turns = both_turns(12. * YEAR, 14_990. * YEAR);

        let body = strip_said(turns, GearedTo::Body);
        assert!(body.contains(&"Body".to_owned()), "{body:?}");
        assert!(body.contains(&"System".to_owned()), "{body:?}");
        assert!(body.contains(&"12 y".to_owned()), "{body:?}");

        let system = strip_said(turns, GearedTo::System);
        assert!(system.contains(&"14,990 y".to_owned()), "{system:?}");
        assert!(
            !system.contains(&"12 y".to_owned()),
            "the body's turn was still marked: {system:?}"
        );
    }

    /// And the switch is not drawn where there is nothing to switch to
    #[test]
    fn one_turn_is_read_without_a_switch() {
        let said = strip_said(system_turn(400. * DAY), GearedTo::System);

        assert!(!said.contains(&"Body".to_owned()), "{said:?}");
        assert!(!said.contains(&"System".to_owned()), "{said:?}");
    }

    /// Geared to a body, the rail is that body's own turn laid evenly
    ///
    /// The span asked for while a body is being watched is one orbit of it,
    /// and nothing about an orbit is lopsided: halfway along the rail is half
    /// a turn, as it is on that body's own phase slider. Which is the whole
    /// difference from the rail over a system, where the spans run from hours
    /// to millennia and only decades of them fit on one rail.
    #[test]
    fn a_rail_geared_to_a_body_is_laid_evenly() {
        let turn = 400. * 86_400.;
        let middle = slid(Geared::Body(turn), &[(0., 0.5)]).offset();

        assert!(
            (middle - turn / 2.).abs() < turn / 50.,
            "halfway along stood at {middle} of {turn}"
        );
        // And its far end is still that one turn, as the system's rail's is.
        assert_eq!(slid(Geared::Body(turn), &[(0., 2.)]).offset(), turn);
    }

    /// And a drag held out past the far end reads as one moment, not two
    ///
    /// Reported as a flicker at the handoff: the rail's far end and the
    /// furthest the clock would go were different numbers, so the stretch
    /// between them asked for spans the clock answered with the one it
    /// stopped at. The handle stood where the pointer put it, the reading
    /// stood at the ceiling, and the drag flicked between the two of them
    /// frame after frame. One range now, ending exactly where the clock does.
    #[test]
    fn a_drag_held_past_the_far_end_stands_at_one_moment() {
        let ctx = crate::tests::context();
        let year = 365.25 * 86_400.;
        // A pair whose own turn runs well past the ceiling, so the rail is
        // the ceiling's rather than the turn's.
        let turn = 400_000. * year;
        let mut clock = Clock::default();
        let control = |input, clock: &mut Clock| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                clock_control(ui, clock, Some(Geared::System(turn)));
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default(), &mut clock);
        let at = control(egui::RawInput::default(), &mut clock);
        for input in dragged(at.left_center(), at.width() * 3.) {
            control(input, &mut clock);
        }

        // Held there, hand still, while the frames go by.
        let mut seen = Vec::new();
        for _ in 0..8 {
            control(egui::RawInput::default(), &mut clock);
            seen.push(clock.offset());
        }

        assert!(
            seen.iter().all(|offset| *offset == Clock::CEILING),
            "the reading moved under a still hand: {seen:?}"
        );
    }

    /// The clock after the status slider is dragged from `from` to `to` of the
    /// rail's own width, gesture after gesture
    ///
    /// Past one is carried off the far end, which is where a drag that means
    /// the whole turn ends up.
    fn slid(geared: Geared, gestures: &[(f32, f32)]) -> Clock {
        let ctx = crate::tests::context();
        let mut clock = Clock::default();
        let mut control = |input| {
            let mut at = egui::Rect::NOTHING;
            let _ = ctx.run_ui(input, |ui| {
                clock_control(ui, &mut clock, Some(geared));
                at = ui.min_rect();
            });
            at
        };

        let _ = control(egui::RawInput::default());
        let at = control(egui::RawInput::default());

        for &(from, to) in gestures {
            let rail = egui::pos2(at.left() + at.width() * from, at.center().y);
            for input in dragged(rail, at.width() * (to - from)) {
                control(input);
            }
        }

        clock
    }

    /// The chrome, drawn with the pointer at `at`, once it stands still
    ///
    /// Several frames, because the pane slides in on an animation and is not
    /// drawn at all while the slide stands at nothing: what is wanted is the
    /// frame after it has finished, which is the pane as the user meets it.
    fn chromed(at: egui::Pos2) -> (egui::Context, egui::FullOutput) {
        let ctx = crate::tests::context();
        let input = egui::RawInput {
            events: vec![egui::Event::PointerMoved(at)],
            predicted_dt: 1. / 60.,
            ..Default::default()
        };
        let mut output = None;
        for _ in 0..60 {
            output = Some(ctx.run_ui(input.clone(), |ui| {
                let ctx = ui.ctx().clone();
                // A ring, as `selection::ring` paints one, into the layer the
                // map puts its annotations in.
                ctx.layer_painter(crate::systems::labels::annotations_layer())
                    .circle_stroke(
                        egui::pos2(PANE_WIDTH * 0.5, 300.),
                        12.,
                        egui::Stroke::new(2_f32, egui::Color32::YELLOW),
                    );
                settings_pane(&ctx, true, |ui| {
                    ui.label("Spyglass");
                });
            }));
        }

        (ctx, output.expect("a frame was drawn"))
    }

    /// A wheel turned over the chrome is not a wheel turned at the map
    ///
    /// Reported: scrolling the settings pane zoomed the map behind it. The
    /// camera asks [`PointerOverUi`], which is `is_pointer_over_egui`, and
    /// that answers false for anything in `Order::Background` inside the root
    /// ui's available rect — which the whole of the chrome was. Nothing about
    /// the guard was wrong; egui did not count the pane as its own.
    #[test]
    fn the_pointer_over_the_chrome_is_egui_s() {
        let (inside, _) = chromed(egui::pos2(20., 300.));
        assert!(
            inside.is_pointer_over_egui(),
            "a pointer over the pane was the map's"
        );

        let (outside, _) = chromed(egui::pos2(PANE_WIDTH + 400., 300.));
        assert!(
            !outside.is_pointer_over_egui(),
            "a pointer out on the map was the chrome's"
        );
    }

    /// And what the map annotates is painted under the chrome, not over it
    ///
    /// The other half of the same report: a selection ring and a name plate
    /// were drawn over the pane. The annotations are one painter list rather
    /// than an area, and a layer that is not an area is drained after every
    /// area of its own order, so sharing `Background` with the chrome put
    /// them on top of it however the two were ordered against each other.
    #[test]
    fn the_chrome_is_painted_over_the_annotations() {
        let (_, output) = chromed(egui::pos2(20., 300.));
        let at = |what: fn(&egui::Shape) -> bool| {
            output.shapes.iter().position(|clipped| what(&clipped.shape))
        };
        let ring = at(|shape| matches!(shape, egui::Shape::Circle(_)))
            .expect("the ring was painted");
        let pane = at(|shape| matches!(shape, egui::Shape::Rect(_)))
            .expect("the pane was painted");

        assert!(ring < pane, "the ring was painted over the pane");
    }

    /// How many of `names` were painted whole, rather than clipped away
    fn shown_whole(output: &egui::FullOutput, names: &[String]) -> usize {
        fn seen(
            clip: egui::Rect,
            shape: &egui::Shape,
            into: &mut Vec<(String, egui::Rect, egui::Rect)>,
        ) {
            match shape {
                egui::Shape::Text(text) => into.push((
                    text.galley.text().into(),
                    egui::Rect::from_min_size(text.pos, text.galley.size()),
                    clip,
                )),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        seen(clip, shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut painted = Vec::new();
        for clipped in &output.shapes {
            seen(clipped.clip_rect, &clipped.shape, &mut painted);
        }
        painted
            .iter()
            .filter(|(said, at, clip)| {
                names.iter().any(|name| name == said) && clip.contains_rect(*at)
            })
            .count()
    }

    /// A list that comes back under a bar which had been shut stands its
    /// whole height
    ///
    /// Reported: the search's results came back three lines tall rather than
    /// five once the form had been put away and opened again. The route's
    /// form was right, and stayed right when the box was turned from the
    /// route to the search, so it showed in the search alone.
    ///
    /// Egui lays an area's contents out in the rectangle the pass before left
    /// behind, and a scroll area holds itself to the height it is offered. So
    /// a list coming back under a bar that had been shut was offered the shut
    /// bar's height, took three lines of it, and the bar then came out three
    /// lines tall — which is the same short offer next pass, for good. The
    /// route's form stands under the list and is tall enough that the offer
    /// was never short, which is why the route never showed it.
    #[test]
    fn a_list_that_comes_back_stands_its_whole_height() {
        let names: Vec<String> = (0..25).map(|n| format!("COL {n}")).collect();
        let listed: Vec<&str> = names.iter().map(String::as_str).collect();
        let offers = results_of(&listed);

        let ctx = crate::tests::context();
        let mut selection = Selection::default();
        let mut shown = 0;
        // Out, away, and out again. Two passes apiece, so that what egui kept
        // of the pass before is a pass in the same state.
        for out in [true, true, false, false, true, true] {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0., 0.),
                    egui::vec2(1440., 900.),
                )),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                let ctx = ui.ctx().clone();
                Dropping {
                    id: "main-bar",
                    standing: Standing::At(egui::pos2(MARGIN, MARGIN)),
                    out,
                    width: BAR_WIDTH,
                    holds_width: true,
                }
                .show(&ctx, |ui| {
                    let mut query = Some("COL".to_owned());
                    ask_box(ui, &mut query, "Search", true, false);
                    if !out {
                        return;
                    }
                    ui.add_space(FIELD_GAP);
                    mode_strip(ui, &mut AskMode::System);
                    found(
                        ui,
                        &offers,
                        None,
                        Picking::Asked,
                        &mut selection,
                        &mut None,
                        &mut None,
                    );
                });
            });
            shown = shown_whole(&output, &names);
        }

        assert_eq!(
            shown, OFFERED,
            "the list came back {shown} lines tall of the {OFFERED} it holds"
        );
    }

    /// A results list holding `names`
    fn results(names: &[&str], _placed: bool) -> SearchResults {
        let found: Vec<_> = names.iter().map(|name| row(name)).collect();
        let mut results = SearchResults::default();
        results.set(found);
        results
    }

    /// What the list says, drawn from `filter`
    fn listed(results: &SearchResults, center: Option<DVec3>) -> Vec<String> {
        words(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                results,
                center,
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        })
    }

    /// Every system found is named, whatever it is
    #[test]
    fn the_list_names_what_was_found() {
        let said = listed(&results(&["SOL", "SOLATI"], true), None);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"SOLATI".to_owned()), "{said:?}");
    }

    /// And says how far off it is, from where the camera is looking
    ///
    /// Said as well as sorted by. A list in an order nobody can see reads as
    /// an order nobody chose.
    #[test]
    fn the_list_says_how_far_off_each_system_is() {
        let said =
            listed(&results(&["SOL"], true), Some(DVec3::new(3., 4., 0.)));

        assert!(said.contains(&"5.0 Ly".to_owned()), "{said:?}");
    }

    /// With no camera there is no distance to give
    #[test]
    fn a_list_measured_from_nowhere_gives_no_distance() {
        let said = listed(&results(&["SOL"], true), None);

        assert!(!said.iter().any(|line| line.contains("Ly")), "{said:?}");
    }

    /// Every system that can be had carries the mark that describes it
    ///
    /// Which is how a list of candidates is read through: several are opened
    /// and compared while the selection stays where the user left it.
    #[test]
    fn each_result_carries_a_mark_that_describes_it() {
        let said = listed(&results(&["SOL", "SOLATI"], true), None);

        assert_eq!(said.iter().filter(|line| *line == INFO).count(), 2);
    }

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
    #[test]
    fn one_modes_list_does_not_take_the_last_modes_place() {
        let systems = results(&["SOL", "SOLATI"], true);
        let factions = [faction_row(1, "The Dukes of Mikunn")];

        let said = crate::tests::between_passes(
            |ui| {
                let mut selection = Selection::default();
                let mut travelled = None;
                let mut described = None;
                found(
                    ui,
                    &systems,
                    None,
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

    /// A click on the line for `name`, with or without the modifier held
    ///
    /// The camera and the panel are not what these are about, so what a line
    /// asks of them is taken and dropped.
    fn clicked(gathering: bool, name: &str, selection: &mut Selection) {
        picked(gathering, &row(name), selection);
    }

    /// A click on the line for `system`, whatever the row says about it
    fn picked(gathering: bool, system: &NameEntry, selection: &mut Selection) {
        let mut travelled = None;
        let mut described = None;
        act_on(
            SystemAction::Select { gathering },
            system,
            selection,
            &mut travelled,
            &mut described,
        );
    }

    /// A plain click picks one out in place of whatever was held
    ///
    /// As clicking a star does. The list is where a search is answered from,
    /// so the two gestures have to mean the same thing.
    #[test]
    fn a_click_on_a_line_replaces_what_is_held() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(false, "SOLATI", &mut selection);

        assert_eq!(selection.len(), 1);
        assert_eq!(selection.get(0).map(Picked::name), Some("SOLATI"));
    }

    /// A click with a modifier held gathers them up instead
    ///
    /// Several candidates come back from one search, and picking a handful of
    /// them out is what the list is for. Made to mean the same in the list as
    /// it does in the sky, since a user who has learnt the gesture on a star
    /// has learnt it.
    #[test]
    fn a_gathered_click_holds_what_was_already_picked() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(true, "SOLATI", &mut selection);

        assert_eq!(selection.get(0).map(Picked::name), Some("SOL"));
        assert_eq!(selection.get(1).map(Picked::name), Some("SOLATI"));
    }

    /// And one already held is let go of
    ///
    /// One gesture that builds a set and takes it apart, as it is on the map.
    #[test]
    fn a_gathered_click_on_one_already_held_lets_go_of_it() {
        let mut selection = Selection::default();

        clicked(false, "SOL", &mut selection);
        clicked(true, "SOLATI", &mut selection);
        clicked(true, "SOL", &mut selection);

        assert_eq!(selection.len(), 1);
        assert_eq!(selection.get(0).map(Picked::name), Some("SOLATI"));
    }

    /// Which system a name stands for
    ///
    /// Derived from the name so that one name means one system across two
    /// passes and two names never mean the same one. Numbering them by where
    /// they sit in the list would give every first row the same system, and a
    /// test for what happens when the system changes would never change it.
    fn address_of(name: &str) -> i64 {
        name.bytes().map(i64::from).sum()
    }

    /// A selection holding `names`
    fn holding(names: &[&str]) -> Selection {
        let mut selection = Selection::default();
        for name in names {
            selection.toggle(Picked::System(crate::systems::tests::named(
                address_of(name),
                name,
            )));
        }
        selection
    }

    /// What the bar says about a selection holding `names`
    fn selection_said(names: &[&str]) -> Vec<String> {
        let mut selection = holding(names);

        words(|ui| {
            let mut panels = Panels::default();
            let mut filters = Filters::default();
            let mut travelled = None;
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut travelled,
                &mut panels,
                &mut filters,
                &mut 0,
            );
        })
    }

    /// Every system picked out gets a row naming it
    ///
    /// A row apiece rather than one row about several, so that no row has to
    /// answer which of them it means.
    #[test]
    fn every_selected_system_gets_a_row() {
        let said = selection_said(&["SOL", "ALPHA CENTAURI", "BARNARD"]);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"ALPHA CENTAURI".to_owned()), "{said:?}");
        assert!(said.contains(&"BARNARD".to_owned()), "{said:?}");
    }

    /// A body called `name`, picked out `away` light years from the origin
    fn body(id: i16, name: &str, away: f64) -> Picked {
        Picked::Body(PickedBody::new(1, id, name, DVec3::new(away, 0., 0.)))
    }

    /// What the bar says about `picked` being picked out
    fn rows_said(picked: &[Picked]) -> Vec<String> {
        let mut selection = Selection::default();
        for one in picked {
            selection.toggle(one.clone());
        }

        words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        })
    }

    /// Every body picked out gets a row naming it
    ///
    /// The same row a system gets and in the same list, since a body is picked
    /// out by the same gesture. The rings are out on the map where the user is
    /// looking, and the bar is where what is picked out is read.
    #[test]
    fn every_picked_body_gets_a_row() {
        let said = rows_said(&[body(3, "SOL 3", 0.), body(4, "SOL 4", 0.)]);

        assert!(said.contains(&"SOL 3".to_owned()), "{said:?}");
        assert!(said.contains(&"SOL 4".to_owned()), "{said:?}");
    }

    /// A body's row says how far off it is in light seconds
    ///
    /// Where a system's row says light years, and measured from the same
    /// focus. A light second is about a thirty millionth of a light year, so
    /// a body given in light years is a row of leading zeroes.
    #[test]
    fn a_body_row_says_how_far_off_it_is_in_light_seconds() {
        // A hundred light seconds, in the light years the map measures in.
        let away = 100. / crate::space::light_seconds(1.);
        let mut selection = Selection::default();
        selection.toggle(body(3, "SOL 3", away));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.contains(&"100.0 Ls".to_owned()), "{said:?}");
    }

    /// A system and a body picked out together are one list of rows
    ///
    /// Which is the whole of what holding them the same way buys: the bar
    /// draws what is picked out, in the order it was picked, without asking
    /// what kind each of them is except to say what stands beside the name.
    #[test]
    fn a_system_and_a_body_share_the_one_list() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert_eq!(selection.len(), 2);
        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(said.contains(&"SOL 3".to_owned()), "{said:?}");
    }

    /// A body picked out is no system, so it is not offered a route or filter
    ///
    /// Both are questions about places. Counting a body among them would put
    /// a filter on the map naming one system where two things are held, and
    /// offer a route to somewhere that is not a destination.
    #[test]
    fn a_picked_body_is_not_counted_among_the_systems() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));

        assert_eq!(selection.addresses(), vec![address_of("SOL")]);
        assert!(stops_of(&selection).is_err());
    }

    /// The rows each answer for themselves, whatever they hold
    #[test]
    fn the_body_rows_do_not_share_ids() {
        let mut selection = holding(&["SOL"]);
        selection.toggle(body(3, "SOL 3", 0.));
        selection.toggle(body(4, "SOL 4", 0.));

        let said = crate::tests::complaints(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// A row says how far off its system is, in as few words as that takes
    ///
    /// The number and the unit and nothing else. The rows of every other list
    /// the map draws end the same way, and what stands at the end of a row is
    /// read as the distance whether or not a word says so, so a word saying so
    /// is a word taking room from the name beside it.
    #[test]
    fn a_selection_row_says_how_far_off_its_system_is() {
        let mut selection = Selection::default();
        selection.toggle(Picked::System(crate::systems::tests::at(1, 12.)));

        let said = words(|ui| {
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                Some(DVec3::ZERO),
                &mut None,
                &mut Panels::default(),
                &mut Filters::default(),
                &mut 0,
            );
        });

        assert!(said.contains(&"12.0 Ly".to_owned()), "{said:?}");
    }

    /// Several picked out says how many, and offers to filter on them
    #[test]
    fn a_gathered_selection_offers_to_filter_on_itself() {
        let said = selection_said(&["SOL", "BARNARD"]);

        assert!(said.contains(&"2 systems".to_owned()), "{said:?}");
        assert!(said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// One picked out says neither
    ///
    /// The row already names it, and a line saying "1 system" over a row
    /// naming that system says the same thing twice. There is nothing to
    /// gather either, a filter over one system being the system itself.
    #[test]
    fn one_selected_system_is_left_to_its_own_row() {
        let said = selection_said(&["SOL"]);

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(!said.contains(&"1 systems".to_owned()), "{said:?}");
        assert!(!said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// Two picked out are offered a route between them
    ///
    /// This is where the feature is found. A user who gathers two systems out
    /// on the map has said everything a route needs but the jump range, and
    /// nothing else on screen would tell them the form dropping out of the
    /// search box has a section about the pair they are already holding.
    #[test]
    fn two_selected_systems_are_offered_a_route() {
        let said = selection_said(&["SOL", "BARNARD"]);

        assert!(said.contains(&"Route".to_owned()), "{said:?}");
    }

    /// Bodies alone are offered no filter, while none can name them
    ///
    /// A body is picked out into the same list as a system and counted with
    /// it, so a pair of them reaches the gathered controls while leaving no
    /// system to gather. The filter that would build names no address, admits
    /// nothing, and blanks the sky under a row saying none was picked, the map
    /// fetching by the same answer it dims by.
    ///
    /// About what a filter can name rather than about bodies. A filter that
    /// can name one makes this case a filter over bodies, and this test the
    /// wrong question.
    #[test]
    fn bodies_alone_are_not_offered_a_filter() {
        let said = rows_said(&[body(1, "SOL A", 0.), body(2, "SOL B", 0.)]);

        assert!(said.contains(&"0 systems".to_owned()), "{said:?}");
        assert!(!said.contains(&"Filter".to_owned()), "{said:?}");
    }

    /// One alone is not, there being no route it could ask for
    ///
    /// A control that leads to a form refusing what it just asked for is
    /// worse than no control: it says the map can do something it cannot.
    /// More than two is a route through all of them, and is offered one.
    #[test]
    fn a_set_that_cannot_be_routed_is_offered_no_route() {
        let alone = selection_said(&["SOL"]);
        let several = selection_said(&["SOL", "BARNARD", "WOLF 359"]);

        assert!(!alone.contains(&"Route".to_owned()), "{alone:?}");
        assert!(several.contains(&"Route".to_owned()), "{several:?}");
        // The rest of the line stands either way.
        assert!(several.contains(&"Filter".to_owned()), "{several:?}");
    }

    /// The keys the pane lists are the keys the README lists
    ///
    /// One table said in two places, and this is what keeps the two saying
    /// the same thing: a key added to one and not the other fails here.
    #[test]
    fn the_pane_and_the_readme_list_the_same_keys() {
        let readme = include_str!("../README.md");
        let rows: Vec<&str> =
            readme.lines().filter(|line| line.starts_with("| `")).collect();

        assert_eq!(rows.len(), BINDINGS.len(), "{rows:?}");
        for ((key, _), row) in BINDINGS.iter().zip(rows) {
            // The README sets each key in its own backticks, `W` `A` `S` `D`
            // for the four that pan, where the pane says them in a run.
            let plain = row
                .trim_start_matches("| ")
                .split(" | ")
                .next()
                .unwrap_or_default()
                .replace('`', "");
            assert_eq!(*key, plain, "{row}");
        }
    }

    /// And the pane draws every one of them
    #[test]
    fn the_pane_says_what_the_keys_do() {
        let said = words(keys_reference);

        assert!(said.contains(&"H".to_owned()), "{said:?}");
        assert!(said.contains(&"Space".to_owned()), "{said:?}");
        assert_eq!(said.len(), BINDINGS.len() * 2);
    }

    /// A set that spans somewhere is offered a frame over the whole of it
    ///
    /// Systems and bodies alike, both being somewhere. Several standing in
    /// one place are not offered it: a frame over nothing is a camera pulled
    /// in to a metre. Nor is one alone, there being no summary line to offer
    /// it from.
    #[test]
    fn a_set_that_spans_somewhere_is_offered_a_frame() {
        let spread = rows_said(&[body(1, "SOL A", 0.), body(2, "SOL B", 5.)]);
        let heaped = rows_said(&[body(1, "SOL A", 3.), body(2, "SOL B", 3.)]);
        let alone = selection_said(&["SOL"]);

        assert!(spread.contains(&"Frame".to_owned()), "{spread:?}");
        assert!(!heaped.contains(&"Frame".to_owned()), "{heaped:?}");
        assert!(!alone.contains(&"Frame".to_owned()), "{alone:?}");
    }

    /// What a frame takes in is the middle of the set and its reach from there
    ///
    /// Which is what tells a frame from a flight: a row says where alone, and
    /// this says how much to stand back for as well.
    #[test]
    fn a_frame_stands_over_the_middle_of_the_set() {
        assert_eq!(
            spanned(&strung_out(&[0., 10., 4.])),
            Some((DVec3::new(5., 0., 0.), 5.))
        );
        assert_eq!(spanned(&strung_out(&[7.])), None);
        assert_eq!(spanned(&Selection::default()), None);
    }

    /// A selection holding a system at each of `places`, on the x axis
    fn strung_out(places: &[f64]) -> Selection {
        let mut selection = Selection::default();
        for (address, away) in places.iter().enumerate() {
            selection.toggle(Picked::System(crate::systems::tests::at(
                address as i64,
                *away,
            )));
        }
        selection
    }

    /// A selection holding a system at each of `places`
    fn scattered(places: &[DVec3]) -> Selection {
        let mut selection = Selection::default();
        for (address, at) in places.iter().enumerate() {
            selection.toggle(Picked::System(crate::systems::tests::placed(
                address as i64,
                *at,
            )));
        }
        selection
    }

    /// How wide the systems at `along` stand
    fn wide(along: &[f64]) -> Option<f64> {
        across(&strung_out(along))
    }

    /// A pair is said to be as far apart as they stand
    ///
    /// The whole of what there is to say about two: a route between them runs
    /// from the one to the other however it gets there.
    #[test]
    fn a_pair_is_as_far_apart_as_it_stands() {
        assert_eq!(wide(&[3., 15.]), Some(12.));
    }

    /// And a longer set is said by how much sky it covers
    ///
    /// Across the whole of them, from the middle of what they span to
    /// whichever is furthest and out the other side. Which is not the legs
    /// added up: the set below covers thirty light years however it is
    /// flown, where walking it end to end is thirty and doubling back over
    /// it is more.
    #[test]
    fn a_longer_set_is_as_wide_as_the_sky_it_covers() {
        assert_eq!(wide(&[0., 30., 10., 20.]), Some(30.));
        // The order it was picked in says nothing about it.
        assert_eq!(wide(&[30., 0., 20., 10.]), Some(30.));
        // Nor does a system standing inside the span.
        assert_eq!(wide(&[0., 30., 15.]), Some(30.));
    }

    /// Taking a system away never makes the set wider
    ///
    /// The figure is over pairs, so dropping a system drops its pairs and
    /// leaves the rest as they were. Measured instead from the middle of the
    /// box the systems fill -- which is what [`spanned`] does for the camera
    /// -- this set read 100.9 Ly and read **114.5** once the third system was
    /// taken out of it: losing that one let the box's middle slide, and left
    /// the far corners further from the new middle than anything had been
    /// from the old one.
    #[test]
    fn taking_a_system_away_does_not_widen_the_set() {
        let places = [
            DVec3::new(-7.1, -35.3, 28.5),
            DVec3::new(10.1, 27.3, 26.9),
            DVec3::new(-48.9, -0.4, 9.8),
            DVec3::new(0.6, -33.2, -3.2),
            DVec3::new(-13.7, -49.9, -30.6),
            DVec3::new(46.7, -5.8, 14.1),
        ];
        let whole = across(&scattered(&places)).expect("a span");
        for dropped in 0..places.len() {
            let mut fewer = places.to_vec();
            fewer.remove(dropped);
            let after = across(&scattered(&fewer)).expect("a span");
            assert!(
                after <= whole,
                "dropping #{dropped} widened {whole:.1} to {after:.1}"
            );
        }
    }

    /// A set with nothing to span is not measured at all
    ///
    /// One system spans nothing, and the form has already said it wants
    /// another.
    #[test]
    fn a_set_that_cannot_be_routed_is_not_measured() {
        assert_eq!(wide(&[]), None);
        assert_eq!(wide(&[3.]), None);
    }

    /// A trip of several says how many legs it is
    ///
    /// Two systems are apart. More stand across a span, and are said as how
    /// many legs the trip will be as well: the figure is no longer a gap
    /// between two things. The legs are [`legs_flown`]'s to count.
    #[test]
    fn a_longer_route_is_said_in_legs() {
        assert_eq!(apart_said(12., 1), "12.0 Ly apart");
        assert_eq!(apart_said(30., 3), "3 legs, 30.0 Ly across");
        assert_eq!(apart_said(4., 0), "4.0 Ly apart");
    }

    /// The selection rows each answer for themselves
    ///
    /// Both over a pair and over a longer set, since a pair carries a control
    /// the longer set does not and two controls in the one summary line are
    /// two more things to collide.
    #[test]
    fn the_selection_rows_do_not_share_ids() {
        for names in [&["SOL", "BARNARD"][..], &["SOL", "BARNARD", "WOLF 359"]]
        {
            let mut selection = holding(names);

            let said = crate::tests::complaints(|ui| {
                let mut panels = Panels::default();
                let mut filters = Filters::default();
                let mut travelled = None;
                selected(
                    ui,
                    &mut selection,
                    &Contents::default(),
                    None,
                    &mut travelled,
                    &mut panels,
                    &mut filters,
                    &mut 0,
                );
            });

            assert!(said.is_empty(), "{names:?}: {said:?}");
        }
    }

    /// Nothing picked out draws no rows at all
    #[test]
    fn an_empty_selection_draws_nothing() {
        assert!(selection_said(&[]).is_empty());
    }

    /// Draw the results list holding `names`
    fn draw_found<'a>(names: &'a [&'a str]) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(names, true),
                None,
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        }
    }

    /// Draw the selection rows holding `names`
    fn draw_selected<'a>(names: &'a [&'a str]) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut selection = holding(names);
            let mut panels = Panels::default();
            let mut filters = Filters::default();
            let mut travelled = None;
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut travelled,
                &mut panels,
                &mut filters,
                &mut 0,
            );
        }
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
        let said = crate::tests::complaints(|ui| {
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

    /// Every style the chrome is drawn in is lettered the same
    ///
    /// Egui keeps a font per text style, and one left proportional is one
    /// heading or one button standing among columns that no longer line up
    /// with it.
    #[test]
    fn the_chrome_is_lettered_in_one_width() {
        let mut style = egui::Style::default();
        styled(&mut style);

        for (kind, font) in &style.text_styles {
            assert_eq!(font.family, egui::FontFamily::Monospace, "{kind:?}");
        }
    }

    /// The gear hangs about the height it is given
    ///
    /// Which is where the bar's search box came out, so that the handle and
    /// the field beside it read as one row. Dropped from the top of the
    /// viewport as the bar is, it would sit level with the top edge of a box
    /// the field is padded inside rather than with the field.
    ///
    /// Twice round, since an area is placed about a pivot from the size it
    /// came out last time and has no size at all the first time it is drawn.
    #[test]
    fn the_gear_hangs_about_the_height_it_is_given() {
        let ctx = Context::default();
        let middle = 40.;
        let mut open = false;

        for _ in 0..2 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                gear(ui.ctx(), 0., middle, &mut open);
            });
        }
        let at = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("settings-gear")))
            .expect("a gear was drawn");

        // Within half a pixel: the gear stands an odd number of them tall,
        // and egui rounds where an area is put onto the pixel grid.
        let off = (at.center().y - middle).abs();
        assert!(off <= 0.5, "{off} off the {middle} it was given");
    }

    /// The instrument itself sees a clash when there is one
    ///
    /// Two widgets given one id at two rects is the fault `complaints` is
    /// there to catch. Without this, a run that finds nothing says only that
    /// nothing was heard, which is not the same as nothing being said.
    #[test]
    fn complaints_hears_a_real_clash() {
        let said = crate::tests::complaints(|ui| {
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
    #[test]
    fn clearing_the_results_does_not_change_the_row_ids() {
        let said = crate::tests::between_passes(
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
    #[test]
    fn letting_go_of_the_selection_does_not_change_the_filter_row_ids() {
        let said = crate::tests::between_passes(
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
        let ctx = crate::tests::context();
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

        let gathered = crate::tests::between_passes(
            draw_bar(&[], fewer, 2),
            draw_bar(&[], &held, 2),
        );
        let let_go = crate::tests::between_passes(
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
    #[test]
    fn dropping_one_of_several_does_not_hand_its_place_to_a_filter() {
        let said = crate::tests::between_passes(
            draw_bar(&[], &["SOL", "BARNARD", "WOLF 359"], 2),
            draw_bar(&[], &["SOL", "BARNARD"], 2),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// The instrument itself hears an id change when there is one
    ///
    /// Two ids at one rectangle across two passes is the fault
    /// [`crate::tests::between_passes`] is there to catch. Without this, a run
    /// that finds nothing says only that nothing was heard, which is not the
    /// same as nothing being said: the listener is installed once per process
    /// and quietly does nothing if something else got there first.
    #[test]
    fn between_passes_hears_a_real_id_change() {
        let at =
            egui::Rect::from_min_size(egui::pos2(0., 0.), egui::vec2(50., 20.));

        let said = crate::tests::between_passes(
            |ui| {
                ui.interact(at, egui::Id::new("one"), egui::Sense::click());
            },
            |ui| {
                ui.interact(at, egui::Id::new("two"), egui::Sense::click());
            },
        );

        assert!(!said.is_empty(), "heard nothing about a real id change");
    }

    /// The rows keep their ids as the set outgrows what the bar shows at once
    ///
    /// Past [`SELECTED`] the rows are drawn inside a scroll area, and a row's
    /// id is taken from the `Ui` it is drawn in. The rows keep their
    /// rectangles across that change, so an id taken from the scroll area's
    /// own `Ui` would be a new id at an old rectangle.
    #[test]
    fn outgrowing_the_bar_does_not_change_the_row_ids() {
        let five = ["SOL", "BARNARD", "WOLF 359", "LUYTEN", "ROSS 128"];
        let six =
            ["SOL", "BARNARD", "WOLF 359", "LUYTEN", "ROSS 128", "LALANDE"];

        let said = crate::tests::between_passes(
            draw_selected(&five),
            draw_selected(&six),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// And as the summary above them comes and goes
    ///
    /// Gathering a second system stands a line saying how many over the rows,
    /// which moves every one of them down a line.
    #[test]
    fn gathering_a_second_system_does_not_change_the_row_ids() {
        let said = crate::tests::between_passes(
            draw_selected(&["SOL"]),
            draw_selected(&["SOL", "BARNARD"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// Drawn filter rows for `names`
    fn draw_filters<'a>(names: &'a [&'a str]) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut filters = Filters::default();
            for name in names {
                filters.add(Filter::Faction {
                    id: name.len() as i32,
                    name: (*name).to_owned(),
                });
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);
        }
    }

    /// Drawn filter rows for a faction and `routes` routes
    fn draw_sections<'a>(routes: usize) -> impl FnMut(&mut Ui) + 'a {
        move |ui: &mut Ui| {
            let mut filters = Filters::default();
            filters.add(Filter::Faction { id: 1, name: "Empire".into() });
            for held in 0..routes {
                filters
                    .add(a_route(&(0..=held as i64 + 1).collect::<Vec<_>>()));
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);
        }
    }

    /// A section growing a count of its own does not hand a row its place
    ///
    /// The second route stands a "2 routes" row over them, which lands in the
    /// rectangle the first route's row was drawn in and moves that row down.
    /// The places are counted across the whole column, so the rectangle keeps
    /// its id and what stands there changes underneath it.
    #[test]
    fn a_section_gaining_its_count_does_not_change_the_row_ids() {
        let said =
            crate::tests::between_passes(draw_sections(1), draw_sections(2));

        assert!(said.is_empty(), "{said:?}");
    }

    /// And losing it does not either
    #[test]
    fn a_section_losing_its_count_does_not_change_the_row_ids() {
        let said =
            crate::tests::between_passes(draw_sections(2), draw_sections(1));

        assert!(said.is_empty(), "{said:?}");
    }

    /// Dropping a filter is not read as a widget changing identity either
    ///
    /// The other half of what the bar does when a row goes: the rows below
    /// move up into the rectangle it left.
    #[test]
    fn dropping_a_filter_is_not_an_id_change() {
        let said = crate::tests::between_passes(
            draw_filters(&["Empire", "Federation", "Alliance"]),
            draw_filters(&["Empire", "Alliance"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// Falling to one filter takes the row over the set with it
    ///
    /// The row stands above the others, so losing it moves every remaining
    /// row up one place. Two rows go at once, which is the shape egui reads
    /// as a widget taking another's state if the ids do not follow the
    /// places.
    #[test]
    fn dropping_to_one_filter_does_not_change_the_row_ids() {
        let said = crate::tests::between_passes(
            draw_filters(&["Empire", "Federation"]),
            draw_filters(&["Empire"]),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// The row over the set says how many are held, and is drawn over two
    ///
    /// The time control says which span it is standing on
    ///
    /// Egui draws a slider's own reading inside the box `show_value` turns
    /// off, and that box is turned off here because what it offers is a field
    /// to type a number into where these are named spans. Without a word
    /// beside it the control is nine positions and nothing to tell them apart.
    #[test]
    fn the_time_control_says_which_span_it_stands_on() {
        let mut watch = Watch(1);
        let mut active = Filters::default();
        let mut standstill = Standstill::default();

        let said = words(|ui| {
            watch_control(ui, &mut watch, &mut active, &mut standstill);
        });

        assert!(said.contains(&"30 days".to_owned()), "{said:?}");
    }

    /// And says so for the whole of its travel, the widest name included
    ///
    /// The slider is sized to leave [`SPAN_WIDTH`] for the name, which is the
    /// one thing that would quietly go wrong: a name too wide for the room
    /// left it is drawn off the end of the row it stands in.
    #[test]
    fn every_span_is_named_beside_the_control() {
        for (place, (name, _)) in SPANS.iter().enumerate() {
            let mut watch = Watch(place);
            let mut active = Filters::default();
            let mut standstill = Standstill::default();

            let said = words(|ui| {
                watch_control(ui, &mut watch, &mut active, &mut standstill);
            });

            assert!(said.contains(&(*name).to_owned()), "{name}: {said:?}");
        }
    }

    /// The rows drawn from `filters`, with `standstill` holding them or not
    fn drawn_rows(filters: &Filters, standstill: &Standstill) -> Vec<String> {
        words(|ui| {
            let mut standing =
                standstill.rows(filters).unwrap_or_else(|| filters.clone());
            let mut panels = Panels::default();
            applied(ui, &mut standing, &mut panels, &mut 0);
        })
    }

    /// A time filter's row offers no panel
    ///
    /// The other filters name a set of systems worth reading as a list. A span
    /// admits the galaxy over and changes while it is being read, so a panel
    /// about one is a question over every system on record answered with an
    /// arbitrary slice of the newest. What it admits is on the map already.
    #[test]
    fn a_time_filters_row_offers_no_panel() {
        let mut filters = Filters::default();
        filters.ask_within("1 day", chrono::Duration::days(1));

        let said = drawn_rows(&filters, &Standstill::default());

        assert!(said.contains(&CLOSE.to_owned()), "no way to let go: {said:?}");
        assert!(!said.contains(&INFO.to_owned()), "a panel was offered");
    }

    /// Where a faction's does
    #[test]
    fn a_faction_filters_row_offers_a_panel() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });

        let said = drawn_rows(&filters, &Standstill::default());

        assert!(said.contains(&INFO.to_owned()), "no panel offered: {said:?}");
    }

    /// The rows stand still while the time control is held
    ///
    /// They are drawn above it, so a row appearing while it is being dragged
    /// takes the control down out from under the pointer. What the drag asks
    /// for goes in as it is asked, and the sky is filtered by it; the row for
    /// it waits until the drag is over.
    #[test]
    fn the_rows_stand_still_while_the_time_control_is_held() {
        let mut before = Filters::default();
        before.add(Filter::Faction { id: 1, name: "Empire".into() });

        let mut standstill = Standstill::default();
        standstill.hold(&before);

        // What the drag asks for, which the sky is filtered by at once.
        let mut asked = before.clone();
        asked.ask_within("1 day", chrono::Duration::days(1));

        assert_eq!(
            drawn_rows(&asked, &standstill),
            drawn_rows(&before, &Standstill::default()),
            "a row landed under the pointer"
        );

        standstill.release();
        assert_ne!(
            drawn_rows(&asked, &standstill),
            drawn_rows(&before, &Standstill::default()),
            "the row never landed"
        );
    }

    /// A row already there reads live while the control is held
    ///
    /// Only its being there is held. What it says follows the drag, so a span
    /// dragged from one to the next says the span it has reached rather than
    /// the one the press landed on.
    #[test]
    fn a_held_time_filter_still_reads_live() {
        let mut asked = Filters::default();
        asked.ask_within("1 day", chrono::Duration::days(1));

        let mut standstill = Standstill::default();
        standstill.hold(&asked);

        asked.ask_within("6 hours", chrono::Duration::hours(6));

        let said = drawn_rows(&asked, &standstill);
        assert!(said.contains(&"Last 6 hours".to_owned()), "{said:?}");
        assert!(!said.contains(&"Last 1 day".to_owned()), "{said:?}");
    }

    /// A control slid to its far end says so where its row stands
    ///
    /// The row stays until the gesture is over, a row going out from under the
    /// pointer taking the control up a row the same way one arriving takes it
    /// down. What it says is what is filtering, which by then is nothing, so
    /// it reads as the control does rather than as the span it was asked as.
    #[test]
    fn a_time_filter_slid_off_says_so_before_it_goes() {
        let mut asked = Filters::default();
        asked.ask_within("1 day", chrono::Duration::days(1));

        let mut standstill = Standstill::default();
        standstill.hold(&asked);

        // Dragged to the far end, which stops asking where the row stands.
        asked.turn_time_off("Off");

        let said = drawn_rows(&asked, &standstill);
        assert!(said.contains(&"Off".to_owned()), "{said:?}");
        assert!(
            !said.contains(&"Last 1 day".to_owned()),
            "the row kept the span it had stopped asking for"
        );
    }

    /// The control's id under `rows` filter rows standing above it
    fn watch_id(rows: usize) -> egui::Id {
        let mut got = None;
        let ctx = crate::tests::context();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let mut filters = Filters::default();
            for id in 0..rows {
                filters.add(Filter::Faction {
                    id: id as i32,
                    name: format!("Faction {id}"),
                });
            }
            let mut panels = Panels::default();
            applied(ui, &mut filters, &mut panels, &mut 0);

            let mut watch = Watch(1);
            let mut active = Filters::default();
            let mut standstill = Standstill::default();
            got = Some(
                watch_control(ui, &mut watch, &mut active, &mut standstill).id,
            );
        });

        got.expect("the control drew")
    }

    /// The control keeps its id as rows come and go above it
    ///
    /// Egui numbers a widget by how many were drawn before it and follows a
    /// drag by id, so a row appearing above this one mid-drag would leave the
    /// user dragging a widget that no longer exists. Asking for a filter here
    /// is what adds that row, so the control moves the moment it is used.
    #[test]
    fn the_time_control_keeps_its_id_under_rows() {
        assert_eq!(watch_id(0), watch_id(1), "one row renamed the control");
        assert_eq!(watch_id(1), watch_id(3), "three rows renamed the control");
    }

    /// Left alone it asks nothing of time
    ///
    /// The moment a span works out to is read from the clock when the control
    /// is moved. Written every frame instead, it would move every frame and
    /// put a fresh question to the database each time.
    #[test]
    fn a_time_control_left_alone_asks_nothing() {
        let mut watch = Watch(1);
        let mut active = Filters::default();
        let mut standstill = Standstill::default();

        painted(|ui| {
            watch_control(ui, &mut watch, &mut active, &mut standstill);
        });

        assert_eq!(active.span(), None, "an untouched control asked");
        assert_eq!(active.iter().count(), 0, "a row appeared unasked");
    }

    /// Not over one, where it would be a second control for what the row
    /// beneath it already does.
    #[test]
    fn the_set_is_summed_up_only_over_more_than_one() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        let mut panels = Panels::default();
        let alone = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        let both = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(
            !alone.iter().any(|line| line.contains("filters")),
            "{alone:?}"
        );
        assert!(both.contains(&"2 filters".to_owned()), "{both:?}");
    }

    /// A route between the systems at `addresses`
    fn a_route(addresses: &[i64]) -> Filter {
        Filter::Route {
            label: format!("A -> B{}", addresses.len()),
            systems: addresses.to_vec(),
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        }
    }

    /// The routes are counted apart from the rest of the filters
    ///
    /// A route is a line drawn across the map and a faction is a way of
    /// reading the sky it is drawn over, so a single count over both said
    /// "3 filters" about a heap of two different things.
    #[test]
    fn the_routes_are_counted_apart_from_the_filters() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"2 filters".to_owned()), "{said:?}");
        assert!(said.contains(&"2 routes".to_owned()), "{said:?}");
        assert!(!said.contains(&"4 filters".to_owned()), "{said:?}");
    }

    /// One route on its own is not counted, as one filter is not
    ///
    /// Its own row already says everything a count of one could, and the
    /// control over it would do what that row's own does.
    #[test]
    fn a_single_route_is_left_to_its_own_row() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(!said.iter().any(|line| line.contains("route")), "{said:?}");
    }

    /// A section with nothing in it says nothing
    ///
    /// Routes alone are routes alone, with no empty count for the filters
    /// standing over them.
    #[test]
    fn a_section_with_nothing_in_it_is_not_drawn() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"2 routes".to_owned()), "{said:?}");
        assert!(!said.iter().any(|line| line.contains("filters")), "{said:?}");
    }

    /// The routes stand under the rest, whatever order they were asked in
    ///
    /// The sky before what is drawn over it. A route plotted between two
    /// factions being asked for would otherwise sit up among them.
    #[test]
    fn the_routes_stand_under_the_rest() {
        let mut filters = Filters::default();
        filters.add(a_route(&[1, 2]));
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        let faction = said.iter().position(|line| line == "Empire");
        let route = said.iter().position(|line| line.contains(ARROW));
        assert!(faction < route, "{said:?}");
    }

    /// The two counts each answer for their own section
    ///
    /// Turning the routes off is no reason to turn the factions off with
    /// them, which is the whole point of the two rows being two rows.
    #[test]
    fn a_section_count_turns_off_its_own_section() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));

        filters.toggle_all(&Section::Routes.rows(&filters));

        assert!(Section::Filters.on(&filters), "the filters went off too");
        assert!(!Section::Routes.on(&filters), "the routes are still asked");
    }

    /// And takes away its own section
    #[test]
    fn a_section_count_takes_away_its_own_section() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        filters.add(a_route(&[1, 2]));
        filters.add(a_route(&[1, 2, 3]));

        filters.clear(&Section::Routes.rows(&filters));

        assert_eq!(filters.iter().count(), 1);
        assert!(Section::Routes.rows(&filters).is_empty());
        assert_eq!(
            filters.get(0).map(|held| held.filter.name()),
            Some("Empire")
        );
    }

    /// A route's row says how many jumps it is, and the rest say nothing
    ///
    /// Where a selection row says how far off its system is. A route is named
    /// for its two ends, so its row would otherwise say nothing about the one
    /// thing it was plotted to find out. A faction's name is all its row has
    /// to say, and a set says how many it holds in its own name already.
    /// A leg of a trip, from `from` to `to`, under the trip called `trip`
    fn leg(from: &str, to: &str, trip: Option<&str>) -> Filter {
        Filter::Route {
            label: format!("{from}{ARROW}{to}"),
            systems: vec![1, 2],
            range: "10".to_owned(),
            trip: trip.map(|trip| trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
        }
    }

    /// The legs of a trip are grouped under a row of their own
    ///
    /// A trip through several systems is several routes, and they read as the
    /// one trip they were asked for rather than as a heap of routes. A route
    /// asked for on its own belongs to no trip and stands among the loose
    /// ones.
    #[test]
    fn the_legs_of_a_trip_are_grouped_under_it() {
        let mut filters = Filters::default();
        filters.add(leg("SOL", "LAVE", Some("SOL -> LAVE -> DISO")));
        filters.add(leg("LAVE", "DISO", Some("SOL -> LAVE -> DISO")));
        filters.add(leg("WOLF 359", "SIRIUS", None));
        filters.add(Filter::Faction { id: 7, name: "Faction".to_owned() });

        let sections = Section::all(&filters);
        let held = |section: &Section| section.rows(&filters).len();

        assert_eq!(
            sections,
            vec![
                Section::Filters,
                Section::Trip("SOL -> LAVE -> DISO".to_owned()),
                Section::Routes,
            ]
        );
        assert_eq!(held(&sections[0]), 1, "the faction");
        assert_eq!(held(&sections[1]), 2, "the trip's two legs");
        assert_eq!(held(&sections[2]), 1, "the route on its own");
    }

    /// A trip is described as the one route it is flown as
    ///
    /// Every system it passes through, in order, with the seams closed: the
    /// stop a leg lands on is the stop the next sets out from and stands in
    /// the list once, or the panel would count a jump from a system to
    /// itself.
    #[test]
    fn a_trip_is_described_as_one_route() {
        let trip = "SOL -> LAVE -> DISO";
        let hops = |from: i64, to: i64| Filter::Route {
            label: "leg".to_owned(),
            systems: vec![from, from + 1, to],
            range: "10".to_owned(),
            trip: Some(trip.to_owned()),
            drive: Drive::Unaided,
            how: Routing::default(),
        };
        let mut filters = Filters::default();
        filters.add(hops(1, 3));
        filters.add(hops(3, 5));
        let rows: Vec<usize> = (0..2).collect();

        let whole = as_one(trip, &rows, &filters).expect("a trip");

        assert_eq!(whole.name(), "2 Leg Route");
        assert_eq!(whole.range(), Some("10"));
        // 3 stands once, being where the first leg landed and the second set
        // out from.
        let Filter::Route { systems, .. } = &whole else { panic!("a route") };
        assert_eq!(systems, &vec![1, 2, 3, 4, 5]);
    }

    /// A trip with nothing landed yet is not described
    ///
    /// A panel about it would be a panel about no systems, and its row
    /// already says how far along the legs are.
    #[test]
    fn a_trip_with_no_legs_yet_is_not_described() {
        let filters = Filters::default();

        assert_eq!(as_one("SOL -> LAVE", &[], &filters), None);
    }

    /// A trip's legs are drawn in from its row
    ///
    /// So the block reads as one trip with its legs under it rather than as a
    /// row and then some routes. A route belonging to no trip is drawn in
    /// from nothing, standing level with the section rows.
    #[test]
    fn a_trips_legs_are_indented_under_it() {
        let trip = "SOL -> LAVE -> DISO";
        let mut filters = Filters::default();
        filters.add(leg("SOL", "LAVE", Some(trip)));
        filters.add(leg("LAVE", "DISO", Some(trip)));
        filters.add(leg("WOLF 359", "SIRIUS", None));
        let mut panels = Panels::default();

        let ctx = crate::tests::context();
        let mut drawn = |filters: &mut Filters| {
            let output = ctx.run_ui(egui::RawInput::default(), |ui| {
                applied(ui, filters, &mut panels, &mut 0);
            });
            let mut lefts = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    lefts.push((text.galley.text().to_owned(), text.pos.x));
                }
            }
            lefts
        };

        // Twice, the first pass being where the rows are placed.
        drawn(&mut filters);
        let lefts = drawn(&mut filters);
        let left_of = |name: &str| {
            lefts
                .iter()
                .find(|(said, _)| said == name)
                .unwrap_or_else(|| panic!("{name} was painted: {lefts:?}"))
                .1
        };

        let under = left_of("2 Leg Route");
        assert!(left_of("SOL -> LAVE") > under, "{lefts:?}");
        assert!(left_of("LAVE -> DISO") > under, "{lefts:?}");
        // The loose route belongs to no trip and is drawn in from nothing.
        assert_eq!(left_of("WOLF 359 -> SIRIUS"), under, "{lefts:?}");
    }

    /// A trip's row is named for how many legs it is
    ///
    /// Not for its stops: they are the rows under it, named there and in that
    /// order, and a trip through six of them spelled out runs longer than the
    /// bar is wide. What the whole of it comes to is the panel's to say.
    #[test]
    fn a_trips_row_is_named_for_its_legs() {
        assert_eq!(
            Section::Trip("SOL -> LAVE".to_owned()).said(1),
            "1 Leg Route"
        );
        assert_eq!(
            Section::Trip("SOL -> LAVE -> DISO".to_owned()).said(2),
            "2 Leg Route"
        );
    }

    /// A section's own row reads the same way as the rows under it
    ///
    /// The dot turns all of them off, a double click frames all of them, and
    /// the mark takes them all away -- the row's three gestures said of the
    /// whole section at once, which is what the row is for.
    ///
    /// A click on the name asks nothing. There is no one filter for a section
    /// to be the one being worked with, so the gesture that would say which
    /// has nothing to say here.
    #[test]
    fn a_sections_row_reads_like_the_rows_under_it() {
        let asked = |close, switch, double| match asked_of_row(
            close, false, switch, double, false,
        ) {
            Some(RowGesture::LetGo) => Some(FilterAction::LetGo),
            Some(RowGesture::Toggle) => Some(FilterAction::Toggle),
            Some(RowGesture::Frame) => Some(FilterAction::Frame),
            _ => None,
        };

        assert_eq!(asked(false, true, false), Some(FilterAction::Toggle));
        assert_eq!(asked(false, false, true), Some(FilterAction::Frame));
        assert_eq!(asked(true, false, false), Some(FilterAction::LetGo));
        // A press on the name alone, which used to turn them all off.
        assert_eq!(asked(false, false, false), None);
    }

    /// A selected system is flown to by a double click, not a single one
    ///
    /// The same press that frames a filter, said of the one system the row
    /// stands for. Every case here carries the click as well: the buttons sit
    /// inside the row and egui answers the first click of a pair as a click,
    /// so a press that means anything else arrives with one beside it and has
    /// to beat it.
    #[test]
    fn a_selected_system_is_flown_to_by_a_double_click() {
        assert_eq!(
            asked_of_selection(false, false, true, true),
            Some(SelectionAction::Travel)
        );
        assert_eq!(
            asked_of_selection(true, false, false, true),
            Some(SelectionAction::LetGo)
        );
        assert_eq!(
            asked_of_selection(false, true, false, true),
            Some(SelectionAction::Describe)
        );

        // A click on the name alone, which used to fly the camera there. The
        // row already stands for something picked out, so it asks nothing.
        assert_eq!(asked_of_selection(false, false, false, true), None);
        assert_eq!(asked_of_selection(false, false, false, false), None);
    }

    /// What a press on a row means depends on where in it it landed
    ///
    /// Five things share the space of a row and a press lands on one of them.
    /// The order is the whole of the rule, and two pairs are why it is
    /// written down rather than left to a chain of ifs nobody re-reads.
    #[test]
    fn a_press_on_a_row_means_the_one_thing_it_landed_on() {
        assert_eq!(asked_of_row(false, false, false, false, false), None);

        // The dot stands inside the row, so a press on it is a press on the
        // row as well -- and it means the dot. Otherwise turning a filter off
        // would pick it out on the way.
        assert_eq!(
            asked_of_row(false, false, true, false, true),
            Some(RowGesture::Toggle)
        );
        // Egui answers the first click of a pair as a click, so a double
        // arrives with a click beside it. The double has to win, or framing a
        // filter would pick it out first.
        assert_eq!(
            asked_of_row(false, false, false, true, true),
            Some(RowGesture::Frame)
        );
        // The marks at the end beat all of it: they are what was pressed.
        assert_eq!(
            asked_of_row(true, false, true, true, true),
            Some(RowGesture::LetGo)
        );
        assert_eq!(
            asked_of_row(false, true, true, true, true),
            Some(RowGesture::Describe)
        );
        // And a plain click on the name means the filter itself.
        assert_eq!(
            asked_of_row(false, false, false, false, true),
            Some(RowGesture::Select)
        );
    }

    /// A click is answered once no double can still arrive, and a double
    /// takes it away
    ///
    /// [`asked_of_row`] above arbitrates the flags of one pass, and egui
    /// raises `clicked()` on the first release of a double click a whole frame
    /// before it reports the double — so the priority is handed the click
    /// alone, first, and acts on it. Which is why the click is held: a
    /// double click on a route's row is a flight to it and must not also
    /// replace what the user has picked out.
    #[test]
    fn a_click_waits_to_see_whether_it_is_half_of_a_double() {
        let ctx = egui::Context::default();
        let row = egui::Id::new("a row");
        // A pass, and what the row was told, at `time` seconds.
        let pass = |time: f64, click: bool, double: bool| {
            let mut answered = false;
            let _ = ctx.run_ui(
                egui::RawInput { time: Some(time), ..Default::default() },
                |ui| {
                    answered = settled_click(ui, row, click, double).is_some();
                },
            );
            answered
        };
        // Longer than egui's own `max_double_click_delay`.
        let window = 0.4;

        // A click alone says nothing yet, and still nothing while a double
        // could arrive.
        assert!(!pass(1.0, true, false), "the click was answered at once");
        assert!(!pass(1.05, false, false), "answered inside the window");
        assert!(pass(1.0 + window, false, false), "never answered at all");
        // And once only.
        assert!(
            !pass(1.0 + window * 2., false, false),
            "the same click was answered twice"
        );

        // The second half of a double takes the pending click with it, so the
        // window passing afterwards answers nothing.
        assert!(!pass(2.0, true, false));
        assert!(!pass(2.05, true, true), "the double read as a click");
        assert!(
            !pass(2.0 + window, false, false),
            "a double click picked its row out as well"
        );
    }

    #[test]
    fn a_route_row_says_how_many_jumps_it_is() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2, 3, 4, 5],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"4 hops".to_owned()), "{said:?}");
    }

    /// A route row too narrow for its name keeps both ends of it
    ///
    /// The row cuts what it draws to what the dot, the count and the marks
    /// leave, and a route cut from the right hand end is a route that no
    /// longer says where it goes.
    #[test]
    fn a_route_row_cut_down_still_says_where_it_goes() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "SIGMA DRACONIS -> MINISTRY".to_owned(),
            systems: vec![1, 2, 3],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            // Too narrow for the name, whatever the bar is set to.
            ui.set_max_width(200.);
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        let name = said
            .iter()
            .find(|line| line.contains(ARROW))
            .unwrap_or_else(|| panic!("{said:?}"));
        let (from, to) = name.split_once(ARROW).expect("a route");
        // Something gave, and what is left of either end is that end: the
        // start of the name it was cut from, rather than a stretch of the
        // other one that happened to be nearer the middle.
        assert!(name.contains(CUT), "{name:?}");
        let (from, to) = (from.trim_end_matches(CUT), to.trim_end_matches(CUT));
        assert!(
            !from.is_empty() && "SIGMA DRACONIS".starts_with(from),
            "{name:?}"
        );
        assert!(!to.is_empty() && "MINISTRY".starts_with(to), "{name:?}");
    }

    /// One jump is a hop rather than one hops
    #[test]
    fn a_route_of_one_jump_says_it_in_the_singular() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2],
            range: "10".to_owned(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.contains(&"1 hop".to_owned()), "{said:?}");
    }

    /// A list whose items change is not read as a widget changing identity
    ///
    /// Egui watches for a rect that keeps its place while everything in it
    /// changes id, and paints a red rectangle over it as well as warning. A
    /// fresh search and a replaced selection are both exactly that shape, so
    /// the rows are keyed on where they sit and the ids stay put while what
    /// they are about changes underneath.
    #[test]
    fn a_list_whose_items_change_is_not_an_id_change() {
        let results = crate::tests::between_passes(
            draw_found(&["SOL", "SOLATI"]),
            draw_found(&["BARNARD", "WOLF 359"]),
        );
        let replaced = crate::tests::between_passes(
            draw_selected(&["SOL"]),
            draw_selected(&["BARNARD"]),
        );
        let shorter = crate::tests::between_passes(
            draw_selected(&["SOL", "BARNARD", "WOLF 359"]),
            draw_selected(&["SOL", "WOLF 359"]),
        );

        assert!(results.is_empty(), "{results:?}");
        assert!(replaced.is_empty(), "{replaced:?}");
        assert!(shorter.is_empty(), "{shorter:?}");
    }

    /// What the bar's box says, holding `query` against `results`
    fn box_said(query: Option<&str>, results: &[&str]) -> Vec<String> {
        words(|ui| {
            let mut value = query.map(str::to_owned);
            let results = results_of(results);
            ask_box(ui, &mut value, "Search", !results.is_empty(), false);
        })
    }

    /// A results list holding `names`, all of them placed
    fn results_of(names: &[&str]) -> SearchResults {
        results(names, true)
    }

    /// What is picked out after a line naming `name` is clicked
    ///
    /// Three passes: egui knows where a widget stands only once it has been
    /// drawn, so the click lands on the pass after the list was laid out.
    fn line_clicked(picking: Picking, held: &[&str], name: &str) -> Selection {
        let ctx = crate::tests::context();
        let offers = results_of(&["SOL", "SOLATI"]);
        let mut selection = holding(held);
        let mut at = None;
        for pass in 0..3 {
            let input = match (pass, at) {
                (2, Some(at)) => clicking(at),
                _ => egui::RawInput::default(),
            };
            let output = ctx.run_ui(input, |ui| {
                let mut travelled = None;
                let mut described = None;
                found(
                    ui,
                    &offers,
                    None,
                    picking,
                    &mut selection,
                    &mut travelled,
                    &mut described,
                );
            });
            if at.is_none() {
                for (said, rect) in text_at(&output.shapes) {
                    if said == name {
                        at = Some(rect.center());
                    }
                }
            }
        }

        selection
    }

    /// Every word the pass painted, and where
    fn text_at(
        shapes: &[egui::epaint::ClippedShape],
    ) -> Vec<(String, egui::Rect)> {
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

        let mut found = Vec::new();
        for shape in shapes {
            walk(&shape.shape, &mut found);
        }
        found
    }

    /// A stop clicked in the route form is added to what is already picked
    ///
    /// Reported as a gap: the route runs between what is picked out, and
    /// while that mode was open there was no way to name a system. The field
    /// is there now, and what a click on its answer means is the whole of the
    /// difference between the two modes. A route wants two systems or more,
    /// so a click that let go of the last stop in order to take the next
    /// could never build one.
    #[test]
    fn a_stop_clicked_in_the_route_joins_what_is_picked() {
        let picked = line_clicked(Picking::Gathers, &["SOL"], "SOLATI");

        assert_eq!(
            stops_of(&picked),
            Ok(vec!["SOL", "SOLATI"]),
            "the stop did not join what was already picked out"
        );
    }

    /// Where a search picks out the one system the click named
    ///
    /// The other half of the same difference. A search is one name asked
    /// about and the system meant picked out, as a star on the map is, and
    /// the modifier is what gathers there.
    #[test]
    fn a_search_result_clicked_stands_in_for_what_was_picked() {
        let picked = line_clicked(Picking::Asked, &["SOL"], "SOLATI");

        assert_eq!(
            picked.systems().map(|system| system.name()).collect::<Vec<_>>(),
            vec!["SOLATI"],
            "the click gathered rather than picking one out"
        );
    }

    /// A box with something in it offers to empty itself
    #[test]
    fn a_box_holding_a_query_offers_to_clear_it() {
        assert!(box_said(Some("SOL"), &[]).iter().any(|line| line == CLOSE));
    }

    /// So does one whose query is gone but whose answer is still standing
    ///
    /// The list outlives what was typed, since picking a system out of it
    /// leaves it up to be picked from again. A mark that went with the query
    /// would leave the list with no way to dismiss it but typing.
    #[test]
    fn a_box_answered_by_a_list_offers_to_clear_it() {
        assert!(box_said(None, &["SOL"]).iter().any(|line| line == CLOSE));
    }

    /// An empty box offers nothing, having nothing to take away
    #[test]
    fn an_empty_box_offers_no_mark() {
        assert!(!box_said(None, &[]).iter().any(|line| line == CLOSE));
    }

    /// Clearing takes the query, the note and the list together
    ///
    /// All three answer the one name, so leaving any of them standing leaves
    /// an answer to a question no longer on screen.
    #[test]
    fn clearing_takes_everything_that_answered_the_name() {
        let mut value = Some("SOL".to_owned());
        let mut note = SearchNote(Some("No system named SOL".to_owned()));
        let mut results = results_of(&["SOLATI"]);

        cleared(&mut value, &mut note, &mut results);

        assert!(value.is_none());
        assert!(note.0.is_none());
        assert!(results.is_empty());
    }

    /// Nothing found draws nothing at all
    ///
    /// Rather than an empty box under the input. A search that found nothing
    /// is answered by the note, and a list standing empty beside it would be
    /// a second answer saying less.
    #[test]
    fn an_empty_list_is_not_drawn() {
        assert!(listed(&SearchResults::default(), None).is_empty());
    }

    /// The list paints in colors something can draw
    ///
    /// The distance at the end of a line is laid out apart from the line and
    /// painted against a placeholder, which is the arrangement that reaches
    /// the tessellator with nothing to draw and panics there.
    #[test]
    fn the_results_paint_in_colors() {
        painted(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(&["SOL", "NOWHERE"], false),
                Some(DVec3::ZERO),
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        });
    }

    /// And each line answers for itself
    #[test]
    fn the_result_lines_do_not_share_ids() {
        let said = crate::tests::complaints(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &results(&["SOL", "SOLATI", "SOLLARO"], true),
                None,
                Picking::Asked,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// A slider row drawn in the pane, as it comes out
    ///
    /// The row it filled, the room it was given, and how wide the box holding
    /// the value came out. Drawn the way the pane draws one rather than
    /// measured off the style, since what is asked for and what is taken are
    /// different questions and only the second is on screen.
    ///
    /// The pane slides in rather than appearing, and `animate_bool` wants time
    /// to pass before it is all the way out, so nothing is drawn inside it on
    /// the first frame. Hence the run of them, with the clock moving.
    struct Row {
        used: f32,
        room: f32,
    }

    /// Draw a real radius slider in a pane `width` wide, indented or not
    fn slider_row(width: f32, indented: bool) -> Row {
        let ctx = crate::tests::context();
        let mut row = Row { used: 0., room: 0. };
        let mut radius = 10_f32;
        for frame in 0..10 {
            let input = egui::RawInput {
                time: Some(frame as f64 * 0.1),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 600.),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                settings_pane(ui.ctx(), true, |ui| {
                    let mut draw = |ui: &mut Ui| {
                        row.room = ui.available_width();
                        row.used =
                            radius_slider(ui, &mut radius, Spyglass::CEILING)
                                .rect
                                .width();
                    };
                    if indented {
                        ui.indent("test", draw);
                    } else {
                        draw(ui);
                    }
                });
            });
        }
        row
    }

    /// What a radius comes out at, having been offered up to `ceiling`
    fn drawn_radius(start: f32, ceiling: f32) -> f32 {
        let ctx = crate::tests::context();
        let mut radius = start;
        for frame in 0..10 {
            let input = egui::RawInput {
                time: Some(frame as f64 * 0.1),
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280., 600.),
                )),
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                settings_pane(ui.ctx(), true, |ui| {
                    radius_slider(ui, &mut radius, ceiling);
                });
            });
        }
        radius
    }

    /// A radius past the galaxy's edge is brought inside it
    ///
    /// That bound does not move, so a radius past it is not a setting the map
    /// cannot honor yet but one it can never honor, and correcting it loses
    /// nothing that could come back.
    #[test]
    fn a_radius_is_held_within_the_galaxy() {
        assert_eq!(drawn_radius(5e6, 5e6), Spyglass::CEILING);
    }

    /// A radius under the rail's least is kept, not written up to it
    ///
    /// The reach follows the camera all the way in
    /// ([`crate::systems::reach_with_camera`]), so a reach drawn in under the
    /// rail is a real setting. Written back, this pane being open would hold
    /// the sky within a thousandth of a light year open every frame and put
    /// the neighbours of a system flown into back on the map.
    #[test]
    fn a_radius_under_the_rail_is_kept() {
        assert_eq!(drawn_radius(1e-6, 100.), 1e-6);
    }

    /// A ceiling that comes down does not take the setting with it
    ///
    /// What is asked for and what can be drawn are two questions. The
    /// spyglass answers the second and moves as the map is flown, so letting
    /// it write the first loses the asking the moment it drops underneath,
    /// with nothing said and no way back but to ask again.
    #[test]
    fn a_radius_over_the_ceiling_is_kept() {
        assert_eq!(drawn_radius(5e4, 100.), 5e4);
    }

    /// How much of the value one pixel of a `rail` pixels wide is worth
    ///
    /// The rail is logarithmic over the whole range, so a pixel is a fixed
    /// multiple of whatever the value is rather than a fixed distance. This
    /// is that multiple, less the one, so it reads as the fraction the drag
    /// speed is also given as.
    fn rail_precision(rail: f32) -> f32 {
        let decades = (Spyglass::CEILING / Spyglass::FLOOR).log10();
        10_f32.powf(decades / rail) - 1.
    }

    /// The box beside a radius is finer than the rail
    ///
    /// Which is the whole reason for having both. The rail crosses four
    /// decades in the width of the pane and so reaches anywhere and settles
    /// on nothing; the box is what settles. A box no finer than the rail is
    /// a second way to do the same coarse thing.
    #[test]
    fn the_radius_box_is_finer_than_its_rail() {
        // What the row came to, less the box at its end and the gap before it.
        let gap = egui::style::Spacing::default().item_spacing.x;
        let rail =
            rail_precision(slider_row(1280., false).used - VALUE_WIDTH - gap);

        assert!(
            RADIUS_DRAG * 5. < rail,
            "a drag worth {RADIUS_DRAG} of the value against a rail worth \
             {rail} of it is no finer to speak of"
        );
    }

    /// A ceiling at the floor is still a radius
    ///
    /// Names reach no further than the spyglass, so a spyglass wound all the
    /// way in leaves them a range with no room in it at all. A logarithmic
    /// rail divides by the span it is given.
    #[test]
    fn a_range_with_no_room_in_it_still_draws() {
        assert_eq!(
            drawn_radius(Spyglass::FLOOR, Spyglass::FLOOR),
            Spyglass::FLOOR
        );
    }

    /// How far a measured width may sit from the one asked for
    ///
    /// Egui rounds a rectangle to whole pixels, so two of them that agree can
    /// still differ by a fraction of one.
    const SLACK: f32 = 1.;

    /// A slider row fills the pane it is drawn in
    ///
    /// Egui sizes a rail from the style rather than from the room it is given,
    /// and sizes the box beside it to the number in it, so left alone a row
    /// is an island of the same hundred pixels and a ragged box, ending
    /// wherever that happens to leave it.
    #[test]
    fn a_slider_row_fills_the_pane() {
        for width in [1280., 400.] {
            let row = slider_row(width, false);

            assert!(
                (row.used - row.room).abs() < SLACK,
                "in a pane {width} wide a row of {} filled {} of it",
                row.used,
                row.room
            );
        }
    }

    /// And fills an indent, which is narrower than the pane
    ///
    /// The name radius hangs under the checkbox that turns names on, so it is
    /// drawn a step in from the edge. Sized once for the pane it would run out
    /// past the end of its own line by exactly that step.
    #[test]
    fn a_slider_row_fills_an_indent() {
        let indented = slider_row(1280., true);
        let plain = slider_row(1280., false);

        assert!(
            indented.room < plain.room,
            "an indent of {} is no narrower than the {} around it",
            indented.room,
            plain.room
        );
        assert!(
            (indented.used - indented.room).abs() < SLACK,
            "a row of {} filled {} of the indent",
            indented.used,
            indented.room
        );
    }

    /// The box holding the value is the width kept for it
    ///
    /// Every one of them the same, so that a column of sliders ends in a
    /// column of numbers rather than in a ragged edge.
    #[test]
    fn the_value_box_is_the_width_kept_for_it() {
        let ctx = crate::tests::context();
        let mut value = 10_f32;
        let mut width = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            width =
                value_box(ui, egui::DragValue::new(&mut value)).rect.width();
        });

        assert!((width - VALUE_WIDTH).abs() < SLACK);
    }

    /// Egui says nothing about the ids the filter rows use
    #[test]
    fn the_filter_rows_do_not_share_ids() {
        use crate::tests::complaints;

        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Alpha".into() });
        filters.add(Filter::Faction { id: 2, name: "Beta".into() });
        filters.add(Filter::Route {
            label: "A -> B".into(),
            systems: vec![1, 2],
            range: "10".into(),
            trip: None,
            drive: Drive::Unaided,
            how: Routing::default(),
        });
        let mut panels = Panels::default();

        let said = complaints(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });

        assert!(said.is_empty(), "{said:?}");
    }

    /// A row is keyed on what it is about, not on where it was drawn
    ///
    /// The rows of the bar do not keep their places: a note comes and goes
    /// above them, the selection's row comes and goes with the selection, and
    /// dropping one filter moves every row below it up. An id taken from the
    /// draw order would hand the row that moved up whatever egui had been
    /// remembering against the one that left.
    #[test]
    fn a_row_is_keyed_on_what_it_is_about() {
        let ctx = crate::tests::context();
        let (mut first, mut moved, mut other) = (None, None, None);

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            first = Some(row_of(ui, 20., ("filter-row", 7)).1.id);
            // Anything at all between them shifts what comes after.
            ui.label("a note that comes and goes");
            moved = Some(row_of(ui, 20., ("filter-row", 7)).1.id);
            other = Some(row_of(ui, 20., ("filter-row", 9)).1.id);
        });

        assert_eq!(first, moved, "the same row moved and changed identity");
        assert_ne!(first, other, "two rows share one identity");
    }

    /// And so are the marks it ends with
    #[test]
    fn the_marks_are_keyed_on_the_row_they_end() {
        let ctx = crate::tests::context();
        let (mut first, mut moved) = (None, None);
        let at =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80., 20.));

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let buttons = lay_out_buttons(ui);
            first = Some(
                place_buttons(ui, at, buttons, ("filter-row", 7)).close.id,
            );
            ui.label("a note that comes and goes");
            let buttons = lay_out_buttons(ui);
            moved = Some(
                place_buttons(ui, at, buttons, ("filter-row", 7)).close.id,
            );
        });

        assert_eq!(first, moved);
    }

    /// What the spyglass reaches is said with nothing asked of the map
    ///
    /// One number, since with nothing excluded the two are the same number
    /// and saying it twice says nothing. Worth saying at all because it is
    /// what tells the user whether they are looking at a handful of systems
    /// or a hundred thousand.
    #[test]
    fn the_reach_alone_is_one_number() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 324, total: 324 },
                false,
                false,
                false,
            )
        });

        assert!(said.contains(&"324 in spyglass".to_owned()), "{said:?}");
    }

    /// With something excluded and drawn faintly, both numbers are said
    ///
    /// The sky behind what is picked out is on screen, so how much of it
    /// there is answers what the user can see.
    #[test]
    fn what_is_dimmed_is_counted_behind_what_is_not() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 8, total: 324 },
                true,
                false,
                false,
            )
        });

        assert!(said.contains(&"8 of 324 in spyglass".to_owned()), "{said:?}");
    }

    /// With it not drawn at all, only what can be seen is said
    ///
    /// The excluded systems are neither on screen nor fetched, so the larger
    /// number describes a place the user cannot see and a sky the map has not
    /// got.
    #[test]
    fn what_is_not_drawn_is_not_counted() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 8, total: 324 },
                false,
                false,
                false,
            )
        });

        assert!(said.contains(&"8 in spyglass".to_owned()), "{said:?}");
        assert!(!said.iter().any(|line| line.contains("324")), "{said:?}");
    }

    /// A route too long for the room keeps both of its ends
    ///
    /// Cut from the right, every route out of one system is called the same
    /// thing, and what goes is the end it was plotted to reach.
    #[test]
    fn a_route_cut_down_still_says_where_it_goes() {
        let said = shortened("SIGMA DRACONIS -> MINISTRY", 18);

        assert_eq!(said, "SIGMA.. -> MINIS..");
        assert_eq!(said.chars().count(), 18);
    }

    /// Two ends of the same length are cut to the same length
    ///
    /// Neither end is worth more than the other, so what one is given the
    /// other is given. An odd character over goes to the name that leads,
    /// that being the one read first.
    #[test]
    fn two_ends_of_a_size_are_cut_to_a_size() {
        let both = shortened("COL 1232312312 -> COL 3211231231", 22);
        assert_eq!(both, "COL 123.. -> COL 321..");

        let odd = shortened("COL 1232312312 -> COL 3211231231", 21);
        assert_eq!(odd, "COL 123.. -> COL 32..");
    }

    /// What fits is left alone, route or not
    #[test]
    fn what_fits_is_said_whole() {
        assert_eq!(shortened("SOL -> WOLF 359", 20), "SOL -> WOLF 359");
        assert_eq!(shortened("Alliance of Sol", 4), "Alliance of Sol");
    }

    /// An end that does not want its half leaves the rest to the other
    ///
    /// Half each is the fair share and not the useful one: a route from SOL
    /// has room going spare at one end and a name being cut at the other.
    #[test]
    fn an_end_with_room_to_spare_gives_it_to_the_other() {
        let said = shortened("SOL -> COL 285 SECTOR SC-K B22-2", 20);

        assert_eq!(said, "SOL -> COL 285 SEC..");
        assert_eq!(said.chars().count(), 20);
    }

    /// A name is cut where the room runs out, word or no word
    ///
    /// Systems are told apart by the tails of their names, so every character
    /// there is room for is worth having. Backed up to the word before it,
    /// `COL 285 SECTOR SC-K B22-2` and `COL 285 SECTOR XY-Z A1-0` are the same
    /// row twice.
    ///
    /// A trailing space goes with the cut, being a character that says
    /// nothing.
    #[test]
    fn a_name_is_cut_where_the_room_runs_out() {
        assert_eq!(clipped("COL 285 SECTOR SC-K B22-2", 12), "COL 285 SE..");
        assert_eq!(clipped("COL 285 SECTOR", 9), "COL 285..");
        assert_eq!(clipped("SIGMA DRACONIS", 8), "SIGMA..");
        assert_eq!(clipped("MINISTRY", 6), "MINI..");
    }

    /// Room for nothing but the mark is answered with the mark
    ///
    /// A row of them is at least a row, where a name cut to no characters at
    /// all is a gap the reader has to work out the meaning of.
    #[test]
    fn a_name_with_no_room_is_all_mark() {
        assert_eq!(clipped("MINISTRY", 2), "..");
        assert_eq!(clipped("MINISTRY", 1), ".");
        assert_eq!(clipped("MINISTRY", 0), "");
    }

    /// The count fits the bar at the size the sky is heading for
    ///
    /// Both numbers grow with what has been synced, and the line has to hold
    /// them on one row: wrapped, it is a line that moves the rows under it
    /// about as the user flies. Seven digits either side comes to 235 of the
    /// 325 the bar is wide, so the sky can grow well past millions of systems
    /// before the line has nowhere left to grow into.
    #[test]
    fn the_count_fits_the_bar_at_millions() {
        let ctx = crate::tests::context();
        let said = format!(
            "{} of {} in spyglass",
            thousands(1_234_567),
            thousands(7_654_321)
        );

        let mut width = 0.;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            width = egui::WidgetText::from(egui::RichText::new(&said).weak())
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Extend),
                    f32::INFINITY,
                    egui::TextStyle::Body,
                )
                .size()
                .x;
        });

        assert!(
            width <= BAR_WIDTH,
            "{said:?} wants {width}px of the {BAR_WIDTH} there are"
        );
    }

    /// An empty sky says nothing rather than saying it is empty
    ///
    /// Which is the map before its first fetch lands, where a nought would
    /// read as an answer rather than as nothing having been asked yet.
    #[test]
    fn an_empty_reach_says_nothing() {
        let said = words(|ui| {
            reaching(
                ui,
                &InReach { admitted: 0, total: 0 },
                false,
                false,
                false,
            )
        });

        assert!(!said.iter().any(|line| line.contains("spyglass")), "{said:?}");
    }

    /// And so does a sky of one, however it comes to be one
    ///
    /// Which is every descent: the camera inside a system holds that system
    /// and nothing else, and `1 in spyglass` beside the system's own name is
    /// a number saying less than the word next to it.
    #[test]
    fn a_reach_of_one_system_says_nothing() {
        for reach in [
            InReach { admitted: 1, total: 1 },
            InReach { admitted: 0, total: 1 },
        ] {
            let dimming = reach.admitted != reach.total;
            let said = words(|ui| reaching(ui, &reach, dimming, false, false));

            assert!(
                !said.iter().any(|line| line.contains("spyglass")),
                "{said:?}"
            );
        }
    }

    /// The filter rows come out in colors something can draw
    ///
    /// Every galley here is laid out strong or weak, which resolves a color,
    /// so the placeholder each is painted with is never reached. That holds
    /// by how the rows happen to be styled and nothing else, and one plain
    /// piece of text would take the whole bar down.
    ///
    /// Covers the marks as well, which the selection's row draws the same
    /// way.
    #[test]
    fn the_filter_rows_paint_in_colors() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Zargon Front".into() });
        filters.add(Filter::Faction { id: 2, name: "Alliance".into() });
        filters.toggle(1);
        let mut panels = Panels::default();

        painted(|ui| {
            applied(ui, &mut filters, &mut panels, &mut 0);
        });
    }

    /// What `contents` painted with the pointer resting at `at`
    ///
    /// Two passes, since egui works out what the pointer is over from where
    /// the widgets were the pass before. The second is the one read.
    fn under_pointer(
        at: egui::Pos2,
        mut contents: impl FnMut(&mut Ui),
    ) -> Vec<egui::Shape> {
        let ctx = crate::tests::context();
        let input = || egui::RawInput {
            events: vec![egui::Event::PointerMoved(at)],
            ..Default::default()
        };
        let _ = ctx.run_ui(input(), |ui| contents(ui));
        let output = ctx.run_ui(input(), |ui| contents(ui));

        output.shapes.into_iter().map(|clipped| clipped.shape).collect()
    }

    /// Every filled rectangle among `shapes`, however deeply nested
    fn rectangles(shapes: &[egui::Shape]) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, into: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) => into.push(rect.rect),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in shapes {
            walk(shape, &mut found);
        }
        found
    }

    /// The color each piece of text among `shapes` was painted in
    fn colors(shapes: &[egui::Shape]) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, into: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.fallback_color),
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        walk(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut found = Vec::new();
        for shape in shapes {
            walk(shape, &mut found);
        }
        found
    }

    /// A mark under the pointer brightens and paints nothing behind itself
    ///
    /// The row a mark sits in lights up under the pointer already, so a
    /// rectangle drawn inside that one reads as a button dropped into the
    /// row rather than as part of it.
    #[test]
    fn a_mark_lights_without_a_background() {
        let row = egui::Rect::from_min_size(
            egui::pos2(0., 0.),
            egui::vec2(200., 20.),
        );
        // Just inside the close mark, which stands outermost.
        let on_the_mark =
            egui::pos2(row.right() - ROW_PADDING - 1., row.center().y);

        let painted = under_pointer(on_the_mark, |ui| {
            let buttons = lay_out_buttons(ui);
            place_buttons(ui, row, buttons, "row");
        });

        // The mark answered the pointer, which is what leaves the assertion
        // below with something to say.
        assert!(
            colors(&painted)
                .contains(&egui::Visuals::default().strong_text_color()),
            "nothing lit up, so nothing was under the pointer"
        );
        assert_eq!(rectangles(&painted), Vec::new(), "a mark painted a box");
    }

    /// A field holding an empty string says what it wants
    ///
    /// Which is the whole of how the placeholder comes back. A field is
    /// clicked into and clicked straight out of again, and the focus moves
    /// between two fields within one frame, so anything that waited to be
    /// told the field had lost the focus would be waiting for a moment the
    /// field is not always drawn to see. Nothing typed is nothing typed,
    /// however the field came to hold an empty string.
    #[test]
    fn a_field_holding_nothing_says_what_it_wants() {
        let mut value = Some(String::new());

        let said = words(|ui| {
            singleline(ui, &mut value, "Search", 0., false);
        });

        assert!(said.contains(&"Search".to_owned()), "{said:?}");
    }

    /// And one holding a name says the name
    #[test]
    fn a_field_holding_a_name_says_the_name() {
        let mut value = Some("SOL".to_owned());

        let said = words(|ui| {
            singleline(ui, &mut value, "Search", 0., false);
        });

        assert!(said.contains(&"SOL".to_owned()), "{said:?}");
        assert!(!said.contains(&"Search".to_owned()), "{said:?}");
    }

    /// A field at rest and the same field with the caret in it
    ///
    /// Answers what each pass painted and what the field was left holding.
    /// Clicked into rather than focused by hand, since where the caret lands
    /// is what settles which of the two the field is drawing.
    fn field_clicked_into() -> (Vec<String>, Vec<String>, Option<String>) {
        let ctx = crate::tests::context();
        let mut value: Option<String> = None;

        let fields = |input, value: &mut Option<String>| {
            let mut at = egui::Rect::NOTHING;
            let output = ctx.run_ui(input, |ui| {
                at = singleline(ui, value, "Search", 0., false).rect;
            });
            let mut said = Vec::new();
            for shape in &output.shapes {
                if let egui::Shape::Text(text) = &shape.shape {
                    said.push(text.galley.text().to_owned());
                }
            }
            (at, said)
        };

        // Two passes with nothing happening, to place the field.
        let _ = fields(egui::RawInput::default(), &mut value);
        let (at, resting) = fields(egui::RawInput::default(), &mut value);

        fields(clicking(at.center()), &mut value);
        let (_, editing) = fields(egui::RawInput::default(), &mut value);

        (resting, editing, value)
    }

    /// A gesture at `from`, released `drift` further on
    ///
    /// Egui reads a press and a release apart as a drag rather than a click,
    /// and it is a click it lets go of the focus on. Zero drift is a click.
    fn dragged(from: egui::Pos2, drift: f32) -> Vec<egui::RawInput> {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let to = egui::pos2(from.x + drift, from.y);
        let frame = |events| egui::RawInput { events, ..Default::default() };

        vec![
            frame(vec![egui::Event::PointerMoved(from), button(from, true)]),
            frame(vec![egui::Event::PointerMoved(to)]),
            frame(vec![button(to, false)]),
        ]
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
        let ctx = crate::tests::context();
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

    /// A field says what it wants whether or not the caret is in it
    ///
    /// The words are the only name it has. A form that puts the caret in a
    /// field the user did not click would otherwise hand them a blank box
    /// with nothing on screen to say what belongs in it.
    #[test]
    fn a_field_says_what_it_wants_either_way() {
        let (resting, editing, _) = field_clicked_into();

        assert!(resting.contains(&"Search".to_owned()), "{resting:?}");
        assert!(editing.contains(&"Search".to_owned()), "{editing:?}");
    }

    /// And holds none of them, however they were drawn
    ///
    /// The words a field stands there wanting are not words anybody typed, so
    /// a field showing them holds nothing. Were they its contents while it was
    /// being typed into, the first keystroke would land on the end of them.
    #[test]
    fn a_field_never_holds_what_it_only_wants() {
        let (_, _, value) = field_clicked_into();

        assert_eq!(typed(&value), None, "{value:?}");
    }

    /// A primary click at `at`, and the pointer moved there to make it
    fn clicking(at: egui::Pos2) -> egui::RawInput {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        egui::RawInput {
            events: vec![
                egui::Event::PointerMoved(at),
                button(true),
                button(false),
            ],
            ..Default::default()
        }
    }

    /// A number short enough to read is left as it is
    ///
    /// Which is most of what is handed over: the systems with nobody living
    /// in them far outnumber the inhabited ones, and a sky with a handful in
    /// reach is a handful.
    #[test]
    fn small_counts_are_left_alone() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(7), "7");
        assert_eq!(thousands(999), "999");
    }

    /// Longer ones are broken into threes from the right
    ///
    /// From the right, so that the leading group is whatever is left over
    /// rather than the number being padded to fit.
    #[test]
    fn long_counts_are_grouped_from_the_right() {
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(22_780), "22,780");
        assert_eq!(thousands(999_999), "999,999");
        assert_eq!(thousands(1_000_000), "1,000,000");
    }

    /// The largest counts on record still read
    ///
    /// The most populous systems run to eleven digits, which is the longest
    /// this is ever handed.
    #[test]
    fn the_largest_counts_are_grouped() {
        assert_eq!(thousands(22_780_919_531), "22,780,919,531");
    }

    /// A separator never leads or trails
    ///
    /// The grouping is decided per digit from how many follow it, so a count
    /// whose length is a multiple of three is where a stray leading comma
    /// would show up.
    #[test]
    fn grouping_never_leads_or_trails() {
        for count in [1u64, 100, 1_000, 100_000, 1_000_000] {
            let grouped = thousands(count);
            assert!(!grouped.starts_with(','), "{grouped} leads with one");
            assert!(!grouped.ends_with(','), "{grouped} trails one");
        }
    }

    /// A field clicked into and left alone holds nothing
    ///
    /// Egui hands back an empty string for it, and taking that as an answer
    /// is what has a form telling the user off for having touched it.
    #[test]
    fn a_field_only_typed_into_holds_anything() {
        assert_eq!(typed(&None), None);
        assert_eq!(typed(&Some(String::new())), None);
        assert_eq!(typed(&Some("   ".to_owned())), None);
        assert_eq!(typed(&Some(" Sol ".to_owned())), Some("Sol"));
    }

    /// A distance is what the range field is for
    #[test]
    fn a_range_is_a_distance() {
        assert_eq!(jump_range("10"), Ok(10.));
        assert_eq!(jump_range("10.5"), Ok(10.5));
    }

    /// Room around what was typed is not what was meant by it
    #[test]
    fn a_range_may_be_typed_with_room_around_it() {
        assert_eq!(jump_range("  10  "), Ok(10.));
    }

    /// Anything that is not a number is not a range
    #[test]
    fn a_range_that_is_not_a_number_is_refused() {
        assert!(jump_range("far").is_err());
        assert!(jump_range("10 Ly").is_err());
    }

    /// A ship that jumps nowhere plots no route
    ///
    /// Both of these parse, so nothing but asking what the number means
    /// would catch them.
    #[test]
    fn a_range_of_nothing_or_less_is_refused() {
        assert!(jump_range("0").is_err());
        assert!(jump_range("-5").is_err());
    }

    /// A supercharged route wants a supercharge table, and says so
    ///
    /// Reported: the table was deleted and the map plotted anyway, handing
    /// back the unaided route — under the supercharged drive's name, a route
    /// carrying what it was plotted with and its panel saying so. So the map
    /// claimed to have plotted something it had not, and the only tell was a
    /// jump count that looked high.
    ///
    /// An empty table is a different answer and not this one: published with
    /// nothing in it says there is nowhere to supercharge, and the unaided
    /// route is right.
    #[test]
    fn a_supercharged_route_is_refused_without_a_table_to_plot_it() {
        let absent = crate::Boosts::absent();
        let published = crate::Boosts::of(Vec::new());

        // Unaided asks nothing of the table either way.
        assert_eq!(plotting("50", Drive::Unaided, &absent), Ok(50.));
        assert_eq!(plotting("50", Drive::Unaided, &published), Ok(50.));

        // A drive that can take a jet cone cannot be answered without one.
        for drive in [Drive::Standard, Drive::Optimised] {
            assert!(
                plotting("50", drive, &absent).is_err(),
                "{drive:?} plotted with no table to plot it from"
            );
            assert_eq!(
                plotting("50", drive, &published),
                Ok(50.),
                "{drive:?} refused against a table that is simply empty"
            );
        }

        // And the range is still asked first, so the nearer trouble is the
        // one reported.
        assert!(plotting("far", Drive::Standard, &published).is_err());
    }

    /// The stops go out in the order they were picked, unless the map is asked
    ///
    /// Which is the whole difference between a trip through stops and a set
    /// of destinations: the first is an itinerary the user wrote and the
    /// second is a question about which way round is cheapest.
    #[test]
    fn the_stops_go_out_in_the_order_they_were_picked() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        assert_eq!(
            asked_in_order(&stops, &picked, Shape::FromFirst, None),
            vec!["Test 0", "Test 1", "Test 2", "Test 3"]
        );
    }

    /// And in the cheapest order where it was
    ///
    /// The first stays where it was put, a trip having to set out from
    /// somewhere; the rest are reached whichever way costs the fewest jumps.
    #[test]
    fn a_cheapest_order_reaches_them_all_the_short_way() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        let asked =
            asked_in_order(&stops, &picked, Shape::FromFirst, Some(10.));

        assert_eq!(asked[0], "Test 0");
        assert_eq!(asked, vec!["Test 0", "Test 3", "Test 2", "Test 1"]);
    }

    /// A trip asked for as a loop is asked for the leg home as well
    ///
    /// The stops are what the legs are cut from, so the way to ask for the
    /// flight home is to name the first stop again at the end. Which is what
    /// makes a loop nothing special to anything downstream: the leg home is
    /// walked, drawn, rowed and costed as the others are.
    ///
    /// In the order picked here, since a loop is a shape rather than an
    /// ordering: closing a trip the user ordered themselves is a run out and
    /// back the way they asked for.
    #[test]
    fn a_looping_trip_comes_home_to_where_it_set_out() {
        let picked = strung_out(&[20., 0., 10., 30.]);
        let stops: Vec<&str> = stops_of(&picked).expect("stops");

        assert_eq!(
            asked_in_order(&stops, &picked, Shape::Loop, None),
            vec!["Test 0", "Test 1", "Test 2", "Test 3", "Test 0"]
        );
    }

    /// The two flags the form holds come to one shape
    ///
    /// A loop is only asked for where there is a trip to close: the control
    /// is not offered under two stops, and a flag left standing from a wider
    /// set is not obeyed either — nobody could see the control to let go of
    /// it. And a loop holds its start whatever the free-start flag says,
    /// every way round a ring costing the same.
    #[test]
    fn a_loop_is_asked_for_only_where_there_is_a_trip_to_close() {
        let asking = |looping, any_start, stops| {
            let fields = BarFields { looping, any_start, ..default() };
            shape_of(&fields, stops)
        };

        assert_eq!(asking(true, false, 3), Shape::Loop);
        assert_eq!(asking(true, true, 3), Shape::Loop);
        assert_eq!(asking(true, false, 2), Shape::FromFirst);
        assert_eq!(asking(true, true, 2), Shape::Anywhere);
        assert_eq!(asking(false, false, 3), Shape::FromFirst);
        assert_eq!(asking(false, true, 3), Shape::Anywhere);
    }

    /// And the legs it is flown in count the leg home
    #[test]
    fn a_loop_is_a_leg_longer_than_the_line_through_the_same_stops() {
        assert_eq!(legs_flown(3, Shape::FromFirst), 2);
        assert_eq!(legs_flown(3, Shape::Loop), 3);
        assert_eq!(legs_flown(0, Shape::Loop), 0);
    }

    /// A route runs through the systems picked out on the map
    #[test]
    fn a_route_runs_between_what_is_picked_out() {
        assert_eq!(
            stops_of(&holding(&["SOL", "SOLATI"])),
            Ok(vec!["SOL", "SOLATI"])
        );
    }

    /// In the order they were picked, the first of them being where it starts
    ///
    /// The two are told apart by nothing but that order, so a set read out in
    /// any other one plots the route backwards half the time.
    #[test]
    fn the_first_picked_is_where_a_route_starts() {
        assert_eq!(
            stops_of(&holding(&["SOLATI", "SOL"])),
            Ok(vec!["SOLATI", "SOL"])
        );
    }

    /// With nothing picked out there are no ends to run between
    #[test]
    fn a_route_with_nothing_picked_out_has_no_ends() {
        assert!(stops_of(&Selection::default()).is_err());
    }

    /// Nor with one, which is an end and no route
    #[test]
    fn a_route_out_of_one_system_is_refused() {
        assert!(stops_of(&holding(&["SOL"])).is_err());
    }

    /// A longer set is a route through the whole of it, in the order picked
    ///
    /// Every one of them is a stop rather than the first two being ends and
    /// the rest going unsaid: a set gathered out on the map is flown in the
    /// order it was gathered.
    #[test]
    fn a_longer_set_is_a_route_through_all_of_it() {
        assert_eq!(
            stops_of(&holding(&["SOL", "SOLATI", "SOLLARO"])),
            Ok(vec!["SOL", "SOLATI", "SOLLARO"])
        );
    }

    /// Each reason to refuse says something of its own
    ///
    /// They are read out of the one line, so a form that answered having
    /// picked nothing and having picked one alike would leave the user with
    /// no way to tell what it is still waiting for.
    #[test]
    fn the_reasons_are_told_apart() {
        let (nothing, single) = (Selection::default(), holding(&["SOL"]));
        let none = stops_of(&nothing);
        let one = stops_of(&single);

        assert!(none.is_err(), "{none:?}");
        assert!(one.is_err(), "{one:?}");
        assert_ne!(none, one);
    }
}
