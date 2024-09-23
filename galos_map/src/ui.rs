use crate::search::Searched;
use crate::systems::fetch::{Poll, Throttle};
use crate::systems::scale::{ScalePopulation, View};
use crate::systems::spawn::{ColorBy, Despawn, ShowNames};
use crate::systems::Spyglass;
use bevy::prelude::*;
use bevy_egui::egui::{Response, Ui};
use bevy_egui::{egui, EguiContexts};

// TODO: Form validation.

/// Map settings and controls
pub fn panel(
    mut contexts: EguiContexts,
    mut spyglass: ResMut<Spyglass>,
    mut view: ResMut<View>,
    mut color_by: ResMut<ColorBy>,
    mut population_scale: ResMut<ScalePopulation>,
    mut show_names: ResMut<ShowNames>,
    mut throttle: ResMut<Throttle>,
    mut poll: ResMut<Poll>,
    mut searched: EventWriter<Searched>,
    mut despawner: EventWriter<Despawn>,
    mut system_name: Local<Option<String>>,
    mut faction_name: Local<Option<String>>,
    mut route_start: Local<Option<String>>,
    mut route_end: Local<Option<String>>,
    mut route_range: Local<Option<String>>,
) {
    if let Some(ctx) = contexts.try_ctx_mut() {
        egui::Window::new("Galos").resizable(false).show(ctx, |ui| {
            ui.collapsing("Search", |ui| {
                search(ui, &mut searched, &mut system_name, &mut faction_name);
            });
            ui.collapsing("Route", |ui| {
                route(
                    ui,
                    &mut searched,
                    &mut route_start,
                    &mut route_end,
                    &mut route_range,
                );
            });
            ui.collapsing("Settings", |ui| {
                settings(
                    ui,
                    &mut spyglass,
                    &mut view,
                    &mut color_by,
                    &mut population_scale,
                    &mut show_names,
                    &mut throttle,
                    &mut poll,
                    &mut despawner,
                );
            });
        });
    }
}

/// Star system search UI by name or faction
pub fn search(
    ui: &mut Ui,
    events: &mut EventWriter<Searched>,
    system_name: &mut Local<Option<String>>,
    faction_name: &mut Local<Option<String>>,
) {
    let response = singleline(ui, &mut **system_name, "System Search");
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        **faction_name = None;
        if let Some(ref search) = **system_name {
            events.send(Searched::System { name: search.clone() });
        }
    }

    let response = singleline(ui, &mut **faction_name, "Faction Search");
    if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
        **system_name = None;
        if let Some(ref search) = **faction_name {
            events.send(Searched::Faction { name: search.clone() });
        }
    }
}

pub fn settings(
    ui: &mut Ui,
    spyglass: &mut ResMut<Spyglass>,
    view: &mut ResMut<View>,
    color_by: &mut ResMut<ColorBy>,
    population_scale: &mut ResMut<ScalePopulation>,
    show_names: &mut ResMut<ShowNames>,
    throttle: &mut ResMut<Throttle>,
    poll: &mut ResMut<Poll>,
    despawner: &mut EventWriter<Despawn>,
) {
    // TODO: IDK why this is necessary, the groups should fill the correct
    // size, no?
    ui.set_width(150.);

    ui.group(|ui| {
        ui.label("Spyglass Radius");
        ui.add(
            egui::Slider::new(&mut spyglass.radius, 10.0..=25000.0)
                .logarithmic(true)
                .drag_value_speed(0.1)
        );
        ui.add_space(2.);
        ui.checkbox(&mut spyglass.disabled, "Override Spyglass");
        ui.add_space(2.);
        ui.collapsing("Advanced", |ui| {
            ui.checkbox(&mut spyglass.fetch, "Fetch Systems");
            if spyglass.fetch {
                ui.horizontal(|ui| {
                    poll_value(ui, &mut poll.0);
                    ui.label("Poll (Hz)");
                });
                ui.add_space(2.);
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut throttle.0).speed(5));
                    ui.label("Throttle (ms)");
                });
            }
            ui.add_space(2.);
            if ui.button("Despawn Systems").clicked() {
                despawner.send(Despawn);
            }
            ui.add_space(2.);
        });
    });

    ui.add_space(5.);

    ui.group(|ui| {
        ui.label("View:");
        ui.radio_value(&mut **view, View::Systems, "Systems");
        ui.radio_value(&mut **view, View::Stars, "Stars");
        ui.separator();

        match **view {
            View::Systems => {
                ui.label("Color By:");
                ui.radio_value(
                    &mut **color_by,
                    ColorBy::Allegiance,
                    "Allegiance",
                );
                ui.radio_value(
                    &mut **color_by,
                    ColorBy::Government,
                    "Government",
                );
                ui.radio_value(&mut **color_by, ColorBy::Security, "Security");
                ui.separator();
                ui.checkbox(&mut population_scale.0, "Scale w/ Population");
            }
            View::Stars => {}
        }

        ui.checkbox(&mut show_names.0, "Show System Names");
    });
}

/// Route finding UI for finding out how to get from A to B
pub fn route(
    ui: &mut Ui,
    events: &mut EventWriter<Searched>,
    start: &mut Local<Option<String>>,
    end: &mut Local<Option<String>>,
    range: &mut Local<Option<String>>,
) {
    singleline(ui, &mut **start, "Start System");
    ui.add_space(2.);
    singleline(ui, &mut **end, "End System");
    ui.add_space(2.);
    singleline(ui, &mut **range, "Range (Ly)");
    ui.add_space(2.);
    if ui.button("Plot Route...").clicked() {
        if let (Some(ref s), Some(ref e), Some(ref r)) =
            (start.as_ref(), end.as_ref(), range.as_ref())
        {
            #[allow(irrefutable_let_patterns)]
            if let Ok(r) = (**r).parse() {
                events.send(Searched::Route {
                    start: (*s).clone(),
                    end: (*e).clone(),
                    range: r,
                });
            }
        }
    }
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
        Some(ref input) => input.clone(),
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
    let mut placeholder = 0.;
    if let Some(ref mut val) = opt {
        ui.add(egui::DragValue::new(val).range(0.0..=60.).speed(0.01));
    } else {
        ui.add(
            egui::DragValue::new(&mut placeholder)
                .custom_formatter(|_, _| "—".into()),
        );
    }
    if placeholder != 0. && opt.is_none() {
        *opt = Some(0.);
    }
}
