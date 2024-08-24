use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::{Searched, SystemMarker};

/// A basic GUI for searching for and generating the appropriate star systems.
pub fn systems_search(
    mut events: EventWriter<Searched>,
    systems_query: Query<Entity, With<SystemMarker>>,
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut search_name: Local<String>,
    mut search_radius: Local<String>,
) {
    egui::Window::new("System Search").show(contexts.ctx_mut(), |ui| {
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
            events.send(Searched::System {
                name: search_name.clone(),
                radius: search_radius.clone(),
            });
        }
    });
}


pub fn faction_search(
    mut events: EventWriter<Searched>,
    systems_query: Query<Entity, With<SystemMarker>>,
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut faction_name: Local<String>,
) {
    egui::Window::new("Faction System Search").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            ui.text_edit_singleline(&mut *faction_name);
        });
        if ui.button("Search").clicked() {
            for entity in systems_query.iter() {
                commands.entity(entity).despawn_recursive();
            }
            events.send(Searched::Faction {
                name: faction_name.clone(),
            });
        }
    });
}
