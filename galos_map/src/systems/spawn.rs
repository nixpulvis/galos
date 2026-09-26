use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::search::Plot;
use crate::space::Galaxy;
use crate::systems::bodies::spawn::{Body, Places};
use crate::systems::route::graph::{Drive, Routing, Tuning};
use crate::systems::{
    System,
    fetch::FetchIndex,
    fetch::FetchTasks,
    fetch::RawSystem,
    filter::{Filtered, Filtering, Filters},
    pointing::{DRAG_THRESHOLD, DragDistance, Indicator, PointedAt},
    route::spawn::spawn_route,
    route::{self, PlottedRoute, Route},
    selection::{Picked, PickedBody, Selection},
};
use crate::ui::{ARROW, Gesture, PressOwner};
use crate::{Names, Populated};
use bevy::asset::RenderAssetUsages;
use bevy::diagnostic::FrameCount;
use bevy::image::{Image, ImageSampler};
use bevy::math::DVec3;
use bevy::picking::pointer::PointerMap;
use bevy::prelude::*;
use bevy::render::render_resource::{
    Extent3d, TextureDimension, TextureFormat,
};
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use big_space::prelude::*;
use chrono::Utc;
use elite_journal::{Allegiance, Government, system::Security};
use galos_index::SystemName;
use galos_index::core::aggregate::bucket_temperature;
use galos_index::records::Economies;
use galos_photometry::Temperature;
use galos_photometry::psf::ProfileKind;
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Deref,
    time::{Duration, Instant},
};

pub fn plugin(app: &mut App) {
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(true));
    app.insert_resource(StarExposure::default());
    app.insert_resource(StarProfile::default());

    app.add_systems(Startup, cut_star_psf);
    app.init_resource::<PendingSpawns>();
    app.add_systems(Update, spawn.in_set(MapSet::Populate));
    // Turns a bounded number of queued systems into entities each frame, so a
    // frame's offers do not all become entities at once. After `spawn`, which
    // fills the queue from what the fetch tasks return.
    app.add_systems(Update, drain_spawns.in_set(MapSet::Populate).after(spawn));
    app.add_systems(Update, update.in_set(MapSet::Populate).before(spawn));
    // Cuts the star texture again when the profile changes; guarded on the
    // change inside, so a resting frame does nothing.
    app.add_systems(Update, reprofile.in_set(MapSet::Populate));

    app.add_observer(select_on_click);
    // Answers what is pointed at this frame, which `point_at` decides.
    app.add_systems(
        Update,
        fly_on_double_click
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
    );
}

/// The apparent magnitude that fills a pixel at exposure zero: the realistic
/// view's zero point, the dial `galos_sky` calls `exposure`.
///
/// [`StarExposure`] rides this in stops, and [`StarExposure::zero_point`] turns
/// the two into the one figure [`galos_photometry::Magnitude::exposure`] reads
/// a star's drawn energy from, so the map and the sky size a star off one law.
/// Near the dark-adapted eye's limit, so the sky comes in dense.
const STAR_ZERO_POINT: f64 = 8.0;

/// The exposure the realistic sky is drawn at, in stops
///
/// One control for the whole sky: the gain a star's flux is drawn against.
/// Opening it a stop doubles every star's peak, which through the point spread
/// (see [`super::scale::size_photometrically`]) both enlarges the stars already
/// shown and draws in fainter ones whose peak now clears the floor — the way
/// turning up an exposure does; closing it does the reverse. So how many stars
/// there are and how large they draw falls out of this and the physics, with no
/// magnitude limit set by hand.
#[derive(Resource)]
pub struct StarExposure(pub f32);

/// The dial rests at zero: neutral, the tuned look, a stop either way from
/// there.
impl Default for StarExposure {
    fn default() -> Self {
        StarExposure(0.)
    }
}

impl StarExposure {
    /// The zero point the stops come to, an apparent magnitude.
    ///
    /// A stop is a factor of two in energy, which is `2.5·log₁₀2 ≈ 0.75`
    /// magnitudes, so opening the exposure lifts the zero point that far and
    /// draws fainter stars in. Fed to
    /// [`galos_photometry::Magnitude::exposure`] as the magnitude that fills a
    /// pixel.
    pub(crate) fn zero_point(&self) -> f64 {
        STAR_ZERO_POINT + self.0 as f64 * 2.5 * 2f64.log10()
    }
}

/// The emission a photometric star of temperature `bucket` deposits at its
/// centre, given the peak its point spread came to
///
/// The one place the realistic view's color is worked out — the field's
/// per-vertex glint reads it (see [`super::field`]). The tint is the bucket's
/// blackbody color, normalized to unit luminance so hue carries no brightness
/// of its own, and `peak` is
/// [`galos_photometry::psf::Profile::peak`] — the energy the star lays on its
/// centre pixel, which the texture's unit-peak shape then falls away from.
///
/// **No compression of its own.** What stood here raised flux to [`GAMMA`]
/// and lifted it by a reference level, on top of a radius that was the log of
/// the same energy — so brightness was compressed twice and the sky came out
/// a field of near-identical dots. The law is now the one `galos_sky` renders
/// with: linear energy through the profile, and the tonemapper is what
/// compresses it. See [`super::scale::psf_draw`].
pub(crate) fn photometric_emissive(bucket: usize, peak: f32) -> LinearRgba {
    let tint = Temperature(bucket_temperature(bucket)).color();
    LinearRgba::rgb(tint[0] * peak, tint[1] * peak, tint[2] * peak)
}

/// How wide the point spread is cut, in texels a side
///
/// Wide enough that the core still has texels to spare once the cut reaches
/// [`super::scale::CORE_WIDTHS`] out: at 256 across and 32 core widths of
/// reach, a core is four texels of radius and the wings have the rest.
const PSF_TEXELS: u32 = 256;

/// The star point spread: the whole [`galos_photometry::psf::Psf`] stack, cut
/// to a texture
///
/// The one shared profile, sampled from the crate so the map and `galos_sky`
/// wear the same instrument — the map cuts its shape into a texture once, the
/// sky evaluates it per pixel, but the `β`, the falloff and the halo behind
/// them are one definition.
///
/// **The stack and not the core alone.** What stood here cut
/// `Psf::new(profile, alpha)`, the bare seeing core, and left out the aureole
/// every one of `galos_sky`'s bright stars wears: eight core widths out the
/// core alone is 2.4e-4 of peak where the stack is 3.0e-2, a hundred and
/// twenty-five fold, and that difference *is* the glow around a bright star.
/// Without it the map drew its brightest stars as bare discs and nothing in
/// the exposure or the sizing could put the halo back.
///
/// **And in float, not eight bits.** A halo is faint by construction: the
/// core-only profile is already under a step of 255 four core widths out, so
/// an 8-bit cut quantizes every wing to black however much light the star
/// carries. `Rgba32Float` holds the whole range, and the emissive it
/// multiplies is HDR anyway.
pub(crate) fn star_psf(profile: ProfileKind) -> Image {
    let n = PSF_TEXELS;
    let centre = (f64::from(n) - 1.) / 2.;
    // The core in texels, so that the cut reaches `CORE_WIDTHS` of them —
    // which is the reach `super::field` maps onto a star's quad and
    // `super::scale::psf_draw` caps a disc at.
    let alpha = f64::from(n) / 2. / crate::systems::scale::CORE_WIDTHS;
    let psf = crate::systems::scale::instrument(profile, alpha as f32);
    // Subtracted so the corner reaches exactly zero and no square edge shows.
    let floor = psf.shape(centre);
    let mut data = Vec::with_capacity((n * n * 16) as usize);
    for y in 0..n {
        for x in 0..n {
            let r = ((f64::from(x) - centre).powi(2)
                + (f64::from(y) - centre).powi(2))
            .sqrt();
            let v =
                (((psf.shape(r) - floor) / (1. - floor)).clamp(0., 1.)) as f32;
            for _ in 0..4 {
                data.extend_from_slice(&v.to_le_bytes());
            }
        }
    }
    let mut image = Image::new(
        Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba32Float,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// The point spread every star's mark is painted through
///
/// One image, cut once and shared by the whole sky, so the map and `galos_sky`
/// wear the same instrument. [`super::field`] samples it per mark in the
/// realistic view — a bright core falling to nothing, which the camera's bloom
/// spreads into a glint — and [`reprofile`] rewrites it in place when the
/// profile changes, repainting every star at once.
#[derive(Resource)]
pub(crate) struct StarSprite {
    /// What [`star_psf`] cut, under the handle [`super::field`] cloned.
    pub psf: Handle<Image>,
}

/// Which point-spread profile the realistic view's stars wear
///
/// The shape [`star_psf`] cuts into the sprite texture — a Moffat with its
/// wings or a tighter Gaussian; see [`galos_photometry::psf::ProfileKind`].
/// The map sizes a star by [`super::scale`]'s own `psf_radius` law either way,
/// so this changes the halo a star wears, not how large it draws.
/// [`reprofile`] cuts the texture again when it changes.
#[derive(Resource, Default)]
pub struct StarProfile(pub ProfileKind);

/// The colors a star may be drawn in
///
/// Named rather than numbered, so that a scheme below says which color it
/// means. One color each, and nothing indexes them: [`super::field`] asks
/// `Hue::light` for the three channels it paints a mark with, and
/// [`super::glow`] for the three it weights a cell's political histogram by.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hue {
    Green,
    Cyan,
    Red,
    Orange,
    Yellow,
    Blue,
    Magenta,
    Grey,
}

impl Hue {
    /// The light the hue lays down, at unit level
    ///
    /// **A chromaticity, not a brightness.** How bright a thing painted in a
    /// hue comes out is the gains' to say — [`super::glow::Gains::mark`] for
    /// a mark, and [`super::glow::Gains::faint`] and
    /// [`super::glow::Gains::unaligned`] for how much of one an uninhabited
    /// or unreported system is worth — and every one of those is already a
    /// statement about how much a system is worth seeing.
    ///
    /// So grey is white here, the absence of a colour rather than a dark
    /// paint. Painted as the swatch grey it reads as, `0.15` in sRGB, it was
    /// discounted twice: that is a fiftieth in the linear light a mark is
    /// added in, so an unreported colony came out at three thousandths of a
    /// unit and a system nobody lives in at nine ten-thousandths — three
    /// levels off black on an eight-bit display, which is the whole of why
    /// the ungoverned galaxy was invisible. [`super::glow`]'s backdrop
    /// channel has deposited neutral for exactly this reason since it
    /// landed; this is the same fix in the two places that were left.
    ///
    /// Written in linear light rather than converted from sRGB per call:
    /// [`super::field`] asks this once a system a frame, and the map draws a
    /// hundred thousand of them. `srgb_to_linear` is checked against the one
    /// value that is not a zero or a one in
    /// [`tests::orange_is_half_way_up_in_srgb`].
    pub(crate) const fn light(self) -> Vec3 {
        match self {
            Hue::Green => Vec3::new(0., 1., 0.),
            Hue::Cyan => Vec3::new(0., 1., 1.),
            Hue::Red => Vec3::new(1., 0., 0.),
            Hue::Orange => Vec3::new(1., 0.214_041_14, 0.),
            Hue::Yellow => Vec3::new(1., 1., 0.),
            Hue::Blue => Vec3::new(0., 0., 1.),
            Hue::Magenta => Vec3::new(1., 0., 1.),
            Hue::Grey => Vec3::ONE,
        }
    }
}

/// Determains what color to draw in system view mode.
#[derive(Resource, Copy, Clone, Debug, PartialEq)]
pub enum ColorBy {
    Allegiance,
    Government,
    Security,
}

/// Whether systems are named
///
/// On to begin with. Names are what makes the map readable as a place rather
/// than as a field of dots, and how many of them are drawn is answered by the
/// map view's [`crate::systems::labels::NameRadius`] or the realistic view's
/// [`crate::systems::labels::NameLimit`], and by the room each is given, rather
/// than by having them off.
#[derive(Resource)]
pub struct ShowNames(pub bool);

/// A system the field paints a star for
///
/// A marker and nothing else. From far enough away nothing in a system can be
/// told apart from anything else in it, so what is drawn is one mark standing
/// for the lot; up close the same mark is the edge of what the system takes
/// up, and its contents are drawn inside it.
///
/// Nothing is drawn where the mark is. A mesh at a system's true coordinate
/// sits out where the f32 clip transform tears it apart, so the mark is
/// painted flat in screen space by [`super::field`], off this entity's
/// position and the size [`super::scale`] writes onto it. That size is an
/// exaggeration far larger than the system — a system drawn at its own scale
/// is invisible from the next one over — and the field divides it back out to
/// pixels, which is the whole of what it is for.
///
/// Not a star. A system is a place, a star is a thing in it, and there may be
/// several; those are read from the `stars` table and drawn within this.
#[derive(Component)]
#[require(super::scale::Drawn)]
pub struct Shell;

/// Pick out whatever was clicked
///
/// Clicking says which thing the user means and nothing more. Where the
/// camera goes is asked for separately, by the row that names what is picked
/// out, so that a system can be pointed out from wherever the user happens
/// to be looking without the map moving out from under them.
///
/// One gesture over stars and over the bodies inside them alike: a plain click
/// holds what was clicked and lets go of everything else, and the modifier
/// gathers instead. A system and a body inside it are two things that can be
/// picked out, and being one thing inside the other says nothing about what a
/// click means.
///
/// The left button orbits the camera as well as selecting, so an orbit that
/// happens to start and end on the same star has to be told apart from a
/// click on it. Picking calls it a drag after a single pixel of movement,
/// which is too eager to use by itself, so measure the travel instead.
//
// TODO: Spawn/despawn system label on Pointer<Over>/Pointer<Out>.
fn select_on_click(
    click: On<Pointer<Click>>,
    pointed_at: Query<&System, With<PointedAt>>,
    pointed_body: Query<(Entity, &Body), With<PointedAt>>,
    places: Places,
    pointers: Res<PointerMap>,
    dragged: Query<&DragDistance>,
    press: Res<PressOwner>,
    frame: Res<FrameCount>,
    keys: Res<ButtonInput<KeyCode>>,
    mut answered: Local<Option<u32>>,
    mut selection: ResMut<Selection>,
) {
    let travelled = pointers
        .get_entity(click.pointer_id)
        .and_then(|pointer| dragged.get(pointer).ok())
        .map_or(0., |travelled| travelled.0);
    if click.button != PointerButton::Primary || travelled > DRAG_THRESHOLD {
        return;
    }
    // A press the UI took is not the map's to answer, so it picks nothing out
    // however squarely it landed on a star: the press that shuts the search
    // form is one gesture, and shutting a form and picking out a system are
    // two things for it to do.
    //
    // Unless it is unowned, which is the map's rather than nobody's. Picking
    // reports a click before the UI has settled whose the press was, so a
    // whole click inside one frame reaches here with no owner at all, and
    // refusing those would be a star that cannot be picked out on a slow map.
    if press.taken_by_ui() {
        return;
    }

    // One click is reported once for everything under the pointer, and
    // since a star stopped blocking what lies behind it there are usually
    // several. They are all the same click, and only one system can be
    // picked out, so the first of them answers for the rest.
    //
    // Counted by frame rather than by which of them is the one that won:
    // picking reports a click before `pointing` has looked at the frame it
    // belongs to, so anything recorded about the winner is a frame old, and
    // a pointer that has just moved would leave the click unanswered.
    if *answered == Some(frame.0) {
        return;
    }
    *answered = Some(frame.0);
    // Whatever is being pointed at is what a click is for, and `pointing`
    // has already settled which system that is, weighing a name over a star
    // lying nearer behind it. Asking it rather than working the hit out
    // again keeps the click on whatever the ring and the tint are on.
    // Held down, a modifier gathers systems up rather than replacing what is
    // held, and lets go of one already held, so the same gesture builds a set
    // and takes it apart.
    //
    // Any of the three, and both sides of each. Which one means "as well as
    // that one" is a matter of what the user came from: control on Windows
    // and Linux, command on macOS. Shift is offered beside them because it is
    // the one no platform reads as asking for something else.
    let gathering = keys.any_pressed([
        KeyCode::ControlLeft,
        KeyCode::ControlRight,
        KeyCode::SuperLeft,
        KeyCode::SuperRight,
        KeyCode::ShiftLeft,
        KeyCode::ShiftRight,
    ]);
    // A body first, as everywhere: once the camera is close enough to see
    // what is inside a system, what is inside it is what a click means.
    //
    // A body is taken as a value here rather than left as the entity it was
    // clicked on, so that what is picked out is one list of one kind of thing.
    // Where it stands is read now because a body does not move, and it is the
    // one thing about a body that is not on the row it carries.
    let picked = if let Ok((entity, body)) = pointed_body.single() {
        places.of(entity).map(|at| {
            Picked::Body(PickedBody::new(body.address, body.id, &body.name, at))
        })
    } else {
        pointed_at.single().ok().cloned().map(Picked::System)
    };

    // Nothing under the pointer is nothing to pick out, and nothing to let go
    // of either. A click on empty sky is a gesture in its own right and
    // [`super::selection::clear_when_nothing_is_clicked`] is what answers it.
    let Some(picked) = picked else { return };

    selection.pick(picked, gathering);
}

/// How long a second click may take to arrive and still make a double
///
/// Seconds. Long enough to be reached without hurrying, short enough that
/// two deliberate clicks on the same system are not read as one gesture.
const DOUBLE_CLICK: f32 = 0.4;

/// Fly the camera to whatever the user double clicks
///
/// One click says which thing is meant and a second says to go there, so
/// the map can be pointed at from where the user is without moving, and
/// travelled with the same hand when they do want to move.
///
/// A system out in the sky and a body inside one alike. The gesture is the
/// same gesture and means the same thing, and what differs is only how the
/// thing aimed at says where it stands: a system carries a galactic position
/// of its own, and a body is placed in metres from the middle of the system
/// holding it, so it is asked through [`Places`].
///
/// A click is weighed by the same three questions everywhere on the map: the
/// primary button, travel short enough to be a click rather than a drag, and
/// the pointer's own business rather than the UI's. What is asked on top of
/// those is that the click before it landed on the same thing, recently.
///
/// The zoom is left where the user set it, as a move that only says where to
/// look should. Flying to a body is then the camera coming to orbit it rather
/// than the system around it, which is what makes the next scroll of the wheel
/// go in towards the body instead of past it.
fn fly_on_double_click(
    gesture: Gesture,
    dragged: Query<&DragDistance>,
    pointed_at: Query<(Entity, &System), With<PointedAt>>,
    // Whatever inside a system is pointed at, which carries no galactic
    // position of its own and is asked where it stands.
    pointed_body: Query<Entity, (With<Body>, With<PointedAt>)>,
    places: Places,
    time: Res<Time<Real>>,
    mut last: Local<LastClick>,
    mut camera: MessageWriter<MoveCamera>,
) {
    if !gesture.on_map() {
        return;
    }
    if dragged.iter().any(|travelled| travelled.0 > DRAG_THRESHOLD) {
        return;
    }

    // A body first, as a click on one means the body rather than the system
    // holding it. Only one thing is ever pointed at, so the two queries
    // cannot both answer, and the order is what it says rather than a choice
    // being made.
    let aimed = if let Ok(body) = pointed_body.single() {
        places.of(body).map(|at| (body, at))
    } else if let Ok((entity, system)) = pointed_at.single() {
        Some((entity, DVec3::from(system.position)))
    } else {
        None
    };
    let Some((what, position)) = aimed else { return };

    if last.doubled(what, time.elapsed_secs()) {
        camera.write(MoveCamera { position: Some(position), framing: None });
    }
}

/// The click a second one would be counted against
///
/// Which thing as well as when, so that two clicks a moment apart on two
/// different stars are two answers rather than one gesture. Stars stand
/// close together on screen at any distance, and picking one out after
/// another is an ordinary thing to do quickly.
///
/// What was clicked rather than which system it was, since a body is
/// something to be aimed at as much as the system holding it is, and the two
/// have nothing in common to be named by but being entities on the map.
#[derive(Default)]
struct LastClick(Option<(Entity, f32)>);

impl LastClick {
    /// Whether a click on `what` at `now` is the second of a pair
    ///
    /// A double is spent as soon as it is answered, so a third click starts
    /// counting afresh rather than making a second pair with the second.
    fn doubled(&mut self, what: Entity, now: f32) -> bool {
        let doubled = matches!(self.0, Some((clicked, when))
            if clicked == what && now - when <= DOUBLE_CLICK);
        self.0 = if doubled { None } else { Some((what, now)) };
        doubled
    }
}

/// Polls the fetch tasks and queues the systems they built for spawning
///
/// The systems arrive already named and colored, built on the task's own
/// thread (see [`super::fetch`]), so nothing here joins a table or clones a
/// row. What lands is queued into [`PendingSpawns`] rather than spawned on the
/// spot, and [`drain_spawns`] turns a bounded number into entities each frame:
/// a trip across the galaxy lands a hundred and forty stops in one task
/// completion, and spawning a batch at once is what stalls the frame.
pub fn spawn(
    route_query: Query<(Entity, &Route)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut plotted: MessageWriter<route::PlottedRoute>,
    mut unflown: MessageWriter<route::UnflownLeg>,
    mut tasks: ResMut<FetchTasks>,
    mut plot: ResMut<Plot>,
    mut pending: ResMut<PendingSpawns>,
    systems: Query<&System>,
) {
    let Ok(grid) = grids.get(galaxy.0) else { return };

    // Every row that arrived this frame, and when the last of them was asked
    // for. Put together first and queued once.
    //
    // One time for however many queries landed together, since what it is for
    // is the line the spawn is logged under. Nothing is stamped with it and
    // nothing measures how stale a row is by it, so the latest of them stands
    // for the batch rather than each row having to carry its own.
    let mut arrived: Vec<(System, bool, bool)> = Vec::new();
    let mut arrived_at = time.startup();
    // This frame's moment, which is when everything polled below landed as
    // far as anyone watching is concerned.
    let landed_at = time.last_update().unwrap_or_else(|| time.startup());

    tasks.fetched.retain(|index, (task, fetched_at)| {
        let status = block_on(future::poll_once(task));
        let retain = status.is_none();
        if let Some(new_systems) = status {
            if let FetchIndex::Route(
                start,
                end,
                range,
                trip,
                drive,
                how,
                tune,
            ) = index
            {
                // A leg is a line between two systems, so one system is no
                // leg. Coming back with nothing is how the router says it
                // could not get from one end to the other in jumps that
                // long, and nothing drawn is the same nothing as a leg still
                // being worked out.
                //
                // Named for the two ends of this leg rather than for the
                // whole trip: what a user who asked for a trip through five
                // systems wants told is which two of them there is no route
                // between, and a leg is exactly that pair.
                //
                // Only ever an answer to a leg still being waited on. A name
                // that resolved to nothing is already said, and said more
                // exactly than this could: the leg was fetched anyway, and it
                // comes back empty for the same reason, so without this the
                // better answer is talked over a moment after it arrives.
                if new_systems.len() < 2 {
                    if *plot == Plot::Working {
                        *plot = Plot::Failed(format!(
                            "No route from {start} to {end} at {range} Ly"
                        ));
                    }
                    // And the leg's own row is told, the form's line being
                    // one answer about the whole plot where a trip has a row
                    // for each of its legs. See [`route::UnflownLeg`].
                    unflown.write(route::UnflownLeg(index.clone()));
                }

                // Said rather than acted on. What a route does to the map is
                // `route::plotted`'s business; this is the one place its
                // systems are in hand, so it is the one place that can say
                // what they are. The systems arrive built, so the line is
                // drawn straight from them before they join the spawn queue.
                // The wait, from the frame the button was pressed to this
                // one. Off the task's own stamp rather than a clock of its
                // own: `fetched_at` is when the leg was handed to the pool,
                // which is what a user waiting on a plot is timing.
                let took = landed_at.saturating_duration_since(*fetched_at);
                if let Some(landed) = plotted_route(
                    &new_systems,
                    range,
                    *drive,
                    *how,
                    *tune,
                    trip.clone(),
                    took,
                ) {
                    spawn_route(
                        &landed.filter(),
                        &new_systems,
                        &route_query,
                        &galaxy,
                        grid,
                        &mut commands,
                        &mut mesh_assets,
                        &mut material_assets,
                    );
                    plotted.write(landed);
                }
            }

            arrived_at = arrived_at.max(*fetched_at);
            // Pinned only where the user picked the systems out and flew to
            // them: those are wanted wherever they lie, as the walk spares
            // them, so the queue's ceiling never turns one away. A route's
            // stops are exempt on the other ground, being asked for by name.
            let pinned = matches!(index, FetchIndex::Systems(..));
            // Asked for by name, both of them: a route's stops are the answer
            // the user is waiting on and the systems of a search are what
            // they typed. Those go in front of the walk's own offers, which
            // are a galaxy nobody named — see [`PendingSpawns`]. A route
            // landed with its stops behind tens of thousands of walk offers
            // drew its line and then filled it in over the seconds it took
            // the queue to reach them.
            let asked = matches!(
                index,
                FetchIndex::Systems(..) | FetchIndex::Route(..)
            );
            arrived.extend(
                new_systems.into_iter().map(|system| (system, pinned, asked)),
            );
        }
        retain
    });

    // Still plotting while any leg of it is. A trip's legs land one at a
    // time, and the first of them used to clear this: the spinner stopped,
    // the stop button went, and the form read as finished while two legs
    // were still searching. The tasks are the authority — a leg that landed
    // was taken off `fetched` in the walk above — so what is left is what
    // is still being worked out.
    //
    // Only where the form is still waiting. A leg that came back with
    // nothing has already said so, and that answer outlives the rest of the
    // trip landing.
    let plotting = tasks
        .fetched
        .keys()
        .any(|index| matches!(index, FetchIndex::Route(..)));
    if *plot == Plot::Working && !plotting {
        *plot = Plot::Nothing;
    }

    // Queue rather than spawn, and only what is not already on the map. A
    // stop is asked for whenever a route names it, so a leg replotted or a
    // trip sharing a stop with the last one delivers systems already drawn;
    // queueing those would drain to nothing but a churn of no-op re-inserts.
    // The queue also holds one entry per address, so a system fetched twice
    // before it is drawn lands once — which is what stops two entities
    // landing for one system. An evicted system is not resident, so it still
    // re-queues and comes back.
    let resident: HashSet<i64> =
        systems.iter().map(|system| system.address).collect();
    for (system, pinned, asked) in arrived {
        if resident.contains(&system.address) {
            continue;
        }
        pending.push(system, pinned, asked, arrived_at);
    }
}

/// What a route that has landed amounts to, if it amounts to a route
///
/// Nothing where fewer than two systems came back, a line between one system
/// being no line.
///
/// `range` comes off the key the leg was fetched under, that being where what
/// the user asked for is still written down. The rows that came back say which
/// systems the ship passes through and nothing about how far it reaches.
///
/// Named for its two ends as the rows spell them, rather than as the user
/// typed them: a leg of a trip is a route like any other, and the map's own
/// spelling is what the rest of the map says.
#[allow(clippy::too_many_arguments)]
fn plotted_route(
    systems: &[System],
    range: &str,
    drive: Drive,
    how: Routing,
    tune: Tuning,
    trip: Option<String>,
    took: Duration,
) -> Option<PlottedRoute> {
    let (first, last) = (systems.first()?, systems.last()?);
    if systems.len() < 2 {
        return None;
    }

    Some(PlottedRoute {
        label: format!("{}{ARROW}{}", first.name(), last.name()),
        // In the order they are travelled, which is the order the route came
        // back in and the order its panel lists.
        systems: systems.iter().map(|system| system.address).collect(),
        range: range.to_owned(),
        drive,
        how,
        tune,
        trip,
        took,
    })
}

/// How many systems are turned into entities while the view is still moving
///
/// A cap on the structural churn the map does per frame, since spawning an
/// entity mutates the world and cannot leave the main thread. A wide view
/// resolves tens of thousands of systems in one pass, and building all of
/// them at once is a visible hitch; spread over frames it streams in instead,
/// which the map already reads as a sky drawing before it has fully loaded.
///
/// **Low because a big budget buys a bigger map, not a sooner one.** Raising
/// it was measured over one flight ([`super::flight`]):
/// 4,096 cost 36% at the ninetieth percentile and 8,192 cost 63%, and both
/// bought a *larger* map rather than a sooner one — 25,540 systems drawn at
/// the peak against 41,684 and 51,449, and 402,071 despawns against 576,634
/// and 584,884. The walk re-offers whatever is undrawn every frame, so while
/// the plan is churning a system drawn sooner is mostly a system despawned
/// sooner, and everything a frame does — the address map, the eviction scan,
/// the keepers scan, the detaching — is over every drawn system.
const SPAWN_BUDGET: usize = 2048;

/// How many points one pass of the walk may offer
///
/// Four frames' worth of the spawn budget. A wide view resolves tens of
/// thousands of points a pass and offering all of them was most of what the
/// pass cost: the offers past this are re-made next frame, by which time the
/// budget has drawn what it took from these. Deep enough that a frame the
/// drain empties still has something left to take from, shallow enough that
/// the offering is not the cost.
const OFFER_BUDGET: usize = SPAWN_BUDGET * 4;

/// How deep the queue is allowed to get
///
/// Thirty-two frames' worth. What is offered past it is dropped unqueued,
/// which costs nothing: the walk runs every frame and offers whatever is
/// still wanted again, so the queue holds what the next half-second can
/// draw rather than everything a view could ever want. Framing a route
/// across the galaxy offered **two million** in one pass, against a picture
/// that wanted a few thousand marks.
const QUEUE_CEILING: usize = SPAWN_BUDGET * 32;

/// One system waiting to be drawn
///
/// **A reference where there is one to keep.** A queued system is mostly a
/// system that never gets drawn — the camera moves and the walk moves with it
/// — so building one to queue it is building what gets thrown away:
/// a name off the names table, the political columns off the populated
/// table, and a `System` the size of both. Millions of those, to draw
/// thousands.
///
/// So what the walk queues is which point of which cell it wants, and the
/// system is built out of the payload at the moment it is drawn. What is
/// already built stays built: the fetch tasks build on their own threads on
/// purpose, and a route's own stop comes out of the names table with no
/// payload behind it at all.
enum Waiting {
    /// Built elsewhere and carried: off a fetch task, or out of the names
    /// table for a stop no cell's prefix answers for.
    ///
    /// Boxed so an entry is the size of a reference and not the size of a
    /// [`System`]: 48 bytes measured against 144, and none of the
    /// allocation, since it is the references that come in millions. The
    /// box costs one allocation on a path that has already allocated the
    /// system's name.
    Built(Box<System>),
    /// A point of a cell the map holds, read when it is drawn.
    ///
    /// No position. It used to carry one so the spyglass path could weigh a
    /// queued point against the reach without reading the payload back; the
    /// walk's offers are not queued now (see [`PendingSpawns`]), and nothing
    /// weighs the queue any more.
    Point {
        /// The index's own cell, not the renderer's grid cell.
        cell: galos_index::CellId,
        at: u32,
    },
}

/// What is waiting under one address, and how it is waiting
struct Offered {
    what: Waiting,
    /// Wanted whatever the queue's depth; see [`PendingSpawns::waiting`].
    pinned: bool,
    /// Already in the queue that goes first, so a second asking does not
    /// put the address in it twice.
    asked: bool,
}

/// One point the walk offered this pass
///
/// Flat, and without an address key, because there is nothing to deduplicate:
/// a payload point belongs to one cell, a cell is walked once a pass, and the
/// walk offers a point only where no entity holds its address yet. What the
/// keyed queue is for is the things that arrive from elsewhere and can arrive
/// twice; see [`PendingSpawns`].
struct Walked {
    address: i64,
    /// The index's own cell, not the renderer's grid cell.
    cell: galos_index::CellId,
    at: u32,
}

/// Systems waiting to become entities
///
/// The fetch tasks return a route's stops and the systems picked out by name,
/// and the walk offers a prefix of every cell it holds; both queue here
/// rather than spawning the lot in the frame they land. [`drain_spawns`]
/// takes [`SPAWN_BUDGET`] of them a frame.
///
/// **Two queues, because a frame's offers are not equally wanted.** What the
/// user asked for by name — the stops of a route just plotted, a system
/// picked out and flown to — goes in `asked` and is drawn first. Everything
/// the walk offers of its own accord goes in `order`, which is arrival order
/// as before. One queue meant the hundred and forty stops of a plotted route
/// waited behind every mark the walk had offered that frame — tens of
/// thousands of them, at 2,048 a frame — so the line landed and then filled
/// in slowly from whatever end the queue reached first.
///
/// Keyed by address so a system offered twice before it is drawn holds one
/// entry, keeping the later row: a re-fetch is a refresh, and one entry is
/// also what stops two entities landing for a system the world does not yet
/// hold when the second copy is read.
///
/// **The walk's own offers are not queued at all.** They used to be, and what
/// that bought was a backlog: a wide view resolves tens of thousands of
/// points a frame against a budget of two thousand, so the queue sat at its
/// ceiling, every offer in it was re-made every frame at a hash lookup
/// apiece, and what it eventually drew was the sky as the walk saw it several
/// frames ago — spawned, found unwanted, and despawned. Measured over one
/// flight ([`super::flight`]): a queue pinned at 63,488 entries and
/// **487,097 systems despawned** to draw at most 21,173.
///
/// So [`Self::opening`] clears the walk's batch at the start of every pass and
/// [`Self::offer`] fills it to [`OFFER_BUDGET`], and the walk offers what is
/// still wanted again next frame — which is what it does every frame anyway.
#[derive(Resource, Default)]
pub struct PendingSpawns {
    asked: VecDeque<i64>,
    order: VecDeque<i64>,
    rows: HashMap<i64, Offered>,
    /// What the walk offered this pass, in the order it walked the cells
    walked: Vec<Walked>,
    arrived_at: Option<Instant>,
}

impl PendingSpawns {
    /// Queue a system already built, keeping its place if it is already
    /// waiting and taking the later row.
    ///
    /// `asked` puts it in front of everything the map offered of its own
    /// accord; see the type.
    pub(crate) fn push(
        &mut self,
        system: System,
        pinned: bool,
        asked: bool,
        at: Instant,
    ) {
        let address = system.address;
        let what = Waiting::Built(Box::new(system));
        self.waiting(address, what, pinned, asked, at);
    }

    /// Start the walk's pass: what it offered last frame is gone
    ///
    /// The walk is the only thing that calls this, and it calls it once a
    /// pass. Nothing is lost by the clearing: an offer that is still wanted
    /// is re-made a few microseconds later by the same pass, and one that is
    /// not is an offer to draw sky the walk has moved off.
    pub(crate) fn opening(&mut self, at: Instant) {
        self.walked.clear();
        self.arrived_at = Some(self.arrived_at.map_or(at, |prev| prev.max(at)));
    }

    /// Offer the `at`th point of `cell`, to be read when it is drawn
    ///
    /// Answers whether there was room: past [`OFFER_BUDGET`] the pass has
    /// offered more than the frame's budget can draw several times over, and
    /// the caller can stop looking for offers — it still has a wanted set to
    /// finish marking.
    pub(crate) fn offer(
        &mut self,
        address: i64,
        cell: galos_index::CellId,
        at: u32,
    ) -> bool {
        if self.walked.len() >= OFFER_BUDGET {
            return false;
        }
        self.walked.push(Walked { address, cell, at });
        true
    }

    /// Queue whichever of the two, under `address`.
    ///
    /// `pinned` marks a system wanted whatever the queue's depth — one picked
    /// out and flown to, a route's own stop. A system queued
    /// again as pinned stays pinned, and one queued again as asked for moves
    /// up. An offer past [`QUEUE_CEILING`] is dropped rather than held,
    /// unless it is pinned or asked for: the walk will offer it again next
    /// frame if it is still wanted, and nothing else will offer a route's
    /// own stops.
    fn waiting(
        &mut self,
        address: i64,
        what: Waiting,
        pinned: bool,
        asked: bool,
        at: Instant,
    ) {
        match self.rows.get_mut(&address) {
            Some(held) => {
                held.what = what;
                held.pinned |= pinned;
                // Moved up rather than left where it was. The stale copy of
                // the address in `order` is skipped when it comes round,
                // the row having been taken by then.
                if asked && !held.asked {
                    held.asked = true;
                    self.asked.push_back(address);
                }
            }
            None => {
                if !pinned && !asked && self.order.len() >= QUEUE_CEILING {
                    return;
                }
                self.rows.insert(address, Offered { what, pinned, asked });
                if asked {
                    self.asked.push_back(address);
                } else {
                    self.order.push_back(address);
                }
            }
        }
        self.arrived_at = Some(self.arrived_at.map_or(at, |prev| prev.max(at)));
    }

    fn is_empty(&self) -> bool {
        self.asked.is_empty() && self.order.is_empty() && self.walked.is_empty()
    }

    /// How many systems are waiting, for the diagnostics panel to read.
    pub fn queued(&self) -> usize {
        self.asked.len() + self.order.len() + self.walked.len()
    }

    /// Take up to `budget` systems, building the ones that are still only a
    /// reference
    ///
    /// What was asked for by name first, then whatever else arrived, then the
    /// walk's own offers — which are this pass's and no older, so what is
    /// taken from them is the sky as the walk sees it now.
    ///
    /// `built` answers [`None`] where the payload a point named is gone or
    /// no longer holds that system — a cell freed or republished while the
    /// offer waited — and the offer is then dropped unread. The walk offers
    /// it again next frame if it is still wanted.
    fn take(
        &mut self,
        budget: usize,
        mut built: impl FnMut(i64, Waiting) -> Option<System>,
    ) -> Vec<System> {
        let mut batch = Vec::with_capacity(budget.min(self.queued()));
        while batch.len() < budget {
            let next =
                self.asked.pop_front().or_else(|| self.order.pop_front());
            let Some(address) = next else { break };
            if let Some(held) = self.rows.remove(&address)
                && let Some(system) = built(address, held.what)
            {
                batch.push(system);
            }
        }
        // The walk's, from the front: the pass walked the cells in the order
        // it holds them, and taking from the back would draw one end of that
        // order every frame and never reach the other.
        let mut walked = self.walked.drain(..).peekable();
        while batch.len() < budget {
            let Some(offer) = walked.next() else { break };
            let what = Waiting::Point { cell: offer.cell, at: offer.at };
            if let Some(system) = built(offer.address, what) {
                batch.push(system);
            }
        }
        // Whatever the budget did not reach is dropped with the iterator, and
        // offered again by the next pass if it is still wanted.
        batch
    }
}

/// Turn a budgeted number of queued systems into entities
///
/// Hands [`SPAWN_BUDGET`] of what is queued to [`spawn_systems`], so the
/// frame's structural work is bounded however much arrived at once.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_spawns(
    mut pending: ResMut<PendingSpawns>,
    systems_query: Query<(Entity, &System)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    filtering: Filtering,
    time: Res<Time<Real>>,
    resident: Res<crate::systems::bounded::ResidentCells>,
    populated: Res<Populated>,
    names: Res<Names>,
    mut commands: Commands,
) {
    if pending.is_empty() {
        return;
    }
    let Ok(grid) = grids.get(galaxy.0) else { return };
    let arrived_at = pending.arrived_at.unwrap_or_else(|| time.startup());
    // The frame's worth, built here and not when it was offered: a queued
    // system is mostly one that never gets drawn, and the name and the
    // political columns are a join apiece. See [`Waiting`].
    //
    // The building and the spawning are a zone apiece: one is a name and a
    // political join per system off the resident tables, the other is bevy
    // structural work, and a batch that spikes is one or the other.
    let batch = {
        let _zone =
            info_span!("build batch", queued = pending.queued()).entered();
        pending.take(SPAWN_BUDGET, |address, what| match what {
            Waiting::Built(system) => Some(*system),
            Waiting::Point { cell, at } => {
                let held = resident.0.cell(cell)?;
                let point = held.points.get(at as usize)?;
                // The cell may have been published again while the offer
                // waited, which renumbers its members: a point that is no
                // longer the system that was offered is not this offer's, and
                // the walk offers whatever is there now next frame.
                (point.id64 as i64 == address).then(|| {
                    crate::systems::bounded::build_from_point(
                        point, &populated, &names,
                    )
                })
            }
        })
    };
    let _zone = info_span!("spawn batch", systems = batch.len()).entered();
    spawn_systems(
        batch,
        &systems_query,
        &galaxy,
        grid,
        &filtering.filters,
        filtering.excluded_are_drawn(),
        &mut commands,
        &time,
        &arrived_at,
    );
}

/// Name and color a raw system from the resident tables
///
/// The cells give an address and a place and nothing political. Everything a
/// [`System`] is colored and filtered by comes from the [`Populated`] table
/// where the system is one of the dynamic set, and its name from [`Names`]. A
/// system absent from `populated` is ungoverned, which is most of the galaxy,
/// and drawn as such.
///
/// When the system was last updated comes off the payload point with the rest
/// of it, so a [`System`] built here says what the database says. [`None`]
/// where the raw system came from the names table instead, which carries no
/// moment; see [`RawSystem::updated_at`].
pub(crate) fn build_system(
    raw: &RawSystem,
    populated: &Populated,
    names: &Names,
) -> System {
    let name = names
        .get(raw.address)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| SystemName::new(raw.address.to_string()));
    // How far it reaches comes from the reaches table rather than from the
    // political one: most systems with anything scanned in them are not
    // populated, and a system drawn at a stood-in size wears a shell many
    // times the orbits inside it.
    let reach = names.reach(raw.address);
    match populated.get(raw.address) {
        Some(p) => System {
            address: raw.address,
            name,
            position: raw.position,
            population: p.population,
            allegiance: p.allegiance,
            government: p.government,
            security: p.security,
            economies: Economies::new(p.primary_economy, p.secondary_economy),
            factions: p.factions.clone(),
            body_count: p.body_count,
            non_body_count: p.non_body_count,
            reach,
            absolute_magnitude: raw.magnitude,
            temp_bucket: raw.temp_bucket,
            updated_at: raw.updated_at,
        },
        None => System {
            address: raw.address,
            name,
            position: raw.position,
            population: 0,
            allegiance: None,
            government: None,
            security: None,
            economies: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            reach,
            absolute_magnitude: raw.magnitude,
            temp_bucket: raw.temp_bucket,
            updated_at: raw.updated_at,
        },
    }
}

/// The drawable system at an address, if the resident tables can place it
///
/// A search or a filter names a system by address; its name comes from the
/// [`Names`] table, its place from the galaxy behind it, and everything
/// political from [`Populated`]. [`None`] where the table does not name it,
/// which is a system the map cannot draw.
pub(crate) fn system_at(
    address: i64,
    populated: &Populated,
    names: &Names,
) -> Option<System> {
    // Named or nothing: one the table cannot name is one the map cannot
    // draw, and `build_system` reads the name itself.
    names.get(address)?;
    // **The place comes from the galaxy, or from the populated table where
    // that already holds it.** The names table stopped holding positions
    // when a name became a function of an address, and what its row would
    // answer with is the middle of a boxel — ten light years across at the
    // class most systems are and 1,280 at the largest, which is a star
    // drawn in the wrong place. A populated system's exact place is
    // resident already, so that is asked first and costs nothing; anything
    // else is one sphere query ([`Names::placed`]).
    let at = match populated.get(address) {
        Some(known) => DVec3::new(
            known.position[0] as f64,
            known.position[1] as f64,
            known.position[2] as f64,
        ),
        None => names.placed(address),
    };
    let raw = RawSystem {
        address,
        position: [at.x, at.y, at.z],
        magnitude: None,
        temp_bucket: None,
        // The names table says where a system is and what it is called, and
        // nothing about when it was last heard from. A span excludes it until
        // its cell payload lands and the system is rebuilt from the point.
        updated_at: None,
    };
    Some(build_system(&raw, populated, names))
}

/// Create or refresh the entities for each row fetched
///
/// A [`System`] carries the database row and the grid placement, is what the
/// rest of the map addresses, and wears the [`Shell`] marker the field paints
/// its star from. Its name is not hung on it as a mesh: names, rings and
/// leaders are painted flat in screen space from the projected position (see
/// [`super::labels::draw_names`]), so nothing has to undo the exaggerated
/// size [`super::scale`] writes here.
///
/// A row already on the map has its [`System`] replaced rather than being
/// respawned, which [`update`] then acts on.
///
/// The filters are asked here rather than left to [`super::filter`]'s `mark`, so that a
/// system arrives already marked and already drawn at the strength it should
/// be. A mark applied by a command lands at the next sync point, by which
/// time the star has been drawn once at full strength.
pub fn spawn_systems(
    new_systems: Vec<System>,
    systems: &Query<(Entity, &System)>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
    filters: &Filters,
    excluded_are_drawn: bool,
    commands: &mut Commands,
    time: &Res<Time<Real>>,
    fetched_at: &Instant,
) {
    let mut existing_systems: HashMap<i64, Entity> = systems
        .iter()
        .map(|(entity, system)| (system.address, entity))
        .collect();

    // One clock for the batch. `admit` weighs every row against the same
    // moment, and a row already built carries this same moment as its own
    // `updated_at`, so reading it once here rather than per row costs nothing
    // in accuracy.
    let now = Utc::now();
    // What this call spawns, gathered and handed to bevy in one batch rather
    // than an entity at a time. One `Commands::spawn` apiece measured 589 ns
    // a star against 479 ns through `spawn_batch` over the map's own eight
    // components — a fifth of the spawning, for a `Vec` of what was going to
    // be spawned anyway. (Most of the 479 ns is the component count itself:
    // the same spawn with three components is 205 ns.)
    //
    // The excluded are their own batch, `Filtered` being the one component
    // that is not on every star, and a batch is one archetype.
    let mut spawning: Vec<Star> = Vec::new();
    let mut filtered: Vec<(Star, Filtered)> = Vec::new();
    for system in new_systems {
        // What no filter admits is dropped rather than dimmed once the dim is
        // zero, so it is never spawned in the first place: the load avoided,
        // not paid and then hidden. Left to [`super::bounded`]'s walk to
        // take off what already stands.
        let excluded = !filters.admit(&system, now);
        if excluded && !excluded_are_drawn {
            continue;
        }
        if let Some(entity) = existing_systems.remove(&system.address) {
            debug!(
                "updating {} @ {:?}",
                system.address,
                fetched_at.duration_since(time.startup())
            );

            commands.entity(entity).insert(system);
        } else {
            debug!(
                "spawning {} {:?}",
                system.address,
                fetched_at.duration_since(time.startup())
            );

            let star = star(system, grid, galaxy.0);
            if excluded {
                filtered.push((star, Filtered));
            } else {
                spawning.push(star);
            }
        }
    }
    if !spawning.is_empty() {
        commands.spawn_batch(spawning);
    }
    if !filtered.is_empty() {
        commands.spawn_batch(filtered);
    }
}

/// Everything a drawn star carries
///
/// Named because it is spawned in batches now and a batch wants one type; see
/// [`spawn_systems`].
type Star = (
    (CellCoord, Transform),
    System,
    Shell,
    Indicator,
    Pickable,
    Visibility,
    ChildOf,
);

/// One drawn star, placed where its row puts it
fn star(system: System, grid: &Grid, galaxy: Entity) -> Star {
    (
        placement(&system, grid),
        system,
        // What the map draws as a star, and no more than a marker: the field
        // paints the mark from this entity's position and the size
        // `super::scale` writes onto it, so there is no mesh, material or
        // render layer to carry.
        Shell,
        // Fitted by `pointing::size_indicators` before the first draw, and
        // what the pointer is tested against.
        Indicator::default(),
        // A system does not block what lies behind it, so a name drawn over
        // one is reported as well and `pointing` can weigh the two.
        Pickable { should_block_lower: false, is_hoverable: true },
        // Whether the system is drawn at all. Nothing inherits it:
        // `field::build_field` and the `labels`, `pointing` and `selection`
        // painters each read the value off the shell in their own query and
        // skip the ones hidden.
        Visibility::default(),
        // A star outside the galaxy's grid is not placed by it, and would be
        // drawn wherever its bare transform happened to put it rather than
        // where the cell says.
        ChildOf(galaxy),
    )
}

/// Carry a changed row onto where its star is drawn
///
/// A row fetched again is written over the one already there, and the
/// position it carries is free to differ from the one it replaces. What a
/// star is drawn *in* follows the row as well.
fn update(
    systems_query: Query<(Entity, Ref<System>)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    mut commands: Commands,
) {
    let Ok(grid) = grids.get(galaxy.0) else { return };

    for (entity, system) in &systems_query {
        if system.is_changed() {
            commands.entity(entity).insert(placement(&system, grid));
        }
    }
}

/// Where a system sits, as the galaxy's grid wants it
///
/// Split into the cell the position falls in and how far into that cell it
/// sits. The cell is an integer, so it stays exact however far out the system
/// is, and the transform left over is small enough to be carried without
/// losing anything.
///
/// A [`System`] holds its position in light years, which is what the database
/// records and what every distance the map states is measured in. The grid is
/// laid out in metres, so this is where the two meet — one of only two such
/// places, the other being the camera's own cell.
///
/// The scale is left alone. This is the system's own transform, and everything
/// hung off it is placed relative to a metre meaning a metre.
fn placement(system: &System, grid: &Grid) -> (CellCoord, Transform) {
    let (cell, translation) = grid.translation_to_grid(crate::space::metres(
        DVec3::from(system.position),
    ));

    (cell, Transform::from_translation(translation))
}

/// Which color a star is drawn in
pub(crate) fn hue(system: &System, color_by: &Res<ColorBy>) -> Hue {
    match color_by.deref() {
        ColorBy::Allegiance => allegiance_hue(system.allegiance),
        ColorBy::Government => government_hue(system.government),
        ColorBy::Security => security_hue(system.security),
    }
}

/// Cut the point spread every star's mark is painted through
///
/// One image for the whole sky, put up before [`super::field`]'s own startup
/// reads it. Nothing else is prepared here: a mark is painted flat in screen
/// space from a system's position and its size, so there is no per-star mesh
/// or material to build.
pub(crate) fn cut_star_psf(
    mut images: ResMut<Assets<Image>>,
    star_profile: Res<StarProfile>,
    mut commands: Commands,
) {
    let psf = images.add(star_psf(star_profile.0));
    commands.insert_resource(StarSprite { psf });
}

/// Cut the star point spread again when the profile changes
///
/// The sprite's texture is the profile's shape ([`star_psf`]); a change to the
/// profile is a change to that one image, and rewriting it in place repaints
/// every star drawn through it at once, the field's glint included.
/// Guarded on the change, since cutting the texture again and re-uploading it
/// is not free.
fn reprofile(
    profile: Res<StarProfile>,
    sprite: Option<Res<StarSprite>>,
    mut images: ResMut<Assets<Image>>,
) {
    if !profile.is_changed() {
        return;
    }
    let Some(sprite) = sprite else {
        return;
    };
    if let Some(mut image) = images.get_mut(&sprite.psf) {
        *image = star_psf(profile.0);
    }
}

/// The color an allegiance is drawn in
///
/// Off the reading rather than off a system, so that the aggregate field
/// colors a cell's allegiance histogram through the same mapping a mark is
/// painted by and the two cannot drift apart. See
/// [`galos_index::read::inhabited::allegiance_at`], which names the reading a
/// bucket counts.
pub(crate) fn allegiance_hue(allegiance: Option<Allegiance>) -> Hue {
    match allegiance {
        Some(Allegiance::Alliance) => Hue::Green,
        Some(Allegiance::Empire) => Hue::Cyan,
        Some(Allegiance::Federation) => Hue::Red,
        // A company rather than a power, as the Pilots Federation is
        Some(Allegiance::PilotsFederation | Allegiance::FrontlineSolutions) => {
            Hue::Orange
        }
        Some(Allegiance::PlayerPilots) => Hue::Yellow,
        Some(Allegiance::Independent) => Hue::Yellow,
        Some(Allegiance::Guardian) => Hue::Blue,
        Some(Allegiance::Thargoid) => Hue::Magenta,
        Some(Allegiance::None) | None => Hue::Grey,
    }
}

/// The color a government is drawn in. See [`allegiance_hue`].
pub(crate) fn government_hue(government: Option<Government>) -> Hue {
    match government {
        Some(Government::Anarchy) => Hue::Yellow,
        // None of the three is a way of governing anybody. A carrier
        // answers to whoever owns it, a megaconstruction site to whoever
        // is building it, and a privately owned settlement to its owner.
        Some(
            Government::Carrier
            | Government::Megaconstruction
            | Government::PrivateOwnership,
        ) => Hue::Green,
        Some(Government::Communism) => Hue::Red,
        Some(Government::Confederacy) => Hue::Red,
        Some(Government::Cooperative) => Hue::Orange,
        Some(Government::Corporate) => Hue::Cyan,
        Some(Government::Democracy) => Hue::Blue,
        Some(Government::Dictatorship) => Hue::Red,
        Some(Government::Engineer) => Hue::Magenta,
        Some(Government::Feudal) => Hue::Red,
        Some(Government::Patronage) => Hue::Red,
        Some(Government::Prison) => Hue::Red,
        Some(Government::PrisonColony) => Hue::Red,
        Some(Government::Theocracy) => Hue::Blue,
        Some(Government::None) | None => Hue::Grey,
    }
}

/// The color a security rating is drawn in. See [`allegiance_hue`].
pub(crate) fn security_hue(security: Option<Security>) -> Hue {
    match security {
        Some(Security::High) => Hue::Blue,
        Some(Security::Medium) => Hue::Cyan,
        Some(Security::Low) => Hue::Green,
        Some(Security::Anarchy) => Hue::Red,
        Some(Security::None) | None => Hue::Grey,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one hue whose light is not a zero or a one is the sRGB transfer
    /// applied by hand, and this is what says it was applied right
    ///
    /// [`Hue::light`] is written in linear light so a mark costs no
    /// conversion, which is worth doing once and worth checking once: every
    /// other channel is an endpoint, where sRGB and linear agree, and orange
    /// is the only one carrying a curve.
    #[test]
    fn orange_is_half_way_up_in_srgb() {
        let converted = LinearRgba::from(Color::srgb(1., 0.5, 0.));
        let light = Hue::Orange.light();
        assert!(
            (light.y - converted.green).abs() < 1e-6,
            "orange's green is {} and sRGB 0.5 is {}",
            light.y,
            converted.green
        );
        assert_eq!(
            Hue::Grey.light(),
            Vec3::ONE,
            "grey is a level, not a paint"
        );
    }

    /// A system named by address is drawn where the galaxy puts it
    ///
    /// **Not where its address puts it.** The names table stopped holding
    /// positions, and what a row would answer with is the middle of the
    /// boxel the address names — up to five light years off at the class
    /// most systems are, and half a sector at the largest. A route's stops
    /// and a searched system are drawn through here, so a row's answer
    /// standing in for the galaxy's is every one of them drawn beside where
    /// it is.
    #[test]
    fn a_system_named_by_address_is_placed_by_the_galaxy() {
        let dir = crate::testing::Scratch::new("placed");
        // Five light years off the middle of its own boxel, boxel edges
        // falling where they fall: enough that reading the row instead of
        // the payload is a different answer.
        let at = [5., 0., 0.];
        let entries = vec![galos_index::records::NameEntry {
            address: crate::testing::boxel_at(at),
            name: "SOMEWHERE".into(),
            position: [at[0] as f32, at[1] as f32, at[2] as f32],
        }];
        let sky = crate::testing::sky_of(dir.path(), &entries);
        let middle = elite_journal::Boxel::of(entries[0].address).place().0;
        assert_ne!(middle, at, "the fixture's place is its boxel middle");

        let names = Names::over(sky, entries.clone(), Vec::new());
        let system =
            system_at(entries[0].address, &Populated::default(), &names)
                .expect("a named system is drawable");

        assert_eq!(system.position, at);
    }

    /// A system at `address`, with nothing else on record
    fn system(address: i64) -> System {
        build_system(
            &RawSystem {
                address,
                position: [address as f64, 0., 0.],
                magnitude: None,
                temp_bucket: None,
                updated_at: None,
            },
            &Populated::default(),
            &Names::default(),
        )
    }

    /// Which systems a batch is about, in order
    fn about(systems: &[System]) -> Vec<i64> {
        systems.iter().map(|system| system.address).collect()
    }

    /// A system named `name` at `address`
    fn called(address: i64, name: &str) -> System {
        let mut system = system(address);
        system.name = name.into();
        system
    }

    /// A leg is named for its two ends, as the rows spell them
    ///
    /// Not as the user typed them: a leg of a trip is a route like any other,
    /// and the map's own spelling is what the rest of the map says.
    #[test]
    fn a_leg_is_named_for_its_ends() {
        let hops =
            [called(1, "SOL"), called(2, "WOLF 359"), called(3, "BARNARD")];

        let landed = plotted_route(
            &hops,
            "10",
            Drive::Unaided,
            Routing::default(),
            Tuning::default(),
            None,
            Duration::from_millis(1200),
        )
        .unwrap();

        assert_eq!(landed.label, "SOL -> BARNARD");
        assert_eq!(landed.systems, vec![1, 2, 3]);
    }

    /// Take a batch, building whatever was queued as a reference
    ///
    /// The tests queue built systems, so nothing here reads a payload; what
    /// it stands in for is [`drain_spawns`]'s closure, which does.
    fn taken(pending: &mut PendingSpawns, budget: usize) -> Vec<System> {
        pending.take(budget, |_, what| match what {
            Waiting::Built(system) => Some(*system),
            Waiting::Point { .. } => None,
        })
    }

    /// An app that polls route tasks, with a galaxy to draw their lines in
    fn landing() -> App {
        use bevy::asset::AssetPlugin;
        use big_space::prelude::BigSpace;

        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::time::TimePlugin,
            AssetPlugin::default(),
        ));
        app.init_asset::<Mesh>();
        app.init_asset::<StandardMaterial>();
        app.add_message::<route::PlottedRoute>();
        app.add_message::<route::UnflownLeg>();
        app.init_resource::<FetchTasks>();
        app.init_resource::<Plot>();
        app.init_resource::<PendingSpawns>();
        let galaxy = app
            .world_mut()
            .spawn((BigSpace::default(), crate::space::galaxy_grid()))
            .id();
        app.insert_resource(Galaxy(galaxy));
        app.add_systems(Update, spawn);
        app
    }

    /// Which leg a route task is keyed on, for the tests below.
    fn leg(start: &str, end: &str) -> FetchIndex {
        FetchIndex::Route(
            start.to_owned(),
            end.to_owned(),
            "10".to_owned(),
            Some("a trip".to_owned()),
            Drive::Unaided,
            Routing::default(),
            Tuning::default(),
        )
    }

    /// The form waits on the whole trip, not on its first leg
    ///
    /// The reported trouble. A trip's legs land one at a time, and the first
    /// of them cleared the spinner: the stop button went with it and the
    /// form read as finished while the other legs were still searching.
    /// What says a plot is still running is whether any leg of it is.
    #[test]
    fn the_form_waits_for_the_last_leg_of_a_trip() {
        let mut app = landing();
        let pool = bevy::tasks::AsyncComputeTaskPool::get();
        let now = Instant::now();
        *app.world_mut().resource_mut::<Plot>() = Plot::Working;

        // One leg landed, one still searching.
        let landed = pool
            .spawn(async move { vec![called(1, "SOL"), called(2, "BARNARD")] });
        let searching = pool
            .spawn(async move { std::future::pending::<Vec<System>>().await });
        {
            let mut tasks = app.world_mut().resource_mut::<FetchTasks>();
            tasks.fetched.insert(leg("SOL", "BARNARD"), (landed, now));
            tasks.fetched.insert(leg("BARNARD", "WOLF 359"), (searching, now));
        }

        app.update();

        assert_eq!(
            *app.world().resource::<Plot>(),
            Plot::Working,
            "the first leg landing said the whole trip was done",
        );

        // And when the last leg goes, so does the wait.
        app.world_mut()
            .resource_mut::<FetchTasks>()
            .fetched
            .remove(&leg("BARNARD", "WOLF 359"));
        app.update();

        assert_eq!(
            *app.world().resource::<Plot>(),
            Plot::Nothing,
            "the form is still waiting on a trip with nothing left",
        );
    }

    /// A system queued twice waits as one entry
    ///
    /// Which is what keeps two stars from being spawned on top of each other.
    /// The map cannot see what it spawned a moment ago, so the queue holds an
    /// address once however many times it is fetched before it is drawn.
    #[test]
    fn a_system_queued_twice_waits_once() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(system(1), false, false, now);
        pending.push(system(2), false, false, now);
        pending.push(system(1), false, false, now);

        assert_eq!(about(&taken(&mut pending, 10)), vec![1, 2]);
    }

    /// A re-queued system keeps its place and takes the later row
    #[test]
    fn a_re_queued_system_keeps_its_place_and_the_later_row() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(system(1), false, false, now);
        pending.push(system(2), false, false, now);
        let mut later = system(1);
        later.position = [9., 9., 9.];
        pending.push(later, false, false, now);

        let batch = taken(&mut pending, 10);
        assert_eq!(about(&batch), vec![1, 2]);
        assert_eq!(batch[0].position, [9., 9., 9.]);
    }

    /// The budget bounds what one frame takes, and the rest waits its turn
    #[test]
    fn the_budget_bounds_what_a_frame_takes() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        for address in 1..=5 {
            pending.push(system(address), false, false, now);
        }

        assert_eq!(about(&taken(&mut pending, 2)), vec![1, 2]);
        assert_eq!(about(&taken(&mut pending, 2)), vec![3, 4]);
        assert_eq!(about(&taken(&mut pending, 2)), vec![5]);
        assert!(pending.is_empty());
    }

    /// An empty queue takes nothing
    #[test]
    fn an_empty_queue_takes_nothing() {
        let mut pending = PendingSpawns::default();
        assert!(taken(&mut pending, 10).is_empty());
        assert!(pending.is_empty());
    }

    /// What the user asked for is drawn before what the map offered
    ///
    /// The reported slowness. A route's stops land behind everything the
    /// walk offered that frame — tens of thousands of marks, at
    /// [`SPAWN_BUDGET`] a frame — so the line was drawn and then filled in
    /// over the seconds it took the queue to reach its stops. A stop is a
    /// system named by hand; the marks are a galaxy nobody asked about.
    #[test]
    fn what_was_asked_for_is_drawn_first() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        for address in 1..=4 {
            pending.push(system(address), false, false, now);
        }
        pending.push(system(9), false, true, now);

        assert_eq!(about(&taken(&mut pending, 2)), vec![9, 1]);
    }

    /// A system already waiting moves up when it is asked for by name
    ///
    /// Which is the walk's own re-offer of a route's stops: the address is
    /// usually in the queue already, somewhere behind a galaxy of marks.
    #[test]
    fn a_waiting_system_moves_up_when_it_is_asked_for() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        for address in 1..=4 {
            pending.push(system(address), false, false, now);
        }
        pending.push(system(4), true, true, now);

        assert_eq!(about(&taken(&mut pending, 2)), vec![4, 1]);
        // And the copy left behind in arrival order is not drawn twice.
        assert_eq!(about(&taken(&mut pending, 10)), vec![2, 3]);
    }

    /// An offered point is a reference until it is drawn
    ///
    /// The whole of what makes framing a galaxy affordable: what the walk
    /// offers is which point of which cell it wants, and nothing is named,
    /// coloured or built until the frame that draws it. A point whose cell
    /// the map has since freed is dropped unbuilt, which is exactly the
    /// system that would otherwise have been built and evicted in one
    /// breath.
    #[test]
    fn an_offered_point_is_built_only_when_it_is_drawn() {
        let mut pending = PendingSpawns::default();
        let cell = galos_index::CellId::ROOT;
        pending.opening(Instant::now());
        assert!(pending.offer(7, cell, 3));
        assert!(pending.offer(8, cell, 4));
        assert_eq!(pending.queued(), 2);

        // Standing in for the payload read: 7 is still there, 8 is not.
        let batch = pending.take(10, |address, what| match what {
            Waiting::Point { at, .. } if address == 7 => {
                assert_eq!(at, 3, "the offer named another point");
                Some(system(7))
            }
            _ => None,
        });

        assert_eq!(about(&batch), vec![7], "8 was built anyway");
        assert!(pending.is_empty());
    }

    /// A pass's offers are the pass's, and the one before it is forgotten
    ///
    /// What the walk offers is the sky as it sees it now. An offer the frame's
    /// budget never reached is not a promise to draw it later: the pass runs
    /// again next frame and offers whatever is still wanted, and holding the
    /// old offers is what drew the sky several frames behind the camera — a
    /// system spawned, found unwanted and despawned. See [`PendingSpawns`].
    #[test]
    fn a_pass_forgets_what_the_pass_before_it_offered() {
        let mut pending = PendingSpawns::default();
        let cell = galos_index::CellId::ROOT;
        pending.opening(Instant::now());
        pending.offer(7, cell, 0);
        pending.offer(8, cell, 1);

        // The next pass, which wants one of the two.
        pending.opening(Instant::now());
        pending.offer(8, cell, 1);

        let taken: Vec<i64> = pending
            .take(10, |address, _| {
                let mut system = system(address);
                system.address = address;
                Some(system)
            })
            .iter()
            .map(|system| system.address)
            .collect();
        assert_eq!(taken, vec![8], "the pass before it was still queued");
    }

    /// What the walk offers in one pass is bounded
    ///
    /// A wide view resolves tens of thousands of points against a budget of
    /// two thousand, and offering all of them was most of what the pass cost.
    /// Past [`OFFER_BUDGET`] the offer is refused and the caller is told, so
    /// it can stop looking; what it refused is offered again next frame.
    #[test]
    fn a_pass_stops_offering_past_its_budget() {
        let mut pending = PendingSpawns::default();
        let cell = galos_index::CellId::ROOT;
        pending.opening(Instant::now());
        for address in 0..OFFER_BUDGET as i64 {
            assert!(pending.offer(address, cell, address as u32));
        }
        assert!(
            !pending.offer(-1, cell, 0),
            "the pass went on offering past its budget"
        );
        assert_eq!(pending.queued(), OFFER_BUDGET);
    }

    /// The queue is bounded, and what it turns away comes round again
    ///
    /// A galaxy-wide frame offers more than any second could draw —
    /// measured at two million on a Sol to Colonia plot — and holding all
    /// of it is holding what the next camera move throws away. Past the
    /// ceiling an offer is dropped unqueued; the walk runs every frame and
    /// offers whatever is still wanted again. A pinned system is never
    /// turned away: it is a route's own stop, and nothing else will offer
    /// it.
    #[test]
    fn the_queue_turns_away_what_it_cannot_hold() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        for address in 1..=(QUEUE_CEILING as i64 + 16) {
            pending.push(system(address), false, false, now);
        }
        assert_eq!(pending.queued(), QUEUE_CEILING, "held past the ceiling");

        pending.push(system(-1), true, false, now);
        assert_eq!(
            pending.queued(),
            QUEUE_CEILING + 1,
            "a pinned stop was turned away"
        );
    }

    /// A thing on the map to be clicked, told apart from the next by `which`
    fn clickable(which: u32) -> Entity {
        Entity::from_raw_u32(which).expect("an entity to click")
    }

    /// One click on its own opens nothing
    #[test]
    fn a_single_click_is_not_a_double() {
        let mut last = LastClick::default();
        assert!(!last.doubled(clickable(1), 0.));
    }

    /// Two clicks in quick succession on one system make a double
    #[test]
    fn two_quick_clicks_on_one_system_are_a_double() {
        let mut last = LastClick::default();
        last.doubled(clickable(1), 0.);
        assert!(last.doubled(clickable(1), DOUBLE_CLICK));
    }

    /// Two clicks far enough apart are two singles
    #[test]
    fn two_slow_clicks_are_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(clickable(1), 0.);
        assert!(!last.doubled(clickable(1), DOUBLE_CLICK + 0.01));
    }

    /// Two clicks on different systems are two singles
    ///
    /// Clicking a system flies the camera to it, so the star that lands
    /// under the pointer next is a different one often enough for this to be
    /// the usual way an accidental double would happen.
    #[test]
    fn two_clicks_on_different_systems_are_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(clickable(1), 0.);
        assert!(!last.doubled(clickable(2), 0.1));
    }

    /// A third quick click does not make a second double
    ///
    /// Otherwise a held-down finger would open a panel per click, and there
    /// would be no way to close one without it coming straight back.
    #[test]
    fn a_third_quick_click_is_not_a_double() {
        let mut last = LastClick::default();
        last.doubled(clickable(1), 0.);
        assert!(last.doubled(clickable(1), 0.1));
        assert!(!last.doubled(clickable(1), 0.2));
    }

    /// A slow click after a double starts a fresh pair
    #[test]
    fn counting_starts_again_after_a_double() {
        let mut last = LastClick::default();
        last.doubled(clickable(1), 0.);
        last.doubled(clickable(1), 0.1);
        assert!(!last.doubled(clickable(1), 0.2));
        assert!(last.doubled(clickable(1), 0.3));
    }

    /// Switching the profile cuts the one star texture again in place
    ///
    /// The sprite's point spread is a shared image; changing the profile has to
    /// rewrite it so every star repaints at once, rather than leaving the sky on
    /// the shape it was cut with. The two profiles draw different textures, so
    /// the bytes must change.
    #[test]
    fn switching_the_profile_cuts_the_star_texture_again() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<Image>>();
        app.insert_resource(StarProfile(ProfileKind::Moffat));
        app.add_systems(Startup, cut_star_psf);
        app.add_systems(Update, reprofile);
        app.update();

        let handle = app.world().resource::<StarSprite>().psf.clone();
        let moffat = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .expect("a star texture cut at startup")
            .data
            .clone();

        app.world_mut().resource_mut::<StarProfile>().0 = ProfileKind::Gaussian;
        app.update();
        let gaussian = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .expect("a re-cut star texture")
            .data
            .clone();

        assert_ne!(moffat, gaussian, "the profile switch did not re-cut");
    }
}
