//! Shows how to iterate over combinations of query results.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts, EguiPlugin};
use galos_db::Database;
use galos_db::systems::System;
use elite_journal::Allegiance;
use async_std::task;

#[derive(Resource, Default)]
struct SystemsSearch {
    name: String,
}

#[derive(Component)]
struct SystemMarker;

#[derive(Resource)]
struct FocusSystem(System);

// Bundle to spawn our custom camera easily
#[derive(Bundle, Default)]
pub struct PanOrbitCameraBundle {
    pub camera: Camera3dBundle,
    pub state: PanOrbitState,
    pub settings: PanOrbitSettings,
}

// The internal state of the pan-orbit controller
#[derive(Component)]
pub struct PanOrbitState {
    pub center: Vec3,
    pub radius: f32,
    pub upside_down: bool,
    pub pitch: f32,
    pub yaw: f32,
}

/// The configuration of the pan-orbit controller
#[derive(Component)]
pub struct PanOrbitSettings {
    /// World units per pixel of mouse motion
    pub pan_sensitivity: f32,
    /// Radians per pixel of mouse motion
    pub orbit_sensitivity: f32,
    /// Exponent per pixel of mouse motion
    pub zoom_sensitivity: f32,
    /// Key to hold for panning
    pub pan_key: Option<KeyCode>,
    /// Key to hold for orbiting
    pub orbit_key: Option<KeyCode>,
    /// Key to hold for zooming
    pub zoom_key: Option<KeyCode>,
    /// What action is bound to the scroll wheel?
    pub scroll_action: Option<PanOrbitAction>,
    /// For devices with a notched scroll wheel, like desktop mice
    pub scroll_line_sensitivity: f32,
    /// For devices with smooth scrolling, like touchpads
    pub scroll_pixel_sensitivity: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanOrbitAction {
    Pan,
    Orbit,
    Zoom,
}

impl Default for PanOrbitState {
    fn default() -> Self {
        PanOrbitState {
            center: Vec3::ZERO,
            radius: 1.0,
            upside_down: false,
            pitch: 0.0,
            yaw: 0.0,
        }
    }
}

impl Default for PanOrbitSettings {
    fn default() -> Self {
        PanOrbitSettings {
            pan_sensitivity: 0.001, // 1000 pixels per world unit
            orbit_sensitivity: 0.1f32.to_radians(), // 0.1 degree per pixel
            zoom_sensitivity: 0.01,
            pan_key: Some(KeyCode::ControlLeft),
            orbit_key: Some(KeyCode::AltLeft),
            zoom_key: Some(KeyCode::ShiftLeft),
            scroll_action: Some(PanOrbitAction::Zoom),
            scroll_line_sensitivity: 16.0, // 1 "line" == 16 "pixels of motion"
            scroll_pixel_sensitivity: 1.0,
        }
    }
}

fn spawn_camera(mut commands: Commands) {
    let mut camera = PanOrbitCameraBundle::default();
    camera.state.center = Vec3::new(0.0, 0.0, 0.0);
    camera.state.radius = 50.0;
    camera.state.pitch = 15.0f32.to_radians();
    camera.state.yaw = 30.0f32.to_radians();
    commands.spawn(camera);
}

fn move_camera(mut query: Query<&mut PanOrbitState>, position: Vec3) {
    let mut state = query.single_mut();
    state.center = position;
}


use bevy::input::mouse::{MouseMotion, MouseScrollUnit, MouseWheel};

use std::f32::consts::{FRAC_PI_2, PI, TAU};

fn pan_orbit_camera(
    kbd: Res<ButtonInput<KeyCode>>,
    mut evr_motion: EventReader<MouseMotion>,
    mut evr_scroll: EventReader<MouseWheel>,
    mut q_camera: Query<(
        &PanOrbitSettings,
        &mut PanOrbitState,
        &mut Transform,
    )>,
) {
    let (settings, state, transform) = &q_camera.single();
    // First, accumulate the total amount of
    // mouse motion and scroll, from all pending events:
    let mut total_motion: Vec2 = evr_motion.read()
        .map(|ev| ev.delta).sum();

    // Reverse Y (Bevy's Worldspace coordinate system is Y-Up,
    // but events are in window/ui coordinates, which are Y-Down)
    total_motion.y = -total_motion.y;

    let mut total_scroll_lines = Vec2::ZERO;
    let mut total_scroll_pixels = Vec2::ZERO;
    for ev in evr_scroll.read() {
        match ev.unit {
            MouseScrollUnit::Line => {
                total_scroll_lines.x += ev.x;
                total_scroll_lines.y -= ev.y;
            }
            MouseScrollUnit::Pixel => {
                total_scroll_pixels.x += ev.x;
                total_scroll_pixels.y -= ev.y;
            }
        }
    }

    for (settings, mut state, mut transform) in &mut q_camera {
        // Check how much of each thing we need to apply.
        // Accumulate values from motion and scroll,
        // based on our configuration settings.

        let mut total_pan = Vec2::ZERO;
        if settings.pan_key.map(|key| kbd.pressed(key)).unwrap_or(false) {
            total_pan -= total_motion * settings.pan_sensitivity;
        }
        if settings.scroll_action == Some(PanOrbitAction::Pan) {
            total_pan -= total_scroll_lines
                * settings.scroll_line_sensitivity * settings.pan_sensitivity;
            total_pan -= total_scroll_pixels
                * settings.scroll_pixel_sensitivity * settings.pan_sensitivity;
        }

        let mut total_orbit = Vec2::ZERO;
        if settings.orbit_key.map(|key| kbd.pressed(key)).unwrap_or(false) {
            total_orbit -= total_motion * settings.orbit_sensitivity;
        }
        if settings.scroll_action == Some(PanOrbitAction::Orbit) {
            total_orbit -= total_scroll_lines
                * settings.scroll_line_sensitivity * settings.orbit_sensitivity;
            total_orbit -= total_scroll_pixels
                * settings.scroll_pixel_sensitivity * settings.orbit_sensitivity;
        }

        let mut total_zoom = Vec2::ZERO;
        if settings.zoom_key.map(|key| kbd.pressed(key)).unwrap_or(false) {
            total_zoom -= total_motion * settings.zoom_sensitivity;
        }
        if settings.scroll_action == Some(PanOrbitAction::Zoom) {
            total_zoom -= total_scroll_lines
                * settings.scroll_line_sensitivity * settings.zoom_sensitivity;
            total_zoom -= total_scroll_pixels
                * settings.scroll_pixel_sensitivity * settings.zoom_sensitivity;
        }

        // Upon starting a new orbit maneuver (key is just pressed),
        // check if we are starting it upside-down
        if settings.orbit_key.map(|key| kbd.just_pressed(key)).unwrap_or(false) {
            state.upside_down = state.pitch < -FRAC_PI_2 || state.pitch > FRAC_PI_2;
        }

        // If we are upside down, reverse the X orbiting
        if state.upside_down {
            total_orbit.x = -total_orbit.x;
        }

        // Now we can actually do the things!

        let mut any = false;

        // To ZOOM, we need to multiply our radius.
        if total_zoom != Vec2::ZERO {
            any = true;
            // in order for zoom to feel intuitive,
            // everything needs to be exponential
            // (done via multiplication)
            // not linear
            // (done via addition)

            // so we compute the exponential of our
            // accumulated value and multiply by that
            state.radius *= (-total_zoom.y).exp();
        }

        // To ORBIT, we change our pitch and yaw values
        if total_orbit != Vec2::ZERO {
            any = true;
            state.yaw += total_orbit.x;
            state.pitch += total_orbit.y;
            // wrap around, to stay between +- 180 degrees
            if state.yaw > PI {
                state.yaw -= TAU; // 2 * PI
            }
            if state.yaw < -PI {
                state.yaw += TAU; // 2 * PI
            }
            if state.pitch > PI {
                state.pitch -= TAU; // 2 * PI
            }
            if state.pitch < -PI {
                state.pitch += TAU; // 2 * PI
            }
        }

        // To PAN, we can get the UP and RIGHT direction
        // vectors from the camera's transform, and use
        // them to move the center point. Multiply by the
        // radius to make the pan adapt to the current zoom.
        if total_pan != Vec2::ZERO {
            any = true;
            let radius = state.radius;
            state.center += transform.right() * total_pan.x * radius;
            state.center += transform.up() * total_pan.y * radius;
        }

        // Finally, compute the new camera transform.
        // (if we changed anything, or if the pan-orbit
        // controller was just added and thus we are running
        // for the first time and need to initialize)
        // if any || state.is_added() {
            // YXZ Euler Rotation performs yaw/pitch/roll.
            transform.rotation =
                Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.0);
            // To position the camera, get the backward direction vector
            // and place the camera at the desired radius from the center.
            transform.translation = state.center + transform.back() * state.radius;
        // }
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(EguiPlugin)
        .insert_resource(ClearColor(Color::BLACK))
        .insert_resource(AmbientLight {
            color: Color::default(),
            brightness: 1000.0,
        })
        .init_resource::<SystemsSearch>()
        .add_systems(Startup, generate_bodies)
        .add_systems(Startup, spawn_camera)
        .add_systems(Update, systems_search_ui)
        .add_systems(Update, pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .run();
}

fn systems_search_ui(
    systems_query: Query<Entity, With<SystemMarker>>,
    camera_query: Query<&mut PanOrbitState>,
    mut search: ResMut<SystemsSearch>,
    mut contexts: EguiContexts,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    egui::Window::new("Systems Search").show(contexts.ctx_mut(), |ui| {
        ui.horizontal(|ui| {
            ui.label("Name: ");
            let response = ui.text_edit_singleline(&mut search.name);
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                for entity in systems_query.iter() {
                    commands.entity(entity).despawn_recursive();
                }
                generate_bodies(camera_query, search.into(), commands, meshes, materials);
            }
        });
    });
}

fn generate_bodies(
    camera_query: Query<&mut PanOrbitState>,
    search: Res<SystemsSearch>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());

    let systems = task::block_on(async {
        let db = Database::new().await.unwrap();
        match System::fetch_in_range_like_name(&db, 100., &search.name).await {
            Ok(systems) if !systems.is_empty() => {
                let origins = System::fetch_like_name(&db, &search.name).await.unwrap();
                let origin = origins.first().unwrap();
                let position = Vec3::new(
                    origin.position.unwrap().x as f32,
                    origin.position.unwrap().y as f32,
                    origin.position.unwrap().z as f32,
                );
                move_camera(camera_query, position);
                systems
            },
            _ => vec![],
        }
    });

    for system in systems {
        let radius: f32 = 0.25;

        let position = Vec3::new(
            system.position.unwrap().x as f32,
            system.position.unwrap().y as f32,
            system.position.unwrap().z as f32,
        );

        commands.spawn((PbrBundle {
            transform: Transform {
                translation: position,
                scale: Vec3::splat(radius),
                ..default()
            },
            mesh: mesh.clone(),
            material: materials.add(system_color(&system)),
            ..default()
        }, SystemMarker));
    }
}

fn system_color(system: &System) -> Color {
    match system.allegiance {
        Some(Allegiance::Alliance)    => Color::srgb(0., 1., 0.),
        Some(Allegiance::Empire)      => Color::srgb(0., 0., 1.),
        Some(Allegiance::Federation)  => Color::srgb(1., 0., 0.),
        Some(Allegiance::Independent) => Color::srgb(1., 1., 0.),
        Some(_)                       => Color::srgb(1., 1., 1.),
        None                          => Color::srgb(0., 0., 0.),
    }
}
