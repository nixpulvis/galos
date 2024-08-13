//! Shows how to iterate over combinations of query results.

use bevy::{color::palettes::css::ORANGE_RED, prelude::*};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use galos_db::Database;
use galos_db::systems::System;
use async_std::task;

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
    // Position our camera using our component,
    // not Transform (it would get overwritten)
    camera.state.center = Vec3::new(1.0, 2.0, 3.0);
    camera.state.radius = 50.0;
    camera.state.pitch = 15.0f32.to_radians();
    camera.state.yaw = 30.0f32.to_radians();
    commands.spawn(camera);
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
        if any || state.is_added() {
            // YXZ Euler Rotation performs yaw/pitch/roll.
            transform.rotation =
                Quat::from_euler(EulerRot::YXZ, state.yaw, state.pitch, 0.0);
            // To position the camera, get the backward direction vector
            // and place the camera at the desired radius from the center.
            transform.translation = state.center + transform.back() * state.radius;
        }
    }
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .insert_resource(ClearColor(Color::BLACK))
        .add_systems(Startup, generate_bodies)
        .add_systems(Startup, spawn_camera)
        // .add_systems(FixedUpdate, (interact_bodies, integrate))
        // .add_systems(Update, look_at_star)
        .add_systems(Update, pan_orbit_camera
            .run_if(any_with_component::<PanOrbitState>))
        .run();
}

const GRAVITY_CONSTANT: f32 = 0.001;
const NUM_BODIES: usize = 100;

#[derive(Component, Default)]
struct Mass(f32);
#[derive(Component, Default)]
struct Acceleration(Vec3);
#[derive(Component, Default)]
struct LastPos(Vec3);
#[derive(Component)]
struct Star;

#[derive(Bundle, Default)]
struct BodyBundle {
    pbr: PbrBundle,
    mass: Mass,
    last_pos: LastPos,
    acceleration: Acceleration,
}

fn generate_bodies(
    time: Res<Time<Fixed>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mesh = meshes.add(Sphere::new(1.0).mesh().ico(3).unwrap());

    let color_range = 0.5..1.0;
    let vel_range = -0.5..0.5;

    // We're seeding the PRNG here to make this example deterministic for testing purposes.
    // This isn't strictly required in practical use unless you need your app to be deterministic.
    let mut rng = ChaCha8Rng::seed_from_u64(19878367467713);

    let systems = task::block_on(async {
        let db = Database::new().await.unwrap();
        System::fetch_in_range_by_name(&db, 500., "Sol").await.unwrap()
    });

    for system in systems {
        let radius: f32 = 0.25;
        let mass_value = radius.powi(3) * 10.;

        let position = Vec3::new(
            system.position.unwrap().x as f32,
            system.position.unwrap().y as f32,
            system.position.unwrap().z as f32,
        );
        // .normalize()
        //     * rng.gen_range(0.2f32..1.0).cbrt()
        //     * 15.;

        commands.spawn(BodyBundle {
            pbr: PbrBundle {
                transform: Transform {
                    translation: position,
                    scale: Vec3::splat(radius),
                    ..default()
                },
                mesh: mesh.clone(),
                material: materials.add(Color::srgb(
                    rng.gen_range(color_range.clone()),
                    rng.gen_range(color_range.clone()),
                    rng.gen_range(color_range.clone()),
                )),
                ..default()
            },
            mass: Mass(mass_value),
            acceleration: Acceleration(Vec3::ZERO),
            last_pos: LastPos(
                position
                    - Vec3::new(
                        rng.gen_range(vel_range.clone()),
                        rng.gen_range(vel_range.clone()),
                        rng.gen_range(vel_range.clone()),
                    ) * time.timestep().as_secs_f32(),
            ),
        });
    }

    // add bigger "star" body in the center
    let star_radius = 1.;
    commands
        .spawn((
            BodyBundle {
                pbr: PbrBundle {
                    transform: Transform::from_scale(Vec3::splat(star_radius)),
                    mesh: meshes.add(Sphere::new(1.0).mesh().ico(5).unwrap()),
                    material: materials.add(StandardMaterial {
                        base_color: ORANGE_RED.into(),
                        emissive: LinearRgba::from(ORANGE_RED) * 2.,
                        ..default()
                    }),
                    ..default()
                },
                mass: Mass(500.0),
                ..default()
            },
            Star,
        ))
        .with_children(|p| {
            p.spawn(PointLightBundle {
                point_light: PointLight {
                    color: Color::WHITE,
                    range: 100.0,
                    radius: star_radius,
                    ..default()
                },
                ..default()
            });
        });
    // commands.spawn(Camera3dBundle {
    //     transform: Transform::from_xyz(0.0, 10.5, -30.0).looking_at(Vec3::ZERO, Vec3::Y),
    //     ..default()
    // });
}

fn interact_bodies(mut query: Query<(&Mass, &GlobalTransform, &mut Acceleration)>) {
    let mut iter = query.iter_combinations_mut();
    while let Some([(Mass(m1), transform1, mut acc1), (Mass(m2), transform2, mut acc2)]) =
        iter.fetch_next()
    {
        let delta = transform2.translation() - transform1.translation();
        let distance_sq: f32 = delta.length_squared();

        let f = GRAVITY_CONSTANT / distance_sq;
        let force_unit_mass = delta * f;
        acc1.0 += force_unit_mass * *m2;
        acc2.0 -= force_unit_mass * *m1;
    }
}

fn integrate(time: Res<Time>, mut query: Query<(&mut Acceleration, &mut Transform, &mut LastPos)>) {
    let dt_sq = time.delta_seconds() * time.delta_seconds();
    for (mut acceleration, mut transform, mut last_pos) in &mut query {
        // verlet integration
        // x(t+dt) = 2x(t) - x(t-dt) + a(t)dt^2 + O(dt^4)

        let new_pos = transform.translation * 2.0 - last_pos.0 + acceleration.0 * dt_sq;
        acceleration.0 = Vec3::ZERO;
        last_pos.0 = transform.translation;
        transform.translation = new_pos;
    }
}

// fn look_at_star(
//     mut camera: Query<&mut Transform, (With<Camera>, Without<Star>)>,
//     star: Query<&Transform, With<Star>>,
// ) {
//     let mut camera = camera.single_mut();
//     let star = star.single();
//     let new_rotation = camera
//         .looking_at(star.translation, Vec3::Y)
//         .rotation
//         .lerp(camera.rotation, 0.1);
//     camera.rotation = new_rotation;
// }
