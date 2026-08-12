//! The chrome standing between the user and the map
//!
//! A gear in the top left corner, the bar beside it, and the settings pane
//! that gear slides out from the left edge. What is known about the system the
//! user picked out is drawn by [`crate::systems::selection`], which owns the
//! fields it reads.
//!
//! The bar leads with a search box, which is what it is asked for most, but it
//! is not a search bar: three sections drop out of it, and search is one of
//! them. Filters are another and have nothing to say to the other two, so they
//! keep their own state in [`FilterBar`] and are reached through that alone. A
//! route is the third, and asks only what it may be flown in: which systems it
//! runs between is what is picked out on the map.

use crate::camera::{MoveCamera, OrbitCamera};
use crate::grid::{Bright, RulerUnit, ShowGrid, ShowMiddle, ShowPicked};
use crate::search::{Plot, Search, SearchNote, SearchResults, Searching};
use crate::systems::bodies::spawn::ShowOrbits;
use crate::systems::bodies::{Contents, Phase};
use crate::systems::despawn::Despawn;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::filter::{
    DimTo, FactionResults, Filter, Filters, Lookup, LookupNote, Resolving,
    SPANS, Watch,
};
use crate::systems::info::Panels;
use crate::systems::info::lasting;
use crate::systems::labels::NameRadius;
use crate::systems::labels::ShowBodyNames;
use crate::systems::pointing::PRIMARY;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::selection::{Picked, SELECTION, Selection};
use crate::systems::spawn::{ColorBy, ShowNames};
use crate::systems::{InReach, Spyglass};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui::{Context, Response, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use chrono::Utc;
use galos_db::factions::Faction as DbFaction;
use galos_db::systems::System as DbSystem;

pub fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.init_resource::<SettingsOpen>();
    app.init_resource::<PressOwner>();
    app.init_resource::<BarFields>();
    // The lettering leads, being what everything after it is drawn in.
    app.add_systems(EguiPrimaryContextPass, (lettering, chrome).chain());
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
pub struct PointerOverUi(pub bool);

/// Whether the settings pane is out
///
/// A resource rather than a local because the pane is drawn before the gear
/// that toggles it, so that the gear knows how far in the pane has come and
/// can stand clear of it.
#[derive(Resource, Default)]
pub struct SettingsOpen(bool);

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
pub struct PressOwner {
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
    pub fn settle(&mut self, buttons: &ButtonInput<MouseButton>, wanted: bool) {
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
    pub fn taken_by_ui(&self) -> bool {
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
pub struct Gesture<'w> {
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
    pub fn dragging_map(&self) -> bool {
        self.press.owner == Some(Owner::Map)
    }

    /// Whether `button` is down
    ///
    /// Which of them is being dragged with, once [`Self::dragging_map`] has
    /// said the drag is the map's at all. Offered here so that asking takes
    /// one thing rather than a system holding its own copy of the input
    /// beside this, which would be two readings of the same buttons sitting
    /// where they could be told apart.
    pub fn pressed(&self, button: MouseButton) -> bool {
        self.buttons.pressed(button)
    }

    /// Whether a click the map owns has just finished
    ///
    /// On the release, where the press landed in an earlier frame and the
    /// owner is already standing. A frame holding the whole click answers a
    /// frame later, through [`PressOwner::carried_over`], which is the one
    /// place that
    /// wait is spelled out.
    pub fn on_map(&self) -> bool {
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
/// What the rail leaves is the box holding the value and the gap between the
/// two, so the row ends flush with whatever it is standing in.
fn fill_width(ui: &mut Ui) {
    let gap = ui.spacing().item_spacing.x;
    ui.spacing_mut().slider_width =
        (ui.available_width() - VALUE_WIDTH - gap).max(0.);
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

/// Offer a radius over the whole range it may take
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
/// `ceiling` is as far as this one may reach, which for names is however far
/// the spyglass reaches. The rail clamps to it, so a radius set wide and then
/// hemmed in comes back to what is on offer.
fn radius_slider(ui: &mut Ui, radius: &mut f32, ceiling: f32) -> Response {
    let ceiling = ceiling.clamp(Spyglass::FLOOR, Spyglass::CEILING);
    // Read before the rail borrows it, and the reason it is read at all.
    let speed = (*radius * RADIUS_DRAG).max(f32::EPSILON) as f64;
    fill_width(ui);

    ui.horizontal(|ui| {
        ui.add(
            egui::Slider::new(radius, Spyglass::FLOOR..=ceiling)
                .logarithmic(true)
                .show_value(false),
        );
        value_box(
            ui,
            egui::DragValue::new(radius)
                .range(Spyglass::FLOOR..=ceiling)
                .speed(speed),
        );
    })
    .response
}

/// What the user has typed into the bar, and how much of it is out
///
/// The search box and the route's jump range, which are the fields the bar
/// itself owns. The filters are drawn between them and keep their own in
/// [`FilterBar`], so that nothing about a filter is reachable from here.
///
/// A resource rather than a local, so that what is typed outlives any one
/// pass over the bar and can be read from outside the system that draws it.
#[derive(Resource, Default)]
pub struct BarFields {
    /// The system named in the box the bar leads with
    system: Option<String>,
    /// How far the ship a route is plotted for jumps
    ///
    /// The one thing about a route that is typed. Which systems it runs
    /// between is picked out on the map, and a range is a fact about a ship
    /// with nothing on the map to point at.
    route_range: Option<String>,
    /// Whether the rest of the form is out below the input
    ///
    /// Turned on when a field takes focus and off when a press lands off the
    /// form, both of which [`main_bar`] settles at the end of a frame from
    /// what it has just drawn. So this is one frame behind, which is as
    /// close as an immediate mode UI gets: a field cannot report that it has
    /// been clicked until it has been drawn, and whether to draw it is the
    /// question being asked.
    expanded: bool,
}

/// Everything the settings pane sets
///
/// One parameter rather than nine. A system may take only sixteen, and these
/// are all the same thing: what the pane is a pane of.
#[derive(SystemParam)]
pub struct Settings<'w> {
    spyglass: ResMut<'w, Spyglass>,
    view: ResMut<'w, View>,
    color_by: ResMut<'w, ColorBy>,
    population_scale: ResMut<'w, ScalePopulation>,
    show_names: ResMut<'w, ShowNames>,
    throttle: ResMut<'w, Throttle>,
    poll: ResMut<'w, Poll>,
    name_radius: ResMut<'w, NameRadius>,
    show_orbits: ResMut<'w, ShowOrbits>,
    phase: ResMut<'w, Phase>,
    show_body_names: ResMut<'w, ShowBodyNames>,
    show_grid: ResMut<'w, ShowGrid>,
    unit: ResMut<'w, RulerUnit>,
    show_middle: ResMut<'w, ShowMiddle>,
    show_picked: ResMut<'w, ShowPicked>,
    bright: ResMut<'w, Bright>,
    despawner: MessageWriter<'w, Despawn>,
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
pub struct FilterBar<'w, 's> {
    /// The filters themselves, which the rows are drawn from and changed in
    ///
    /// Named for what it holds rather than for its type, since this is
    /// reached through a parameter that is already about filters and
    /// `filter.filters` says the word twice and the thing once.
    active: ResMut<'w, Filters>,
    /// How much of the sky is getting through them
    in_reach: Res<'w, InReach>,
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
}

pub fn chrome(
    mut contexts: EguiContexts,
    mut settings: Settings,
    mut bar: SearchBar,
    mut over_ui: ResMut<PointerOverUi>,
    mut open: ResMut<SettingsOpen>,
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

    // The pane first, since where it has reached is where the gear stands.
    let edge = settings_pane(ctx, open.0, |ui| {
        heading(ui, "Spyglass", false);
        ui.checkbox(&mut settings.spyglass.follow_camera, "Follow Camera");
        ui.add_space(FIELD_GAP);
        ui.label("Radius (Ly)");
        // Shown either way, since the reach is what the bar's count is
        // counting within and a number left off the pane is one the user has
        // nowhere to read. Greyed while the camera sets it: dragging it would
        // be overwritten on the next frame, and a control that springs back is
        // worse than one that says it is not yours to move.
        ui.add_enabled_ui(!settings.spyglass.follow_camera, |ui| {
            radius_slider(ui, &mut settings.spyglass.radius, Spyglass::CEILING);
        });
        // The camera cannot both be told where to stand and be asked where it
        // is standing, so the one that reads the camera hides the one that
        // writes it.
        if !settings.spyglass.follow_camera {
            ui.add_space(FIELD_GAP);
            ui.checkbox(&mut settings.spyglass.lock_camera, "Lock Camera");
        }

        // Folded away under what it is about. Everything in here is the
        // spyglass going about its work rather than a choice about the sky,
        // and it is reached for once in a session if at all, so it is shut
        // until it is asked for.
        ui.add_space(FIELD_GAP);
        ui.collapsing("Advanced", |ui| {
            // Fetching first, the reach being of no use until the systems
            // within it are on the map. Clearing is what to do with them once
            // they are.
            //
            // Named for the halves alone. Standing inside the spyglass's own
            // section, a name saying so again would be saying it twice.
            ui.checkbox(&mut settings.spyglass.fetch, "Fetch");
            if settings.spyglass.fetch {
                // The throttle first, being the wait before asking about
                // somewhere new, which is what moving the camera does and so
                // what the map spends its time doing. The poll is the wait
                // before asking a second time about somewhere it has already
                // been, which only a map being sat still ever reaches.
                ui.horizontal(|ui| {
                    field_name(ui, "Throttle");
                    ui.add(
                        egui::DragValue::new(&mut settings.throttle.0)
                            .suffix(" ms"),
                    );
                });
                ui.horizontal(|ui| poll_value(ui, &mut settings.poll.0));
            }
            ui.add_space(FIELD_GAP);
            ui.checkbox(&mut settings.spyglass.clear, "Clear");
            ui.add_space(FIELD_GAP);
            if ui.button("Despawn Systems").clicked() {
                settings.despawner.write(Despawn);
            }
        });

        // Its own section rather than a row under either view, because it is
        // the one thing on the pane that belongs to both: the same ruled plane
        // carries the map from light years out among the systems to light
        // seconds inside one, and a switch filed under either would read as
        // turning off only that half of it.
        heading(ui, "Scale", true);
        ui.checkbox(&mut settings.show_grid.0, "Grid");
        if settings.show_grid.0 {
            // Indented under what turns them on, the same as the names are,
            // since a unit for a ruler that is not drawn is a choice about
            // nothing. Left to the map by default, which turns the ruler over
            // as it descends into a system; pinned either way for reading a
            // system's distances in light years or a neighbourhood's in light
            // seconds.
            ui.indent("said", |ui| {
                ui.checkbox(
                    &mut settings.show_middle.0,
                    "Show Center Position",
                );
                ui.checkbox(
                    &mut settings.show_picked.0,
                    "Show Selected Positions",
                );
                ui.add_space(FIELD_GAP);
                // How loudly the whole ruling is drawn, lines and numbers
                // together. Past a hundred for a ruler that has to be read off
                // a bright field, under it for one that should stay out of the
                // way of a busy sky.
                ui.label("Brightness (%)");
                let mut bright = settings.bright.0 * 100.;
                fill_width(ui);
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
                ui.label("Units");
                ui.radio_value(
                    &mut *settings.unit,
                    RulerUnit::Automatic,
                    "Automatic",
                );
                ui.radio_value(
                    &mut *settings.unit,
                    RulerUnit::LightYears,
                    "Light Years",
                );
                ui.radio_value(
                    &mut *settings.unit,
                    RulerUnit::LightSeconds,
                    "Light Seconds",
                );
            });
        }

        heading(ui, "Galaxy View", true);
        // Whether a system is named is a choice about what the map draws, the
        // same as which color a star comes out and how large it is, so it
        // stands with those rather than alone.
        ui.checkbox(&mut settings.show_names.0, "Show Labels");
        if settings.show_names.0 {
            // Indented under what turns them on, since neither means anything
            // without it. The rule egui draws down the side of an indent says
            // as much, and says it without a heading standing over nothing
            // whenever the box is unchecked.
            ui.indent("names", |ui| {
                ui.checkbox(
                    &mut settings.name_radius.follow_spyglass,
                    "Names Follow Spyglass",
                );
                if !settings.name_radius.follow_spyglass {
                    // A name can only be drawn for a system that is drawn,
                    // and the spyglass decides that. One that is not clearing
                    // draws everything loaded, and then names may be asked
                    // for beyond its reach.
                    let ceiling = if settings.spyglass.clear {
                        settings.spyglass.radius
                    } else {
                        Spyglass::CEILING
                    };
                    ui.label("Name Radius (Ly)");
                    radius_slider(
                        ui,
                        &mut settings.name_radius.radius,
                        ceiling,
                    );
                }
            });
        }

        ui.add_space(FIELD_GAP);
        ui.radio_value(&mut *settings.view, View::Systems, "Systems");
        ui.radio_value(&mut *settings.view, View::Stars, "Stars");
        if *settings.view == View::Systems {
            ui.add_space(FIELD_GAP);
            ui.label("Color By");
            ui.radio_value(
                &mut *settings.color_by,
                ColorBy::Allegiance,
                "Allegiance",
            );
            ui.radio_value(
                &mut *settings.color_by,
                ColorBy::Government,
                "Government",
            );
            ui.radio_value(
                &mut *settings.color_by,
                ColorBy::Security,
                "Security",
            );
            ui.add_space(FIELD_GAP);
            ui.checkbox(
                &mut settings.population_scale.0,
                "Scale w/ Population",
            );
        }

        // What is drawn once the camera is inside a system, rather than what
        // the galaxy is drawn as. Its own section for that reason, and not
        // under the view above it: which of the two ways the sky is drawn says
        // nothing about what a system looks like from within.
        heading(ui, "System View", true);
        ui.checkbox(&mut settings.show_body_names.0, "Show Labels");
        ui.checkbox(&mut settings.show_orbits.0, "Orbit Lines");
        year_control(ui, &mut settings.phase, &contents);

        // How the filters answer, rather than which they are: the filters
        // themselves are asked for in the bar, and this is the one thing
        // about them that is set once and left alone.
        heading(ui, "Filters", true);
        ui.label("Filtered Opacity (%)");
        let mut showing = filter.dim.0 * 100.;
        fill_width(ui);
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
        // Only on a change, since writing every frame would mark the resource
        // changed every frame and have every dimmed star repainted for
        // nothing.
        if slider.changed() {
            filter.dim.0 = showing / 100.;
        }
        // Which is a filter in the plainer sense: this kind of system and
        // none of the rest. Said because the rows in the bar go on saying
        // how many are getting through, and a sky with nothing faint left in
        // it gives no other sign of why.
        if filter.dim.0 == 0. {
            ui.label(egui::RichText::new("Not drawn at all").weak());
        }
    });

    // The bar next, in the room the gear is not standing in, and the gear
    // last: it stands level with the search box, which is not known until the
    // bar has drawn it.
    let (shut, middle) = main_bar(
        ctx,
        edge + MARGIN + GEAR_ROOM,
        // Whether the search box's answer is late enough to say so. Settled
        // where the clock is, which is the system that put the question; the
        // bar draws during egui's own pass and has no clock of its own.
        bar.pending.waiting(),
        &mut search,
        &mut bar.search,
        &mut bar.note,
        &mut bar.results,
        &mut selection,
        &contents,
        &mut camera,
        orbit.single().map(|camera| camera.center).ok(),
        &mut panels,
        &mut bar.plot,
        &mut filter,
    );
    gear(ctx, edge, middle, &mut open.0);

    // `egui_wants_pointer_input` covers a drag that began on a control and
    // has since been pulled off it, which being over one does not.
    over_ui.0 = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();

    // Whose the press is, now that the UI has drawn and knows what it wanted
    // of it. A press spent shutting the form counts as the UI's even where it
    // landed on the sky, that being what shutting a form by pressing off it
    // means.
    press.settle(&buttons, over_ui.0 || shut);

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

    egui::Area::new(egui::Id::new("settings-pane"))
        // Above the selection window, which is the user's to put where they
        // like. The chrome is not, so it does not go under.
        .order(egui::Order::Foreground)
        // The pane stands off the left of the viewport while it is shut, and
        // egui would otherwise pull it back into view.
        .constrain(false)
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
    let clicked = egui::Area::new(egui::Id::new("settings-gear"))
        .order(egui::Order::Foreground)
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

/// Ask for a system, and for whatever else the user unfolds
///
/// One field at the top of the viewport, since it is the one question the map
/// is asked over and over. Focusing it brings a pane up behind it and drops
/// the rest of the form out below, and a press landing off the form puts it
/// away again.
///
/// It keeps its box while the bar is at rest, so that what stands at the top
/// of the viewport reads as somewhere to type rather than as a word painted
/// on the map.
///
/// The pane is the input's own frame drawn in nothing while the bar is at
/// rest, rather than a frame left out and put back. Nothing shifts as it
/// comes up, because nothing about the layout has changed.
///
/// It stands beside the gear, in the room past `left`, and rides the settings
/// pane's edge as the gear does, so that the whole of the chrome is gathered
/// into one corner. Down the middle it would stand over the sky the spyglass
/// fills, which is drawn about the middle of the viewport and is what the map
/// is for.
///
/// The note is not part of what drops down. It answers the name in the input,
/// and is worth reading whether or not the rest is out.
///
/// Answers whether a press was spent shutting the form, and where the search
/// box came out, that being the height the gear is hung at.
fn main_bar(
    ctx: &Context,
    left: f32,
    asking: bool,
    search: &mut BarFields,
    searched: &mut MessageWriter<Search>,
    note: &mut SearchNote,
    results: &mut SearchResults,
    selection: &mut Selection,
    contents: &Contents,
    camera: &mut MessageWriter<MoveCamera>,
    center: Option<DVec3>,
    panels: &mut Panels,
    plot: &mut Plot,
    filter: &mut FilterBar,
) -> (bool, f32) {
    let style = ctx.global_style();
    let mut frame =
        egui::Frame::popup(&style).inner_margin(egui::Margin::same(PADDING));
    if !search.expanded {
        frame = frame
            .fill(egui::Color32::TRANSPARENT)
            .stroke(egui::Stroke::new(
                frame.stroke.width,
                egui::Color32::TRANSPARENT,
            ))
            .shadow(egui::Shadow::NONE);
    }

    let bar = egui::Area::new(egui::Id::new("main-bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left + MARGIN, MARGIN))
        .show(ctx, |ui| {
            frame
                .show(ui, |ui| {
                    // Fixed, so that the bar keeps its width and its place as
                    // the form drops out of it.
                    ui.set_width(BAR_WIDTH);
                    let mut taken = false;

                    let (response, cleared) = search_box(
                        ui,
                        &mut search.system,
                        note,
                        results,
                        asking,
                    );
                    taken |= response.gained_focus();
                    // Where the gear stands, the two of them being one row.
                    let middle = response.rect.center().y;
                    // Both answer a name, so neither is any answer at all
                    // once that name is being typed over. The mark has
                    // already taken all three where it was the one asked.
                    if response.changed() && !cleared {
                        note.0 = None;
                        results.clear();
                    }
                    // Return and nothing else. Tab moves between the
                    // fields of a form, and a form that went off and asked
                    // the database something on the way past would be
                    // answering a question nobody had finished asking.
                    // The name as a name, since the room around one is not
                    // part of it and a field holding nothing but room is a
                    // field holding nothing. Both reach the database as
                    // letters to match otherwise, and a search for two
                    // spaces answers with every system that has two.
                    if entered(&response, ui)
                        && let Some(name) =
                            typed(&search.system).map(str::to_owned)
                    {
                        searched.write(Search::System { name });
                    }

                    // Both, and in this order: the note answers the query
                    // in the input above it, and the status below says what
                    // is picked out, which after a search that failed is
                    // some other system entirely.
                    if let Some(note) = &note.0 {
                        ui.colored_label(egui::Color32::LIGHT_RED, note);
                    }
                    // Between the two, since it answers the query above it
                    // as the note does, and what is picked out of it shows up
                    // in the status below.
                    let mut travelled = None;
                    let mut described = None;
                    found(
                        ui,
                        results,
                        search.expanded,
                        center,
                        selection,
                        &mut travelled,
                        &mut described,
                    );
                    if let Some(position) = travelled {
                        camera.write(MoveCamera {
                            position: Some(position),
                            framing: None,
                        });
                    }
                    if let Some(system) = described {
                        panels.open_system(system);
                    }
                    let mut went = None;
                    // One count for the whole column rather than one per
                    // kind of row. The rows are the same height and stand one
                    // after another, so letting go of a selection moves every
                    // filter row up into a rectangle a selection row was drawn
                    // in. Numbered apart, the two would put a fresh id at a
                    // rectangle that kept its place, which is what egui reads
                    // as a widget taking another's state.
                    let mut place = 0;
                    let routing = selected(
                        ui,
                        selection,
                        contents,
                        center,
                        &mut went,
                        panels,
                        &mut filter.active,
                        &mut place,
                    );
                    if let Some(position) = went {
                        camera.write(MoveCamera {
                            position: Some(position),
                            framing: None,
                        });
                    }
                    // Drawn whether or not the form is out, as the selection
                    // is and for the same reason. A half lit sky with
                    // nothing on screen to say why is the one thing a filter
                    // must not leave behind.
                    applied(ui, &mut filter.active, panels, &mut place);
                    // Two numbers only where there is a sky behind what is
                    // picked out and the user can see it: something has to be
                    // excluded, and what is excluded has to be drawn.
                    let dimming =
                        filter.active.any_enabled() && filter.dim.0 > 0.;
                    reaching(ui, &filter.in_reach, dimming);

                    // Asking for a route out of the summary line is what opens
                    // the form, in the same pass, so that the section it asked
                    // for is under the control that asked the moment it is
                    // clicked rather than a frame later.
                    search.expanded |= routing;
                    if search.expanded {
                        taken |= filter_section(ui, filter);
                        taken |= route_section(
                            ui, search, selection, searched, plot, routing,
                        );
                    }

                    (taken, middle)
                })
                .inner
        });

    let over = ctx
        .pointer_latest_pos()
        .is_some_and(|at| bar.response.rect.contains(at));
    let dismissed = !over && ctx.input(|i| i.pointer.any_pressed());
    // Only a press that actually shut something is spent, or every press off
    // the bar would be one the map is not free to answer.
    //
    // And only a press of the button the map answers. Any button shuts the
    // form, but the map weighs the primary alone, so a spend charged against
    // some other button is one it never comes to collect: it would sit there
    // and be taken out of the next primary click instead.
    let shut = search.expanded
        && dismissed
        && ctx
            .input(|i| i.pointer.button_pressed(egui::PointerButton::Primary));
    // Two moments, and nothing else: a field in the form takes focus, or a
    // press lands off the form. Moments rather than states, so that neither
    // can undo the other. A field goes on holding focus after the press that
    // shut the form, and asking whether it holds focus would open the form
    // again the very next frame.
    let (took_focus, middle) = bar.inner;
    if took_focus {
        search.expanded = true;
    }
    if dismissed {
        search.expanded = false;
    }
    (shut, middle)
}

/// The whole of the bar's searching
///
/// What was asked, what came back, and whether an answer is late. Gathered as
/// the filters are: the bar is drawn by one system, a system may take sixteen
/// things, and the bar asks about more than sixteen.
#[derive(SystemParam)]
pub struct SearchBar<'w> {
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
}

/// The search box, and the mark that empties it
///
/// The mark stands inside the box at its right hand end, and only while there
/// is something to clear. A search leaves three things behind that answer the
/// name typed into it: the query itself, the note about a name that resolved
/// to nothing, and the list of what it might have meant. All three are the
/// one answer and the mark takes all three, since clearing the query and
/// leaving the list standing under it would leave the answer to a question
/// that is no longer on screen.
///
/// Answers whether the box was cleared, which is a change to what is typed
/// there and reads as one everywhere that watches for it.
fn search_box(
    ui: &mut Ui,
    value: &mut Option<String>,
    note: &mut SearchNote,
    results: &mut SearchResults,
    waiting: bool,
) -> (Response, bool) {
    // Laid out first, since the room it wants is room the field cannot have.
    // In nothing, so the color can be chosen once the pointer has been asked
    // about, which cannot happen until the field has been placed.
    let showing = typed(value).is_some() || !results.is_empty();
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
    let response = singleline(ui, value, "Search", reserved, waiting);

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
    let clearing =
        ui.interact(at, ui.id().with("clear-search"), egui::Sense::click());
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
        cleared(value, note, results);
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
fn gathering(ui: &Ui) -> bool {
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
    system: &DbSystem,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::systems::System>,
) {
    // Only a line for a system with somewhere to be answers at all, so this
    // is the same question asked twice and the second asking has nothing to
    // report.
    let placed = crate::systems::System::try_from(system).ok();
    match action {
        SystemAction::Select { gathering } => {
            if let Some(system) = placed {
                selection.pick(Picked::System(system), gathering);
            }
        }
        SystemAction::Travel => {
            *travelled = crate::systems::system_to_vec(system)
        }
        SystemAction::Describe => *described = placed,
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
/// out in place of the rest, a click with ctrl, command or shift held gathers
/// it up alongside them and lets go of one already held, and a double click
/// sends the camera there. Gathering reaches across searches: the list goes
/// when the next name is typed and what was picked out of it stays, so a set
/// can be built a name at a time.
///
/// A system with no position on record is listed and cannot be picked. Three
/// quarters of the systems on record are in that state, and knowing one exists
/// is worth the line it takes; there is simply nothing to select, since what
/// the map marks is a place.
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
/// `showing` is whether the form is out. The list is put away with it and not
/// let go of: it answers a name the user is in the middle of asking about, and
/// a list standing under a shut form is an answer to a question that is no
/// longer on screen. What was found is kept, so opening the form again is
/// where they left off rather than a search to do a second time.
///
/// Unlike the rows below it, which stand whether or not the form is out. A
/// selection and a filter outlive the asking and go on saying what the map is
/// doing; a list of candidates is the asking itself.
fn found(
    ui: &mut Ui,
    results: &SearchResults,
    showing: bool,
    center: Option<DVec3>,
    selection: &mut Selection,
    travelled: &mut Option<DVec3>,
    described: &mut Option<crate::systems::System>,
) {
    if !showing || results.is_empty() {
        return;
    }

    let Some((system, action)) =
        system_list(ui, results.iter(), center, "result")
    else {
        return;
    };
    act_on(action, system, selection, travelled, described);
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
/// has yet to say. Where it is measured from is said in the same slot as why a
/// system cannot be reached at all, so the column reads down either way.
///
/// `salt` keys one list's lines apart from another's. Within a list they are
/// keyed by place rather than by which system a line is about, as the rows in
/// the bar are keyed and for the reason given there: a fresh search leaves the
/// lines where they were and makes every one of them about something else.
pub(crate) fn system_list<'a>(
    ui: &mut Ui,
    systems: impl Iterator<Item = &'a DbSystem>,
    center: Option<DVec3>,
    salt: &str,
) -> Option<(&'a DbSystem, SystemAction)> {
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
            // Where it is if it can be reached, and why it cannot if not.
            let trailing = match (at, center) {
                (Some(at), Some(center)) => {
                    Some(format!("{:.1} Ly", center.distance(at)))
                }
                (Some(_), None) => None,
                (None, _) => Some("no position".to_owned()),
            };
            let asked = system_line(
                ui,
                &system.name,
                trailing,
                at.is_some(),
                (salt, index),
            );
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
/// The line is the control that sends the camera to what is picked out.
/// Clicking the answer to go to what it names beats a button saying so in
/// words, and the dot in the ring's own color says which mark out on the
/// map is about to be flown to.
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
/// rather than this, as [`found`] does and for the same reason.
///
/// Answers whether a route between what is picked out was asked for, which is
/// [`whole_selection`]'s to say and the caller's to act on: the form that
/// answers it
/// is drawn further down the bar.
fn selected(
    ui: &mut Ui,
    selection: &mut Selection,
    contents: &Contents,
    center: Option<DVec3>,
    travelled: &mut Option<DVec3>,
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

    let routing =
        selection.len() > 1 && whole_selection(ui, selection, filters);

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

            if close.clicked() {
                chose = Some((index, SelectionAction::LetGo));
            } else if info.is_some_and(|info| info.clicked()) {
                chose = Some((index, SelectionAction::Describe));
            } else if row.clicked() {
                chose = Some((index, SelectionAction::Travel));
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
            SelectionAction::Travel => *travelled = selection.position(index),
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
enum SelectionAction {
    /// Send the camera to it, as the one row always did
    Travel,
    /// Open the panel describing it
    Describe,
    /// Let go of this one, and hold the rest
    LetGo,
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
/// Answers whether a route between them was asked for. Only two of them can be
/// routed between, so only two of them carry the control, and it appears the
/// moment the second is picked. That is what there is to find: a set gathered
/// out on the map says here what can be done with it, rather than leaving the
/// user to guess that the form dropping out of the search box has a section
/// about the systems they have already picked.
///
/// It reaches the form rather than plotting, since a route still wants a jump
/// range and there is nowhere here to say one.
fn whole_selection(
    ui: &mut Ui,
    selection: &Selection,
    filters: &mut Filters,
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
        if ends_of(selection).is_ok() {
            routing = ui.button("Route").clicked();
        }
    });
    routing
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

/// The two systems a route runs between, or why it has no pair to run between
///
/// What is picked out on the map, in the order it was picked, rather than
/// names typed into fields of the form. Picking a system out is already how
/// the user says which one they mean, to the panel that describes it and to
/// the filter built from it, so a route with fields of its own would be
/// asking twice about systems the map is holding for them.
///
/// Two of them. A longer set is a route running through every system in it,
/// leg by leg, and that is a different plot from this one rather than this one
/// done several times.
// TODO: Plot through the whole selection, leg by leg in the order it was
// picked, instead of refusing every set but the pair.
fn ends_of(selection: &Selection) -> Result<(&str, &str), &'static str> {
    // The systems alone. A route runs between places, and a body picked out
    // beside them is a thing inside one rather than an end to plot to.
    let mut systems = selection.systems();
    match (systems.next(), systems.next(), systems.count()) {
        (Some(start), Some(end), 0) => Ok((start.name(), end.name())),
        (None, _, _) => Err("Pick out two systems to plot between"),
        (Some(_), None, _) => Err("Pick out a second system to plot to"),
        _ => Err("A route runs between two systems for now"),
    }
}

/// How far apart the two systems a route would run between are
///
/// In a straight line, which is as short as a route between them could be and
/// is knowable before one is asked for. What comes back is longer: a route is
/// flown in jumps, and each of them lands on a system rather than on a point
/// along the line.
///
/// Only over a pair, since a pair is what [`ends_of`] answers with. A distance
/// standing under a line saying there is no route to plot yet would be
/// answering a question the form has just said it cannot take.
fn apart(selection: &Selection) -> Option<f64> {
    // The two the route runs between, which are the two systems picked out.
    let places: Vec<_> = selection.systems().collect();
    let [start, end] = places[..] else { return None };

    Some(start.position().distance(end.position()))
}

/// Ask what a route may be flown in, and say where it would run
///
/// Answers whether its field has just taken focus. Which systems a route runs
/// between is what is picked out on the map, which [`ends_of`] settles, so the
/// only thing left to ask is the jump range.
///
/// How it is getting on is said between the field and the button, where
/// what it is about is on either side of it. The note under the search input
/// answers a name typed into the search input, and a route's answer read out
/// up there would sit a long way from its question.
///
/// `asked_for` is the control up in the selection's summary line having just
/// been pressed, which is how a user who has picked two systems out on the map
/// reaches this. The range takes the caret, that being the one thing left to
/// say, so the gesture reads as one move rather than as a section appearing
/// somewhere for the user to go and find.
fn route_section(
    ui: &mut Ui,
    search: &mut BarFields,
    selection: &Selection,
    searched: &mut MessageWriter<Search>,
    plot: &mut Plot,
    asked_for: bool,
) -> bool {
    heading(ui, "Route", true);
    let mut taken = false;

    // Which two systems it runs between. Said rather than asked for, since
    // what answers it is a gesture out on the map, and a form with nothing on
    // it about the ends is a form that plots between systems it never names.
    let ends = ends_of(selection);
    match ends {
        Ok((start, end)) => ui.label(format!("{start} -> {end}")),
        // Weakly. Nothing has gone wrong: the user is part way through
        // asking, and a form in red before it has been filled in is a form
        // scolding whoever fills it in.
        Err(why) => ui.label(egui::RichText::new(why).weak()),
    };
    // And how far apart they are, which is the one thing about the plot the
    // map can say before it is asked for. Under the names rather than at the
    // end of them, since two long names and a number on one line wrap into a
    // paragraph in a bar this wide.
    if let Some(away) = apart(selection) {
        ui.label(egui::RichText::new(format!("{away:.1} Ly apart")).weak());
    }
    ui.add_space(FIELD_GAP);

    // The range is typed rather than looked up, so it never waits on
    // anything.
    let range =
        singleline(ui, &mut search.route_range, "Jump Range (Ly)", 0., false);
    if asked_for {
        range.request_focus();
    }
    taken |= range.gained_focus();
    // Return in the range asks for the route, as pressing the button does. It
    // is the one thing a route waits on, and a form with one thing left to do
    // should not have to be reached for.
    let submitted = entered(&range, ui);
    // What came back of the last route asked for answers the field as it was
    // then, so it goes as soon as it is not. Work still under way is not an
    // answer to anything yet, and stays.
    if range.changed() && matches!(*plot, Plot::Failed(_)) {
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

    ui.add_space(FIELD_GAP);
    // The two things a route is made of: which systems it runs between, and
    // what it may be flown in. The button is dead until both are in hand,
    // since a plot missing one of them is nothing to ask the database about.
    let asked = ends.ok().zip(typed(&search.route_range));
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

    if (button.response.clicked() || submitted)
        && let Some(((start, end), range)) = asked
    {
        *plot = match jump_range(range) {
            Ok(range) => {
                searched.write(Search::Route {
                    start: start.to_owned(),
                    end: end.to_owned(),
                    // Back to text, since a route is fetched under a key
                    // made of what was asked for and a float is no kind of
                    // key.
                    range: range.to_string(),
                });
                Plot::Working
            }
            Err(trouble) => Plot::Failed(trouble.to_owned()),
        };
    }

    taken
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
) {
    if filters.is_empty() {
        return;
    }

    // Settled after the sections are drawn, since the rows are drawn from the
    // same filters they change.
    let mut toggling = None;
    let mut removing = None;
    let mut opening = None;
    let mut whole = None;

    for section in Section::ALL {
        let rows = section.rows(filters);
        if rows.is_empty() {
            continue;
        }

        // Above the rows it stands for, where a heading stands, and over two
        // or more of them: one row already says everything a count of one
        // could, and the control over it would do what that row's own does.
        if rows.len() > 1
            && let Some(asked) = whole_set(
                ui,
                &section.said(rows.len()),
                section.on(filters),
                place,
            )
        {
            whole = Some((asked, rows.clone()));
        }

        section_rows(
            ui,
            filters,
            &rows,
            place,
            &mut toggling,
            &mut removing,
            &mut opening,
        );
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
        Some((FilterAction::Toggle, rows)) => filters.toggle_all(&rows),
        Some((FilterAction::LetGo, rows)) => filters.clear(&rows),
        None => {}
    }
}

/// Which group of the bar's filter rows a filter stands in
///
/// Routes apart from the rest. A route is a line drawn across the map between
/// two systems the user named, and a faction or a hand-picked set is a way of
/// reading the sky it is drawn over. They are worth different questions: how
/// many routes am I comparing, and how much of the sky am I picking out.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Section {
    /// Factions and hand-picked sets, which pick the sky out
    Filters,
    /// Routes, which are drawn over it
    Routes,
}

impl Section {
    /// Every section, in the order the bar draws them
    ///
    /// The sky before what is drawn over it, which is the order the map is
    /// built up in and the order the two read in.
    const ALL: [Section; 2] = [Section::Filters, Section::Routes];

    /// Whether this section holds `filter`
    fn holds(&self, filter: &Filter) -> bool {
        filter.is_route() == (*self == Section::Routes)
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

    /// What a row standing over `count` of them says
    fn said(&self, count: usize) -> String {
        match self {
            Section::Filters => format!("{count} filters"),
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

        let buttons = lay_out_buttons(ui);

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

        if close.clicked() {
            *removing = Some(index);
        } else if info.is_some_and(|info| info.clicked()) {
            *opening = Some(active.filter.clone());
        } else if row.clicked() {
            *toggling = Some(index);
        }
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
/// Said in as few words as it can be. The bar is [`BAR_WIDTH`] wide and the
/// numbers are what grow: the sky runs to millions of systems, and a line
/// that has to wrap to hold two of them is a line that moves the rows under
/// it about as the user flies.
fn reaching(ui: &mut Ui, in_reach: &InReach, dimming: bool) {
    let InReach { admitted, total } = *in_reach;
    if total == 0 {
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
    ui.label(egui::RichText::new(said).weak());
}

/// What the bar can be asked to do with the filters as a set
enum FilterAction {
    /// Turn every filter off, or every one back on
    Toggle,
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
    place: &mut usize,
) -> Option<FilterAction> {
    let gap = ui.spacing().item_spacing.x;
    let buttons = lay_out_close(ui);

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

    let Buttons { close, .. } = place_buttons(ui, rect, buttons, of);

    let asked = if close.clicked() {
        Some(FilterAction::LetGo)
    } else if row.clicked() {
        Some(FilterAction::Toggle)
    } else {
        None
    };
    row.on_hover_cursor(egui::CursorIcon::PointingHand);
    asked
}

/// Ask for a filter by naming a faction
///
/// Answers whether its field has just taken focus. Above the route's section
/// because what it adds shows up above it, in the rows under the search box,
/// and a control should sit near what it does.
///
/// The field empties once a faction has been asked for. What was typed is a
/// row by then, and the field's next job is the next filter.
///
/// What went wrong is said under the field that went wrong, as a route's
/// trouble is said between its fields and its button. The note under the
/// search input answers a name typed into the search input, and a faction
/// read out up there would sit a long way from its question.
fn filter_section(ui: &mut Ui, filter: &mut FilterBar) -> bool {
    heading(ui, "Filters", true);

    let response = singleline(
        ui,
        &mut filter.input,
        "Faction Name",
        0.,
        filter.pending.waiting(),
    );
    // Both answer a name, so neither is any answer at all once that name is
    // being typed over.
    if response.changed() {
        *filter.note = LookupNote::Nothing;
        filter.found.clear();
    }
    if entered(&response, ui)
        && let Some(name) = typed(&filter.input).map(str::to_owned)
    {
        filter.lookup.write(Lookup::Faction { name });
    }

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
        filter.active.add(Filter::Faction {
            id: faction.id,
            name: faction.name.clone(),
        });
        *filter.input = None;
        filter.found.clear();
    }

    watch_control(ui, filter);

    response.gained_focus()
}

/// Move a system through one turn of its slowest body
///
/// The rail ends at exactly that turn, so every body comes round at least once
/// along it. Nothing shorter is true of all of them: the slowest body of a system
/// takes a median 993 times as long to come round as its fastest, and in Sol it
/// is four million times, Persephone against Enceladus.
///
/// Which is why the rail is logarithmic. Laid out evenly in time, the whole of
/// the inner system would live in the first pixel and every drag past it would
/// throw the inner bodies through thousands of turns. Logarithmic, a pixel is
/// minutes at one end and centuries at the other, and every body has a stretch
/// of rail where it moves a step at a time.
///
/// What sets the span is said, and named. Fifteen thousand years is a strange
/// span for a system read as nine planets, and knowing it belongs to one far
/// flung object is the difference between a control that looks broken and one
/// that is telling the truth.
///
/// Nothing to set where nothing goes round anything, which is a lone star. The
/// rail is drawn all the same and does nothing, a control that comes and goes
/// being harder to find than one that is plainly not offered.
fn year_control(ui: &mut Ui, phase: &mut Phase, contents: &Contents) {
    let orbits = contents.orbits();
    let turn = orbits.slowest();

    ui.add_space(FIELD_GAP);
    ui.label("Round The Slowest Orbit");

    let year = turn.map_or(1., |(_, year)| year);
    let mut elapsed = phase.0 * year;
    fill_width(ui);
    let moved = ui
        .add_enabled_ui(turn.is_some(), |ui| {
            ui.add(
                egui::Slider::new(&mut elapsed, 0.0..=year)
                    .logarithmic(true)
                    .smallest_positive(60.)
                    .custom_formatter(|secs, _| lasting(secs as f32)),
            )
        })
        .inner;

    // Only on a change. Writing every frame would mark the phase changed every
    // frame and have every body in the system put back where it already stands.
    if moved.changed() {
        phase.0 = elapsed / year;
    }

    ui.label(
        egui::RichText::new(match turn {
            Some((id, year)) => format!(
                "One turn of {} is {}",
                named_body(contents, id),
                lasting(year as f32),
            ),
            None => "Nothing here goes round anything".to_owned(),
        })
        .weak(),
    );
}

/// What the thing with `id` inside the held system is called
///
/// A barycenter has no name of its own, and is what a close pair of stars goes
/// round, so it is said as what it is rather than left blank.
fn named_body(contents: &Contents, id: i16) -> String {
    if let Some(body) = contents.body(id) {
        return body.name.clone();
    }
    if let Some(star) = contents.star(id) {
        return star.name.clone();
    }
    "a shared centre".to_owned()
}

/// Ask for a filter by how lately a system was heard from
///
/// A rail over named spans rather than a field to type a time into. What is
/// being asked is roughly how fresh, and the spans are the answers anybody
/// wants: the far end of a typed time is a database going back years and the
/// near end is the last minute.
///
/// Only on a change, as the opacity beside it is. The filter carries the moment
/// the span worked out to be, so writing it every frame would move that moment
/// every frame and put a fresh question to the database each time.
fn watch_control(ui: &mut Ui, filter: &mut FilterBar) {
    ui.add_space(FIELD_GAP);
    ui.label("Heard From Within");

    let mut standing = filter.watch.0;
    fill_width(ui);
    let rail = ui.add(
        egui::Slider::new(&mut standing, 0..=SPANS.len() - 1)
            .show_value(false)
            .custom_formatter(|at, _| SPANS[at as usize].0.to_owned()),
    );

    if rail.changed() {
        filter.watch.0 = standing;
        match filter.watch.span() {
            // Worked out here and not where it is asked. `Utc::now` is the one
            // thing in this that cannot be tested, so it is read at the one
            // place that turns a span into a moment.
            Some(span) => filter.active.ask_since(Utc::now() - span),
            None => filter.active.ask_nothing_of_time(),
        }
    }

    // The span is what was asked for and the row says the moment it came to,
    // which stop agreeing as soon as the clock moves on. Said here so the two
    // readings are side by side rather than looking like a disagreement.
    ui.label(
        egui::RichText::new(match filter.watch.span() {
            Some(_) => "Everything since, and whatever arrives",
            None => "Not asked",
        })
        .weak(),
    );
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
/// `reachable` is whether the system is one the map can place. A line that is
/// not answers nothing and carries no mark: there is nowhere to send the
/// camera and nothing for a panel to describe, and `trailing` is where the
/// line says as much.
///
/// `salt` keys the mark apart from the marks on the lines around it. The
/// caller chooses it, knowing what its own list does between one pass and the
/// next.
pub(crate) fn system_line(
    ui: &mut Ui,
    name: &str,
    trailing: Option<String>,
    reachable: bool,
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

    // A panel is about a system the map can place, so a system with nowhere
    // to be is not offered one. Nothing is left standing in its place: the
    // line already says why, in the slot the mark would sit beside.
    let mark = reachable.then(|| {
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
    });

    let reserved = mark.as_ref().map_or(0., |mark| mark.size().x + gap)
        + trailing.as_ref().map_or(0., |text| text.size().x + gap);
    let (rect, answer) =
        line(ui, egui::RichText::new(name), reserved, reachable);
    let middle = rect.center().y;

    // Asked about after the line, so that it is the one answering where the
    // two overlap. Under it the line would have to work out what it was not
    // being clicked on.
    let describing = mark.map(|mark| {
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
    });

    // Between the name and the mark, right against whichever of them ends the
    // line, so the distances line up down the list rather than following the
    // names.
    if let Some(text) = trailing {
        let size = text.size();
        let right = describing
            .as_ref()
            .map_or(rect.right() - LINE_PADDING, |(at, _)| at.left() - gap);
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
    if describing.is_some_and(|(_, mark)| mark.clicked()) {
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
pub(crate) fn scrolling<R>(
    ui: &mut Ui,
    height: f32,
    salt: impl std::hash::Hash,
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    // In a scope of its own, since a style set on a `Ui` is set on the rest
    // of that `Ui`, and this is asked for by the list rather than by whatever
    // follows it.
    ui.scope(|ui| {
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

/// Name the control beside it, as plainly as a checkbox names itself
///
/// Egui paints a label in the color it keeps for what cannot be interacted
/// with, a shade under the text it puts on a checkbox. That is the right
/// answer for a caption and the wrong one for the name of the box next to it,
/// which stands in a column of checkboxes and is no lesser thing than any of
/// them. Reading the color off the style rather than naming one keeps it
/// with them through whatever theme is set.
fn field_name(ui: &mut Ui, name: &str) {
    let named = ui.visuals().widgets.inactive.fg_stroke.color;
    ui.label(egui::RichText::new(name).color(named));
}

fn poll_value(ui: &mut Ui, opt: &mut Option<f64>) {
    let mut enabled = opt.is_some();
    if ui.checkbox(&mut enabled, "Poll").changed() {
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

    /// A results list holding `names`, the last of them nowhere in particular
    fn results(names: &[&str], placed: bool) -> SearchResults {
        let mut found: Vec<_> = names.iter().map(|name| row(name)).collect();
        if !placed && let Some(last) = found.last_mut() {
            last.position = None;
        }
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
                true,
                center,
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

    /// A system with nowhere to be says so, in the slot the distance uses
    ///
    /// It is listed rather than dropped, since knowing the name is on record
    /// is most of what was asked, and the line says why it cannot be had.
    #[test]
    fn a_system_with_nowhere_to_be_says_so() {
        let said = listed(&results(&["SOL", "NOWHERE"], false), None);

        assert!(said.contains(&"NOWHERE".to_owned()), "{said:?}");
        assert!(said.contains(&"no position".to_owned()), "{said:?}");
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

    /// A shut form puts the list away without letting go of it
    ///
    /// It answers a name the user is in the middle of asking about, and a
    /// list standing under a shut form is an answer to a question that is no
    /// longer on screen. What was found is kept, so opening the form again is
    /// where they left off rather than a search to do a second time.
    #[test]
    fn a_shut_form_puts_the_list_away_without_clearing_it() {
        let held = results(&["SOL", "SOLATI"], true);

        let out = listed(&held, None);
        let away = words(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &held,
                false,
                None,
                &mut selection,
                &mut travelled,
                &mut described,
            );
        });

        assert!(out.iter().any(|line| line == "SOL"), "{out:?}");
        assert!(away.is_empty(), "{away:?}");
        assert_eq!(held.iter().count(), 2, "the list let go of what it found");
    }

    /// A faction the search found, by id, called after it
    fn faction_row(id: i32, name: &str) -> DbFaction {
        DbFaction { id, name: name.to_owned() }
    }

    /// The lists drawn at once do not take each other's scroll area
    ///
    /// Egui works a scroll area's id out from where it sits unless it is
    /// told, and says so in red over the map when two land on the same one.
    /// The bar draws two at once: what a search found, and the factions a
    /// name typed into the filter's field might mean.
    #[test]
    fn the_lists_do_not_share_a_scroll_area() {
        let systems = results(&["SOL", "SOLATI"], true);
        let factions = [faction_row(1, "The Dukes of Mikunn")];

        let said = crate::tests::complaints(|ui| {
            let mut selection = Selection::default();
            let mut travelled = None;
            let mut described = None;
            found(
                ui,
                &systems,
                true,
                None,
                &mut selection,
                &mut travelled,
                &mut described,
            );
            faction_list(ui, factions.iter());
        });

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

    /// A system with nowhere to be is offered no panel
    ///
    /// A panel is about a system the map can place, so there is nothing for
    /// the mark to open and it is not stood there to be tried.
    #[test]
    fn a_system_with_nowhere_to_be_carries_no_mark() {
        let said = listed(&results(&["SOL", "NOWHERE"], false), None);

        assert_eq!(said.iter().filter(|line| *line == INFO).count(), 1);
    }

    /// A click on the line for `name`, with or without the modifier held
    ///
    /// The camera and the panel are not what these are about, so what a line
    /// asks of them is taken and dropped.
    fn clicked(gathering: bool, name: &str, selection: &mut Selection) {
        picked(gathering, &row(name), selection);
    }

    /// A click on the line for `system`, whatever the row says about it
    fn picked(gathering: bool, system: &DbSystem, selection: &mut Selection) {
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
        assert_eq!(selection.name(0), Some("SOLATI"));
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

        assert_eq!(selection.name(0), Some("SOL"));
        assert_eq!(selection.name(1), Some("SOLATI"));
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
        assert_eq!(selection.name(0), Some("SOLATI"));
    }

    /// Gathering a system with nowhere to be holds what was already held
    ///
    /// There is nothing to select, what the map marks being a place, and the
    /// set that was gathered is no reason to take apart over it.
    #[test]
    fn a_gathered_click_on_a_system_with_nowhere_to_be_holds_the_rest() {
        let mut selection = Selection::default();
        let mut nowhere = row("NOWHERE");
        nowhere.position = None;

        clicked(false, "SOL", &mut selection);
        picked(true, &nowhere, &mut selection);

        assert_eq!(selection.len(), 1);
        assert_eq!(selection.name(0), Some("SOL"));
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
        assert!(ends_of(&selection).is_err());
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

    /// Any other number is not, there being no route it could ask for
    ///
    /// A control that leads to a form refusing what it just asked for is
    /// worse than no control: it says the map can do something it cannot.
    #[test]
    fn a_set_that_cannot_be_routed_is_offered_no_route() {
        let alone = selection_said(&["SOL"]);
        let several = selection_said(&["SOL", "BARNARD", "WOLF 359"]);

        assert!(!alone.contains(&"Route".to_owned()), "{alone:?}");
        assert!(!several.contains(&"Route".to_owned()), "{several:?}");
        // The rest of the line stands, so it is the route alone that goes.
        assert!(several.contains(&"Filter".to_owned()), "{several:?}");
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

    /// The two ends of a route are said to be as far apart as they are
    ///
    /// The one thing about the plot that can be said before it is asked for,
    /// and what says whether a ship could make the trip at all.
    #[test]
    fn two_systems_are_as_far_apart_as_they_stand() {
        assert_eq!(apart(&strung_out(&[3., 15.])), Some(12.));
    }

    /// Any other number is not measured at all
    ///
    /// One is not a pair, and more than two is a route the form refuses. A
    /// distance under a line saying so would answer a question the form has
    /// just said it cannot take.
    #[test]
    fn a_set_that_cannot_be_routed_is_not_measured() {
        assert_eq!(apart(&strung_out(&[])), None);
        assert_eq!(apart(&strung_out(&[3.])), None);
        assert_eq!(apart(&strung_out(&[3., 15., 20.])), None);
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
                true,
                None,
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
            let mut note = SearchNote(None);
            let mut offers = results(&["SOL", "SOLATI", "SOLLARO"], true);
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

            search_box(ui, &mut query, &mut note, &mut offers, false);
            found(
                ui,
                &offers,
                true,
                None,
                &mut selection,
                &mut travelled,
                &mut described,
            );
            selected(
                ui,
                &mut selection,
                &Contents::default(),
                None,
                &mut travelled,
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
            let mut note = SearchNote(None);
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

            search_box(ui, &mut query, &mut note, &mut offers, false);
            found(
                ui,
                &offers,
                true,
                None,
                &mut held,
                &mut travelled,
                &mut described,
            );
            selected(
                ui,
                &mut held,
                &Contents::default(),
                None,
                &mut travelled,
                &mut panels,
                &mut applied_to,
                &mut place,
            );
            applied(ui, &mut applied_to, &mut panels, &mut place);
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

    /// Nor does letting go of the selection hand its rows to the filters
    ///
    /// The two kinds of row are drawn one after the other in the one column,
    /// so a filter row moves up into a rectangle a selection row was in.
    #[test]
    fn letting_go_of_the_selection_does_not_change_the_filter_row_ids() {
        let said = crate::tests::between_passes(
            draw_bar(&[], &["SOL", "BARNARD"], 2),
            draw_bar(&[], &[], 2),
        );

        assert!(said.is_empty(), "{said:?}");
    }

    /// Gathering past what the bar holds hands no place to the filters
    ///
    /// The selection's rows scroll once there are more than [`SELECTED`] of
    /// them, so from there the column stops growing however many are picked
    /// out. The filter rows below keep the rectangles they had, and a count
    /// that went on rising would put a fresh id at a rectangle that never
    /// moved.
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
    /// column, so dropping a selection row moves every filter row up by
    /// exactly one row: each lands in a rectangle a selection row was drawn
    /// in. The summary line stays put through this, more than one system
    /// being held either way, so nothing else takes up the slack.
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
    /// Not over one, where it would be a second control for what the row
    /// beneath it already does.
    #[test]
    fn the_set_is_summed_up_only_over_more_than_one() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Empire".into() });
        let mut panels = Panels::default();
        let alone = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

        filters.add(Filter::Faction { id: 2, name: "Federation".into() });
        let both = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

        assert_eq!(filters.len(), 1);
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
    #[test]
    fn a_route_row_says_how_many_jumps_it_is() {
        let mut filters = Filters::default();
        filters.add(Filter::Route {
            label: "A -> B".to_owned(),
            systems: vec![1, 2, 3, 4, 5],
            range: "10".to_owned(),
        });
        let mut panels = Panels::default();

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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
        });
        let mut panels = Panels::default();

        let said = words(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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

    /// What the search box says, holding `query` against `results`
    fn box_said(query: Option<&str>, results: &[&str]) -> Vec<String> {
        words(|ui| {
            let mut value = query.map(str::to_owned);
            let mut note = SearchNote::default();
            let mut results = results_of(results);
            search_box(ui, &mut value, &mut note, &mut results, false);
        })
    }

    /// A results list holding `names`, all of them placed
    fn results_of(names: &[&str]) -> SearchResults {
        results(names, true)
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
                true,
                Some(DVec3::ZERO),
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
                true,
                None,
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

    /// A radius is held inside what it is offered
    ///
    /// The ceiling moves: names reach no further than the spyglass does, so
    /// one set wide and then hemmed in has to come back to what is on offer
    /// rather than go on saying a distance the map will not draw to.
    #[test]
    fn a_radius_is_held_within_what_is_offered() {
        assert_eq!(drawn_radius(5e4, 100.), 100.);
        assert_eq!(drawn_radius(0.01, 100.), Spyglass::FLOOR);
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

    /// And inside the galaxy, whatever ceiling it is handed
    ///
    /// The one the names are given is the spyglass's own radius, which is a
    /// number the user may type.
    #[test]
    fn a_radius_reaches_no_further_than_the_galaxy() {
        assert_eq!(drawn_radius(5e6, 5e6), Spyglass::CEILING);
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
        });
        let mut panels = Panels::default();

        let said =
            complaints(|ui| applied(ui, &mut filters, &mut panels, &mut 0));

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
            reaching(ui, &InReach { admitted: 324, total: 324 }, false)
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
            reaching(ui, &InReach { admitted: 8, total: 324 }, true)
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
            reaching(ui, &InReach { admitted: 8, total: 324 }, false)
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
        let said =
            words(|ui| reaching(ui, &InReach { admitted: 0, total: 0 }, false));

        assert!(!said.iter().any(|line| line.contains("spyglass")), "{said:?}");
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

        painted(|ui| applied(ui, &mut filters, &mut panels, &mut 0));
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

    /// A route runs between the two systems picked out on the map
    #[test]
    fn a_route_runs_between_what_is_picked_out() {
        assert_eq!(
            ends_of(&holding(&["SOL", "SOLATI"])),
            Ok(("SOL", "SOLATI"))
        );
    }

    /// In the order they were picked, the first of them being where it starts
    ///
    /// The two are told apart by nothing but that order, so a set read out in
    /// any other one plots the route backwards half the time.
    #[test]
    fn the_first_picked_is_where_a_route_starts() {
        assert_eq!(
            ends_of(&holding(&["SOLATI", "SOL"])),
            Ok(("SOLATI", "SOL"))
        );
    }

    /// With nothing picked out there are no ends to run between
    #[test]
    fn a_route_with_nothing_picked_out_has_no_ends() {
        assert!(ends_of(&Selection::default()).is_err());
    }

    /// Nor with one, which is an end and no route
    #[test]
    fn a_route_out_of_one_system_is_refused() {
        assert!(ends_of(&holding(&["SOL"])).is_err());
    }

    /// A longer set is a route through all of it, which is not the plot on
    /// offer
    ///
    /// Refused rather than answered with the first two. Plotting between two
    /// of several picked out would draw a line the user did not ask for and
    /// say nothing about the rest.
    #[test]
    fn a_route_out_of_a_longer_set_is_refused() {
        assert!(ends_of(&holding(&["SOL", "SOLATI", "SOLLARO"])).is_err());
    }

    /// Each of those says something of its own
    ///
    /// They are read out of the one line, so two of them saying the same
    /// thing would be a form that answers having picked nothing, having picked
    /// one, and having picked too many all alike.
    #[test]
    fn the_reasons_are_told_apart() {
        let (none, one, several) = (
            Selection::default(),
            holding(&["SOL"]),
            holding(&["SOL", "SOLATI", "SOLLARO"]),
        );
        let said = [ends_of(&none), ends_of(&one), ends_of(&several)];

        for (at, one) in said.iter().enumerate() {
            for other in &said[at + 1..] {
                assert_ne!(one, other);
            }
        }
    }
}
