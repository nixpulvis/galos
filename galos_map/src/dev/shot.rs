//! A scripted camera and a capture, so a view can be looked at, and a flight
//! profiled or recorded, without a human at the window.
//!
//! `galos_map/profile.sh` flies its scenarios through this; see the README's
//! Profiling. `galos_map/media.sh` records the README's picture and animation
//! through it.
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
use bevy::window::PrimaryWindow;

#[derive(Resource)]
struct Shot {
    path: String,
    at: DVec3,
    back: f32,
    wait: u32,
    frame: u32,
    /// Frames the pose stands still before it starts to move, from
    /// `GALOS_SHOT_HOLD`: time for the first view to load, so a recording
    /// does not open on an empty sky filling in.
    hold: u32,
    /// Every how many frames of the motion to capture, from
    /// `GALOS_SHOT_EVERY`. Zero captures the one frame at `wait`; anything
    /// else makes `GALOS_SHOT` a directory and fills it with the motion as
    /// `0000.png`, `0001.png`, ... instead.
    every: u32,
    /// How many frames of the motion have been captured.
    taken: u32,
    /// Which presentation to capture: the realistic sky where `GALOS_SHOT_VIEW`
    /// says `real`, the political map otherwise.
    real: bool,
    /// Whether systems and bodies are named: not where `GALOS_SHOT_NAMES`
    /// says `off`.
    names: bool,
    /// Light years back inside which the names come on, from
    /// `GALOS_SHOT_NAMES_WITHIN`, in place of `names`: a flight that is
    /// named once it is close, and bare on the way. Zero leaves it to
    /// `names`.
    names_within: f32,
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
    /// which is a sphere about the look-at point sized off the zoom. On a
    /// flight it is the reach at the start, and the map's own by the end.
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
    /// Where a flight ends, from `GALOS_SHOT_TO_X`, `_TO_Y`, `_TO_Z`,
    /// `_TO_BACK`, `_TO_YAW` and `_TO_PITCH`: over the frames from `hold` to
    /// `wait` the camera goes from its pose to this one, in place of `ZOOM`
    /// and `PAN`. No flight where `to_back` is zero.
    to: DVec3,
    to_back: f32,
    to_yaw: f32,
    to_pitch: f32,
    /// What was drawn last frame, for that count.
    last: std::collections::HashSet<u64>,
    /// What has left lately and when, so a star that comes back can be told
    /// from one that merely went.
    gone: std::collections::HashMap<u64, u32>,
}

/// Where the camera stands on one frame.
struct Pose {
    /// The point it holds.
    at: DVec3,
    /// How far back of that point, in light years.
    back: f32,
    yaw: f32,
    pitch: f32,
    /// How far through a flight, eased: nought at the start, one at the end,
    /// and nought throughout where there is no flight.
    through: f32,
}

impl Shot {
    /// Where the camera stands this frame.
    fn pose(&self) -> Pose {
        let step = self.frame.min(self.wait).saturating_sub(self.hold);
        let yaw = self.yaw + self.spin * step as f32;
        if self.to_back <= 0. {
            let step = step as f32;
            return Pose {
                at: self.at + DVec3::X * f64::from(self.pan * step),
                back: self.back * (1. + self.zoom).powf(step),
                yaw,
                pitch: self.pitch,
                through: 0.,
            };
        }
        // Eased at both ends, and the distance back taken in ratios, so the
        // zoom looks as fast at the galaxy as at a star for the same ease.
        let frames = self.wait.saturating_sub(self.hold).max(1);
        let s = f64::from(step) / f64::from(frames);
        let s = s * s * (3. - 2. * s);
        let (from, to) = (f64::from(self.back), f64::from(self.to_back));
        let back = from * (to / from).powf(s);
        // The held point moves by as much as the distance back has closed,
        // which keeps where the flight ends at one place on the screen until
        // the last of the zoom brings it to the middle, rather than letting
        // it run off the edge on the way.
        let along = (from - back) / (from - to);
        Pose {
            at: self.at + (self.to - self.at) * along,
            back: back as f32,
            yaw: yaw + (self.to_yaw - self.yaw) * s as f32,
            pitch: self.pitch + (self.to_pitch - self.pitch) * s as f32,
            through: s as f32,
        }
    }
}

pub fn plugin(app: &mut App) {
    let Ok(path) = std::env::var("GALOS_SHOT") else { return };
    let number = |key: &str, fallback: f64| {
        std::env::var(key)
            .ok()
            .and_then(|it| it.parse::<f64>().ok())
            .unwrap_or(fallback)
    };
    let every = number("GALOS_SHOT_EVERY", 0.) as u32;
    if every > 0 {
        let _ = std::fs::create_dir_all(&path);
    }
    let at = DVec3::new(
        number("GALOS_SHOT_X", 0.),
        number("GALOS_SHOT_Y", 0.),
        number("GALOS_SHOT_Z", 25_900.),
    );
    let pitch = number("GALOS_SHOT_PITCH", 0.);
    let yaw = number("GALOS_SHOT_YAW", 0.);
    app.insert_resource(Shot {
        path,
        at,
        back: number("GALOS_SHOT_BACK", 30_000.) as f32,
        wait: number("GALOS_SHOT_WAIT", 900.) as u32,
        frame: 0,
        hold: number("GALOS_SHOT_HOLD", 0.) as u32,
        every,
        taken: 0,
        names: std::env::var("GALOS_SHOT_NAMES").as_deref() != Ok("off"),
        names_within: number("GALOS_SHOT_NAMES_WITHIN", 0.) as f32,
        stops: number("GALOS_SHOT_EXPOSURE", 0.) as f32,
        yaw: yaw as f32,
        pitch: pitch as f32,
        reach: number("GALOS_SHOT_REACH", 0.) as f32,
        spin: number("GALOS_SHOT_SPIN", 0.) as f32,
        zoom: number("GALOS_SHOT_ZOOM", 0.) as f32,
        pan: number("GALOS_SHOT_PAN", 0.) as f32,
        to: DVec3::new(
            number("GALOS_SHOT_TO_X", at.x),
            number("GALOS_SHOT_TO_Y", at.y),
            number("GALOS_SHOT_TO_Z", at.z),
        ),
        to_back: number("GALOS_SHOT_TO_BACK", 0.) as f32,
        to_yaw: number("GALOS_SHOT_TO_YAW", yaw) as f32,
        to_pitch: number("GALOS_SHOT_TO_PITCH", pitch) as f32,
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
    // The window's size, where the run names one, so a capture comes out the
    // same on any display: `GALOS_SHOT_WIDTH` by `GALOS_SHOT_HEIGHT` points,
    // at `GALOS_SHOT_SCALE` pixels a point.
    let width = number("GALOS_SHOT_WIDTH", 0.) as f32;
    let height = number("GALOS_SHOT_HEIGHT", 0.) as f32;
    let scale = number("GALOS_SHOT_SCALE", 0.) as f32;
    app.add_systems(
        Startup,
        move |mut windows: Query<&mut Window, With<PrimaryWindow>>| {
            let Ok(mut window) = windows.single_mut() else { return };
            // Size first, in the scale the window has now: bevy_winit
            // carries the size across a change of scale in points, so the
            // override after it keeps these points and changes the pixels.
            if width > 0. && height > 0. {
                window.resolution.set(width, height);
            }
            if scale > 0. {
                window.resolution.set_scale_factor_override(Some(scale));
            }
        },
    );
}

fn capture(
    mut shot: ResMut<Shot>,
    mut cameras: Query<(&mut OrbitCamera, &Camera, &Projection)>,
    systems: Query<(
        &System,
        &Visibility,
        &crate::map::paint::sizing::Drawn,
        &crate::map::bodies::spawn::Strength,
    )>,
    mut view: ResMut<crate::map::paint::sizing::View>,
    mut exposure: ResMut<crate::map::galaxy::spawn::StarExposure>,
    mut spyglass: ResMut<crate::map::galaxy::Spyglass>,
    mut show_names: ResMut<crate::map::galaxy::spawn::ShowNames>,
    mut show_body_names: ResMut<crate::map::labels::ShowBodyNames>,
    blobs: Res<crate::map::galaxy::walk::Blobs>,
    mut commands: Commands,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok((mut camera, lens, projection)) = cameras.single_mut() else {
        return;
    };
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
    // Held every frame rather than set once: the controls smooth toward a
    // target and anything else touching the pose would drift it.
    //
    // Three gestures, since the three are reported to flicker differently:
    // `SPIN` turns, `ZOOM` pulls back by a fraction a frame, `PAN` slides the
    // point held along x. Or a flight, from one pose to another. None of
    // them starts until `hold` has run out.
    let pose = shot.pose();
    let named = if shot.names_within > 0. {
        pose.back <= shot.names_within
    } else {
        shot.names
    };
    if show_names.0 != named {
        show_names.0 = named;
    }
    if show_body_names.0 != named {
        show_body_names.0 = named;
    }
    if shot.frame <= shot.wait {
        camera.holds(pose.at, pose.back);
        camera.yaw = pose.yaw;
        camera.target_yaw = pose.yaw;
        camera.pitch = pose.pitch;
        camera.target_pitch = pose.pitch;
    }
    // A reach the run was told to hold, rather than the one the zoom sets.
    // A flight hands it back to the map as it goes: from the reach it was
    // told at the start to the one the camera takes where it ends up, by
    // how far through it is.
    if shot.reach > 0. {
        let followed =
            |back| crate::map::galaxy::followed(back, Some(projection));
        let reach = if shot.to_back > 0. {
            followed(pose.back)
                * (shot.reach / followed(shot.back)).powf(1. - pose.through)
        } else {
            shot.reach
        };
        spyglass.follow_camera = false;
        if spyglass.radius != reach {
            spyglass.radius = reach;
        }
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
    // The motion as a run of frames, from the pose it holds to where it ends.
    if shot.every > 0
        && shot.frame >= shot.hold
        && shot.frame <= shot.wait
        && (shot.frame - shot.hold).is_multiple_of(shot.every)
    {
        let path = format!("{}/{:04}.png", shot.path, shot.taken);
        shot.taken += 1;
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
    }
    if shot.every == 0 && shot.frame == shot.wait {
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
