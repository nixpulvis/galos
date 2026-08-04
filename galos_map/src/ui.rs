//! The chrome standing between the user and the map
//!
//! A bar centred at the top, a gear in the corner, and the settings pane that
//! gear slides out from the left edge. What is known about the system the user
//! picked out is drawn by [`crate::systems::selection`], which owns the fields
//! it reads.
//!
//! The bar leads with a search box, which is what it is asked for most, but it
//! is not a search bar: three sections drop out of it, and search is one of
//! them. Filters are another and have nothing to say to the other two, so they
//! keep their own state in [`FilterBar`] and are reached through that alone. A
//! route is the third, and does read what the search box holds, since a route
//! starts from the system named up there.

use crate::camera::{MoveCamera, OrbitCamera};
use crate::search::{Plot, SearchNote, Searched};
use crate::systems::despawn::Despawn;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::filter::{Asked, Filters, Wanted};
use crate::systems::info::Panels;
use crate::systems::labels::NameRadius;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::selection::{SELECTION, Selection};
use crate::systems::spawn::{ColorBy, ShowNames};
use crate::systems::{InReach, Spyglass};
use bevy::ecs::system::SystemParam;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui::{Context, Response, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.init_resource::<SettingsOpen>();
    app.init_resource::<PressAnswered>();
    app.add_systems(EguiPrimaryContextPass, chrome);
}

/// Whether the pointer is busy with the UI
///
/// The camera and the UI both want the same drags, and only the UI knows
/// which ones are its own. It answers here rather than the camera guessing
/// from window rectangles it would have to be told about.
///
/// Egui lays out during its own pass, so this is what the last frame's
/// layout concluded. A press landing on a control the same frame it appears
/// therefore reaches the map as well, which no control the map has is
/// arranged to do.
#[derive(Resource, Default)]
pub struct PointerOverUi(pub bool);

/// Whether the settings pane is out
///
/// A resource rather than a local because the pane is drawn before the gear
/// that toggles it, so that the gear knows how far in the pane has come and
/// can stand clear of it.
#[derive(Resource, Default)]
pub struct SettingsOpen(bool);

/// Whether the press under way has already been answered by the UI
///
/// Shutting the bar's form is done by pressing somewhere off it, and the
/// map has no business answering that press as well: letting go of a
/// selection because the press that closed a form happened to land on empty
/// sky is one gesture doing two things.
///
/// Set when the form is shut and held until the button comes up, since the
/// map weighs a click on its release and the form weighs it on its press.
#[derive(Resource, Default)]
pub struct PressAnswered(pub bool);

// TODO: Form validation.

/// How wide the settings pane stands when it is out
const PANE_WIDTH: f32 = 240.;

/// How wide the bar stands, unfolded or not
const BAR_WIDTH: f32 = 220.;

/// How tall the gear is drawn
const GEAR_SIZE: f32 = 18.;

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

/// The colour that dot is drawn in
///
/// [`SELECTION`] in egui's terms, so that the status line under the search
/// box and the ring out on the map are one mark in two places rather than
/// two colours to be matched up.
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
/// The search box and the route's two fields. Together because the route
/// reads what the search box holds, not merely because they are drawn near
/// each other: the filters are drawn between them and keep their own field in
/// [`FilterBar`], having nothing to say to either.
#[derive(Default)]
pub struct BarFields {
    /// The system named in the box the bar leads with
    ///
    /// Read by the route as well as by the search, since a route starts from
    /// whatever system is named up there. That is the one thing two of the
    /// bar's sections share, and it is shared on purpose.
    system: Option<String>,
    route_end: Option<String>,
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
/// are all the same thing: the knobs the pane is a pane of.
#[derive(SystemParam)]
pub struct Knobs<'w> {
    spyglass: ResMut<'w, Spyglass>,
    view: ResMut<'w, View>,
    color_by: ResMut<'w, ColorBy>,
    population_scale: ResMut<'w, ScalePopulation>,
    show_names: ResMut<'w, ShowNames>,
    throttle: ResMut<'w, Throttle>,
    poll: ResMut<'w, Poll>,
    name_radius: ResMut<'w, NameRadius>,
    despawner: MessageWriter<'w, Despawn>,
}

/// The whole of the bar's filter section
///
/// One parameter for the same reason [`Knobs`] is one: a system may take only
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
    field: Local<'s, Option<String>>,
    /// Where a filter the user has typed is sent to be looked up
    wanted: MessageWriter<'w, Wanted>,
    /// What became of the last one asked for
    asked: ResMut<'w, Asked>,
}

pub fn chrome(
    mut contexts: EguiContexts,
    mut knobs: Knobs,
    mut searched: MessageWriter<Searched>,
    mut search_note: ResMut<SearchNote>,
    mut over_ui: ResMut<PointerOverUi>,
    mut settings: ResMut<SettingsOpen>,
    mut search: Local<BarFields>,
    mut selection: ResMut<Selection>,
    mut camera: MessageWriter<MoveCamera>,
    orbit: Query<&OrbitCamera>,
    mut press: ResMut<PressAnswered>,
    mut panels: ResMut<Panels>,
    mut plot: ResMut<Plot>,
    mut filter: FilterBar,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // The pane first, since where it has reached is where the gear stands.
    let edge = settings_pane(ctx, settings.0, |ui| {
        heading(ui, "Spyglass", false);
        ui.label("Radius (Ly)");
        radius_slider(ui, &mut knobs.spyglass.radius, Spyglass::CEILING);
        ui.add_space(FIELD_GAP);
        ui.checkbox(&mut knobs.spyglass.lock_camera, "Lock Camera");
        ui.checkbox(&mut knobs.spyglass.disabled, "Override Spyglass");

        heading(ui, "View", true);
        // Whether a system is named is a choice about what the map draws, the
        // same as which colour a star comes out and how large it is, so it
        // stands with those rather than alone.
        ui.checkbox(&mut knobs.show_names.0, "Show System Names");
        if knobs.show_names.0 {
            // Indented under what turns them on, since neither means anything
            // without it. The rule egui draws down the side of an indent says
            // as much, and says it without a heading standing over nothing
            // whenever the box is unchecked.
            ui.indent("names", |ui| {
                ui.checkbox(
                    &mut knobs.name_radius.follow_spyglass,
                    "Names Follow Spyglass",
                );
                if !knobs.name_radius.follow_spyglass {
                    // A name can only be drawn for a system that is drawn,
                    // and the spyglass decides that. Overriding it draws
                    // everything loaded, and then names may be asked for
                    // beyond its reach.
                    let ceiling = if knobs.spyglass.disabled {
                        Spyglass::CEILING
                    } else {
                        knobs.spyglass.radius
                    };
                    ui.label("Name Radius (Ly)");
                    radius_slider(ui, &mut knobs.name_radius.radius, ceiling);
                }
            });
        }

        ui.add_space(FIELD_GAP);
        ui.radio_value(&mut *knobs.view, View::Systems, "Systems");
        ui.radio_value(&mut *knobs.view, View::Stars, "Stars");
        if *knobs.view == View::Systems {
            ui.add_space(FIELD_GAP);
            ui.label("Color By");
            ui.radio_value(
                &mut *knobs.color_by,
                ColorBy::Allegiance,
                "Allegiance",
            );
            ui.radio_value(
                &mut *knobs.color_by,
                ColorBy::Government,
                "Government",
            );
            ui.radio_value(&mut *knobs.color_by, ColorBy::Security, "Security");
            ui.add_space(FIELD_GAP);
            ui.checkbox(&mut knobs.population_scale.0, "Scale w/ Population");
        }

        // Last, and folded away. Everything above says what the map looks
        // like; these say how it goes about it, and are reached for once in a
        // session if at all.
        heading(ui, "Advanced", true);
        ui.checkbox(&mut knobs.spyglass.fetch, "Fetch Systems");
        if knobs.spyglass.fetch {
            ui.horizontal(|ui| poll_value(ui, &mut knobs.poll.0));
            ui.horizontal(|ui| {
                ui.label("Throttle (ms)");
                ui.add(egui::DragValue::new(&mut knobs.throttle.0));
            });
        }
        ui.add_space(FIELD_GAP);
        if ui.button("Despawn Systems").clicked() {
            knobs.despawner.write(Despawn);
        }
    });

    gear(ctx, edge, &mut settings.0);
    let shut = main_bar(
        ctx,
        &mut search,
        &mut searched,
        &mut search_note,
        &mut selection,
        &mut camera,
        orbit.single().map(|camera| camera.focus).ok(),
        &mut panels,
        &mut plot,
        &mut filter,
    );
    if shut {
        press.0 = true;
    }

    // `egui_wants_pointer_input` covers a drag that began on a control and
    // has since been pulled off it, which being over one does not.
    over_ui.0 = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();

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
fn gear(ctx: &Context, left: f32, open: &mut bool) {
    let style = ctx.global_style();
    let clicked = egui::Area::new(egui::Id::new("settings-gear"))
        .order(egui::Order::Foreground)
        .fixed_pos(egui::pos2(left + MARGIN, MARGIN))
        .show(ctx, |ui| {
            let mut gear = egui::RichText::new("⚙").size(GEAR_SIZE);
            if *open {
                gear = gear.color(style.visuals.strong_text_color());
            }
            ui.add(egui::Button::new(gear).frame(false)).clicked()
        })
        .inner;

    if clicked {
        *open = !*open;
    }
}

/// Ask for a system, and for whatever else the user unfolds
///
/// One field at the top of the viewport, centred, since it is the one
/// question the map is asked over and over. Focusing it brings a pane up
/// behind it and drops the rest of the form out below, and a press landing
/// off the form puts it away again.
///
/// It keeps its box while the bar is at rest, so that what stands at the top
/// of the viewport reads as somewhere to type rather than as a word painted
/// on the map.
///
/// The pane is the input's own frame drawn in nothing while the bar is at
/// rest, rather than a frame left out and put back. Nothing shifts as it
/// comes up, because nothing about the layout has changed.
///
/// Centred rather than set against an edge, so that it holds still while the
/// settings pane slides in and out beside it.
///
/// The note is not part of what drops down. It answers the name in the input,
/// and is worth reading whether or not the rest is out.
fn main_bar(
    ctx: &Context,
    search: &mut BarFields,
    searched: &mut MessageWriter<Searched>,
    note: &mut SearchNote,
    selection: &mut Selection,
    camera: &mut MessageWriter<MoveCamera>,
    focus: Option<DVec3>,
    panels: &mut Panels,
    plot: &mut Plot,
    filter: &mut FilterBar,
) -> bool {
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
        .anchor(egui::Align2::CENTER_TOP, egui::vec2(0., MARGIN))
        .show(ctx, |ui| {
            frame
                .show(ui, |ui| {
                    // Fixed, so that the bar keeps its width and its place as
                    // the form drops out of it.
                    ui.set_width(BAR_WIDTH);
                    let mut taken = false;

                    let response = singleline(ui, &mut search.system, "Search");
                    taken |= response.gained_focus();
                    // The note answers a name, so it is no answer at all
                    // once that name is being typed over.
                    if response.changed() {
                        note.0 = None;
                    }
                    // Return and nothing else. Tab moves between the
                    // fields of a form, and a form that went off and asked
                    // the database something on the way past would be
                    // answering a question nobody had finished asking.
                    if entered(&response, ui)
                        && let Some(name) = search.system.clone()
                    {
                        searched.write(Searched::System { name });
                    }

                    // Both, and in this order: the note answers the query
                    // in the input above it, and the status below says what
                    // is picked out, which after a search that failed is
                    // some other system entirely.
                    if let Some(note) = &note.0 {
                        ui.colored_label(egui::Color32::LIGHT_RED, note);
                    }
                    selected(ui, selection, focus, camera, panels);
                    // Drawn whether or not the form is out, as the selection
                    // is and for the same reason. A half lit sky with
                    // nothing on screen to say why is the one thing a filter
                    // must not leave behind.
                    applied(ui, &mut filter.active, &filter.in_reach, panels);

                    if search.expanded {
                        taken |= filter_section(ui, filter);
                        taken |= route_section(ui, search, searched, plot);
                    }

                    taken
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
    if bar.inner {
        search.expanded = true;
    }
    if dismissed {
        search.expanded = false;
    }
    shut
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
/// words, and the dot in the ring's own colour says which mark out on the
/// map is about to be flown to.
///
/// Measured from where the camera is looking rather than from the camera
/// itself, since that is the distance the spyglass and the fetch are
/// measured in: a system nearer than the spyglass radius is one that is
/// drawn.
fn selected(
    ui: &mut Ui,
    selection: &mut Selection,
    focus: Option<DVec3>,
    camera: &mut MessageWriter<MoveCamera>,
    panels: &mut Panels,
) {
    // Owned, so that the borrow on the selection ends before the row asks to
    // let go of it.
    let Some(name) = selection.name().map(str::to_owned) else { return };
    let away = selection
        .position()
        .zip(focus)
        .map(|(at, focus)| format!("{:.1} Ly away", focus.distance(at)));

    // Laid out and painted rather than assembled from labels. A label is a
    // widget in its own right, and two of them under one clickable row leave
    // three widgets bidding for the pointer: the row answers over the gaps
    // and the labels answer over the words, so it flickers between being a
    // control and not as the pointer crosses them.
    let away = away.map(|line| {
        egui::WidgetText::from(egui::RichText::new(line).weak()).into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            egui::TextStyle::Body,
        )
    });

    let marks = lay_out_marks(ui);

    // Whatever the dot, the distance and the marks leave the name. System
    // names run to "Col 285 Sector XY-Z b12-34", and one laid out against no
    // bound at all is painted straight out past the edge of the bar.
    let gap = ui.spacing().item_spacing.x;
    let room = ui.available_width()
        - ROW_PADDING * 2.
        - DOT
        - gap
        - marks_width(&marks, gap)
        - away.as_ref().map_or(0., |away| away.size().x + gap);
    let name = egui::WidgetText::from(egui::RichText::new(name).strong())
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room.max(0.),
            egui::TextStyle::Body,
        );
    let (outer, row) = row_of(
        ui,
        // The width the rest of the form is laid out in, so that the row
        // lines up with the fields above and below it rather than being
        // measured against anything of its own.
        name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
        "selection-row",
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
        // The galleys carry the colours they were laid out in, so there is
        // nothing for a fallback to answer for.
        ui.painter().galley(
            egui::pos2(x, middle - size.y / 2.),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += size.x + gap;
    }

    let Marks { info, close } = place_marks(ui, rect, marks, "selection-row");

    if close.clicked() {
        selection.clear();
    } else if info.clicked() {
        if let Some(system) = selection.system() {
            panels.open_system(system.clone());
        }
    } else if row.clicked() {
        camera.write(MoveCamera {
            position: selection.position(),
            framing: None,
        });
    }
    row.on_hover_cursor(egui::CursorIcon::PointingHand);
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

/// Ask where a route ends and what it may be flown in
///
/// Answers whether a field of it has just taken focus. A route runs from the
/// name in the search box above, which is where a system is named, and it
/// runs from it as typed: a name has to be searched for to be picked out,
/// and asking a question to reach the question you wanted is no way to fill
/// in a form.
///
/// How it is getting on is said between the fields and the button, where
/// what it is about is on either side of it. The note under the search input
/// answers a name typed into the search input, and a route's answer read out
/// up there would sit a long way from its question.
fn route_section(
    ui: &mut Ui,
    search: &mut BarFields,
    searched: &mut MessageWriter<Searched>,
    plot: &mut Plot,
) -> bool {
    heading(ui, "Route", true);
    let mut taken = false;

    let end = singleline(ui, &mut search.route_end, "End System");
    taken |= end.gained_focus();
    ui.add_space(FIELD_GAP);
    let range = singleline(ui, &mut search.route_range, "Jump Range (Ly)");
    taken |= range.gained_focus();
    // Return in either field asks for the route, as pressing the button
    // does. They are the last two things a route waits on, and a form with
    // one thing left to do should not have to be reached for.
    let submitted = entered(&end, ui) || entered(&range, ui);
    // What came back of the last route asked for answers the fields as they
    // were then, so it goes as soon as they are not. Work still under way is
    // not an answer to anything yet, and stays.
    if (end.changed() || range.changed()) && matches!(*plot, Plot::Trouble(_)) {
        *plot = Plot::Nothing;
    }

    // How the last route asked for is getting on. Only ever a route that
    // was asked for: a field being typed into is not an attempt at
    // anything, and a form that answers back before it has been submitted
    // is a form scolding whoever fills it in.
    if let Plot::Trouble(trouble) = &*plot {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, trouble);
    }

    ui.add_space(FIELD_GAP);
    // The name in the search box, as it stands. A route starts from the
    // system named up there, and naming it is enough: waiting for it to
    // have been searched for would have the user ask a question they did
    // not want the answer to before they could ask the one they did.
    let asked = typed(&search.system)
        .zip(typed(&search.route_end))
        .zip(typed(&search.route_range));
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
                searched.write(Searched::Route {
                    start: start.to_owned(),
                    end: end.to_owned(),
                    // Back to text, since a route is fetched under a key
                    // made of what was asked for and a float is no kind of
                    // key.
                    range: range.to_string(),
                });
                Plot::Working
            }
            Err(trouble) => Plot::Trouble(trouble.to_owned()),
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
/// The mark at the end takes it away for good.
///
/// Laid out and painted rather than assembled from widgets, as the selection
/// row is and for the same reason. A checkbox and a button under one row are
/// three things bidding for the pointer, and it flickers between being a
/// control and not as the pointer crosses them.
///
/// The count is of the whole set rather than one per row, since what the user
/// is looking at is the sky as all of them leave it. It counts what is within
/// the spyglass rather than what has been loaded, which is what the user is
/// looking at rather than everywhere the camera has been. Said in the
/// spyglass's own name, since that is the control that decides it and the one
/// to reach for to see more.
fn applied(
    ui: &mut Ui,
    filters: &mut Filters,
    in_reach: &InReach,
    panels: &mut Panels,
) {
    if filters.iter().next().is_none() {
        return;
    }

    // Settled after the loop, since the rows are drawn from the same filters
    // they change.
    let mut toggling = None;
    let mut removing = None;
    let mut opening = None;
    let gap = ui.spacing().item_spacing.x;

    for (index, active) in filters.iter().enumerate() {
        let marks = lay_out_marks(ui);

        // Whatever the dot and the marks leave. Faction names run long, and
        // one laid out against no bound is painted out past the edge of the
        // bar.
        let room = ui.available_width()
            - ROW_PADDING * 2.
            - DOT
            - gap
            - marks_width(&marks, gap);
        let text = egui::RichText::new(active.filter.name());
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

        let (outer, row) = row_of(
            ui,
            name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
            ("filter-row", &active.filter),
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
        // The galley carries the colour it was laid out in, so there is
        // nothing for a fallback to answer for.
        ui.painter().galley(
            egui::pos2(x, middle - name.size().y / 2.),
            name,
            egui::Color32::PLACEHOLDER,
        );

        let Marks { info, close } =
            place_marks(ui, rect, marks, ("filter-row", &active.filter));

        if close.clicked() {
            removing = Some(index);
        } else if info.clicked() {
            opening = Some(active.filter.clone());
        } else if row.clicked() {
            toggling = Some(index);
        }
        row.on_hover_cursor(egui::CursorIcon::PointingHand);
    }

    let InReach { admitted, total } = *in_reach;
    if total > 0 {
        ui.label(
            egui::RichText::new(format!(
                "{admitted} of {total} within spyglass"
            ))
            .weak(),
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

    // Emptied once what it asked for is standing in a row of its own, and not
    // before: a name is looked up a frame after it is asked for, so taking
    // the text away at the moment return is pressed takes it away from a name
    // that turns out not to resolve.
    if filter.asked.answered() {
        *filter.field = None;
        *filter.asked = Asked::Nothing;
    }

    let response = singleline(ui, &mut filter.field, "Faction Name");
    // The answer is about a name, so it is no answer at all once that name is
    // being typed over.
    if response.changed() {
        *filter.asked = Asked::Nothing;
    }
    if entered(&response, ui)
        && let Some(name) = typed(&filter.field).map(str::to_owned)
    {
        filter.wanted.write(Wanted::Faction { name });
    }

    if let Asked::Trouble(trouble) = &*filter.asked {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, trouble);
    }

    response.gained_focus()
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
/// Every id in these rows is spelled out rather than taken from the order they
/// were drawn in. Egui hands out an id per widget from its place in that
/// order, and the rows of the bar do not keep their places: the note about a
/// name that resolved to nothing comes and goes above them, the selection's
/// row comes and goes with the selection, and dropping one filter moves every
/// row below it up. Any of those hands a row's id, and whatever egui was
/// remembering against it, to whatever slid into its place.
///
/// `push_id` does not answer this. A child `Ui` is keyed on its salt and on
/// the parent's running count of children, so it moves with the rest.
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

/// The two marks a row in the bar ends with
///
/// Info opens a panel about whatever the row names, and close lets go of it.
/// Close stands outermost, where a window's own close button stands, so that
/// the gesture is in the same place wherever it is offered.
struct Marks {
    info: Response,
    close: Response,
}

/// The glyphs those marks are drawn with, outermost first
const MARKS: [&str; 2] = [CLOSE, INFO];

/// Lay the marks out without placing them
///
/// A row needs their width before it can be allocated, since what is left is
/// the room its name has, and it cannot be painted into before it exists. So
/// they are measured here and placed by [`place_marks`] once there is a row
/// to place them in.
fn lay_out_marks(ui: &Ui) -> Vec<std::sync::Arc<egui::Galley>> {
    MARKS
        .iter()
        .map(|glyph| {
            // Laid out in nothing, so that the colour can be chosen once the
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

/// How much room the marks take at the end of a row, gaps included
fn marks_width(marks: &[std::sync::Arc<egui::Galley>], gap: f32) -> f32 {
    marks.iter().map(|mark| mark.size().x + gap).sum()
}

/// Paint the marks into the right hand end of `rect` and answer for each
///
/// Asked about after the row they sit in, so that they are the ones answering
/// where they overlap it. Under it the row would have to work out what it was
/// not being clicked on.
fn place_marks(
    ui: &mut Ui,
    rect: egui::Rect,
    marks: Vec<std::sync::Arc<egui::Galley>>,
    of: impl std::hash::Hash,
) -> Marks {
    let middle = rect.center().y;
    let gap = ui.spacing().item_spacing.x;
    let mut right = rect.right() - ROW_PADDING;
    let mut answers = Vec::new();

    for (which, galley) in marks.into_iter().enumerate() {
        let width = galley.size().x;
        let at = egui::Rect::from_min_max(
            egui::pos2(right - width, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let response = ui.interact(
            at,
            ui.id().with((&of, "row-mark", which)),
            egui::Sense::click(),
        );
        // Lit for the pointer resting on it and for the keyboard reaching it
        // alike. A stop that shows nothing when it is reached reads as the
        // focus having gone missing.
        let lit = response.hovered() || response.has_focus();
        if lit {
            ui.painter().rect_filled(
                at,
                ui.visuals().widgets.hovered.corner_radius,
                ui.visuals().widgets.hovered.weak_bg_fill,
            );
        }
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
    // In `MARKS` order, which is close first.
    let close = answers.next().expect("a close mark");
    let info = answers.next().expect("an info mark");
    Marks { info, close }
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
    contents: impl FnOnce(&mut Ui) -> R,
) -> R {
    // In a scope of its own, since a style set on a `Ui` is set on the rest
    // of that `Ui`, and this is asked for by the list rather than by whatever
    // follows it.
    ui.scope(|ui| {
        ui.spacing_mut().scroll.floating = false;
        egui::ScrollArea::vertical()
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
fn singleline(
    ui: &mut Ui,
    value: &mut Option<String>,
    placeholer: &str,
) -> Response {
    let mut text = match value {
        Some(input) => input.clone(),
        None => placeholer.into(),
    };

    // In a scope of its own, since a style set on a `Ui` is set on the rest
    // of that `Ui`: the grey a field wants for what it is holding would go
    // on to grey the headings under it.
    let response = ui
        .scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = FIELD_BORDER;
            if value.is_none() {
                ui.visuals_mut().override_text_color =
                    Some(egui::Color32::GRAY);
            }
            ui.add_sized(
                egui::vec2(ui.available_width(), 0.),
                egui::TextEdit::singleline(&mut text).margin(FIELD_PADDING),
            )
        })
        .inner;

    if response.gained_focus() {
        *value = Some("".into());
    }

    if text != placeholer {
        *value = Some(text);
    }

    if response.lost_focus() {
        if let Some(ref search) = *value {
            if search == "" {
                *value = None;
            }
        }
    }

    response
}

fn poll_value(ui: &mut Ui, opt: &mut Option<f64>) {
    let mut enabled = opt.is_some();
    if ui.checkbox(&mut enabled, "Poll").changed() {
        if enabled {
            *opt = Some(1.);
        } else {
            *opt = None
        }
    }

    if let Some(val) = opt {
        ui.label("Every");
        ui.add(
            egui::DragValue::new(val).range(0.0..=60.).speed(0.01).suffix(" s"),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::systems::filter::Filter;
    use crate::tests::painted;

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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        filters
            .add(Filter::Route { label: "A -> B".into(), systems: vec![1, 2] });
        let mut panels = Panels::default();

        let said = complaints(|ui| {
            applied(
                ui,
                &mut filters,
                &InReach { admitted: 1, total: 2 },
                &mut panels,
            )
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
        let (mut first, mut moved) = (None, None);
        let at =
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(80., 20.));

        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let marks = lay_out_marks(ui);
            first =
                Some(place_marks(ui, at, marks, ("filter-row", 7)).close.id);
            ui.label("a note that comes and goes");
            let marks = lay_out_marks(ui);
            moved =
                Some(place_marks(ui, at, marks, ("filter-row", 7)).close.id);
        });

        assert_eq!(first, moved);
    }

    /// The filter rows come out in colours something can draw
    ///
    /// Every galley here is laid out strong or weak, which resolves a colour,
    /// so the placeholder each is painted with is never reached. That holds
    /// by how the rows happen to be styled and nothing else, and one plain
    /// piece of text would take the whole bar down.
    ///
    /// Covers the marks as well, which the selection's row draws the same
    /// way.
    #[test]
    fn the_filter_rows_paint_in_colours() {
        let mut filters = Filters::default();
        filters.add(Filter::Faction { id: 1, name: "Zargon Front".into() });
        filters.add(Filter::Faction { id: 2, name: "Alliance".into() });
        filters.toggle(1);
        let mut panels = Panels::default();

        painted(|ui| {
            applied(
                ui,
                &mut filters,
                &InReach { admitted: 3, total: 40 },
                &mut panels,
            )
        });
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
}
