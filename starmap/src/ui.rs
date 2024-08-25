use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::Searched;

// TODO: Form validation.

/// A basic GUI for searching for and generating the appropriate star systems.
pub fn systems_search(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut search_name: Local<String>,
) {
    egui::Window::new("System Search")
        .default_size([150.,0.])
        .show(contexts.ctx_mut(), |ui|
    {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            let response = ui.text_edit_singleline(&mut *search_name);
            if response.lost_focus() &&
               ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                events.send(Searched::System {
                    name: search_name.clone(),
                });
            }
        });


        // TODO: This should be a slider I think. That way we can provide
        // a reasonable range.
        // ui.horizontal(|ui| {
        //     ui.label("Radius (Ly): ");
        //     ui.text_edit_singleline(&mut *search_radius);
        // });
    });
}

pub fn faction_search(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut faction_name: Local<String>,
) {
    egui::Window::new("Faction System Search").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            ui.text_edit_singleline(&mut *faction_name);
        });
        if ui.button("Search").clicked() {
            events.send(Searched::Faction {
                name: faction_name.clone(),
            });
        }
    });
}

pub fn route_search(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut start: Local<String>,
    mut end: Local<String>,
    mut range: Local<String>,
) {
    egui::Window::new("Route").show(contexts.ctx_mut(), |ui| {
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
        if ui.button("Search").clicked() {
            events.send(Searched::Route {
                start: start.clone(),
                end: end.clone(),
                range: range.parse().unwrap_or("10".into())
            });
        }
    });
}
