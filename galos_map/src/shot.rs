//! Throwaway: a scripted capture, so a view can be looked at without a human
//! at the window.
//!
//! ```sh
//! GALOS_SHOT=/tmp/shot.png GALOS_SHOT_BACK=30000 \
//!   cargo run --release -p galos_map -- -i .index/full
//! ```

use crate::camera::OrbitCamera;
use crate::systems::System;
use bevy::app::AppExit;
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, save_to_disk};

#[derive(Resource)]
struct Shot {
    path: String,
    at: DVec3,
    back: f32,
    wait: u32,
    frame: u32,
    /// Which presentation to capture: the realistic sky where `GALOS_SHOT_VIEW`
    /// says `real`, the political map otherwise.
    real: bool,
    /// The realistic sky's exposure, in stops, from `GALOS_SHOT_EXPOSURE`.
    stops: f32,
}

pub fn plugin(app: &mut App) {
    let Ok(path) = std::env::var("GALOS_SHOT") else { return };
    let number = |key: &str, fallback: f64| {
        std::env::var(key)
            .ok()
            .and_then(|it| it.parse::<f64>().ok())
            .unwrap_or(fallback)
    };
    app.insert_resource(Shot {
        path,
        at: DVec3::new(
            number("GALOS_SHOT_X", 0.),
            number("GALOS_SHOT_Y", 0.),
            number("GALOS_SHOT_Z", 25_900.),
        ),
        back: number("GALOS_SHOT_BACK", 30_000.) as f32,
        wait: number("GALOS_SHOT_WAIT", 900.) as u32,
        frame: 0,
        stops: number("GALOS_SHOT_EXPOSURE", 0.) as f32,
        real: std::env::var("GALOS_SHOT_VIEW").as_deref() == Ok("real"),
    });
    app.add_systems(Update, capture);
}

fn capture(
    mut shot: ResMut<Shot>,
    mut cameras: Query<(&mut OrbitCamera, &Camera)>,
    systems: Query<&System>,
    mut view: ResMut<crate::systems::scale::View>,
    mut exposure: ResMut<crate::systems::spawn::StarExposure>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok((mut camera, lens)) = cameras.single_mut() else { return };
    let drawn = systems.iter().count();
    // The clock starts when the map has something on it: the loading screen
    // runs at a thousand frames a second and a frame count spent there is a
    // capture of nothing.
    if shot.frame == 0 && drawn == 0 {
        return;
    }
    shot.frame += 1;
    // Held rather than set once, as the pose is: nothing else writes it, but
    // the resource is only there once the map is up.
    let wanted = if shot.real {
        crate::systems::scale::View::Realistic
    } else {
        crate::systems::scale::View::Map
    };
    if *view != wanted {
        *view = wanted;
    }
    if exposure.0 != shot.stops {
        exposure.0 = shot.stops;
    }
    // Held every frame rather than set once: the controls smooth toward a
    // target and anything else touching the pose would drift it.
    if shot.frame <= shot.wait {
        let (at, back) = (shot.at, shot.back);
        camera.holds(at, back);
    }
    if shot.frame % 120 == 0 {
        info!(
            "shot: frame {} of {}, {} systems drawn",
            shot.frame,
            shot.wait,
            systems.iter().count()
        );
    }
    if shot.frame == shot.wait {
        info!(
            "shot: capturing {} systems to {}",
            systems.iter().count(),
            shot.path
        );
        if let Some(seen) = crate::systems::aggregate::view(&camera, lens) {
            info!(
                "shot: eye {:?} forward {:?} up {:?} fov_y {} height {} \
                 aspect {}",
                seen.eye,
                seen.forward,
                seen.up,
                seen.fov_y,
                seen.viewport_height,
                seen.aspect
            );
        }
        // The drawn set itself beside the frame, so the marks can be counted
        // and profiled rather than eyeballed off a screenshot.
        let mut dump = String::new();
        for system in systems.iter() {
            let at = system.position();
            dump.push_str(&format!(
                "{} {} {} {}\n",
                at.x,
                at.y,
                at.z,
                system.absolute_magnitude()
            ));
        }
        let _ = std::fs::write(format!("{}.marks", shot.path), dump);
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(shot.path.clone()));
    }
    if shot.frame == shot.wait + 60 {
        exit.write(AppExit::Success);
    }
}
