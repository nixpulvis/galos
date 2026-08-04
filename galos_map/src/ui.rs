//! The chrome standing between the user and the map
//!
//! A search bar centred at the top, a gear in the corner, and the settings
//! pane that gear slides out from the left edge. What is known about the
//! system the user picked out is drawn by [`crate::systems::selection`],
//! which owns the fields it reads.

use crate::camera::{MoveCamera, OrbitCamera};
use crate::search::{Plot, SearchNote, Searched};
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::info::Panels;
use crate::systems::labels::NameRadius;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::selection::{SELECTION, Selection};
use crate::systems::spawn::{ColorBy, ShowNames};
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
/// Shutting the search form is done by pressing somewhere off it, and the
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

/// How wide the search bar stands, unfolded or not
const BAR_WIDTH: f32 = 220.;

/// How tall the gear is drawn
const GEAR_SIZE: f32 = 18.;

/// How far the chrome stands from the edges of the viewport, and from itself
///
/// Read by [`crate::systems::info`] as well, so that the panels it opens
/// against the right edge stand off it by as much as the gear stands off the
/// left.
pub(crate) const MARGIN: f32 = 8.;

/// How far the search bar's contents stand from the pane behind them
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

/// The mark on the control that opens what is known about the selection
const INFO: &str = "ℹ";

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

/// The scales a radius is offered at, and how finely each one steps
///
/// Width of the galaxy is 105,700 Ly.
const RADIUS_SCALES: [(f32, f32, f64, f64); 3] =
    [(1., 50., 0.1, 0.2), (10., 500., 1., 0.2), (10., 1.1e5, 10., 0.5)];

/// Offer one radius at each scale it might be wanted at
///
/// A single slider over five orders of magnitude has no purchase near the
/// bottom, where a light year is a real distance, and no reach at the top.
/// Three ranges over the same number give both, and whichever is at hand is
/// the one that suits the value at the time.
///
/// None of them clamps, since the narrowest would otherwise drag the value
/// back down every frame it was drawn. `ceiling` clamps instead, once, after
/// all three have had their say, and a range past it is not offered at all.
fn radius_sliders(ui: &mut Ui, radius: &mut f32, ceiling: f32) {
    let mut reached = 0.;
    for (low, high, step, speed) in RADIUS_SCALES {
        let high = high.min(ceiling);
        // Each scale has to reach further than the last to earn a slider.
        // Under a low ceiling they clamp to the same number, and a second
        // slider over a range already offered says nothing the first did
        // not.
        if low >= high || high <= reached {
            continue;
        }
        reached = high;
        ui.label(format!("{low} - {high} Ly"));
        ui.add(
            egui::Slider::new(radius, low..=high)
                .clamping(egui::SliderClamping::Never)
                .logarithmic(true)
                .step_by(step)
                .drag_value_speed(speed),
        );
    }
    *radius = radius.clamp(RADIUS_SCALES[0].0, ceiling);
}

/// What the user has typed into the search bar, and how much of it is shown
///
/// One form, so one piece of state. Held together rather than as five
/// separate locals because a system param is a scarce thing and these are
/// only ever read and cleared as a group.
#[derive(Default)]
pub struct SearchFields {
    system: Option<String>,
    route_end: Option<String>,
    route_range: Option<String>,
    faction: Option<String>,
    /// Whether the rest of the form is out below the input
    ///
    /// Turned on when a field takes focus and off when a press lands off the
    /// form, both of which [`search_bar`] settles at the end of a frame from
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

pub fn chrome(
    mut contexts: EguiContexts,
    mut knobs: Knobs,
    mut searched: MessageWriter<Searched>,
    mut search_note: ResMut<SearchNote>,
    mut over_ui: ResMut<PointerOverUi>,
    mut settings: ResMut<SettingsOpen>,
    mut search: Local<SearchFields>,
    selection: Res<Selection>,
    mut camera: MessageWriter<MoveCamera>,
    orbit: Query<&OrbitCamera>,
    mut press: ResMut<PressAnswered>,
    mut panels: ResMut<Panels>,
    mut plot: ResMut<Plot>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // The pane first, since where it has reached is where the gear stands.
    let edge = settings_pane(ctx, settings.0, |ui| {
        ui.label("Spyglass Radius");
        ui.group(|ui| {
            radius_sliders(ui, &mut knobs.spyglass.radius, 1.1e5);
            ui.add_space(2.);
            ui.checkbox(&mut knobs.spyglass.lock_camera, "Lock Camera");
            ui.add_space(2.);
            ui.checkbox(&mut knobs.spyglass.disabled, "Override Spyglass");
            ui.add_space(2.);
            ui.collapsing("Advanced", |ui| {
                ui.checkbox(&mut knobs.spyglass.fetch, "Fetch Systems");
                if knobs.spyglass.fetch {
                    ui.horizontal(|ui| poll_value(ui, &mut knobs.poll.0));
                    ui.add_space(2.);
                    ui.horizontal(|ui| {
                        ui.label("Throttle (ms)");
                        ui.add(egui::DragValue::new(&mut knobs.throttle.0));
                    });
                }
                ui.add_space(2.);
                if ui.button("Despawn Systems").clicked() {
                    knobs.despawner.write(Despawn);
                }
                ui.add_space(2.);
            });
        });

        ui.add_space(5.);

        ui.group(|ui| {
            ui.label("View:");
            ui.radio_value(&mut *knobs.view, View::Systems, "Systems");
            ui.radio_value(&mut *knobs.view, View::Stars, "Stars");
            ui.separator();

            match *knobs.view {
                View::Systems => {
                    ui.label("Color By:");
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
                    ui.radio_value(
                        &mut *knobs.color_by,
                        ColorBy::Security,
                        "Security",
                    );
                    ui.separator();
                    ui.checkbox(
                        &mut knobs.population_scale.0,
                        "Scale w/ Population",
                    );
                }
                View::Stars => {}
            }

            ui.checkbox(&mut knobs.show_names.0, "Show System Names");
            if knobs.show_names.0 {
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
                        1.1e5
                    } else {
                        knobs.spyglass.radius
                    };
                    ui.label("Name Radius");
                    ui.group(|ui| {
                        radius_sliders(
                            ui,
                            &mut knobs.name_radius.radius,
                            ceiling,
                        )
                    });
                }
            }
        });
    });

    gear(ctx, edge, &mut settings.0);
    let shut = search_bar(
        ctx,
        &mut search,
        &mut searched,
        &mut search_note,
        &selection,
        &mut camera,
        orbit.single().map(|camera| camera.focus).ok(),
        &mut panels,
        &mut plot,
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
fn search_bar(
    ctx: &Context,
    search: &mut SearchFields,
    searched: &mut MessageWriter<Searched>,
    note: &mut SearchNote,
    selection: &Selection,
    camera: &mut MessageWriter<MoveCamera>,
    focus: Option<DVec3>,
    panels: &mut Panels,
    plot: &mut Plot,
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

    let bar = egui::Area::new(egui::Id::new("search-bar"))
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
                    // Tab as well as return. A route starts from whatever
                    // is picked out and nothing is picked out until a name
                    // has been asked for, so tabbing on from a name typed
                    // here is how a route is begun. Not so of the faction
                    // field below, where the same would empty the map on
                    // the way past.
                    let tabbed = response.lost_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Tab));
                    if entered(&response, ui) || tabbed {
                        search.faction = None;
                        if let Some(name) = search.system.clone() {
                            searched.write(Searched::System { name });
                        }
                    }

                    // Both, and in this order: the note answers the query
                    // in the input above it, and the status below says what
                    // is picked out, which after a search that failed is
                    // some other system entirely.
                    if let Some(note) = &note.0 {
                        ui.colored_label(egui::Color32::LIGHT_RED, note);
                    }
                    selected(ui, selection, focus, camera, panels);

                    if search.expanded {
                        taken |= route_section(
                            ui, search, searched, selection, plot,
                        );
                        taken |= faction_section(ui, search, searched);
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
    selection: &Selection,
    focus: Option<DVec3>,
    camera: &mut MessageWriter<MoveCamera>,
    panels: &mut Panels,
) {
    let Some(name) = selection.name() else { return };
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

    // Laid out in nothing, so that the colour it comes out in can be chosen
    // once the pointer has been asked about, which cannot happen until the
    // row it sits in has been placed.
    let icon = egui::WidgetText::from(
        egui::RichText::new(INFO).color(egui::Color32::PLACEHOLDER),
    )
    .into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );

    // Whatever the dot, the distance and the mark leave the name. System
    // names run to "Col 285 Sector XY-Z b12-34", and one laid out against no
    // bound at all is painted straight out past the edge of the bar.
    let gap = ui.spacing().item_spacing.x;
    let room = ui.available_width()
        - ROW_PADDING * 2.
        - DOT
        - gap
        - icon.size().x
        - gap
        - away.as_ref().map_or(0., |away| away.size().x + gap);
    let name = egui::WidgetText::from(egui::RichText::new(name).strong())
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            room.max(0.),
            egui::TextStyle::Body,
        );
    let (outer, row) = ui.allocate_exact_size(
        // The width the rest of the form is laid out in, so that the row
        // lines up with the fields above and below it rather than being
        // measured against anything of its own.
        egui::vec2(
            ui.available_width(),
            name.size().y.max(DOT) + (ROW_PADDING + ROW_MARGIN) * 2.,
        ),
        egui::Sense::click(),
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

    // Asked for after the row, so that it is the one answering where the two
    // overlap. Under it the row would have to work out what it was not being
    // clicked on.
    let mark = egui::Rect::from_min_max(
        egui::pos2(rect.right() - ROW_PADDING - icon.size().x, rect.top()),
        egui::pos2(rect.right() - ROW_PADDING, rect.bottom()),
    );
    let opening =
        ui.interact(mark, ui.id().with("open-info"), egui::Sense::click());
    // Lit for the pointer resting on it and for the keyboard reaching it
    // alike. It stands in the tab order between the row and the route
    // fields, and a stop that shows nothing when it is reached reads as the
    // focus having gone missing.
    let lit = opening.hovered() || opening.has_focus();
    if lit {
        ui.painter().rect_filled(
            mark,
            ui.visuals().widgets.hovered.corner_radius,
            ui.visuals().widgets.hovered.weak_bg_fill,
        );
    }
    ui.painter().galley(
        egui::pos2(mark.left(), middle - icon.size().y / 2.),
        icon,
        if lit {
            ui.visuals().strong_text_color()
        } else {
            ui.visuals().weak_text_color()
        },
    );

    if opening.clicked() {
        if let Some(system) = selection.system() {
            panels.open(system.clone());
        }
    } else if row.clicked() {
        camera.write(MoveCamera { position: selection.position() });
    }
    row.on_hover_cursor(egui::CursorIcon::PointingHand);
    opening.on_hover_cursor(egui::CursorIcon::PointingHand);
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
/// Answers whether a field of it has just taken focus. A route runs from
/// whatever is picked out, since that is the one system the map is holding,
/// and not from the search box, which holds a query and will one day answer
/// with a list. So the section is greyed until something is picked out,
/// rather than appearing and disappearing under the pointer.
///
/// How it is getting on is said between the fields and the button, where
/// what it is about is on either side of it. The note under the search input
/// answers a name typed into the search input, and a route's answer read out
/// up there would sit a long way from its question.
fn route_section(
    ui: &mut Ui,
    search: &mut SearchFields,
    searched: &mut MessageWriter<Searched>,
    selection: &Selection,
    plot: &mut Plot,
) -> bool {
    heading(ui, "Route");
    let start = selection.name();
    let mut taken = false;

    // Live whether or not a system is picked out. What waits on a start is
    // the button, since only it needs one, and fields that come and go with
    // the selection cannot be tabbed into on the way from naming a system to
    // asking where to go from it: they are still disabled on the frame the
    // tab lands, the selection being a frame behind the name that set it.
    let end = singleline(ui, &mut search.route_end, "End System");
    taken |= end.gained_focus();
    ui.add_space(FIELD_GAP);
    let range = singleline(ui, &mut search.route_range, "Jump Range (Ly)");
    taken |= range.gained_focus();
    // What came back of the last route asked for answers the fields as they
    // were then, so it goes as soon as they are not. Work still under way is
    // not an answer to anything yet, and stays.
    if (end.changed() || range.changed()) && matches!(*plot, Plot::Trouble(_)) {
        *plot = Plot::Nothing;
    }

    // One line, for what the route still wants or for how the last one is
    // getting on. Only ever a route that was asked for: a field being typed
    // into is not an attempt at anything, and a form that answers back
    // before it has been submitted is a form scolding whoever fills it in.
    if start.is_none() {
        ui.add_space(FIELD_GAP);
        ui.label(egui::RichText::new("Pick out a system to route from").weak());
    } else if let Plot::Trouble(trouble) = &*plot {
        ui.add_space(FIELD_GAP);
        ui.colored_label(egui::Color32::LIGHT_RED, trouble);
    }

    ui.add_space(FIELD_GAP);
    let asked =
        start.zip(typed(&search.route_end)).zip(typed(&search.route_range));
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

    if button.response.clicked()
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

/// Ask after a faction by name
///
/// Answers whether its field has just taken focus. Its own section rather
/// than a
/// field on the end of the route's, since a faction has nothing to do with
/// either end of a route and searching for one clears the system search.
fn faction_section(
    ui: &mut Ui,
    search: &mut SearchFields,
    searched: &mut MessageWriter<Searched>,
) -> bool {
    heading(ui, "Faction");
    let response = singleline(ui, &mut search.faction, "Faction Name");
    if entered(&response, ui) {
        search.system = None;
        if let Some(name) = search.faction.clone() {
            searched.write(Searched::Faction { name });
        }
    }
    response.gained_focus()
}

/// Open a section of the form
///
/// The rule is the break between one section and the next, and needs no
/// run-up of its own: the row above it keeps as much room under itself as it
/// keeps over, and a section gap on top of that would sit the row nearer the
/// input than the section and read as belonging to neither.
fn heading(ui: &mut Ui, name: &str) {
    ui.separator();
    ui.label(egui::RichText::new(name).strong());
    ui.add_space(FIELD_GAP);
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
        ui.label("(Hz)");
        ui.add(egui::DragValue::new(val).range(0.0..=60.).speed(0.01));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
