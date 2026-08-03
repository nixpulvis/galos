//! The chrome standing between the user and the map
//!
//! A search bar centred at the top, a gear in the corner, and the settings
//! pane that gear slides out from the left edge. What is known about the
//! system the user picked out is drawn by [`crate::systems::selection`],
//! which owns the fields it reads.

use self::Edging::{Bare, Boxed};
use crate::search::{SearchNote, Searched};
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::labels::NameRadius;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::spawn::{ColorBy, ShowNames};
use bevy::prelude::*;
use bevy_egui::egui::{Context, Response, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.init_resource::<SettingsOpen>();
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

/// How far one section of a form stands from the last
const SECTION_GAP: f32 = 6.;

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
    /// Last frame's answer, rather than a setting the user turns on and off.
    /// A field cannot report that it holds focus until it has been drawn, and
    /// whether to draw it is the question being asked, so [`search_bar`]
    /// settles it at the end of one frame and the next frame draws what it
    /// settled on.
    expanded: bool,
}

pub fn chrome(
    mut contexts: EguiContexts,
    mut spyglass: ResMut<Spyglass>,
    mut view: ResMut<View>,
    mut color_by: ResMut<ColorBy>,
    mut population_scale: ResMut<ScalePopulation>,
    mut show_names: ResMut<ShowNames>,
    mut throttle: ResMut<Throttle>,
    mut poll: ResMut<Poll>,
    mut name_radius: ResMut<NameRadius>,
    mut searched: MessageWriter<Searched>,
    search_note: Res<SearchNote>,
    mut over_ui: ResMut<PointerOverUi>,
    mut despawner: MessageWriter<Despawn>,
    mut settings: ResMut<SettingsOpen>,
    mut search: Local<SearchFields>,
) -> Result {
    let ctx = contexts.ctx_mut()?;

    // The pane first, since where it has reached is where the gear stands.
    let edge = settings_pane(ctx, settings.0, |ui| {
        ui.label("Spyglass Radius");
        ui.group(|ui| {
            radius_sliders(ui, &mut spyglass.radius, 1.1e5);
            ui.add_space(2.);
            ui.checkbox(&mut spyglass.lock_camera, "Lock Camera");
            ui.add_space(2.);
            ui.checkbox(&mut spyglass.disabled, "Override Spyglass");
            ui.add_space(2.);
            ui.collapsing("Advanced", |ui| {
                ui.checkbox(&mut spyglass.fetch, "Fetch Systems");
                if spyglass.fetch {
                    ui.horizontal(|ui| poll_value(ui, &mut poll.0));
                    ui.add_space(2.);
                    ui.horizontal(|ui| {
                        ui.label("Throttle (ms)");
                        ui.add(egui::DragValue::new(&mut throttle.0));
                    });
                }
                ui.add_space(2.);
                if ui.button("Despawn Systems").clicked() {
                    despawner.write(Despawn);
                }
                ui.add_space(2.);
            });
        });

        ui.add_space(5.);

        ui.group(|ui| {
            ui.label("View:");
            ui.radio_value(&mut *view, View::Systems, "Systems");
            ui.radio_value(&mut *view, View::Stars, "Stars");
            ui.separator();

            match *view {
                View::Systems => {
                    ui.label("Color By:");
                    ui.radio_value(
                        &mut *color_by,
                        ColorBy::Allegiance,
                        "Allegiance",
                    );
                    ui.radio_value(
                        &mut *color_by,
                        ColorBy::Government,
                        "Government",
                    );
                    ui.radio_value(
                        &mut *color_by,
                        ColorBy::Security,
                        "Security",
                    );
                    ui.separator();
                    ui.checkbox(&mut population_scale.0, "Scale w/ Population");
                }
                View::Stars => {}
            }

            ui.checkbox(&mut show_names.0, "Show System Names");
            if show_names.0 {
                ui.checkbox(
                    &mut name_radius.follow_spyglass,
                    "Names Follow Spyglass",
                );
                if !name_radius.follow_spyglass {
                    // A name can only be drawn for a system that is drawn,
                    // and the spyglass decides that. Overriding it draws
                    // everything loaded, and then names may be asked for
                    // beyond its reach.
                    let ceiling =
                        if spyglass.disabled { 1.1e5 } else { spyglass.radius };
                    ui.label("Name Radius");
                    ui.group(|ui| {
                        radius_sliders(ui, &mut name_radius.radius, ceiling)
                    });
                }
            }
        });
    });

    gear(ctx, edge, &mut settings.0);
    search_bar(ctx, &mut search, &mut searched, &search_note);

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
/// One bare line of text at the top of the viewport, centred, since it is the
/// one question the map is asked over and over. Focusing it brings a pane up
/// behind it and drops the rest of the form out below.
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
    note: &SearchNote,
) {
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
                    let mut busy = false;

                    // Bare while the bar is at rest, so that what stands at
                    // the top of the viewport is a line of text. Boxed once
                    // the pane is up, so that it reads as the first field of
                    // the form rather than as a caption over it.
                    let edging = if search.expanded { Boxed } else { Bare };
                    let response =
                        singleline(ui, &mut search.system, "Search", edging);
                    busy |= response.has_focus();
                    if entered(&response, ui) {
                        search.faction = None;
                        if let Some(name) = search.system.clone() {
                            searched.write(Searched::System { name });
                        }
                    }

                    if let Some(note) = &note.0 {
                        ui.colored_label(egui::Color32::LIGHT_RED, note);
                    }

                    if search.expanded {
                        busy |= route_section(ui, search, searched);
                        busy |= faction_section(ui, search, searched);
                    }

                    busy
                })
                .inner
        });

    let over = ctx
        .pointer_latest_pos()
        .is_some_and(|at| bar.response.rect.contains(at));
    // Settled from what the bar has just reported rather than switched, so
    // there is no state to be left stuck in: the moment nothing in the form
    // holds focus and the pointer is elsewhere, it is shut. The pointer only
    // holds out a form that is already out, since resting on something is not
    // the same as asking for it.
    search.expanded = bar.inner || (search.expanded && over);
}

/// Ask where a route ends and what it may be flown in
///
/// Answers whether the user is busy with it. Where the route starts is the
/// name in the search input above, so the whole section is greyed until there
/// is one, rather than appearing and disappearing under the pointer.
fn route_section(
    ui: &mut Ui,
    search: &mut SearchFields,
    searched: &mut MessageWriter<Searched>,
) -> bool {
    heading(ui, "Route");
    let named = search.system.is_some();
    let section = ui.add_enabled_ui(named, |ui| {
        let mut busy = false;
        busy |= singleline(ui, &mut search.route_end, "End System", Boxed)
            .has_focus();
        ui.add_space(FIELD_GAP);
        busy |=
            singleline(ui, &mut search.route_range, "Jump Range (Ly)", Boxed)
                .has_focus();
        ui.add_space(FIELD_GAP);
        if ui.button("Plot Route").clicked() {
            plot_route(search, searched);
        }
        busy
    });

    if !named {
        section
            .response
            .on_disabled_hover_text("Name a system above to plot a route from");
    }
    section.inner
}

/// Ask after a faction by name
///
/// Answers whether the user is busy with it. Its own section rather than a
/// field on the end of the route's, since a faction has nothing to do with
/// either end of a route and searching for one clears the system search.
fn faction_section(
    ui: &mut Ui,
    search: &mut SearchFields,
    searched: &mut MessageWriter<Searched>,
) -> bool {
    heading(ui, "Faction");
    let response = singleline(ui, &mut search.faction, "Faction Name", Boxed);
    if entered(&response, ui) {
        search.system = None;
        if let Some(name) = search.faction.clone() {
            searched.write(Searched::Faction { name });
        }
    }
    response.has_focus()
}

/// Open a section of the form
fn heading(ui: &mut Ui, name: &str) {
    ui.add_space(SECTION_GAP);
    ui.separator();
    ui.label(egui::RichText::new(name).strong());
    ui.add_space(FIELD_GAP);
}

/// Ask for a route between the two systems named, at the range given
///
/// Says nothing at all when a field is empty or the range will not parse,
/// which is what form validation is a TODO for.
fn plot_route(search: &SearchFields, searched: &mut MessageWriter<Searched>) {
    let (Some(start), Some(end), Some(range)) = (
        search.system.as_ref(),
        search.route_end.as_ref(),
        search.route_range.as_ref(),
    ) else {
        return;
    };
    #[allow(irrefutable_let_patterns)]
    if let Ok(range) = range.parse() {
        searched.write(Searched::Route {
            start: start.clone(),
            end: end.clone(),
            range,
        });
    }
}

/// Whether the user has just finished with a field by pressing return
fn entered(response: &Response, ui: &Ui) -> bool {
    response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// Whether a text field draws a box around itself
///
/// Every field of the form is boxed, since stacked bare fields read as one
/// paragraph rather than as a form. Only the one the bar leads with goes
/// bare, and only while the bar is at rest, so that what stands at the top of
/// the viewport asking nothing of anyone is a line of text.
///
/// Both are laid out the same, so a field can change from one to the other
/// without moving. Egui pads a text field as part of building the box around
/// it, which means a bare field has to be given [`FIELD_PADDING`] by hand to
/// come out the same size.
#[derive(PartialEq)]
enum Edging {
    Boxed,
    Bare,
}

/// How far a text field's text stands from its own edge
///
/// The same for a bare field as for a boxed one, so that the input the bar
/// leads with keeps its place as the box comes up around it.
const FIELD_PADDING: egui::Margin =
    egui::Margin { left: 4, right: 4, top: 4, bottom: 4 };

/// One text field, showing what it wants when it holds nothing
fn singleline(
    ui: &mut Ui,
    value: &mut Option<String>,
    placeholer: &str,
    edging: Edging,
) -> Response {
    if value.is_none() {
        ui.style_mut().visuals.override_text_color = Some(egui::Color32::GRAY);
    }

    let mut text = match value {
        Some(input) => input.clone(),
        None => placeholer.into(),
    };

    let mut field = egui::TextEdit::singleline(&mut text).margin(FIELD_PADDING);
    if edging == Bare {
        // A frame given by name is taken as given, padding and all, where
        // one left to egui is built around the field's own margin. So the
        // padding has to be repeated here to survive losing the box.
        field = field.frame(egui::Frame::new().inner_margin(FIELD_PADDING));
    }
    let response = ui.add_sized(egui::vec2(ui.available_width(), 0.), field);

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
