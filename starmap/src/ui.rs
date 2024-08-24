use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::{SystemsSearched, SystemMarker};

/// A basic GUI for searching for and generating the appropriate star systems.
pub fn systems_search(
    mut events: EventWriter<SystemsSearched>,
    systems_query: Query<Entity, With<SystemMarker>>,
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut search_name: Local<String>,
    mut search_radius: Local<String>,
) {
    egui::Window::new("Systems Search").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            ui.text_edit_singleline(&mut *search_name);
        });
        ui.horizontal(|ui| {
            ui.label("Radius (Ly): ");
            ui.text_edit_singleline(&mut *search_radius);
        });
        if ui.button("Search").clicked() {
            for entity in systems_query.iter() {
                commands.entity(entity).despawn_recursive();
            }
            events.send(SystemsSearched {
                name: search_name.clone(),
                radius: search_radius.clone(),
            });
        }
    });
}
