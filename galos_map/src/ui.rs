use crate::search::{SearchNote, Searched};
use crate::systems::Spyglass;
use crate::systems::despawn::Despawn;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::labels::NameRadius;
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::spawn::{ColorBy, ShowNames};
use bevy::prelude::*;
use bevy_egui::egui::{Response, Ui};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};

pub fn plugin(app: &mut App) {
    app.init_resource::<PointerOverUi>();
    app.add_systems(EguiPrimaryContextPass, panels);
}

/// Whether the pointer is busy with the settings windows
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

// TODO: Form validation.

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
    for (low, high, step, speed) in RADIUS_SCALES {
        let high = high.min(ceiling);
        if low >= high {
            continue;
        }
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

/// Map settings and controls
/// What the user has typed into the search boxes
///
/// One form, so one piece of state. Held together rather than as four
/// separate locals because a system param is a scarce thing and these are
/// only ever read and cleared as a group.
#[derive(Default)]
pub struct SearchFields {
    system: Option<String>,
    route_end: Option<String>,
    route_range: Option<String>,
    faction: Option<String>,
}

pub fn panels(
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
    mut search: Local<SearchFields>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    egui::Window::new("Search").default_open(false).resizable(false).show(
        ctx,
        |ui| {
            ui.set_width(125.);

            let response = singleline(ui, &mut search.system, "System Name");
            if response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                search.faction = None;
                if let Some(name) = search.system.clone() {
                    searched.write(Searched::System { name });
                }
            }
            if let Some(note) = &search_note.0 {
                ui.colored_label(egui::Color32::LIGHT_RED, note);
            }
            if search.system.is_some() {
                ui.add_space(2.);
                ui.label("Route");
                singleline(ui, &mut search.route_end, "End System");
                ui.add_space(2.);
                singleline(ui, &mut search.route_range, "Range (Ly)");
                ui.add_space(3.);

                if ui.button("Plot Route...").clicked() {
                    if let (Some(ref s), Some(ref e), Some(ref r)) = (
                        search.system.as_ref(),
                        search.route_end.as_ref(),
                        search.route_range.as_ref(),
                    ) {
                        #[allow(irrefutable_let_patterns)]
                        if let Ok(r) = r.parse() {
                            searched.write(Searched::Route {
                                start: (*s).clone(),
                                end: (*e).clone(),
                                range: r,
                            });
                        }
                    }
                }
                ui.add_space(2.);
            }

            ui.separator();

            let response = singleline(ui, &mut search.faction, "Faction Name");
            if response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                search.system = None;
                if let Some(name) = search.faction.clone() {
                    searched.write(Searched::Faction { name });
                }
            }
        },
    );

    egui::Window::new("Settings").default_open(false).resizable(false).show(
        ctx,
        |ui| {
            // TODO: IDK why this is necessary, the groups should fill the correct
            // size, no?
            ui.set_width(150.);

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
                        ui.checkbox(
                            &mut population_scale.0,
                            "Scale w/ Population",
                        );
                    }
                    View::Stars => {}
                }

                ui.checkbox(&mut show_names.0, "Show System Names");
                if show_names.0 {
                    // A name can only be drawn for a system that is drawn,
                    // and the spyglass is what decides that, so asking for
                    // names further out than it reaches asks for nothing.
                    // Overriding it draws everything loaded, and then the
                    // question is open again.
                    let ceiling =
                        if spyglass.disabled { 1.1e5 } else { spyglass.radius };
                    ui.label("Name Radius");
                    ui.group(|ui| {
                        radius_sliders(ui, &mut name_radius.0, ceiling)
                    });
                }
            });
        },
    );

    // `egui_wants_pointer_input` covers a drag that began on a control and
    // has since been pulled off it, which being over one does not.
    over_ui.0 = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();

    Ok(())
}

fn singleline(
    ui: &mut Ui,
    value: &mut Option<String>,
    placeholer: &str,
) -> Response {
    if value.is_none() {
        ui.style_mut().visuals.override_text_color = Some(egui::Color32::GRAY);
    }

    let mut text = match value {
        Some(input) => input.clone(),
        None => placeholer.into(),
    };

    let response = ui
        .add_sized(egui::vec2(125., 0.), egui::TextEdit::singleline(&mut text));

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
