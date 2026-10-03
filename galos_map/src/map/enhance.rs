//! Enhance: the view drawn again at several times the window's resolution,
//! every system in it, as a flat picture laid over the map
//!
//! **Every system, and not the map's budget of them.** The map draws what a
//! frame can afford: the walk spends a frame's marks over what is in reach
//! and the glow stands in for the rest, so a frame of the galaxy draws a few
//! tens of thousands of its two hundred million systems. A picture is not a
//! frame. The camera stands still while it is made, so nothing has to be
//! spawned, settled or drawn again next frame: each system in view is read
//! straight off its cell's payload, projected through the view's own lens
//! ([`galos_index::read::screen::Projector`], the walk's projection), and the
//! light the map would lay for it — its hue's [`Hue::light`] at
//! [`system_light`] — is added into the pixel it lands in, what the filters
//! exclude into a sum of its own, laid under at the dim ([`Half`]). In
//! short: the map's own reading at a resolution the map's budget never
//! reaches, in the key's own colors.
//!
//! **A window-sized piece at a time, off the main thread.** The picture is
//! `scale` windows across, and is summed a piece at a time on a thread of its
//! own with workers reading the cells that land in the piece ([`lands_in`],
//! through the map's own mirrored [`plan::Lens`]): only one piece's sums are
//! ever held, forty-eight bytes a pixel of a window, whatever the picture's
//! size. Each piece is turned into colors as it finishes and laid over the
//! map, the middle first, and the map's own cameras stand down under it
//! ([`Covered`]).
//!
//! **One curve over the whole picture.** What a pixel gathers runs from one
//! unscanned star to thousands in the bubble, so it is drawn on a log curve,
//! and every piece has to be drawn on the same one or they would not meet.
//! The curve's top is read off the base: the whole view at the window's own
//! resolution, summed first, which stands under the pieces until they land.
//!
//! **And it stays where it is.** The picture is the view from one eye, so
//! looking closer into it is a narrower lens from the same eye, which is what
//! [`Frame`] is: the wheel and a drag move about in the picture, the map
//! underneath follows to the pixel, and the names and rings drawn over it
//! stand on what they name at every magnification; `WASD` and `F`/`R` move
//! and zoom the window over it as they would the map. The camera stands
//! still under it, and only the close button and escape put it away.
//!
//! [`Hue::light`]: crate::map::galaxy::spawn::Hue::light

use crate::input::{Keyboard, bare};
use crate::map::camera::{Frame, OrbitCamera};
use crate::map::filter::{DimTo, Filters};
use crate::map::galaxy::spawn::ColorBy;
use crate::map::galaxy::walk::{Along, candidate};
use crate::map::galaxy::{Spyglass, plan};
use crate::map::index::{Populated, ResidentIndex, Transport};
use crate::map::keys::{PAN_PER_SECOND, ZOOM_PER_SECOND};
use crate::map::paint::glow::{Gains, system_light};
use crate::map::schedule::{MapSet, PaintSet};
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Hdr, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::gpu_readback::{Readback, ReadbackComplete};
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat,
    TextureUsages,
};
use bevy::render::renderer::RenderDevice;
use bevy::shader::ShaderRef;
use bevy::tasks::{IoTaskPool, Task, block_on, poll_once};
use bevy::window::PrimaryWindow;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use chrono::{DateTime, Utc};
use galos_index::prelude::{CellId, Source, View as Viewpoint};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{
    AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed,
};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "enhance.wgsl");
    app.add_plugins(MaterialPlugin::<PieceMaterial>::default());
    app.init_resource::<Enhance>();
    app.init_resource::<Covered>();
    app.add_systems(Startup, spawn_overlay);
    // The camera stands still while there is a picture: it is the view from
    // where the camera stood, and every look into it is measured from that
    // one eye. The keys that would have moved it look about in the picture
    // instead ([`look`]).
    app.configure_sets(Update, MapSet::Camera.run_if(idle));
    app.add_systems(
        Update,
        (look, drive, lay, hide_chrome).chain().after(MapSet::Present),
    );
    app.add_systems(EguiPrimaryContextPass, controls.in_set(PaintSet::Ui));
    app.add_observer(captured);
    script(app);
}

/// An enhance run with nobody at the window, from `GALOS_ENHANCE`
///
/// `GALOS_ENHANCE=3` enhances the view the map opens on three windows across,
/// saves the picture, and with `GALOS_ENHANCE_EXIT` set closes the map when it
/// is written; the window's own capture beside it is `GALOS_SHOT`'s,
/// `crate::dev::shot`, whose pose a run here can be pointed with. What
/// `media.sh` would record a picture with, and how a change to this is seen
/// working without a person pressing the button.
fn script(app: &mut App) {
    let Ok(scale) = std::env::var("GALOS_ENHANCE") else { return };
    // One of the scales offered, or the one a picture starts at.
    let scale = scale
        .parse()
        .ok()
        .filter(|scale| SCALES.contains(scale))
        .unwrap_or(SCALE);
    let exit = std::env::var("GALOS_ENHANCE_EXIT").is_ok();
    app.insert_resource(Scripted { scale, exit, waited: 0, told: false });
    app.add_systems(
        Update,
        scripted
            .before(drive)
            .run_if(in_state(crate::map::index::load::Opening::Drawn)),
    );
}

/// Frames in a row the camera has to have stood still before a scripted run
/// asks for a picture: a pose `GALOS_SHOT` holds is eased into, and a picture
/// asked for on the way would be of somewhere else.
const SCRIPT_WAIT: u32 = 30;

/// Where a scripted run is; see [`script`]
#[derive(Resource)]
struct Scripted {
    scale: u32,
    exit: bool,
    /// Frames in a row the camera has stood still, and past [`SCRIPT_WAIT`]
    /// the picture asked for
    waited: u32,
    /// Whether where it was saved has been said
    told: bool,
}

fn scripted(
    mut scripted: ResMut<Scripted>,
    mut enhance: ResMut<Enhance>,
    cameras: Query<&OrbitCamera>,
    mut exit: MessageWriter<AppExit>,
) {
    if scripted.waited < SCRIPT_WAIT {
        let still = cameras.single().is_ok_and(OrbitCamera::is_settled);
        scripted.waited = if still { scripted.waited + 1 } else { 0 };
        if scripted.waited == SCRIPT_WAIT {
            enhance.scale = scripted.scale;
            enhance.asked = Some(Ask::Start);
        }
        return;
    }
    match (&enhance.phase, &enhance.saved) {
        (Phase::Shown, None)
            if enhance
                .picture
                .as_ref()
                .is_some_and(|it| it.saving.is_none()) =>
        {
            // Where a picture with nobody at the window is written: the
            // working directory, under the name the dialog would offer.
            enhance.asked = Some(Ask::SaveTo(PathBuf::from(picture_name())));
        }
        (_, Some(saved)) if !scripted.told => {
            scripted.told = true;
            match saved {
                Ok(path) => {
                    info!("enhanced picture saved to {}", path.display())
                }
                Err(why) => error!("enhanced picture not saved: {why}"),
            }
            if scripted.exit {
                exit.write(AppExit::Success);
            }
        }
        _ => {}
    }
}

/// Whether nothing is being enhanced, for the camera, which stands still
/// while something is
fn idle(enhance: Res<Enhance>) -> bool {
    matches!(enhance.phase, Phase::Off)
}

/// Ask for a picture, and look about in a shown one, from the keyboard
///
/// `P`, for picture, asks for one at the scale last picked, as the
/// launcher's chip would. Once one is shown the camera stands still under it,
/// so the keys that would have moved it move the window over the picture
/// instead, at the rates they move the map: `WASD` across it, `F` in and `R`
/// out about the window's middle. The keys that turn the camera have nothing
/// to turn in a flat picture and do nothing.
fn look(
    keys: Res<ButtonInput<KeyCode>>,
    keyboard: Res<Keyboard>,
    time: Res<Time<Real>>,
    mut enhance: ResMut<Enhance>,
) {
    if keyboard.typing || !bare(&keys) {
        return;
    }
    if enhance.phase == Phase::Off && keys.just_pressed(KeyCode::KeyP) {
        enhance.asked = Some(Ask::Start);
        enhance.choosing = false;
        return;
    }
    if enhance.phase != Phase::Shown {
        return;
    }
    let Some(picture) = &mut enhance.picture else { return };
    let held = |key| keys.pressed(key);
    let mut way = Vec2::ZERO;
    for (key, toward) in [
        (KeyCode::KeyD, Vec2::X),
        (KeyCode::KeyA, Vec2::NEG_X),
        (KeyCode::KeyW, Vec2::NEG_Y),
        (KeyCode::KeyS, Vec2::Y),
    ] {
        if held(key) {
            way += toward;
        }
    }
    // Across the window as a key crosses the map, a share of what is on
    // screen a second; and the picture moves the other way to the eye.
    if let Some(way) = way.try_normalize() {
        let rate = PAN_PER_SECOND * picture.viewport.x;
        picture.pan(-way * rate * time.delta_secs());
    }
    let zoom = f32::from(u8::from(held(KeyCode::KeyF)))
        - f32::from(u8::from(held(KeyCode::KeyR)));
    if zoom != 0. {
        let middle = picture.viewport / 2.;
        let factor = (zoom * ZOOM_PER_SECOND * time.delta_secs()).exp();
        picture.zoom_about(middle, factor);
    }
}

/// How many windows across a picture is, to begin with
const SCALE: u32 = 3;

/// The scales offered, windows across
///
/// One window is the view as it is. Every piece is held on the GPU while a
/// picture is shown, so ten across is a hundred windows, a gigabyte and a
/// half at a 1440p window: offered for the wide views that want it, and
/// nothing past it until pieces are drawn on demand.
const SCALES: [u32; 6] = [2, 3, 4, 5, 6, 10];

/// How far past a texel to a pixel a picture may be looked into
///
/// Up to [`Picture::scale`] the window shows the picture at its own
/// resolution or finer; past it each texel is spread over several pixels,
/// which is how a reader reads a single system's pixel and its neighbours.
const DEEPER: f32 = 8.;

/// What enhancing is doing, and the picture it is doing it to
#[derive(Resource)]
pub(crate) struct Enhance {
    phase: Phase,
    /// How many windows across the next picture is
    scale: u32,
    /// The picture being drawn or shown
    picture: Option<Picture>,
    /// What the controls asked for this frame, for [`drive`] to carry out
    asked: Option<Ask>,
    /// Where the last picture saved went, or why it did not
    saved: Option<Result<PathBuf, String>>,
    /// When the last picture was written, which the line says for
    /// [`SAVED_FOR`] after
    saved_at: Option<Instant>,
    /// Whether the launcher's scales are out
    choosing: bool,
    /// The folder the last picture was saved to, which the dialog opens in
    last_dir: Option<PathBuf>,
}

impl Default for Enhance {
    fn default() -> Self {
        Enhance {
            phase: Phase::Off,
            scale: SCALE,
            picture: None,
            asked: None,
            saved: None,
            saved_at: None,
            choosing: false,
            last_dir: None,
        }
    }
}

/// Where a picture is
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// No picture: the map as it always is
    Off,
    /// Drawing one, a piece at a time
    Drawing,
    /// Drawn, and laid over the map
    Shown,
}

/// What the controls asked for
#[derive(Clone, Debug, PartialEq)]
enum Ask {
    Start,
    /// Stop drawing, or put the picture away
    Close,
    /// Ask where to save it
    Save,
    /// Save it there
    SaveTo(PathBuf),
}

/// A picture several windows across, in pieces a window each
struct Picture {
    /// Windows across
    scale: u32,
    /// The window it was started in, logical pixels
    viewport: Vec2,
    /// And physical, which is a piece's size in texels
    physical: UVec2,
    /// The view as it stood, at the window's own resolution, laid under the
    /// pieces
    base: Part,
    /// The pieces, row by row
    pieces: Vec<Part>,
    /// The one quad every part is laid with
    mesh: Handle<Mesh>,
    /// How far into the picture the window is looking: how many times the
    /// picture's own size on the window it is shown at, from one, the
    /// whole picture on the window, through [`Picture::scale`], a piece's
    /// pixel to the window's, up to [`DEEPER`] times that
    zoom: f32,
    /// Where the window's top left stands in the picture, in its logical
    /// pixels
    corner: Vec2,
    /// The thread drawing it, until every part is laid
    job: Option<Job>,
    /// The PNG being written, a row of pieces at a time
    saving: Option<Saving>,
    /// The save dialog, while it is out: where the picture is to go, or
    /// [`None`] where it was cancelled
    asking_where: Option<Task<Option<PathBuf>>>,
}

/// One part of a picture, and once it is drawn, what lays it over the map
struct Part {
    /// Where it stands in the picture, in pieces
    column: u32,
    row: u32,
    laid: Option<Laid>,
}

/// A drawn part: its texture and the quad it is laid over the map with
struct Laid {
    image: Handle<Image>,
    material: Handle<PieceMaterial>,
    quad: Entity,
}

/// What the map is read through for a picture: everything a system's light
/// depends on, and where the systems are read from
#[derive(SystemParam)]
struct Under<'w> {
    color_by: Res<'w, ColorBy>,
    filters: Res<'w, Filters>,
    dim: Res<'w, DimTo>,
    gains: Res<'w, Gains>,
    /// With the index and the transport, there once the index has loaded.
    populated: Option<Res<'w, Populated>>,
    index: Option<Res<'w, ResidentIndex>>,
    transport: Option<Res<'w, Transport>>,
    spyglass: Res<'w, Spyglass>,
}

/// The render layer the picture is laid over the window on
///
/// Its own, past every layer the map draws on. See
/// [`crate::map::camera::FIELD_LAYER`] for why a number is never reused.
const LAYER: usize = 10;

/// The camera that lays the picture over the window
#[derive(Component)]
struct OverlayCamera;

/// One part's quad
#[derive(Component)]
struct PieceQuad;

/// What lays a part over the window, averaging the texels each pixel
/// covers
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct PieceMaterial {
    #[uniform(0)]
    footprint: Footprint,
    #[texture(1)]
    #[sampler(2)]
    piece: Handle<Image>,
}

/// How many of a part's texels a pixel of the window covers, along either
/// axis
#[derive(ShaderType, Clone, Copy, Debug, Default, PartialEq)]
struct Footprint {
    across: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

impl Material for PieceMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://galos_map/map/enhance.wgsl".into()
    }
}

/// The overlay's camera, standing by until there is a picture
///
/// Over the map's own cameras and under the annotations, so the names and
/// rings land over the picture as they do over the map, and clearing the
/// window once the base is down: the picture covers whatever the map draws.
///
/// [`Hdr`] and the scene's own MSAA, so it draws into the one texture the
/// scene, the curve and the annotations share. On a texture of its own it
/// would be laid on the window and then covered whole by the annotations',
/// which carries the scene and is laid last.
fn spawn_overlay(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Camera {
            order: crate::map::camera::ENHANCE_ORDER,
            clear_color: ClearColorConfig::Custom(Color::BLACK),
            is_active: false,
            ..default()
        },
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::WindowSize,
            ..OrthographicProjection::default_3d()
        }),
        RenderLayers::layer(LAYER),
        OverlayCamera,
    ));
}

/// Carry out what was asked, and lay each part as the thread drawing the
/// picture hands it over
#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut enhance: ResMut<Enhance>,
    mut cameras: Query<(&mut OrbitCamera, &Camera)>,
    window: Query<&Window, With<PrimaryWindow>>,
    reading: Under,
    keys: Res<ButtonInput<KeyCode>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PieceMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok((mut orbit, camera)) = cameras.single_mut() else { return };
    let Ok(window) = window.single() else { return };
    let enhance = &mut *enhance;

    // Escape puts away whatever is under way, as it does anything else the
    // map opens.
    let mut asked = enhance.asked.take();
    if keys.just_pressed(KeyCode::Escape) {
        enhance.choosing = false;
        if enhance.phase != Phase::Off {
            asked = Some(Ask::Close);
        }
    }

    // **The window changed size.** The pieces are cut to the window they were
    // drawn in and laid in its pixels; in another they would be laid wrong.
    // Nothing else puts a picture away but the close and the escape: the
    // camera stands still under it, and what it shows is the map as it was
    // when it was asked for.
    if let Some(picture) = &enhance.picture
        && picture.physical != window.physical_size().max(UVec2::ONE)
        && asked != Some(Ask::Start)
    {
        asked = Some(Ask::Close);
    }

    match asked {
        // Held until the camera stands still: a picture is the view from
        // one eye, and one asked for on the way somewhere would be of where
        // the camera happened to be.
        Some(Ask::Start)
            if enhance.phase == Phase::Off && !orbit.is_settled() =>
        {
            enhance.asked = Some(Ask::Start);
        }
        Some(Ask::Start) if enhance.phase == Phase::Off => {
            let (Some(index), Some(transport), Some(populated)) =
                (&reading.index, &reading.transport, &reading.populated)
            else {
                return;
            };
            let Some(view) = plan::view(&orbit, camera) else { return };
            let mut picture = Picture::new(enhance.scale, window, &mut meshes);
            picture.job = Some(Job::start(Spec::new(
                &picture,
                view,
                &index.0,
                transport.0.clone(),
                (**populated).clone(),
                orbit.center(),
                &reading,
            )));
            enhance.picture = Some(picture);
            enhance.phase = Phase::Drawing;
            enhance.saved = None;
            orbit.frame = Frame::WHOLE;
            return;
        }
        Some(Ask::Close) => {
            if let Some(picture) = enhance.picture.take() {
                picture.put_away(&mut commands, &mut images, &mut materials);
            }
            enhance.phase = Phase::Off;
            orbit.frame = Frame::WHOLE;
            return;
        }
        Some(Ask::Save) => {
            if let Some(picture) = &mut enhance.picture
                && enhance.phase == Phase::Shown
                && picture.saving.is_none()
                && picture.asking_where.is_none()
            {
                picture.asking_where =
                    Some(ask_where(enhance.last_dir.as_deref()));
            }
        }
        Some(Ask::SaveTo(path)) => {
            if let Some(picture) = &mut enhance.picture
                && enhance.phase == Phase::Shown
                && picture.saving.is_none()
            {
                start_saving(
                    picture,
                    path,
                    &mut enhance.saved,
                    &mut enhance.last_dir,
                    &mut commands,
                );
            }
        }
        _ => {}
    }

    let Some(picture) = &mut enhance.picture else { return };
    match enhance.phase {
        Phase::Off => {}
        Phase::Drawing => {
            let Some(job) = &picture.job else { return };
            let finished: Vec<Finished> =
                job.finished.lock().expect("the parts").try_iter().collect();
            let failed =
                job.progress.failed.lock().expect("the failure").take();
            let unread = job.progress.unread.load(Relaxed);
            let (counted, elapsed) =
                (job.progress.counted.load(Relaxed), job.started.elapsed());
            for Finished { part, rgba } in finished {
                picture.lay_part(
                    part,
                    rgba,
                    &mut commands,
                    &mut images,
                    &mut materials,
                );
            }
            if let Some(why) = failed {
                error!("enhance: {why}");
                if let Some(picture) = enhance.picture.take() {
                    picture.put_away(
                        &mut commands,
                        &mut images,
                        &mut materials,
                    );
                }
                enhance.phase = Phase::Off;
                enhance.saved = Some(Err(why));
                return;
            }
            if picture.base.laid.is_some()
                && picture.pieces.iter().all(|part| part.laid.is_some())
            {
                info!(
                    "enhance: {}× drawn, {counted} systems in {elapsed:.1?}",
                    picture.scale,
                );
                if unread > 0 {
                    warn!("enhance: {unread} cells could not be read");
                }
                picture.job = None;
                enhance.phase = Phase::Shown;
                orbit.frame = picture.looking();
            }
        }
        Phase::Shown => {
            let looking = picture.looking();
            if orbit.frame != looking {
                orbit.frame = looking;
            }
            // A row the writer had no room for, offered again; and what the
            // writer came to, once it has finished.
            if let Some(mut saving) = picture.saving.take() {
                saving.hand_on(picture, &mut commands);
                match saving.finished() {
                    Some(ended) => {
                        enhance.saved = Some(ended);
                        enhance.saved_at = Some(Instant::now());
                    }
                    None => picture.saving = Some(saving),
                }
            }
            // The dialog answered: somewhere to save to, or cancelled.
            if let Some(task) = &mut picture.asking_where
                && let Some(chosen) = block_on(poll_once(task))
            {
                picture.asking_where = None;
                if let Some(path) = chosen {
                    start_saving(
                        picture,
                        path,
                        &mut enhance.saved,
                        &mut enhance.last_dir,
                        &mut commands,
                    );
                }
            }
        }
    }
}

impl Picture {
    fn new(scale: u32, window: &Window, meshes: &mut Assets<Mesh>) -> Picture {
        let part = |column: u32, row: u32| Part { column, row, laid: None };
        Picture {
            scale,
            viewport: Vec2::new(window.width(), window.height()),
            physical: window.physical_size().max(UVec2::ONE),
            base: part(0, 0),
            pieces: (0..scale)
                .flat_map(|row| (0..scale).map(move |column| (column, row)))
                .map(|(column, row)| part(column, row))
                .collect(),
            mesh: meshes.add(Rectangle::new(1., 1.)),
            zoom: 1.,
            corner: Vec2::ZERO,
            job: None,
            saving: None,
            asking_where: None,
        }
    }

    /// The pieces in the order they are drawn: out from the middle, which is
    /// what the view was pointed at
    fn order(&self) -> Vec<usize> {
        let middle = (self.scale as f32 - 1.) / 2.;
        let away = |piece: &Part| {
            Vec2::new(piece.column as f32 - middle, piece.row as f32 - middle)
                .length_squared()
        };
        let mut order: Vec<usize> = (0..self.pieces.len()).collect();
        order.sort_by(|&a, &b| {
            away(&self.pieces[a]).total_cmp(&away(&self.pieces[b]))
        });
        order
    }

    /// Lay a drawn part over the map: [`None`] the base, or a piece by number
    fn lay_part(
        &mut self,
        part: Option<usize>,
        rgba: Vec<u8>,
        commands: &mut Commands,
        images: &mut Assets<Image>,
        materials: &mut Assets<PieceMaterial>,
    ) {
        let mut image = Image::new(
            Extent3d {
                width: self.physical.x,
                height: self.physical.y,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            rgba,
            TextureFormat::Rgba8UnormSrgb,
            // The GPU's alone once uploaded: a picture is gigabytes'
            // worth of pieces held twice otherwise. Saving reads it back.
            RenderAssetUsages::RENDER_WORLD,
        );
        image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
        image.sampler = ImageSampler::linear();
        let image = images.add(image);
        let material = materials.add(PieceMaterial {
            footprint: Footprint::default(),
            piece: image.clone(),
        });
        // The pieces stand nearer the overlay's camera than the base, so
        // each covers it where it is laid.
        let depth = if part.is_some() { -1. } else { -2. };
        let quad = commands
            .spawn((
                Mesh3d(self.mesh.clone()),
                MeshMaterial3d(material.clone()),
                RenderLayers::layer(LAYER),
                NoFrustumCulling,
                Transform::from_xyz(0., 0., depth),
                Visibility::Hidden,
                PieceQuad,
            ))
            .id();
        let laid = Some(Laid { image, material, quad });
        match part {
            None => self.base.laid = laid,
            Some(at) => self.pieces[at].laid = laid,
        }
    }

    /// The frame the map draws in under the picture as it is being looked
    /// at, so what is drawn over it lands where the picture shows it
    fn looking(&self) -> Frame {
        let shown = self.zoom / self.scale as f32;
        let frame = Frame { scale: self.zoom, corner: self.corner * shown };
        if (frame.scale - 1.).abs() < 1e-6 && frame.corner.length() < 1e-3 {
            Frame::WHOLE
        } else {
            frame
        }
    }

    /// Zoom about `at`, a point on the window in logical pixels, by
    /// `factor`, holding the picture's point under it where it is
    fn zoom_about(&mut self, at: Vec2, factor: f32) {
        let before = self.zoom / self.scale as f32;
        let under = self.corner + at / before;
        self.zoom = (self.zoom * factor).clamp(1., self.scale as f32 * DEEPER);
        let after = self.zoom / self.scale as f32;
        self.corner = under - at / after;
        self.hold();
    }

    /// Move the picture by `by` logical pixels of the window
    fn pan(&mut self, by: Vec2) {
        let shown = self.zoom / self.scale as f32;
        self.corner -= by / shown;
        self.hold();
    }

    /// Keep the window inside the picture
    fn hold(&mut self) {
        let shown = self.zoom / self.scale as f32;
        let picture = self.viewport * self.scale as f32;
        let seen = self.viewport / shown;
        self.corner =
            self.corner.clamp(Vec2::ZERO, (picture - seen).max(Vec2::ZERO));
    }

    /// Let the parts' textures and quads go, and the thread drawing them
    fn put_away(
        self,
        commands: &mut Commands,
        images: &mut Assets<Image>,
        materials: &mut Assets<PieceMaterial>,
    ) {
        for laid in std::iter::once(self.base)
            .chain(self.pieces)
            .filter_map(|part| part.laid)
        {
            commands.entity(laid.quad).despawn();
            images.remove(&laid.image);
            materials.remove(&laid.material);
        }
    }
}

/// A picture being drawn: the thread summing it, how far it has got, and
/// the parts it has finished
///
/// Dropped with the picture, which stops the thread at its next cell.
struct Job {
    progress: Arc<Progress>,
    finished: Mutex<mpsc::Receiver<Finished>>,
    started: Instant,
}

impl Drop for Job {
    fn drop(&mut self) {
        self.progress.cancelled.store(true, Relaxed);
    }
}

/// How far a picture has got, counted in systems against what the cells in
/// view own, which the index says before a payload is read
#[derive(Default)]
struct Progress {
    /// Systems summed so far, over every part
    counted: AtomicU64,
    /// Systems every part will sum, once the parts have been culled
    total: AtomicU64,
    /// Whether `total` is known yet
    planned: AtomicBool,
    /// Parts finished
    parts: AtomicUsize,
    /// Cells whose payload would not read, left out of the picture
    unread: AtomicU64,
    /// Set to stop the thread
    cancelled: AtomicBool,
    /// Why the thread gave up, where it did
    failed: Mutex<Option<String>>,
}

/// A finished part, colored: [`None`] the base, or a piece by number
struct Finished {
    part: Option<usize>,
    rgba: Vec<u8>,
}

impl Job {
    fn start(spec: Spec) -> Job {
        let progress = Arc::new(Progress::default());
        let (send, finished) = mpsc::channel();
        let drawing = progress.clone();
        let spawned = std::thread::Builder::new()
            .name("enhance".into())
            .spawn(move || spec.draw(&drawing, &send));
        if let Err(why) = spawned {
            *progress.failed.lock().expect("the failure") =
                Some(format!("no thread to draw on: {why}"));
        }
        Job {
            progress,
            finished: Mutex::new(finished),
            started: Instant::now(),
        }
    }
}

/// Everything the thread draws a picture from, taken off the map when it is
/// asked for
struct Spec {
    /// The cells that own systems, and how many each owns
    cells: Vec<(CellId, u64)>,
    source: Arc<dyn Source>,
    populated: Populated,
    filters: Filters,
    now: DateTime<Utc>,
    /// The spyglass's bubble, where it clears what is past it: the centre
    /// the camera looks at and the radius, as the map's own walk clamps to
    bubble: Option<(bevy::math::DVec3, f64)>,
    light: Light,
    /// The view at the window's resolution, in its physical pixels
    base: Viewpoint,
    /// The same view `scale` windows across
    picture: Viewpoint,
    physical: UVec2,
    /// The window's height in logical pixels, which the map's mark floor is
    /// measured in
    logical: f32,
    scale: u32,
    /// The pieces' columns and rows, by number
    places: Vec<UVec2>,
    order: Vec<usize>,
}

/// How many cells a worker takes off the shared list at once
const BATCH: usize = 16;

/// How many fixed-point units the faintest light the map lays comes to
///
/// The units are set off the faintest, not the brightest: along a political
/// axis a system nobody lives in is worth some thousandth of a colony, and
/// units set off the colony rounded it to nothing, so the uninhabited were
/// not drawn at all. And a mark shares its light among the pixels it
/// covers, a hundred or so at most, so the faintest must split that finely
/// and keep its share. Sums are `u64`, which holds any galaxy's worth of
/// the brightest at this many.
const FAINTEST: f32 = 32_768.;

/// Of the base's lit pixels, the share drawn below the top of the curve
///
/// The brightest few go to white rather than the curve being set by the
/// one pixel the bubble's core lands in, which would leave the rest of the
/// galaxy dim.
const WHITE_AT: f64 = 0.995;

/// Where the base's middling lit pixel is drawn on the curve, in linear
/// light: a mid grey, so the bulk of what is lit reads whether it is a
/// pixel of a dozen systems or of a fraction of one
const MIDDLE: f64 = 0.18;

impl Spec {
    fn new(
        picture: &Picture,
        view: Viewpoint,
        index: &galos_index::prelude::Index,
        source: Arc<dyn Source>,
        populated: Populated,
        center: bevy::math::DVec3,
        reading: &Under,
    ) -> Spec {
        // In physical pixels, so a pixel of a piece is a texel of it.
        let physical = picture.physical;
        let at = |height: u32| Viewpoint {
            viewport_height: height as f32,
            aspect: physical.x as f32 / physical.y as f32,
            ..view
        };
        Spec {
            cells: index
                .cells()
                .filter(|cell| cell.slice_len() > 0)
                .map(|cell| (cell.id, cell.slice_len()))
                .collect(),
            source,
            populated,
            filters: reading.filters.clone(),
            now: Utc::now(),
            bubble: reading
                .spyglass
                .clear
                .then(|| (center, f64::from(reading.spyglass.radius))),
            light: Light::new(
                *reading.color_by,
                &reading.gains,
                reading.dim.opacity(),
            ),
            base: at(physical.y),
            picture: at(physical.y * picture.scale),
            physical,
            logical: picture.viewport.y,
            scale: picture.scale,
            places: picture
                .pieces
                .iter()
                .map(|piece| UVec2::new(piece.column, piece.row))
                .collect(),
            order: picture.order(),
        }
    }

    /// Whether the spyglass reaches into a cell's box at all
    fn reaches_cell(&self, id: CellId) -> bool {
        self.bubble.is_none_or(|(center, radius)| {
            id.bounds().distance_to(center.to_array()) <= radius
        })
    }

    /// Whether the spyglass reaches a system, as the map's walk asks it
    fn reaches(&self, at: [f64; 3]) -> bool {
        self.bubble
            .is_none_or(|(center, radius)| center.distance(at.into()) <= radius)
    }

    /// Draw the picture: the base, and from its light the curve, then each
    /// piece, handing each over as it is colored
    fn draw(self, progress: &Progress, send: &mpsc::Sender<Finished>) {
        // What each part reads, and so how much there is to count. Culled
        // across the workers, a part each: tens of millions of corners at
        // six windows across.
        let parts: Vec<(Viewpoint, UVec2)> =
            std::iter::once((self.base, UVec2::ZERO))
                .chain(self.order.iter().map(|&piece| {
                    (self.picture, self.places[piece] * self.physical)
                }))
                .collect();
        let culled = self.cull(&parts, progress);
        if progress.cancelled.load(Relaxed) {
            return;
        }
        let total =
            culled.iter().flatten().map(|&(_, owned)| owned).sum::<u64>();
        progress.total.store(total, Relaxed);
        progress.planned.store(true, Relaxed);

        // Each half's curve, the admitted's and the excluded's, read off
        // the base and held for every piece.
        let mut curves = None;
        for (at, ((view, origin), cells)) in
            parts.into_iter().zip(&culled).enumerate()
        {
            let Some(sums) = self.sum(view, origin, cells, progress) else {
                return;
            };
            // A base pixel takes in `scale` squared of the picture's.
            let per = if at == 0 { f64::from(self.scale).powi(2) } else { 1. };
            let curves = *curves.get_or_insert_with(|| {
                [Half::Admitted, Half::Excluded]
                    .map(|half| Curve::of(&sums, half, per, self.light.unit))
            });
            let rgba = tone(&sums, per, curves, &self.light);
            drop(sums);
            let part = (at > 0).then(|| self.order[at - 1]);
            progress.parts.fetch_add(1, Relaxed);
            if send.send(Finished { part, rgba }).is_err() {
                return;
            }
        }
    }

    /// The radius, in `view`'s pixels, that the map's smallest mark comes to
    /// in a part: as large on the window, shown whole, as the map draws it
    fn floor(&self, view: &Viewpoint) -> f64 {
        f64::from(
            crate::map::paint::field::SMALLEST * view.viewport_height
                / self.logical.max(1.),
        )
    }

    /// The cells each part reads: those whose box lands in it, or within a
    /// mark of it, a mark at the edge laying some of itself over the line
    fn cull(
        &self,
        parts: &[(Viewpoint, UVec2)],
        progress: &Progress,
    ) -> Vec<Vec<(CellId, u64)>> {
        let size = self.physical.as_dvec2();
        let next = AtomicUsize::new(0);
        let culled: Vec<Mutex<Vec<(CellId, u64)>>> =
            parts.iter().map(|_| Mutex::default()).collect();
        std::thread::scope(|scope| {
            for _ in 0..workers() {
                scope.spawn(|| {
                    loop {
                        let at = next.fetch_add(1, Relaxed);
                        if at >= parts.len() || progress.cancelled.load(Relaxed)
                        {
                            return;
                        }
                        let (view, origin) = parts[at];
                        let lens = plan::Lens::of(&view);
                        let margin = self.floor(&view);
                        let low = origin.as_dvec2() - margin;
                        let high = origin.as_dvec2() + size + margin;
                        let rect = [low.x, low.y, high.x, high.y];
                        let cells = self
                            .cells
                            .iter()
                            .filter(|(id, _)| self.reaches_cell(*id))
                            .filter(|(id, _)| lands_in(&lens, *id, rect))
                            .copied()
                            .collect();
                        *culled[at].lock().expect("a part's cells") = cells;
                    }
                });
            }
        });
        culled
            .into_iter()
            .map(|cells| cells.into_inner().expect("a part's cells"))
            .collect()
    }

    /// Sum the light every system in `cells` lays into the part of `view`
    /// whose top left is `origin`, or [`None`] where the picture was put
    /// away part way
    fn sum(
        &self,
        view: Viewpoint,
        origin: UVec2,
        cells: &[(CellId, u64)],
        progress: &Progress,
    ) -> Option<Vec<AtomicU64>> {
        let (width, height) =
            (self.physical.x as usize, self.physical.y as usize);
        // Six a pixel: the light the filters admit, and the light of what
        // they exclude, each red, green and blue. See [`Half`].
        let sums: Vec<AtomicU64> =
            (0..width * height * CHANNELS).map(|_| AtomicU64::new(0)).collect();
        // The map's own screen, mirrored as it draws the galaxy.
        let lens = plan::Lens::of(&view);
        let floor = self.floor(&view);
        let crowding = Crowding::of(&lens, origin, self.physical, cells);
        let origin = origin.as_dvec2();
        // How far a point may stand off the part and still lay some of its
        // mark in it.
        let reach = floor.ceil();
        let next = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..workers() {
                scope.spawn(|| {
                    let prepared = self.filters.prepared();
                    let along = Along::of(self.light.color_by, &self.populated);
                    loop {
                        let at = next.fetch_add(BATCH, Relaxed);
                        if at >= cells.len() || progress.cancelled.load(Relaxed)
                        {
                            return;
                        }
                        for &(id, owned) in
                            &cells[at..(at + BATCH).min(cells.len())]
                        {
                            let Ok(points) =
                                bevy::tasks::block_on(self.source.payload(id))
                            else {
                                progress.unread.fetch_add(1, Relaxed);
                                progress.counted.fetch_add(owned, Relaxed);
                                continue;
                            };
                            for point in &points {
                                if !self.reaches(point.position) {
                                    continue;
                                }
                                let Some([x, y]) = lens.project(point.position)
                                else {
                                    continue;
                                };
                                let (x, y) = (x - origin.x, y - origin.y);
                                if x < -reach
                                    || y < -reach
                                    || x >= width as f64 + reach
                                    || y >= height as f64 + reach
                                {
                                    continue;
                                }
                                let admitted = prepared.admits(
                                    &candidate(point, &self.populated),
                                    self.now,
                                );
                                let peopled = self
                                    .populated
                                    .get(point.id64 as i64)
                                    .is_some_and(|row| row.population > 0);
                                let Some((half, light)) = self.light.of(
                                    along.bucket(point),
                                    peopled,
                                    admitted,
                                ) else {
                                    continue;
                                };
                                let radius = crowding.radius(x, y, floor);
                                // A mark shares the system's light among the
                                // pixels it covers, as much light as a point.
                                let share = covered(x, y, radius, width, height)
                                    .count()
                                    .max(1)
                                    as u64;
                                for (px, py) in
                                    covered(x, y, radius, width, height)
                                {
                                    let pixel = CHANNELS * (py * width + px)
                                        + half.offset();
                                    for (channel, value) in
                                        light.iter().enumerate()
                                    {
                                        sums[pixel + channel].fetch_add(
                                            u64::from(*value) / share,
                                            Relaxed,
                                        );
                                    }
                                }
                            }
                            progress.counted.fetch_add(owned, Relaxed);
                        }
                    }
                });
            }
        });
        (!progress.cancelled.load(Relaxed)).then_some(sums)
    }
}

/// Whether any of a cell's box lands in `rect` of `lens`'s frame, in its
/// pixels: left, top, right, bottom
///
/// Its eight corners projected and the rectangle round them laid against the
/// part, which keeps a little more than it must and never less. A box
/// reaching behind the eye is kept: it has no rectangle on screen. No margin:
/// a system is drawn as a point, and lands in the part it projects into and
/// nowhere else.
fn lands_in(lens: &plan::Lens, id: CellId, rect: [f64; 4]) -> bool {
    let Some([low_x, low_y, high_x, high_y]) = footprint(lens, id) else {
        return true;
    };
    let [left, top, right, bottom] = rect;
    high_x >= left && low_x <= right && high_y >= top && low_y <= bottom
}

/// How crowded a part's sky is: systems a pixel, over tiles of [`TILE`]
/// pixels, from the cells' own counts spread over where each lands
///
/// **A point where the sky is crowded, a mark where it is not, and either
/// as much light.** Drawn a pixel each, a picture of a few hundred stars
/// close in is a few hundred pixels of millions, and shown whole each is
/// averaged with the dark around it into almost nothing. Drawn at the map's
/// floor everywhere, the crowded sky's systems land on one another and the
/// picture's resolution is spent on blur. So a system is drawn as large as
/// it can be without meeting its neighbours, by the room each has where it
/// lands, and no larger than the map would draw it — but its light spread
/// over the mark, not laid whole in each pixel of it: laid whole, every
/// sky sparse enough for marks came out as bright as one dense with them,
/// and the far views' thin edges glared. Read off every cell landing there
/// and not the system's own: looking into the bubble, cells stand dozens
/// deep over the same pixels, and each one's own room was a crowd's.
struct Crowding {
    across: usize,
    down: usize,
    /// Systems a pixel, tile by tile, row by row
    per_pixel: Vec<f32>,
}

/// The side of a [`Crowding`] tile, in a part's pixels
const TILE: usize = 16;

impl Crowding {
    fn of(
        lens: &plan::Lens,
        origin: UVec2,
        size: UVec2,
        cells: &[(CellId, u64)],
    ) -> Crowding {
        let across = (size.x as usize).div_ceil(TILE);
        let down = (size.y as usize).div_ceil(TILE);
        let mut systems = vec![0f64; across * down];
        let tile = TILE as f64;
        let origin = origin.as_dvec2();
        for &(id, owned) in cells {
            // A box reaching behind the eye lands nowhere in particular.
            let Some([left, top, right, bottom]) = footprint(lens, id) else {
                continue;
            };
            let (left, right) = (left - origin.x, right - origin.x);
            let (top, bottom) = (top - origin.y, bottom - origin.y);
            let area = ((right - left) * (bottom - top)).max(1.);
            let each = owned as f64 / area;
            let first = |low: f64| ((low / tile).floor().max(0.)) as usize;
            let last = |high: f64, of: usize| {
                ((high / tile).floor().max(0.) as usize).min(of - 1)
            };
            if right < 0. || bottom < 0. {
                continue;
            }
            for row in first(top)..=last(bottom, down) {
                let (low, high) = (row as f64 * tile, (row + 1) as f64 * tile);
                let tall = bottom.min(high) - top.max(low);
                if tall <= 0. {
                    continue;
                }
                for column in first(left)..=last(right, across) {
                    let (low, high) =
                        (column as f64 * tile, (column + 1) as f64 * tile);
                    let wide = right.min(high) - left.max(low);
                    if wide > 0. {
                        systems[row * across + column] += each * wide * tall;
                    }
                }
            }
        }
        let per_pixel = systems
            .into_iter()
            .map(|count| (count / (tile * tile)) as f32)
            .collect();
        Crowding { across, down, per_pixel }
    }

    /// The radius a system at `x`, `y` is drawn at: half the room each
    /// system has there, up to `floor`; under the room for a mark, a point
    fn radius(&self, x: f64, y: f64, floor: f64) -> f64 {
        let column = ((x.max(0.) as usize) / TILE).min(self.across - 1);
        let row = ((y.max(0.) as usize) / TILE).min(self.down - 1);
        let crowd = f64::from(self.per_pixel[row * self.across + column]);
        let radius = match crowd > 0. {
            true => (0.5 / crowd.sqrt()).min(floor),
            false => floor,
        };
        if radius < 0.75 { 0. } else { radius }
    }
}

/// The rectangle a cell's box lands in on `lens`'s frame, or [`None`]
/// where it reaches behind the eye
fn footprint(lens: &plan::Lens, id: CellId) -> Option<[f64; 4]> {
    let bounds = id.bounds();
    let (mut low, mut high) = ([f64::MAX; 2], [f64::MIN; 2]);
    for corner in 0..8 {
        let at = [
            if corner & 1 == 0 { bounds.min[0] } else { bounds.max[0] },
            if corner & 2 == 0 { bounds.min[1] } else { bounds.max[1] },
            if corner & 4 == 0 { bounds.min[2] } else { bounds.max[2] },
        ];
        let [x, y] = lens.project(at)?;
        low = [low[0].min(x), low[1].min(y)];
        high = [high[0].max(x), high[1].max(y)];
    }
    Some([low[0], low[1], high[0], high[1]])
}

/// The pixels of a `width` by `height` part a mark of `radius` at `x`, `y`
/// covers: the one it lands in for a point, every one whose middle is inside
/// the disc for a mark
fn covered(
    x: f64,
    y: f64,
    radius: f64,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let reach = radius.ceil() as i64;
    let (cx, cy) = (x.floor() as i64, y.floor() as i64);
    (-reach..=reach)
        .flat_map(move |dy| (-reach..=reach).map(move |dx| (dx, dy)))
        .filter(move |&(dx, dy)| {
            let (px, py) = ((cx + dx) as f64 + 0.5, (cy + dy) as f64 + 0.5);
            reach == 0 || (px - x).powi(2) + (py - y).powi(2) <= radius * radius
        })
        .map(move |(dx, dy)| (cx + dx, cy + dy))
        .filter(move |&(px, py)| {
            px >= 0 && py >= 0 && px < width as i64 && py < height as i64
        })
        .map(|(px, py)| (px as usize, py as usize))
}

/// How many threads sum a picture: all but two, which the map's own frame
/// and its render keep
fn workers() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .saturating_sub(2)
        .max(1)
}

/// The light one system lays into a picture, by its bucket along the axis
/// and whether anybody lives there: the map's own light for a mark
/// ([`system_light`] times the hue's
/// [`crate::map::galaxy::spawn::Hue::light`]), in fixed-point units with
/// the faintest at [`FAINTEST`]
#[derive(Clone)]
struct Light {
    color_by: ColorBy,
    /// By bucket, then unpeopled and peopled
    table: Vec<[[u32; 3]; 2]>,
    /// How many units the brightest system's light comes to, which the
    /// curve is read in
    unit: f64,
    /// The opacity what the filters exclude is drawn at, [`DimTo::opacity`];
    /// spent on the excluded light after its curve, not on each system
    opacity: f32,
}

impl Light {
    fn new(color_by: ColorBy, gains: &Gains, opacity: f32) -> Light {
        let lights: Vec<[Vec3; 2]> = (0..color_by.buckets())
            .map(|bucket| {
                let hue = color_by.hue_of(bucket);
                [false, true].map(|peopled| {
                    hue.light() * system_light(color_by, hue, peopled, gains)
                })
            })
            .collect();
        let levels =
            || lights.iter().flatten().map(|light| light.max_element());
        let brightest = levels().fold(f32::MIN_POSITIVE, f32::max);
        let faintest =
            levels().filter(|&level| level > 0.).fold(brightest, f32::min);
        let per_light = FAINTEST / faintest;
        let fixed =
            |light: Vec3| (light * per_light).round().as_uvec3().to_array();
        Light {
            color_by,
            table: lights.iter().map(|by| by.map(fixed)).collect(),
            unit: f64::from(brightest * per_light),
            opacity,
        }
    }

    /// Which half of a pixel one system lays its light into, and the light,
    /// or [`None`] where it is not drawn at all: a system the filters exclude
    /// with the dim at zero
    fn of(
        &self,
        bucket: usize,
        peopled: bool,
        admitted: bool,
    ) -> Option<(Half, [u32; 3])> {
        let half = match admitted {
            true => Half::Admitted,
            false if self.opacity > 0. => Half::Excluded,
            false => return None,
        };
        Some((half, self.table.get(bucket)?[usize::from(peopled)]))
    }
}

/// Which of a pixel's two sums a system's light goes to
///
/// **What the filters exclude is dimmed after its curve, not before.** The
/// map draws an excluded system as one faint mark, and a crowd of them is a
/// crowd of faint marks: never brighter than one. Summed with the rest and
/// dimmed a system at a time, the excluded outnumbered what was asked for —
/// the whole galaxy's unscanned under a key showing one class — and their
/// sum came up over it in grey. So each is summed apart and drawn on its own
/// curve, and the excluded laid under the admitted at the dim's opacity:
/// the backdrop the map's dim draws, however many stand in it.
#[derive(Clone, Copy)]
enum Half {
    Admitted,
    Excluded,
}

impl Half {
    /// Where its three channels start in a pixel's sums
    fn offset(self) -> usize {
        match self {
            Half::Admitted => 0,
            Half::Excluded => 3,
        }
    }
}

/// How many sums a pixel holds: red, green and blue, for each [`Half`]
const CHANNELS: usize = 6;

/// How a half's light is drawn: a log curve, white at [`WHITE_AT`] of the
/// base's lit pixels and bent so its middling lit pixel lands at [`MIDDLE`]
///
/// **Two ends, read off the view.** A curve whose knee stood at one
/// brightest system drew a sky of a fraction of a system a pixel — the
/// close views, every pixel of the bubble a few stars deep at most —
/// along its straight foot, a few percent of white: black. Bent where the
/// view's own middling pixel lies, a close view and a far one each come
/// out with their bulk at a grey that reads, and their brightest at white.
#[derive(Clone, Copy)]
struct Curve {
    /// The light at white, as light a pixel of the picture gathers
    white: f64,
    /// Where the curve bends: light well under it is drawn in proportion,
    /// light well over it by its logarithm
    knee: f64,
}

impl Curve {
    fn of(sums: &[AtomicU64], half: Half, per: f64, unit: f64) -> Curve {
        let mut lit: Vec<f64> = sums
            .as_chunks::<CHANNELS>()
            .0
            .iter()
            .map(|pixel| brightest(of(pixel, half), unit) / per)
            .filter(|&light| light > 0.)
            .collect();
        if lit.is_empty() {
            return Curve { white: 1., knee: 1. };
        }
        let mut at = |share: f64| {
            let at = ((lit.len() - 1) as f64 * share) as usize;
            *lit.select_nth_unstable_by(at, f64::total_cmp).1
        };
        let white = at(WHITE_AT);
        let middle = at(0.5);
        Curve { white, knee: knee(middle, white) }
    }

    /// Light on the curve, `0..=1`
    fn level(&self, light: f64) -> f64 {
        ((1. + light / self.knee).ln() / (1. + self.white / self.knee).ln())
            .min(1.)
    }
}

/// The knee that draws `middle` at [`MIDDLE`] on a curve white at `white`
///
/// The higher the knee, the straighter the curve and the darker its middle;
/// so it is found by halving, over its logarithm. Where even a straight
/// line draws the middle bright enough, the knee stands far off and the
/// curve is that line.
fn knee(middle: f64, white: f64) -> f64 {
    let level =
        |knee: f64| (1. + middle / knee).ln() / (1. + white / knee).ln();
    let (mut low, mut high) = ((white * 1e-9).ln(), (white * 1e9).ln());
    if level(high.exp()) >= MIDDLE {
        return high.exp();
    }
    for _ in 0..60 {
        let mid = (low + high) / 2.;
        // A lower knee bends harder, lifting the middle.
        if level(mid.exp()) < MIDDLE { high = mid } else { low = mid }
    }
    low.exp()
}

/// `half`'s three channels of a pixel
fn of(pixel: &[AtomicU64; CHANNELS], half: Half) -> &[AtomicU64] {
    &pixel[half.offset()..half.offset() + 3]
}

/// Light along its brightest channel, in the brightest system's: `unit`
/// fixed-point units to one
fn brightest(channels: &[AtomicU64], unit: f64) -> f64 {
    channels
        .iter()
        .map(|channel| channel.load(Relaxed) as f64)
        .fold(0., f64::max)
        / unit
}

/// One half of a pixel on its curve, its hue held, in linear light
fn curved(
    channels: &[AtomicU64],
    per: f64,
    curve: Curve,
    unit: f64,
) -> [f64; 3] {
    let light = brightest(channels, unit) / per;
    if light <= 0. {
        return [0.; 3];
    }
    // The brightest channel goes to the curve and the others with it, so a
    // mix keeps the color the key gives it.
    let scale = curve.level(light) / (light * per * unit);
    [0, 1, 2].map(|at| channels[at].load(Relaxed) as f64 * scale)
}

/// Color a part: the admitted on their curve, and the excluded on theirs
/// laid under them at the dim's opacity, as sRGB
fn tone(
    sums: &[AtomicU64],
    per: f64,
    curves: [Curve; 2],
    light: &Light,
) -> Vec<u8> {
    let mut rgba = Vec::with_capacity(sums.len() / CHANNELS * 4);
    for pixel in sums.as_chunks::<CHANNELS>().0 {
        let admitted =
            curved(of(pixel, Half::Admitted), per, curves[0], light.unit);
        let excluded =
            curved(of(pixel, Half::Excluded), per, curves[1], light.unit);
        for (shown, faint) in admitted.into_iter().zip(excluded) {
            let linear = shown + faint * f64::from(light.opacity);
            rgba.push((encode(linear as f32) * 255.).round() as u8);
        }
        rgba.push(255);
    }
    rgba
}

/// Linear light in `0..=1` encoded for an sRGB texture
fn encode(linear: f32) -> f32 {
    let linear = linear.clamp(0., 1.);
    if linear <= 0.003_130_8 {
        linear * 12.92
    } else {
        1.055 * linear.powf(1. / 2.4) - 0.055
    }
}

/// The name a picture is offered under: when it was saved
fn picture_name() -> String {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    format!("galos-enhanced-{stamp}.png")
}

/// Put the platform's save dialog up, opened on `dir` where a picture was
/// saved before, and answer where the picture is to go
///
/// Off the frame: the dialog is the platform's own, and waiting on it would
/// stop the map. A name typed without the extension has it added, the file
/// being a PNG whatever it is called.
fn ask_where(dir: Option<&Path>) -> Task<Option<PathBuf>> {
    let mut dialog = rfd::AsyncFileDialog::new()
        .set_title("Save the enhanced picture")
        .add_filter("PNG image", &["png"])
        .set_file_name(picture_name());
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    IoTaskPool::get().spawn(async move {
        let path = dialog.save_file().await?.path().to_owned();
        let png =
            path.extension().is_some_and(|it| it.eq_ignore_ascii_case("png"));
        Some(match png {
            true => path,
            false => {
                let mut named = path.into_os_string();
                named.push(".png");
                PathBuf::from(named)
            }
        })
    })
}

/// Start writing `picture` to `path`, a row of pieces at a time, or say why
/// it could not be
fn start_saving(
    picture: &mut Picture,
    path: PathBuf,
    saved: &mut Option<Result<PathBuf, String>>,
    last_dir: &mut Option<PathBuf>,
    commands: &mut Commands,
) {
    *saved = None;
    match Saving::start(picture, path) {
        Ok(saving) => {
            *last_dir = saving.path.parent().map(Path::to_owned);
            saving.ask_row(picture, commands);
            picture.saving = Some(saving);
        }
        Err(why) => *saved = Some(Err(why)),
    }
}

/// A picture being written: read back off the GPU a row of pieces at a time
/// on the frame, and encoded on a thread of its own
///
/// A row is read back, handed to the writer, and the next asked for once
/// the writer has taken it: what is held is a row being read, one waiting
/// and one being written, whatever the picture's size. Encoding a picture
/// of a gigabyte is seconds of deflate, which on the frame stopped the map
/// for as long; on its own thread it costs the map nothing, and counts the
/// lines it has written for the line to read as it goes.
struct Saving {
    path: PathBuf,
    /// The row of pieces being read back
    row: u32,
    /// Its pieces as they come back, by column
    read: Vec<Option<Vec<u8>>>,
    /// A whole row read back, waiting for the writer to have room
    held: Option<Vec<Vec<u8>>>,
    /// Rows to the writer, let go after the last so it finishes
    send: Option<mpsc::SyncSender<Vec<Vec<u8>>>>,
    /// Lines of the picture written so far
    written: Arc<AtomicU32>,
    /// The writer, until it has finished the file or given up on it
    writer: Option<std::thread::JoinHandle<Result<(), String>>>,
}

impl Saving {
    fn start(picture: &Picture, path: PathBuf) -> Result<Saving, String> {
        let file =
            std::fs::File::create(&path).map_err(|err| err.to_string())?;
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let mut encoder = png::Encoder::new(
            std::io::BufWriter::new(file),
            picture.physical.x * picture.scale,
            picture.physical.y * picture.scale,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let stream = encoder
            .write_header()
            .and_then(png::Writer::into_stream_writer)
            .map_err(|err| err.to_string())?;
        // Room for one row waiting while the writer works on another.
        let (send, rows) = mpsc::sync_channel(1);
        let written = Arc::new(AtomicU32::new(0));
        let writer = std::thread::Builder::new()
            .name("enhance-save".into())
            .spawn({
                let (path, written) = (path.clone(), written.clone());
                let piece = picture.physical;
                move || write(stream, rows, &path, piece, &written)
            })
            .map_err(|err| err.to_string())?;
        Ok(Saving {
            path,
            row: 0,
            read: vec![None; picture.scale as usize],
            held: None,
            send: Some(send),
            written,
            writer: Some(writer),
        })
    }

    /// How much of the picture is written, `0..=1`
    fn share(&self, picture: &Picture) -> f32 {
        let lines = picture.physical.y * picture.scale;
        self.written.load(Relaxed) as f32 / lines.max(1) as f32
    }

    /// Ask the row of pieces being read back off the GPU
    fn ask_row(&self, picture: &Picture, commands: &mut Commands) {
        let scale = picture.scale as usize;
        let row = self.row as usize;
        for (at, piece) in
            picture.pieces.iter().enumerate().skip(row * scale).take(scale)
        {
            if let Some(laid) = &piece.laid {
                commands.spawn((
                    Readback::texture(laid.image.clone()),
                    Reading(at),
                ));
            }
        }
    }

    /// Take a piece read back, and once its row is whole hold it for the
    /// writer
    fn took(&mut self, at: usize, data: Vec<u8>, picture: &Picture) {
        let scale = picture.scale as usize;
        if at / scale != self.row as usize {
            return;
        }
        self.read[at % scale] = Some(data);
        if self.read.iter().all(Option::is_some) {
            self.held =
                Some(self.read.iter_mut().flat_map(Option::take).collect());
        }
    }

    /// Hand the writer the row held, where it has room, and ask the next
    /// off the GPU; after the last, let the writer finish
    fn hand_on(&mut self, picture: &Picture, commands: &mut Commands) {
        let (Some(row), Some(send)) = (self.held.take(), &self.send) else {
            return;
        };
        match send.try_send(row) {
            Ok(()) => {
                self.row += 1;
                if self.row < picture.scale {
                    self.ask_row(picture, commands);
                } else {
                    self.send = None;
                }
            }
            Err(mpsc::TrySendError::Full(row)) => self.held = Some(row),
            // The writer gave up; why is its answer, read when it is joined.
            Err(mpsc::TrySendError::Disconnected(_)) => self.send = None,
        }
    }

    /// What the writer came to, once it has finished
    fn finished(&mut self) -> Option<Result<PathBuf, String>> {
        if !self.writer.as_ref()?.is_finished() {
            return None;
        }
        let ended = match self.writer.take()?.join() {
            Ok(written) => written,
            Err(_) => Err("the writer stopped".to_owned()),
        };
        Some(ended.map(|()| self.path.clone()))
    }
}

/// Write the rows of pieces as they come, line by line across each row,
/// counting the lines into `written`; the file is taken away again where it
/// could not be finished, so a picture left part written is not left lying
fn write(
    mut stream: png::StreamWriter<'static, std::io::BufWriter<std::fs::File>>,
    rows: mpsc::Receiver<Vec<Vec<u8>>>,
    path: &Path,
    piece: UVec2,
    written: &AtomicU32,
) -> Result<(), String> {
    let row_bytes = piece.x as usize * 4;
    let stride = RenderDevice::align_copy_bytes_per_row(row_bytes);
    let lines = piece.y as usize;
    let each = || -> Result<(), String> {
        // Until the frame lets go of its end: after the last row, or when
        // the picture is put away part way, which leaves the PNG short.
        for pieces in rows {
            for data in &pieces {
                if data.len() < stride * (lines - 1) + row_bytes {
                    return Err(format!(
                        "a piece came back {} bytes rather than {}",
                        data.len(),
                        stride * lines
                    ));
                }
            }
            for y in 0..lines {
                for data in &pieces {
                    stream
                        .write_all(&data[y * stride..y * stride + row_bytes])
                        .map_err(|err| err.to_string())?;
                }
                written.fetch_add(1, Relaxed);
            }
        }
        Ok(())
    };
    let ended =
        each().and_then(|()| stream.finish().map_err(|err| err.to_string()));
    if ended.is_err() {
        let _ = std::fs::remove_file(path);
    }
    ended
}

/// Which piece a read-back is of
#[derive(Component)]
struct Reading(usize);

/// Take a piece read back into the row being read, and hand the row to the
/// writer once it is whole
fn captured(
    read: On<ReadbackComplete>,
    readings: Query<&Reading>,
    mut commands: Commands,
    mut enhance: ResMut<Enhance>,
) {
    let Ok(Reading(at)) = readings.get(read.entity) else { return };
    // Once is enough: a readback left standing reads again every frame, and
    // one or two more may land before this despawn does.
    commands.entity(read.entity).try_despawn();
    let Some(picture) = &mut enhance.picture else { return };
    let Some(mut saving) = picture.saving.take() else { return };
    saving.took(*at, read.data.clone(), picture);
    saving.hand_on(picture, &mut commands);
    picture.saving = Some(saving);
}

/// Whether an enhanced picture covers the window
///
/// From the frame its base is laid until it is put away. The map's own
/// cameras stand down for as long, the picture standing over all of them:
/// the scene's here, the field's and the curve's in
/// `crate::map::paint::curve`.
#[derive(Resource, Default)]
pub(crate) struct Covered(pub(crate) bool);

/// Put the chrome away while there is a picture, as `I` does, and back as it
/// was once the picture is
///
/// A picture is read, not worked on: the bar's filters and rows would change
/// a map the picture no longer follows. What stays is what `I` leaves, the
/// bare color key the picture was drawn in, the rose and the picture's own
/// controls.
fn hide_chrome(
    enhance: Res<Enhance>,
    mut hidden: ResMut<crate::ui::hide::ChromeHidden>,
    // How the chrome stood when the picture was asked for.
    mut was: Local<Option<bool>>,
) {
    match (enhance.picture.is_some(), *was) {
        (true, None) => {
            *was = Some(hidden.0);
            hidden.0 = true;
        }
        // Held hidden, the eye in the corner included.
        (true, Some(_)) if !hidden.0 => hidden.0 = true,
        (false, Some(stood)) => {
            hidden.0 = stood;
            *was = None;
        }
        _ => {}
    }
}

/// Lay the parts that are down over the window, where the window is looking
/// into the picture
fn lay(
    enhance: Res<Enhance>,
    mut covered: ResMut<Covered>,
    mut overlay: Query<&mut Camera, With<OverlayCamera>>,
    mut scene: Query<&mut Camera, (With<OrbitCamera>, Without<OverlayCamera>)>,
    mut quads: Query<(&mut Transform, &mut Visibility), With<PieceQuad>>,
    mut materials: ResMut<Assets<PieceMaterial>>,
) {
    // Over the window once the base is down, and not before: until then
    // the map itself is the view as it stands.
    let active = enhance
        .picture
        .as_ref()
        .is_some_and(|picture| picture.base.laid.is_some());
    if covered.0 != active {
        covered.0 = active;
    }
    for mut camera in &mut overlay {
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
    for mut camera in &mut scene {
        if camera.is_active == active {
            camera.is_active = !active;
        }
    }
    let Some(picture) = &enhance.picture else { return };
    // How large a picture pixel is on the window, in window pixels, and so
    // how many of a piece's texels each window pixel takes in.
    let shown = picture.zoom / picture.scale as f32;
    let across = 1. / shown;
    let half = picture.viewport / 2.;
    let mut place = |part: &Part, rect: Rect, across: f32| {
        let Some(laid) = &part.laid else { return };
        let Ok((mut transform, mut visibility)) = quads.get_mut(laid.quad)
        else {
            return;
        };
        if *visibility != Visibility::Visible {
            *visibility = Visibility::Visible;
        }
        let centre = rect.center();
        let placed = Transform::from_xyz(
            centre.x - half.x,
            half.y - centre.y,
            transform.translation.z,
        )
        .with_scale(rect.size().extend(1.));
        if *transform != placed {
            *transform = placed;
        }
        if let Some(material) = materials.get(&laid.material)
            && material.footprint.across != across
            && let Some(mut material) = materials.get_mut(&laid.material)
        {
            material.footprint.across = across;
        }
    };
    let picture_rect = |low: Vec2, size: Vec2| {
        Rect::from_corners(
            (low - picture.corner) * shown,
            (low + size - picture.corner) * shown,
        )
    };
    // The base is the view as it stood, a window's worth of picture: it
    // covers the whole picture at a piece's resolution over `scale`.
    let whole = picture.viewport * picture.scale as f32;
    place(
        &picture.base,
        picture_rect(Vec2::ZERO, whole),
        across / picture.scale as f32,
    );
    for piece in &picture.pieces {
        let low =
            UVec2::new(piece.column, piece.row).as_vec2() * picture.viewport;
        place(piece, picture_rect(low, picture.viewport), across);
    }
}

/// A count of systems as the progress reads it
fn systems(count: u64) -> String {
    match count {
        0..1_000 => format!("{count}"),
        1_000..1_000_000 => format!("{:.0}k", count as f64 / 1e3),
        _ => format!("{:.1}M", count as f64 / 1e6),
    }
}

/// What stands over a picture: the pointer over it, and its own line at the
/// foot of the window
///
/// The line, rather than a card in a corner: the chrome is put away while
/// there is a picture, and what is left is read along the bottom edge
/// without covering the rose or the key. Asking for a picture is
/// [`launcher`]'s, in the chrome's own column.
fn controls(
    mut contexts: EguiContexts,
    mut enhance: ResMut<Enhance>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let margin = crate::ui::MARGIN;
    let enhance = &mut *enhance;

    // While there is a picture, the window under the chrome is the
    // picture's: the map takes no pointer, and in a shown picture the wheel
    // and a drag look about in it.
    if let Some(picture) = &mut enhance.picture {
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("enhance-picture"))
            .order(egui::Order::Background)
            .fixed_pos(screen.min)
            .constrain(false)
            .show(ctx, |ui| {
                let response =
                    ui.allocate_rect(screen, egui::Sense::click_and_drag());
                if enhance.phase != Phase::Shown {
                    return;
                }
                if response.dragged() {
                    let by = response.drag_delta();
                    picture.pan(Vec2::new(by.x, by.y));
                }
                if response.hovered() {
                    let scroll = ui.input(|input| input.smooth_scroll_delta.y);
                    if scroll != 0.
                        && let Some(at) = response.hover_pos()
                    {
                        let at = at - screen.min;
                        picture.zoom_about(
                            Vec2::new(at.x, at.y),
                            (scroll / 200.).exp(),
                        );
                    }
                }
            });
    }

    // The picture's own line. Nothing while there is no picture: the
    // launcher in the chrome's column is what asks for one.
    if enhance.phase == Phase::Off {
        return Ok(());
    }
    crate::ui::zone("enhance")
        .anchor(egui::Align2::CENTER_BOTTOM, egui::vec2(0., -margin))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let asked = match enhance.phase {
                        Phase::Off => None,
                        Phase::Drawing => {
                            if let Some(picture) = &enhance.picture
                                && let Some(job) = &picture.job
                            {
                                drawing(ui, picture, job);
                            }
                            ui.small_button("cancel")
                                .clicked()
                                .then_some(Ask::Close)
                        }
                        Phase::Shown => {
                            enhance.picture.as_ref().and_then(|picture| {
                                shown(
                                    ui,
                                    picture,
                                    enhance.saved.as_ref(),
                                    enhance.saved_at,
                                )
                            })
                        }
                    };
                    if asked.is_some() {
                        enhance.asked = asked;
                    }
                });
            });
        });
    Ok(())
}

/// How far a picture being drawn has got, along one line: the part under
/// way, the systems summed against what the cells in view own, and what is
/// left; the rate under the pointer
fn drawing(ui: &mut egui::Ui, picture: &Picture, job: &Job) {
    let progress = &job.progress;
    let parts = picture.pieces.len() + 1;
    let done = progress.parts.load(Relaxed).min(parts);
    let part = match done {
        0 => "the view".to_owned(),
        n if n < parts => format!("piece {n} of {}", parts - 1),
        _ => "laying it down".to_owned(),
    };
    ui.label(format!("{}× enhance · {part}", picture.scale));
    let bar = |share: f32| egui::ProgressBar::new(share).desired_width(140.);
    if !progress.planned.load(Relaxed) {
        ui.add(bar(0.));
        ui.weak("finding the cells in view");
        return;
    }
    let counted = progress.counted.load(Relaxed);
    let total = progress.total.load(Relaxed).max(1);
    let elapsed = job.started.elapsed();
    let rate = counted as f64 / elapsed.as_secs_f64().max(1e-3);
    ui.add(bar((counted as f32 / total as f32).clamp(0., 1.)))
        .on_hover_text(format!("{} systems a second", systems(rate as u64)));
    let left = (counted > 0)
        .then(|| (total - counted.min(total)) as f64 / rate.max(1.));
    ui.weak(format!(
        "{} of {} systems{}",
        systems(counted),
        systems(total),
        left.map_or(String::new(), |left| match left < 10. {
            true => format!(" · {left:.1}s left"),
            false => format!(" · {left:.0}s left"),
        }),
    ));
}

/// How long the line says a picture was saved
///
/// Long enough to be read, and gone after: it is news, and the path stays
/// under the pointer of nothing once it is. Why one was not saved stays,
/// being something to act on.
const SAVED_FOR: Duration = Duration::from_secs(3);

/// A shown picture's line: how far into it the window is, saving it and
/// putting it away; what is asked of it, where anything was
fn shown(
    ui: &mut egui::Ui,
    picture: &Picture,
    saved: Option<&Result<PathBuf, String>>,
    saved_at: Option<Instant>,
) -> Option<Ask> {
    ui.label(format!("{}× enhance · {:.1}× zoom", picture.scale, picture.zoom));
    let mut asked = None;
    match (&picture.saving, &picture.asking_where) {
        (Some(saving), _) => {
            ui.add_enabled(
                false,
                egui::Button::new(format!(
                    "saving {:.0}%",
                    saving.share(picture) * 100.
                ))
                .small(),
            );
        }
        (None, Some(_)) => {
            ui.add_enabled(false, egui::Button::new("choosing where…").small());
        }
        (None, None) => {
            if ui.small_button("save png").clicked() {
                asked = Some(Ask::Save);
            }
        }
    }
    match saved {
        Some(Ok(path))
            if saved_at.is_some_and(|at| at.elapsed() < SAVED_FOR) =>
        {
            ui.weak("saved").on_hover_text(path.display().to_string());
        }
        Some(Err(why)) => {
            ui.weak("not saved").on_hover_text(why);
        }
        // Nothing saved, or saved long enough ago to be old news.
        _ => {}
    }
    if ui.small_button("close").clicked() {
        asked = Some(Ask::Close);
    }
    asked
}

/// How large the launcher's mark is drawn, the eye's own size
const MARK: f32 = 18.;

/// The button that asks for a picture, in the chrome's own column under the
/// eye, and the scales it offers
///
/// A viewfinder, painted as the eye and the gear above it are, and nothing
/// else standing on the map: a click puts the scales out beside it, and a
/// scale clicked asks for the picture there and then. `P` asks for one at
/// the scale last picked. Nothing while there is a picture, the chrome
/// being put away then.
pub(crate) fn launcher(
    ctx: &egui::Context,
    at: egui::Pos2,
    enhance: &mut Enhance,
) {
    if enhance.phase != Phase::Off {
        return;
    }
    let mark = crate::ui::zone("enhance-launcher")
        .fixed_pos(at)
        .show(ctx, |ui| {
            let (rect, response) = ui.allocate_exact_size(
                egui::Vec2::splat(MARK),
                egui::Sense::click(),
            );
            let ink = ui.style().interact(&response).fg_stroke.color;
            paint_viewfinder(ui.painter(), rect, ink);
            response.on_hover_text("Enhance this view (P)")
        })
        .inner;
    if mark.clicked() {
        enhance.choosing = !enhance.choosing;
    }
    if !enhance.choosing {
        return;
    }
    let margin = crate::ui::MARGIN;
    let scales = crate::ui::zone("enhance-scales")
        .pivot(egui::Align2::LEFT_CENTER)
        .fixed_pos(egui::pos2(
            mark.rect.right() + margin / 2.,
            mark.rect.center().y,
        ))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("enhance");
                    for offered in SCALES {
                        if ui
                            .selectable_label(
                                offered == enhance.scale,
                                format!("{offered}×"),
                            )
                            .on_hover_text(format!(
                                "Every system in this view, drawn {offered} \
                                 windows across"
                            ))
                            .clicked()
                        {
                            enhance.scale = offered;
                            enhance.asked = Some(Ask::Start);
                            enhance.choosing = false;
                        }
                    }
                });
                if let Some(Err(why)) = &enhance.saved {
                    ui.weak(format!("the last was not drawn: {why}"));
                }
            });
        });
    if scales.response.clicked_elsewhere() && !mark.clicked() {
        enhance.choosing = false;
    }
}

/// A viewfinder in `rect`: four corners and a point between them
///
/// Not a magnifier, which stands beside the search box and reads as one.
fn paint_viewfinder(
    painter: &egui::Painter,
    rect: egui::Rect,
    ink: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.5_f32, ink);
    let frame = rect.shrink(rect.width() * 0.08);
    let arm = frame.width() * 0.3;
    for (corner, x, y) in [
        (frame.left_top(), 1., 1.),
        (frame.right_top(), -1., 1.),
        (frame.left_bottom(), 1., -1.),
        (frame.right_bottom(), -1., -1.),
    ] {
        painter
            .line_segment([corner, corner + egui::vec2(x * arm, 0.)], stroke);
        painter
            .line_segment([corner, corner + egui::vec2(0., y * arm)], stroke);
    }
    painter.circle_filled(frame.center(), frame.width() * 0.1, ink);
}
