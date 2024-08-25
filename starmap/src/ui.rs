use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::Searched;
use crate::systems::AlwaysDespawn;
use crate::SystemMarker;

// TODO: Form validation.

pub fn settings(
    systems_query: Query<Entity, With<SystemMarker>>,
    mut commands: Commands,
    mut contexts: EguiContexts,
    mut always_despawn: ResMut<AlwaysDespawn>,
) {
    egui::Window::new("Settings")
        .fixed_size([150.,0.])
        .show(contexts.ctx_mut(), |ui|
    {
        let last_value = always_despawn.0;
        ui.checkbox(&mut always_despawn.0, "Always Despawn Systems");
        if always_despawn.0 && always_despawn.0 != last_value {
            for entity in systems_query.iter() {
                commands.entity(entity).despawn_recursive();
            }
        }
    });
}

/// A basic GUI for searching for and generating the appropriate star systems.
pub fn search(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut system_name: Local<String>,
    mut faction_name: Local<String>,
) {
    egui::Window::new("System Search")
        .fixed_size([150.,0.])
        .show(contexts.ctx_mut(), |ui|
    {
        ui.horizontal(|ui| {
            ui.label("System: ");
            let response = ui.text_edit_singleline(&mut *system_name);
            if response.lost_focus() &&
               ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                *faction_name = "".into();
                events.send(Searched::System {
                    name: system_name.clone(),
                });
            }
        });

        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Faction: ");
            let response = ui.text_edit_singleline(&mut *faction_name);
            if response.lost_focus() &&
               ui.input(|i| i.key_pressed(egui::Key::Enter))
            {
                *system_name = "".into();
                events.send(Searched::Faction {
                    name: faction_name.clone(),
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

pub fn route(
    mut events: EventWriter<Searched>,
    mut contexts: EguiContexts,
    mut start: Local<String>,
    mut end: Local<String>,
    mut range: Local<String>,
) {
    egui::Window::new("Route")
        .fixed_size([150.,0.])
        .show(contexts.ctx_mut(), |ui|
    {
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
