use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};
use crate::{SystemsSearch, SystemMarker};
use crate::camera::PanOrbitState;
use crate::generate;

pub fn systems_search(
    systems_query: Query<Entity, With<SystemMarker>>,
    camera_query: Query<&mut PanOrbitState>,
    mut search: ResMut<SystemsSearch>,
    mut contexts: EguiContexts,
    mut commands: Commands,
    meshes: ResMut<Assets<Mesh>>,
    materials: ResMut<Assets<StandardMaterial>>,
) {
    egui::Window::new("Systems Search").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            ui.text_edit_singleline(&mut search.name);
        });
        ui.horizontal(|ui| {
            ui.label("Radius (Ly): ");
            ui.text_edit_singleline(&mut search.radius);
        });
        if ui.button("Search").clicked() {
            for entity in systems_query.iter() {
                commands.entity(entity).despawn_recursive();
            }
            generate::bodies(camera_query, search.into(), commands, meshes, materials);
        }
    });
}
