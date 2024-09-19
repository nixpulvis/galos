use crate::search::Searched;
use crate::systems::{ColorBy, ScalePopulation, ShowNames, Spyglass, View};
use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

// TODO: Form validation.

/// Global settings for the map
pub fn settings(
    mut contexts: EguiContexts,
    mut spyglass: ResMut<Spyglass>,
    mut view: ResMut<View>,
    mut color_by: ResMut<ColorBy>,
    mut population_scale: ResMut<ScalePopulation>,
    mut show_names: ResMut<ShowNames>,
) {
    if let Some(ctx) = contexts.try_ctx_mut() {
        // TODO: We really need to figure out how to trigger a re-fetch after
        // these settings change in a clean way.
        // I suspect I'll make a new SettingsChanged event or something, but
        // bevy Observers may also help, I just need to learn about them.
        egui::Window::new("Settings").fixed_size([150., 0.]).show(ctx, |ui| {
            ui.checkbox(&mut spyglass.fetch, "Always Fetch Systems");
            ui.add(
                egui::Slider::new(&mut spyglass.radius, 10.0..=25000.0)
                    .logarithmic(true)
                    .text("Radius"),
            );
            // ui.checkbox(&mut spyglass.filter, "Hide Systems Outside Spyglass");

            ui.radio_value(&mut *view, View::Systems, "Systems View");
            ui.radio_value(&mut *view, View::Stars, "Stars View");

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
                    ui.checkbox(&mut population_scale.0, "Scale w/ Population");
                }
                View::Stars => {}
            }

            ui.checkbox(&mut show_names.0, "Show System Names");
        });
    }
}

/// Star system search UI by name or faction
pub fn search(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut system_name: Local<String>,
    mut faction_name: Local<String>,
) {
    if let Some(ctx) = contexts.try_ctx_mut() {
        egui::Window::new("Search").fixed_size([150., 0.]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("System: ");
                let response = ui.text_edit_singleline(&mut *system_name);
                if response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    *faction_name = "".into();
                    events.send(Searched::System { name: system_name.clone() });
                }
            });

            ui.separator();

            ui.horizontal(|ui| {
                ui.label("Faction: ");
                let response = ui.text_edit_singleline(&mut *faction_name);
                if response.lost_focus()
                    && ui.input(|i| i.key_pressed(egui::Key::Enter))
                {
                    *system_name = "".into();
                    events
                        .send(Searched::Faction { name: faction_name.clone() });
                }
            });
        });
    }
}

/// Route finding UI for finding out how to get from A to B
pub fn route(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut start: Local<String>,
    mut end: Local<String>,
    mut range: Local<String>,
) {
    if let Some(ctx) = contexts.try_ctx_mut() {
        egui::Window::new("Route").fixed_size([150., 0.]).show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label("Start: ");
                ui.text_edit_singleline(&mut *start);
            });
            ui.horizontal(|ui| {
                ui.label("End: ");
                ui.text_edit_singleline(&mut *end);
            });
            ui.horizontal(|ui| {
                ui.label("Range: ");
                ui.text_edit_singleline(&mut *range);
            });
            if ui.button("Plot Route...").clicked() {
                events.send(Searched::Route {
                    start: start.clone(),
                    end: end.clone(),
                    range: range.parse().unwrap_or("10".into()),
                });
            }
        });
    }
}
