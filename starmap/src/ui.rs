use crate::search::Searched;
use crate::systems::{
    ColorBy, Despawn, ScalePopulation, ShowNames, Spyglass, View,
};
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
    mut searched: EventWriter<Searched>,
    mut despawner: EventWriter<Despawn>,
    mut system_name: Local<Option<String>>,
    mut faction_name: Local<Option<String>>,
    mut route_start: Local<String>,
    mut route_end: Local<String>,
    mut route_range: Local<String>,
) {
    if let Some(ctx) = contexts.try_ctx_mut() {
        egui::Window::new("Galos").resizable(false).show(ctx, |ui| {
            ui.collapsing("Search", |ui| {
                search(ui, &mut searched, &mut system_name, &mut faction_name);
            });
            ui.collapsing("Settings", |ui| {
                settings(
                    ui,
                    &mut spyglass,
                    &mut view,
                    &mut color_by,
                    &mut population_scale,
                    &mut show_names,
                    &mut despawner,
                );
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

fn settings(
    ui: &mut Ui,
    spyglass: &mut ResMut<Spyglass>,
    view: &mut ResMut<View>,
    color_by: &mut ResMut<ColorBy>,
    population_scale: &mut ResMut<ScalePopulation>,
    show_names: &mut ResMut<ShowNames>,
    despawner: &mut EventWriter<Despawn>,
) {
    // TODO: IDK why this is necessary, the groups should fill the correct
    // size, no?
    ui.set_width(125.);

    ui.group(|ui| {
        ui.add(
            egui::Slider::new(&mut spyglass.radius, 10.0..=25000.0)
                .logarithmic(true)
                .drag_value_speed(0.1)
                .text("Radius"),
        );
        ui.add_space(2.);
        ui.checkbox(&mut spyglass.fetch, "Load Systems from DB");
        ui.add_space(2.);
        ui.checkbox(&mut spyglass.filter, "Spyglass Filter");
        ui.add_space(2.);
        if ui.button("Despawn Systems").clicked() {
            despawner.send(Despawn);
        }
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
    start: &mut Local<String>,
    end: &mut Local<String>,
    range: &mut Local<String>,
) {
    ui.label("Start: ");
    ui.add_sized(
        egui::vec2(125., 0.),
        egui::TextEdit::singleline(&mut **start),
    );
    ui.add_space(2.);
    ui.label("End: ");
    ui.add_sized(egui::vec2(125., 0.), egui::TextEdit::singleline(&mut **end));
    ui.add_space(2.);
    ui.label("Range (Ly): ");
    ui.add_sized(egui::vec2(50., 0.), egui::TextEdit::singleline(&mut **range));
    ui.add_space(5.);
    if ui.button("Plot Route...").clicked() {
        events.send(Searched::Route {
            start: start.clone(),
            end: end.clone(),
            range: range.parse().unwrap_or("10".into()),
        });
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
