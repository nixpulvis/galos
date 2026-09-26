//! Throwaway: a scripted capture, so a view can be looked at without a human
//! at the window.
//!
//! ```sh
//! GALOS_SHOT=/tmp/shot.png GALOS_SHOT_BACK=30000 \
//!   cargo run --release -p galos_map -- -i .index/full
//! ```

use crate::map::camera::OrbitCamera;
use crate::map::galaxy::System;
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
    /// Where the camera is aimed from, in radians about the point it holds:
    /// `GALOS_SHOT_YAW` and `GALOS_SHOT_PITCH`. `holds` stands the eye due
    /// +Z of its target, which sees one direction of sky; a capture of
    /// anything else has to say which way to look.
    yaw: f32,
    pitch: f32,
    /// The reach to hold the spyglass at, in light years, from
    /// `GALOS_SHOT_REACH`. Zero leaves the spyglass as the map sets it,
    /// which is a sphere about the look-at point sized off the zoom.
    reach: f32,
    /// Radians of yaw a frame, from `GALOS_SHOT_SPIN`: a turning camera, so
    /// what the painted set does under rotation can be counted rather than
    /// watched.
    spin: f32,
    /// Fraction of the distance back added a frame, from `GALOS_SHOT_ZOOM`.
    zoom: f32,
    /// Light years a frame the held point slides along x, from
    /// `GALOS_SHOT_PAN`.
    pan: f32,
    /// What was drawn last frame, for that count.
    last: std::collections::HashSet<u64>,
    /// What has left lately and when, so a star that comes back can be told
    /// from one that merely went.
    gone: std::collections::HashMap<u64, u32>,
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
        yaw: number("GALOS_SHOT_YAW", 0.) as f32,
        pitch: number("GALOS_SHOT_PITCH", 0.) as f32,
        reach: number("GALOS_SHOT_REACH", 0.) as f32,
        spin: number("GALOS_SHOT_SPIN", 0.) as f32,
        zoom: number("GALOS_SHOT_ZOOM", 0.) as f32,
        pan: number("GALOS_SHOT_PAN", 0.) as f32,
        last: std::collections::HashSet::new(),
        gone: std::collections::HashMap::new(),
        real: std::env::var("GALOS_SHOT_VIEW").as_deref() == Ok("real"),
    });
    // After the field is built, so what is counted is what was painted this
    // frame and not what the last one left behind.
    app.add_systems(
        Update,
        capture.after(crate::map::paint::field::build_field),
    );
}

fn capture(
    mut shot: ResMut<Shot>,
    mut cameras: Query<(&mut OrbitCamera, &Camera)>,
    systems: Query<(
        &System,
        &Visibility,
        &crate::map::paint::sizing::Drawn,
        &crate::map::bodies::spawn::Strength,
    )>,
    mut view: ResMut<crate::map::paint::sizing::View>,
    mut exposure: ResMut<crate::map::galaxy::spawn::StarExposure>,
    mut spyglass: ResMut<crate::map::galaxy::Spyglass>,
    blobs: Res<crate::map::galaxy::walk::Blobs>,
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
        crate::map::paint::sizing::View::Realistic
    } else {
        crate::map::paint::sizing::View::Map
    };
    if *view != wanted {
        *view = wanted;
    }
    if exposure.0 != shot.stops {
        exposure.0 = shot.stops;
    }
    // A reach the run was told to hold, rather than the one the zoom sets.
    if shot.reach > 0. {
        spyglass.follow_camera = false;
        if spyglass.radius != shot.reach {
            spyglass.radius = shot.reach;
        }
    }
    // Held every frame rather than set once: the controls smooth toward a
    // target and anything else touching the pose would drift it.
    //
    // Three gestures, since the three are reported to flicker differently:
    // `SPIN` turns, `ZOOM` pulls back by a fraction a frame, `PAN` slides the
    // point held along x.
    if shot.frame <= shot.wait {
        let step = shot.frame as f32;
        let back = shot.back * (1. + shot.zoom).powf(step);
        let at = shot.at + DVec3::X * f64::from(shot.pan * step);
        let yaw = shot.yaw + shot.spin * step;
        camera.holds(at, back);
        camera.yaw = yaw;
        camera.target_yaw = yaw;
        camera.pitch = shot.pitch;
        camera.target_pitch = shot.pitch;
    }
    // **What the field painted**, which is not what the map holds. A system
    // keeps its entity while the spyglass hides it and while its mark shrinks
    // under the floor, and either of those is a star that was on the screen
    // last frame and is not on it now — a blink the entity count cannot see.
    // So this asks exactly what `build_field` asks: visible, and a radius the
    // view does not drop. See `field::drawn_radius`.
    if shot.spin != 0.
        || shot.zoom != 0.
        || shot.pan != 0.
        || std::env::var("GALOS_SHOT_CHURN").is_ok()
    {
        let floor = crate::map::paint::field::floor(false);
        let cot = lens.clip_from_view().y_axis.y;
        let height =
            lens.logical_viewport_size().unwrap_or(Vec2::splat(720.)).y;
        let mut hidden = 0usize;
        let mut sub_floor = 0usize;
        let mut now: std::collections::HashSet<u64> =
            std::collections::HashSet::new();
        for (system, visibility, drawn, strength) in systems.iter() {
            let at = system.position();
            let key = at.x.to_bits()
                ^ at.y.to_bits().rotate_left(21)
                ^ at.z.to_bits().rotate_left(42);
            if *visibility == Visibility::Hidden {
                hidden += 1;
                continue;
            }
            let away =
                crate::map::space::metres(camera.eye_from(at)).length() as f32;
            let per_pixel =
                crate::map::screen::world_per_pixel(cot, height, away.max(1.));
            if crate::map::paint::field::drawn_radius(
                &view, drawn.0, per_pixel, floor,
            )
            .is_none()
                || strength.0 <= 0.
            {
                sub_floor += 1;
                continue;
            }
            now.insert(key);
        }
        let left = shot.last.difference(&now).count();
        let arrived = now.difference(&shot.last).count();
        // A star that left and came back is the blink; one that only left is
        // the sky genuinely receding as the eye travels.
        let returned =
            now.iter().filter(|it| shot.gone.contains_key(it)).count();
        let frame = shot.frame;
        let went: Vec<u64> = shot.last.difference(&now).copied().collect();
        for it in went {
            shot.gone.insert(it, frame);
        }
        shot.gone.retain(|it, at| frame - *at < 120 && !now.contains(it));
        if shot.frame > 1 && (left > 0 || arrived > 0) {
            info!(
                "shot: frame {} churn: {} painted, {left} left, {arrived} \
                 arrived, {returned} returned ({hidden} hidden, \
                 {sub_floor} under the floor)",
                shot.frame,
                now.len()
            );
        }
        shot.last = now;
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
            "shot: capturing {} systems and {} merged marks to {}",
            systems.iter().count(),
            blobs.0.len(),
            shot.path
        );
        if let Some(seen) = crate::map::galaxy::plan::view(&camera, lens) {
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
        for (system, ..) in systems.iter() {
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
