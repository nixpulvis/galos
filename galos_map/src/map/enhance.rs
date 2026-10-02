//! Enhance: the view drawn again at several times the window's resolution,
//! a piece at a time, over the map it was drawn from
//!
//! **One picture, larger than the window, from the same eye.** The map's
//! detail is measured in pixels: the walk splits a cell once its contents
//! are half a pixel across, merges marks closer than [`MERGE_PX`] and spends
//! the frame's marks by its area. So a picture `scale` windows across from
//! the same eye is the same view with that much more in it, and nothing
//! about the map has to change for it but the size it believes the screen
//! to be.
//!
//! **Drawn a window at a time.** The picture is cut into pieces the size of
//! the window and each is drawn as an ordinary frame, off-centre through
//! [`Frame`]: the walk plans the whole picture once, so every piece strikes
//! the same share and the pieces meet without a seam, and each piece reads
//! and builds only the cells that land in it ([`Window`]), so a piece costs
//! a frame's worth of systems whatever the picture's size.
//!
//! **Where it is drawn, over the map it is drawn from.** The camera stands
//! still while it works and each piece is laid down in its place as it
//! finishes, the middle first, until the picture covers the map. The view
//! the button was pressed on is drawn first into a picture of its own and
//! stands underneath, so the map's own frame — which is busy drawing pieces
//! out of sight — is never what the window shows.
//!
//! **And it stays where it is.** The picture is the view from one eye, so
//! looking closer into it is a narrower lens from the same eye, which is
//! what [`Frame`] is: the wheel and a drag move about in the picture, the
//! map underneath follows to the pixel, and the names and rings drawn over
//! it stand on what they name at every magnification. Anything that would
//! move the eye puts the picture away.
//!
//! [`MERGE_PX`]: galos_index::read::walk::MERGE_PX
//! [`Window`]: crate::map::galaxy::plan::Window

use crate::map::camera::{Frame, OrbitCamera};
use crate::map::filter::Filters;
use crate::map::galaxy::spawn::{Building, ColorBy, PendingSpawns};
use crate::map::galaxy::walk::{BoundedTasks, Reconciled};
use crate::map::paint::sizing::View;
use crate::map::schedule::{MapSet, PaintSet};
use bevy::asset::embedded_asset;
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers};
use bevy::camera::{Hdr, ImageRenderTarget, RenderTarget, ScalingMode};
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::ecs::system::SystemParam;
use bevy::image::ImageSampler;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType, TextureFormat};
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};
use bevy::shader::ShaderRef;
use bevy::window::{PrimaryWindow, WindowRef};
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::path::PathBuf;

pub fn plugin(app: &mut App) {
    embedded_asset!(app, "enhance.wgsl");
    app.add_plugins(MaterialPlugin::<PieceMaterial>::default());
    app.init_resource::<Enhance>();
    app.add_systems(Startup, spawn_overlay);
    // The camera stands still while a picture is drawn or shown: the
    // picture is the view from where it stood, and every piece and every
    // look into it is measured from that one eye.
    app.configure_sets(Update, MapSet::Camera.run_if(idle));
    // After the frame has been drawn into the map's own resources, so a
    // piece is judged on what this frame worked out, and the piece it moves
    // to is the next frame's.
    app.add_systems(Update, drive.after(MapSet::Present));
    // After the field's own routing, which this overrides while a picture
    // is being drawn.
    app.add_systems(
        Update,
        (route, lay)
            .chain()
            .after(drive)
            .after(crate::map::paint::curve::Routed),
    );
    app.add_systems(EguiPrimaryContextPass, controls.in_set(PaintSet::Ui));
    // Under everything else in the pass, so the map's own annotations
    // follow it.
    app.add_systems(
        EguiPrimaryContextPass,
        hide_annotations.before(PaintSet::Map),
    );
    app.add_observer(captured);
    script(app);
}

/// An enhance run with nobody at the window, from `GALOS_ENHANCE`
///
/// `GALOS_ENHANCE=3` enhances the view the map opens on three windows across
/// once it has loaded, saves the picture, and with `GALOS_ENHANCE_EXIT` set
/// closes the map when it is written; the window's own capture beside it is
/// `GALOS_SHOT`'s, `crate::dev::shot`. What `media.sh` would record a
/// picture with, and how a change to this is seen working without a person
/// pressing the button.
fn script(app: &mut App) {
    let Ok(scale) = std::env::var("GALOS_ENHANCE") else { return };
    let scale =
        scale.parse().unwrap_or(SCALE).clamp(*SCALES.start(), *SCALES.end());
    let exit = std::env::var("GALOS_ENHANCE_EXIT").is_ok();
    app.insert_resource(Scripted {
        scale,
        exit,
        loaded: 0,
        asked: false,
        told: false,
    });
    app.add_systems(
        Update,
        scripted
            .before(drive)
            .run_if(in_state(crate::map::index::load::Opening::Drawn)),
    );
}

/// Where a scripted run is; see [`script`]
#[derive(Resource)]
struct Scripted {
    scale: u32,
    exit: bool,
    /// Frames in a row the opening view has read as loaded
    loaded: u32,
    /// Whether the picture has been asked for
    asked: bool,
    /// Whether where it was saved has been said
    told: bool,
}

fn scripted(
    mut scripted: ResMut<Scripted>,
    mut enhance: ResMut<Enhance>,
    loading: Loading,
    mut exit: MessageWriter<AppExit>,
) {
    if !scripted.asked {
        scripted.loaded = if loading.done() { scripted.loaded + 1 } else { 0 };
        // A second's worth of frames loaded, so the opening view has
        // settled rather than paused between two waves of reads.
        if scripted.loaded >= 60 {
            enhance.scale = scripted.scale;
            enhance.asked = Some(Ask::Start);
            scripted.asked = true;
        }
        return;
    }
    match (&enhance.phase, &enhance.saved) {
        (Phase::Shown, None)
            if enhance
                .picture
                .as_ref()
                .is_some_and(|it| it.reading.is_none()) =>
        {
            enhance.asked = Some(Ask::Save);
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

/// Whether nothing is being enhanced, for the systems that stand still
/// while something is
pub(crate) fn idle(enhance: Res<Enhance>) -> bool {
    matches!(enhance.phase, Phase::Off)
}

/// How many windows across a picture is, to begin with
const SCALE: u32 = 3;

/// The scales offered, windows across
///
/// One window is the view as it is, and nothing to draw. Past six, a
/// picture is thirty-six windows held as textures at once, which at a
/// large window is gigabytes.
const SCALES: std::ops::RangeInclusive<u32> = 2..=6;

/// Frames a piece is given before whether it has loaded is asked
///
/// bevy works the lens out a frame after the piece is set, and the plan,
/// the reads it asks for and the walk over what landed each follow the
/// frame before: a piece that read as loaded before all of that had
/// happened once would be the last piece's systems in this one's place.
const SETTLING: u32 = 6;

/// Frames in a row a piece has to read as loaded before it is taken
///
/// A view loads in waves — the reads land, the walk offers what they hold,
/// the builds come back — and between two of them a frame can find nothing
/// outstanding without being finished.
const STEADY: u32 = 4;

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
}

impl Default for Enhance {
    fn default() -> Self {
        Enhance {
            phase: Phase::Off,
            scale: SCALE,
            picture: None,
            asked: None,
            saved: None,
        }
    }
}

/// Where a picture is
#[derive(Clone, Copy, Debug, PartialEq)]
enum Phase {
    /// No picture: the map as it always is
    Off,
    /// Drawing one, a piece at a time
    Drawing(Drawing),
    /// Drawn, and laid over the map
    Shown,
}

/// Which piece is being drawn and how far it has got
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drawing {
    /// The piece, by its place in [`Picture::order`]; [`None`] is the view
    /// as the button found it, drawn first to stand under the rest
    piece: Option<usize>,
    /// Frames since the piece was set
    frames: u32,
    /// Frames in a row it has read as loaded
    steady: u32,
    /// The most it has had outstanding, which how far it has got is
    /// measured against
    most: usize,
    /// What it has outstanding now
    left: usize,
}

impl Drawing {
    fn of(piece: Option<usize>) -> Drawing {
        Drawing { piece, frames: 0, steady: 0, most: 0, left: 0 }
    }
}

/// What the controls asked for
#[derive(Clone, Copy, Debug, PartialEq)]
enum Ask {
    Start,
    /// Stop drawing, or put the picture away
    Close,
    Save,
}

/// A picture several windows across, in pieces a window each
struct Picture {
    /// Windows across
    scale: u32,
    /// The window it was started in, logical pixels
    viewport: Vec2,
    /// And physical
    physical: UVec2,
    /// The window's scale factor
    scale_factor: f32,
    /// The view as it stood, drawn first and laid under the pieces
    base: Part,
    /// The pieces, row by row
    pieces: Vec<Part>,
    /// The order they are drawn in: out from the middle
    order: Vec<usize>,
    /// How far into the picture the window is looking: how many times the
    /// picture's own size on the window it is shown at, from one, the
    /// whole picture on the window, up to [`Picture::scale`], a piece's
    /// pixel to the window's
    zoom: f32,
    /// Where the window's top left stands in the picture, in its pixels
    corner: Vec2,
    /// What the picture was drawn under, which changing puts it away
    against: Against,
    /// The pieces read back so far, for saving: the base first, then the
    /// pieces by number
    reading: Option<Vec<Option<Image>>>,
}

/// One piece of a picture: the texture it is drawn into and the quad it is
/// laid over the map with
struct Part {
    image: Handle<Image>,
    material: Handle<PieceMaterial>,
    quad: Entity,
    /// Where it stands in the picture, in pieces
    column: u32,
    row: u32,
    done: bool,
}

/// What a picture is drawn under: the reading of the map it shows
#[derive(Clone, Copy, Debug, PartialEq)]
struct Against {
    view: View,
    color_by: ColorBy,
    window: UVec2,
}

/// Whether the map has drawn everything the view asks of it
///
/// Nothing queued to read or on the wire, nothing waiting to be built or
/// being built, and the walk's last pass finished rather than put off. The
/// plan and the field are worked out within the frame and need no asking.
#[derive(SystemParam)]
pub(crate) struct Loading<'w> {
    reads: Res<'w, BoundedTasks>,
    spawns: Res<'w, PendingSpawns>,
    building: Res<'w, Building>,
    reconciled: Res<'w, Reconciled>,
}

impl Loading<'_> {
    /// How much the view is still waiting on: cells and systems, together
    fn outstanding(&self) -> usize {
        self.reads.outstanding()
            + self.spawns.queued()
            + self.building.outstanding()
    }

    /// Whether it has all been drawn
    pub(crate) fn done(&self) -> bool {
        self.reads.is_empty()
            && self.spawns.is_empty()
            && self.building.outstanding() == 0
            && self.reconciled.0
    }
}

/// The render layer the picture is laid over the window on
///
/// Its own, past every layer the map draws on. See
/// [`crate::map::camera::FIELD_LAYER`] for why a number is never reused.
const LAYER: usize = 10;

/// The camera that lays the picture over the window
#[derive(Component)]
struct OverlayCamera;

/// One piece's quad
#[derive(Component)]
struct PieceQuad;

/// What lays a piece over the window, averaging the texels each pixel
/// covers
#[derive(Asset, TypePath, AsBindGroup, Clone)]
pub(crate) struct PieceMaterial {
    #[uniform(0)]
    footprint: Footprint,
    #[texture(1)]
    #[sampler(2)]
    piece: Handle<Image>,
}

/// How many of a piece's texels a pixel of the window covers, along
/// either axis
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
/// window: while the pieces are drawn the map's cameras draw elsewhere, and
/// once they are all down the picture covers whatever the map draws.
fn spawn_overlay(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Hdr,
        Tonemapping::None,
        Msaa::Off,
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

/// Carry out what was asked, and move the picture on a piece when the one
/// being drawn has loaded
#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    mut enhance: ResMut<Enhance>,
    mut cameras: Query<&mut OrbitCamera>,
    window: Query<&Window, With<PrimaryWindow>>,
    view: Res<View>,
    color_by: Res<ColorBy>,
    filters: Res<Filters>,
    loading: Loading,
    keys: Res<ButtonInput<KeyCode>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<PieceMaterial>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let Ok(mut orbit) = cameras.single_mut() else { return };
    let Ok(window) = window.single() else { return };
    let enhance = &mut *enhance;

    // Escape puts away whatever is under way, as it does anything else the
    // map opens.
    let mut asked = enhance.asked.take();
    if keys.just_pressed(KeyCode::Escape) && enhance.phase != Phase::Off {
        asked = Some(Ask::Close);
    }

    // **What the picture shows has moved.** A picture is one reading of the
    // map; drawn on under another, its pieces would not agree.
    let against = Against {
        view: *view,
        color_by: *color_by,
        window: window.physical_size(),
    };
    if let Some(picture) = &enhance.picture
        && (picture.against != against || filters.is_changed())
        && asked != Some(Ask::Start)
    {
        asked = Some(Ask::Close);
    }

    match asked {
        Some(Ask::Start) if enhance.phase == Phase::Off => {
            let picture = Picture::new(
                enhance.scale,
                window,
                against,
                &mut commands,
                &mut images,
                &mut materials,
                &mut meshes,
            );
            enhance.picture = Some(picture);
            enhance.phase = Phase::Drawing(Drawing::of(None));
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
                && picture.reading.is_none()
            {
                picture.read_back(&mut commands);
            }
        }
        _ => {}
    }

    let Some(picture) = &mut enhance.picture else { return };
    match &mut enhance.phase {
        Phase::Off => {}
        Phase::Drawing(drawing) => {
            drawing.frames += 1;
            let left = loading.outstanding();
            drawing.left = left;
            drawing.most = drawing.most.max(left);
            let loaded = drawing.frames > SETTLING && loading.done();
            drawing.steady = if loaded { drawing.steady + 1 } else { 0 };
            if drawing.steady < STEADY {
                return;
            }
            // This piece is down: lay it, and set the next.
            match drawing.piece {
                None => picture.base.done = true,
                Some(at) => picture.pieces[picture.order[at]].done = true,
            }
            let next = drawing.piece.map_or(0, |at| at + 1);
            if next < picture.order.len() {
                *drawing = Drawing::of(Some(next));
                orbit.frame = picture.frame_of(picture.order[next]);
            } else {
                enhance.phase = Phase::Shown;
                orbit.frame = picture.looking();
            }
        }
        Phase::Shown => {
            let looking = picture.looking();
            if orbit.frame != looking {
                orbit.frame = looking;
            }
        }
    }
}

impl Picture {
    fn new(
        scale: u32,
        window: &Window,
        against: Against,
        commands: &mut Commands,
        images: &mut Assets<Image>,
        materials: &mut Assets<PieceMaterial>,
        meshes: &mut Assets<Mesh>,
    ) -> Picture {
        let physical = window.physical_size().max(UVec2::ONE);
        let viewport = Vec2::new(window.width(), window.height());
        let mesh = meshes.add(Rectangle::new(1., 1.));
        let mut part = |column: u32, row: u32, depth: f32| {
            let mut image = Image::new_target_texture(
                physical.x,
                physical.y,
                TextureFormat::Rgba8UnormSrgb,
                None,
            );
            image.sampler = ImageSampler::linear();
            let image = images.add(image);
            let material = materials.add(PieceMaterial {
                footprint: Footprint::default(),
                piece: image.clone(),
            });
            let quad = commands
                .spawn((
                    Mesh3d(mesh.clone()),
                    MeshMaterial3d(material.clone()),
                    RenderLayers::layer(LAYER),
                    NoFrustumCulling,
                    Transform::from_xyz(0., 0., depth),
                    Visibility::Hidden,
                    PieceQuad,
                ))
                .id();
            Part { image, material, quad, column, row, done: false }
        };
        // The pieces stand nearer the overlay's camera than the base, so each
        // covers it where it is laid.
        let base = part(0, 0, -2.);
        let pieces: Vec<Part> = (0..scale)
            .flat_map(|row| (0..scale).map(move |column| (column, row)))
            .map(|(column, row)| part(column, row, -1.))
            .collect();
        // Out from the middle, which is what the view was pointed at.
        let middle = (scale as f32 - 1.) / 2.;
        let mut order: Vec<usize> = (0..pieces.len()).collect();
        order.sort_by(|&a, &b| {
            let away = |piece: &Part| {
                Vec2::new(
                    piece.column as f32 - middle,
                    piece.row as f32 - middle,
                )
                .length_squared()
            };
            away(&pieces[a]).total_cmp(&away(&pieces[b]))
        });
        Picture {
            scale,
            viewport,
            physical,
            scale_factor: window.scale_factor(),
            base,
            pieces,
            order,
            zoom: 1.,
            corner: Vec2::ZERO,
            against,
            reading: None,
        }
    }

    /// The frame the `piece`th piece is drawn in
    fn frame_of(&self, piece: usize) -> Frame {
        let piece = &self.pieces[piece];
        Frame {
            scale: self.scale as f32,
            corner: UVec2::new(piece.column, piece.row).as_vec2()
                * self.viewport,
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

    /// The part being drawn into, if any
    fn drawing<'a>(&'a self, phase: &Phase) -> Option<&'a Part> {
        match phase {
            Phase::Drawing(Drawing { piece: None, .. }) => Some(&self.base),
            Phase::Drawing(Drawing { piece: Some(at), .. }) => {
                Some(&self.pieces[self.order[*at]])
            }
            _ => None,
        }
    }

    /// Zoom about `at`, a point on the window in logical pixels, by
    /// `factor`, holding the picture's point under it where it is
    fn zoom_about(&mut self, at: Vec2, factor: f32) {
        let before = self.zoom / self.scale as f32;
        let under = self.corner + at / before;
        self.zoom = (self.zoom * factor).clamp(1., self.scale as f32);
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

    /// Ask the pieces back off the GPU, for [`captured`] to put together
    fn read_back(&mut self, commands: &mut Commands) {
        self.reading = Some(vec![None; self.pieces.len()]);
        for (at, piece) in self.pieces.iter().enumerate() {
            commands
                .spawn(Screenshot::image(piece.image.clone()))
                .insert(Reading(at));
        }
    }

    /// Let the pieces' textures and quads go
    fn put_away(
        self,
        commands: &mut Commands,
        images: &mut Assets<Image>,
        materials: &mut Assets<PieceMaterial>,
    ) {
        for part in std::iter::once(self.base).chain(self.pieces) {
            commands.entity(part.quad).despawn();
            images.remove(&part.image);
            materials.remove(&part.material);
        }
    }

    /// Put the read-back pieces together into one picture and write it
    fn save(&self, read: &[Option<Image>]) -> Result<PathBuf, String> {
        let width = self.physical.x * self.scale;
        let height = self.physical.y * self.scale;
        let mut pixels = vec![0u8; width as usize * height as usize * 4];
        for (piece, image) in self.pieces.iter().zip(read) {
            let image = image.as_ref().ok_or("a piece did not come back")?;
            let data = image.data.as_ref().ok_or("a piece came back empty")?;
            let size = image.size();
            if size != self.physical {
                return Err(format!(
                    "a piece came back {size} rather than {}",
                    self.physical
                ));
            }
            // Four bytes a texel, whichever way round the channels are.
            let swap = matches!(
                image.texture_descriptor.format,
                TextureFormat::Bgra8Unorm | TextureFormat::Bgra8UnormSrgb
            );
            let row_bytes = size.x as usize * 4;
            for y in 0..size.y as usize {
                let from = &data[y * row_bytes..(y + 1) * row_bytes];
                let into_y = piece.row as usize * size.y as usize + y;
                let into_x = piece.column as usize * size.x as usize;
                let at = (into_y * width as usize + into_x) * 4;
                let into = &mut pixels[at..at + row_bytes];
                into.copy_from_slice(from);
                for texel in into.chunks_exact_mut(4) {
                    if swap {
                        texel.swap(0, 2);
                    }
                    texel[3] = 255;
                }
            }
        }
        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = PathBuf::from(format!("galos-enhanced-{stamp}.png"));
        let file =
            std::fs::File::create(&path).map_err(|err| err.to_string())?;
        let mut encoder =
            png::Encoder::new(std::io::BufWriter::new(file), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
        let mut writer =
            encoder.write_header().map_err(|err| err.to_string())?;
        writer.write_image_data(&pixels).map_err(|err| err.to_string())?;
        writer.finish().map_err(|err| err.to_string())?;
        Ok(std::fs::canonicalize(&path).unwrap_or(path))
    }
}

/// Which piece a read-back screenshot is of
#[derive(Component)]
struct Reading(usize);

/// Take a piece read back, and save the picture once every piece is
fn captured(
    capture: On<ScreenshotCaptured>,
    readings: Query<&Reading>,
    mut enhance: ResMut<Enhance>,
) {
    let Ok(Reading(at)) = readings.get(capture.entity) else { return };
    let enhance = &mut *enhance;
    let Some(picture) = &mut enhance.picture else { return };
    let Some(reading) = &mut picture.reading else { return };
    if let Some(slot) = reading.get_mut(*at) {
        *slot = Some(capture.image.clone());
    }
    if reading.iter().all(Option::is_some) {
        let read = picture.reading.take().unwrap_or_default();
        enhance.saved = Some(picture.save(&read));
    }
}

/// The map's cameras that draw onto the window
type OntoWindow = Or<(
    With<OrbitCamera>,
    With<crate::map::paint::curve::CurveCamera>,
    With<crate::map::paint::field::FieldCamera>,
)>;

/// Send the map's cameras where the phase draws: into the piece being
/// drawn, or onto the window
///
/// The scene's camera and the curve's draw onto the window, and in the
/// realistic view the field's does too; while a piece is drawn all three
/// draw into its texture instead, the window being the overlay's. The
/// field's camera in the map view draws into a target of its own and is
/// left alone.
fn route(
    enhance: Res<Enhance>,
    mut overlay: Query<&mut Camera, With<OverlayCamera>>,
    mut cameras: Query<(Entity, &mut RenderTarget), OntoWindow>,
    // The cameras sent into a piece, which are the ones to send back.
    mut ours: Local<Vec<Entity>>,
) {
    let active = enhance.phase != Phase::Off;
    for mut camera in &mut overlay {
        if camera.is_active != active {
            camera.is_active = active;
        }
    }
    let into = enhance.picture.as_ref().and_then(|picture| {
        picture
            .drawing(&enhance.phase)
            .map(|part| (part.image.clone(), picture.scale_factor))
    });
    let Some((image, scale_factor)) = into else {
        for entity in ours.drain(..) {
            if let Ok((_, mut target)) = cameras.get_mut(entity) {
                *target = RenderTarget::Window(WindowRef::Primary);
            }
        }
        return;
    };
    for (entity, mut target) in &mut cameras {
        let send = match &*target {
            RenderTarget::Window(_) => true,
            // Into the last piece, and on to this one.
            RenderTarget::Image(held) => {
                ours.contains(&entity) && held.handle != image
            }
            _ => false,
        };
        if send {
            *target = RenderTarget::Image(ImageRenderTarget {
                handle: image.clone(),
                scale_factor,
            });
            if !ours.contains(&entity) {
                ours.push(entity);
            }
        }
    }
}

/// Lay the pieces that are down over the window, where the window is
/// looking into the picture
fn lay(
    enhance: Res<Enhance>,
    mut quads: Query<(&mut Transform, &mut Visibility), With<PieceQuad>>,
    mut materials: ResMut<Assets<PieceMaterial>>,
) {
    let Some(picture) = &enhance.picture else { return };
    // How large a picture pixel is on the window, in window pixels, and so
    // how many of a piece's texels each window pixel takes in.
    let shown = picture.zoom / picture.scale as f32;
    let across = 1. / shown;
    let half = picture.viewport / 2.;
    let mut place = |part: &Part, shown: bool, rect: Rect, across: f32| {
        let Ok((mut transform, mut visibility)) = quads.get_mut(part.quad)
        else {
            return;
        };
        let wanted =
            if shown { Visibility::Visible } else { Visibility::Hidden };
        if *visibility != wanted {
            *visibility = wanted;
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
        if let Some(material) = materials.get(&part.material)
            && material.footprint.across != across
            && let Some(mut material) = materials.get_mut(&part.material)
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
    // covers the whole picture at a piece's resolution over `scale`. Shown
    // from the first frame, which is the frame it is first drawn in: the
    // overlay's camera draws after the map's, and the view was on the
    // window, loaded, when the button was pressed.
    let whole = picture.viewport * picture.scale as f32;
    place(
        &picture.base,
        true,
        picture_rect(Vec2::ZERO, whole),
        across / picture.scale as f32,
    );
    for piece in &picture.pieces {
        let low =
            UVec2::new(piece.column, piece.row).as_vec2() * picture.viewport;
        place(piece, piece.done, picture_rect(low, picture.viewport), across);
    }
}

/// Keep the map's own annotations off the window while pieces are drawn
///
/// They are drawn for whichever piece the map is drawing, and would stand
/// a piece's width from what they name; once the picture is down the map
/// draws through the window's own frame again and they come back.
fn hide_annotations(
    mut contexts: EguiContexts,
    enhance: Res<Enhance>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let drawing = matches!(enhance.phase, Phase::Drawing(_));
    let transform = if drawing {
        egui::emath::TSTransform::from_translation(egui::vec2(1e6, 1e6))
    } else {
        egui::emath::TSTransform::IDENTITY
    };
    ctx.set_transform_layer(crate::map::screen::annotations_layer(), transform);
    Ok(())
}

/// The button, the progress, and the picture's own controls
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

    crate::ui::zone("enhance")
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-margin, -margin))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| match enhance.phase {
                Phase::Off => {
                    ui.horizontal(|ui| {
                        let scale = &mut enhance.scale;
                        egui::ComboBox::from_id_salt("enhance-scale")
                            .selected_text(format!("{scale}×"))
                            .width(48.)
                            .show_ui(ui, |ui| {
                                for offered in SCALES {
                                    ui.selectable_value(
                                        scale,
                                        offered,
                                        format!("{offered}×"),
                                    );
                                }
                            });
                        if ui
                            .button("enhance")
                            .on_hover_text(
                                "Draw this view again at several times the \
                                 window's resolution, over the map",
                            )
                            .clicked()
                        {
                            enhance.asked = Some(Ask::Start);
                        }
                    });
                }
                Phase::Drawing(drawing) => {
                    let Some(picture) = &enhance.picture else { return };
                    let pieces = picture.pieces.len();
                    let (done, label) = match drawing.piece {
                        None => (0., "the view".to_owned()),
                        Some(at) => (
                            at as f32 + 1.,
                            format!("piece {} of {pieces}", at + 1),
                        ),
                    };
                    // How far the piece under way has got, by what it still
                    // has to read and build against the most it has had.
                    let within = if drawing.most == 0 {
                        0.
                    } else {
                        1. - drawing.left as f32 / drawing.most as f32
                    };
                    let progress =
                        ((done + within) / (pieces as f32 + 1.)).clamp(0., 1.);
                    ui.set_width(260.);
                    ui.label(format!("enhancing {}×: {label}", picture.scale));
                    ui.add(egui::ProgressBar::new(progress).show_percentage());
                    if ui.button("cancel").clicked() {
                        enhance.asked = Some(Ask::Close);
                    }
                }
                Phase::Shown => {
                    let Some(picture) = &enhance.picture else { return };
                    ui.label(format!(
                        "{}× · {:.1}× in",
                        picture.scale, picture.zoom
                    ));
                    ui.horizontal(|ui| {
                        let saving = picture.reading.is_some();
                        if ui
                            .add_enabled(
                                !saving,
                                egui::Button::new(if saving {
                                    "saving…"
                                } else {
                                    "save png"
                                }),
                            )
                            .clicked()
                        {
                            enhance.asked = Some(Ask::Save);
                        }
                        if ui.button("close").clicked() {
                            enhance.asked = Some(Ask::Close);
                        }
                    });
                    match &enhance.saved {
                        Some(Ok(path)) => {
                            ui.label(format!("saved {}", path.display()));
                        }
                        Some(Err(why)) => {
                            ui.label(format!("not saved: {why}"));
                        }
                        None => {}
                    }
                }
            });
        });
    Ok(())
}
