use crate::camera::{MoveCamera, OrbitCamera};
use crate::schedule::MapSet;
use crate::search::Plot;
use crate::space::Galaxy;
use crate::systems::bodies::spawn::{Body, Places};
use crate::systems::{
    Spyglass, System,
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
use chrono::{DateTime, Utc};
use elite_journal::{Allegiance, Government, system::Security};
use galos_index::aggregate::bucket_temperature;
use galos_index::meta::Economies;
use galos_photometry::psf::{ProfileKind, Psf};
use galos_photometry::{Magnitude, Temperature};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ops::Deref,
    time::Instant,
};

pub fn plugin(app: &mut App) {
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(true));
    app.insert_resource(StarExposure::default());
    app.insert_resource(StarProfile::default());

    app.add_systems(Startup, bake_star_psf);
    app.init_resource::<PendingSpawns>();
    app.add_systems(Update, spawn.in_set(MapSet::Populate));
    // Turns a bounded number of queued systems into entities each frame, so a
    // wide region does not build its whole payload in one. After `spawn`,
    // which fills the queue from what the fetch tasks return.
    app.add_systems(Update, drain_spawns.in_set(MapSet::Populate).after(spawn));
    app.add_systems(Update, update.in_set(MapSet::Populate).before(spawn));
    // Rebakes the star texture when the profile changes; guarded on the
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

/// The apparent-magnitude range the palette resolves brightness over
///
/// Wide enough that no star a real vantage shows is clamped at either end: the
/// bright end is past the brightest apparent magnitude anything reaches, the
/// faint end past what any exposure draws. Not a cap on a star — the range only
/// sets how finely the ramp between them is stepped.
const MAG_HI: f64 = -6.;
const MAG_LO: f64 = 12.;

/// How finely the brightness ramp is stepped, in handles across the range
const MAG_STEPS: usize = 90;

/// How hard flux is compressed onto the display, as an exponent
///
/// A star's core is its flux raised to this. Flux spans ten powers of ten
/// across a sky, more than a display holds, so it is compressed toward the
/// eye's own near-logarithmic response: below one it lifts the faint end up out
/// of the floor and pulls the bright end down out of a uniform white, so the
/// whole range reads. A smooth curve, not a clamp — every star keeps its order,
/// the brightest simply does not run away.
const GAMMA: f64 = 0.4;

/// How bright a magnitude-zero star's core is emitted, at exposure zero
///
/// The reference the compressed ramp hangs from: a magnitude-zero star (flux
/// one) is emitted at this, and [`StarExposure`] lifts or lowers the whole ramp
/// by stops from there.
///
/// High because the field draws this level through the point-spread profile
/// (see [`super::field`]), which piles the light into a tight core and lets it
/// fall to dim wings — most of a mark is wing. A flat mark of the same peak
/// spread its whole level across the disc and read far brighter, so it sat near
/// eight; the profile deposits a fraction of that, so the reference climbs to
/// meet it. Set by eye, roughly five stops up.
const BRIGHT: f32 = 256.;

/// Which brightness step an apparent magnitude falls on, clamped to the range
pub(crate) fn mag_step(apparent: f64) -> usize {
    let f = (apparent - MAG_HI) / (MAG_LO - MAG_HI);
    (f.clamp(0., 1.) * MAG_STEPS as f64).round() as usize
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
    /// The linear gain the stops come to: a doubling per stop.
    pub(crate) fn factor(&self) -> f32 {
        2f32.powf(self.0)
    }

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

/// The emission a photometric star of temperature `bucket` and brightness
/// `step` is drawn at, at exposure `factor`
///
/// The one place the realistic view's colour is worked out — the field's
/// per-vertex glint reads it (see [`super::field`]). The tint is the bucket's
/// blackbody colour; the strength is the step's flux compressed by [`GAMMA`],
/// lifted by [`BRIGHT`] and the exposure, so a bright star's core outshines a
/// faint one's.
pub(crate) fn photometric_emissive(
    bucket: usize,
    step: usize,
    factor: f32,
) -> LinearRgba {
    let tint = Temperature(bucket_temperature(bucket)).color();
    let mag = MAG_HI + (step as f64 / MAG_STEPS as f64) * (MAG_LO - MAG_HI);
    let level = BRIGHT * Magnitude(mag).flux().0.powf(GAMMA) as f32 * factor;
    LinearRgba::rgb(tint[0] * level, tint[1] * level, tint[2] * level)
}

/// How wide the baked point spread is, in texels a side
const PSF_TEXELS: u32 = 128;

/// The star point spread: [`galos_photometry::psf::Moffat`], baked to a texture
///
/// The one shared profile, sampled from the crate so the map and `galos_sky`
/// wear the same instrument — the map bakes its shape into a texture once, the
/// sky evaluates it per pixel, but the `β` and the falloff are one definition.
/// A bright core with power-law wings falling to nothing by the edge; a
/// brighter star clears more of it above the eye's floor (see
/// [`super::scale::size_photometrically`]), so brightness reads as size with no
/// disc ever drawn. Linear rather than sRGB, so it multiplies the emissive
/// straight; the channels carry the shape and the tint is the material's.
pub(crate) fn star_psf(profile: ProfileKind) -> Image {
    let n = PSF_TEXELS;
    let centre = (n as f32 - 1.) / 2.;
    // A compact core: a tenth of its peak a fifth of the way out, all but gone
    // by the edge.
    let alpha = n as f32 / 16.;
    let psf = Psf::new(profile, alpha as f64);
    let shape = |r: f32| psf.shape(r as f64) as f32;
    // Subtracted so the corner reaches exactly zero and no square edge shows.
    let floor = shape(centre);
    let mut data = Vec::with_capacity((n * n * 4) as usize);
    for y in 0..n {
        for x in 0..n {
            let r = ((x as f32 - centre).powi(2) + (y as f32 - centre).powi(2))
                .sqrt();
            let v = ((shape(r) - floor) / (1. - floor)).clamp(0., 1.);
            let b = (v * 255.) as u8;
            data.extend_from_slice(&[b, b, b, b]);
        }
    }
    let mut image = Image::new(
        Extent3d { width: n, height: n, depth_or_array_layers: 1 },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8Unorm,
        RenderAssetUsages::RENDER_WORLD,
    );
    image.sampler = ImageSampler::linear();
    image
}

/// The point spread every star's mark is painted through
///
/// One baked image shared by the whole sky, so the map and `galos_sky` wear
/// the same instrument. [`super::field`] samples it per mark in the realistic
/// view — a bright core falling to nothing, which the camera's bloom spreads
/// into a glint — and [`reprofile`] rewrites it in place when the profile
/// changes, repainting every star at once.
#[derive(Resource)]
pub(crate) struct StarSprite {
    /// What [`star_psf`] baked, under the handle [`super::field`] cloned.
    pub psf: Handle<Image>,
}

/// Which point-spread profile the realistic view's stars wear
///
/// The shape [`star_psf`] bakes into the sprite texture — a Moffat with its
/// wings or a tighter Gaussian; see [`galos_photometry::psf::ProfileKind`].
/// The map sizes a star by [`super::scale`]'s own `psf_radius` law either way,
/// so this changes the halo a star wears, not how large it draws.
/// [`reprofile`] rebakes the texture when it changes.
#[derive(Resource, Default)]
pub struct StarProfile(pub ProfileKind);

/// The colors a star may be drawn in
///
/// Named rather than numbered, so that a scheme below says which color it
/// means. One colour each, and nothing indexes them: [`super::field`] asks
/// `Hue::color` for the three channels it paints a mark with.
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
    /// What the hue is painted in
    ///
    /// The colour alone. What [`super::field`] paints a mark at is these
    /// three channels and a fade of its own — how much of the mark is left as
    /// it goes out, and how far the filters have dimmed it — so an alpha
    /// carried here would be read by nobody. The grey a system with nothing
    /// on record comes out is darker than the rest, so that an unknown system
    /// does not read as a finding.
    pub(crate) const fn color(self) -> Color {
        match self {
            Hue::Green => Color::srgb(0., 1., 0.),
            Hue::Cyan => Color::srgb(0., 1., 1.),
            Hue::Red => Color::srgb(1., 0., 0.),
            Hue::Orange => Color::srgb(1., 0.5, 0.),
            Hue::Yellow => Color::srgb(1., 1., 0.),
            Hue::Blue => Color::srgb(0., 0., 1.),
            Hue::Magenta => Color::srgb(1., 0., 1.),
            Hue::Grey => Color::srgb(0.15, 0.15, 0.15),
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
/// The systems arrive already named and coloured, built on the task's own
/// thread (see [`super::fetch`]), so nothing here joins a table or clones a
/// row. What lands is queued into [`PendingSpawns`] rather than spawned on the
/// spot, and [`drain_spawns`] turns a bounded number into entities each frame:
/// a wide region delivers its whole payload in one task completion, and
/// spawning all of it at once is what stalls the frame.
pub fn spawn(
    route_query: Query<(Entity, &Route)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut plotted: MessageWriter<route::PlottedRoute>,
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
    let mut arrived: Vec<(System, bool)> = Vec::new();
    let mut arrived_at = time.startup();
    // Taken down while the tasks are being walked and applied after, the walk
    // holding the tasks and the taking writing the surveys beside them.
    let mut answered: Vec<(FetchIndex, DateTime<Utc>)> = Vec::new();

    tasks.fetched.retain(|index, (task, fetched_at)| {
        let status = block_on(future::poll_once(task));
        let retain = status.is_none();
        if let Some((new_systems, at)) = status {
            // What the map can answer for from here on. Written where the
            // answer lands rather than where it was asked for: until it is in
            // hand the map holds nothing, and a question that errored leaves
            // no moment and so leaves the region to be asked about again.
            if let Some(at) = at {
                answered.push((index.clone(), at));
            }
            if let FetchIndex::Route(start, end, range, trip) = index {
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
                if *plot == Plot::Working && new_systems.len() < 2 {
                    *plot = Plot::Failed(format!(
                        "No route from {start} to {end} at {range} Ly"
                    ));
                } else if *plot == Plot::Working {
                    *plot = Plot::Nothing;
                }

                // Said rather than acted on. What a route does to the map is
                // `route::plotted`'s business; this is the one place its
                // systems are in hand, so it is the one place that can say
                // what they are. The systems arrive built, so the line is
                // drawn straight from them before they join the spawn queue.
                if let Some(landed) =
                    plotted_route(&new_systems, range, trip.clone())
                {
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
            // them: those are wanted wherever they lie, as the evictor keeps
            // them. A region and a route's stops are weighed against the reach.
            let pinned = matches!(index, FetchIndex::Systems(..));
            arrived
                .extend(new_systems.into_iter().map(|system| (system, pinned)));
        }
        retain
    });

    for (index, at) in answered {
        tasks.surveyed(index, at);
    }

    // Queue rather than spawn, and only what is not already on the map. The
    // fetch is by region, so zooming out re-delivers the whole wider sphere,
    // most of it systems already drawn; queueing those would drain to nothing
    // but a churn of no-op re-inserts. The queue also holds one entry per
    // address, so a system fetched twice before it is drawn lands once — which
    // is what stops two entities landing for one system. An evicted system is
    // not resident, so it still re-queues and comes back.
    let resident: HashSet<i64> =
        systems.iter().map(|system| system.address).collect();
    for (system, pinned) in arrived {
        if resident.contains(&system.address) {
            continue;
        }
        pending.push(system, pinned, arrived_at);
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
fn plotted_route(
    systems: &[System],
    range: &str,
    trip: Option<String>,
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
        trip,
    })
}

/// How many systems are turned into entities in one frame
///
/// A cap on the structural churn the map does per frame, since spawning an
/// entity mutates the world and cannot leave the main thread. A wide region
/// arrives as one payload of tens of thousands of systems, and building all of
/// them at once is a visible hitch; spread over frames it streams in instead,
/// which the map already reads as a region drawing before it has fully loaded.
const SPAWN_BUDGET: usize = 2048;

/// Systems fetched and built, waiting to become entities
///
/// The fetch tasks return whole regions at once, and [`spawn`] queues them
/// here rather than spawning the lot in the frame they land. [`drain_spawns`]
/// takes [`SPAWN_BUDGET`] of them a frame, in arrival order.
///
/// Keyed by address so a system fetched twice before it is drawn holds one
/// entry, keeping the later row: a re-fetch is a refresh, and one entry is
/// also what stops two entities landing for a system the world does not yet
/// hold when the second copy is read.
#[derive(Resource, Default)]
pub struct PendingSpawns {
    order: VecDeque<i64>,
    rows: HashMap<i64, (System, bool)>,
    arrived_at: Option<Instant>,
}

impl PendingSpawns {
    /// Queue `system`, keeping its place if it is already waiting and taking
    /// the later row.
    ///
    /// `pinned` marks a system wanted whatever the reach — one picked out and
    /// flown to — which [`prune`](Self::prune) never drops. A system queued
    /// again as pinned stays pinned.
    pub(crate) fn push(&mut self, system: System, pinned: bool, at: Instant) {
        let address = system.address;
        match self.rows.get_mut(&address) {
            Some((held, held_pinned)) => {
                *held = system;
                *held_pinned |= pinned;
            }
            None => {
                self.rows.insert(address, (system, pinned));
                self.order.push_back(address);
            }
        }
        self.arrived_at = Some(self.arrived_at.map_or(at, |prev| prev.max(at)));
    }

    /// Drop the queued systems the reach has since left behind
    ///
    /// A wide region can be queued and then flown or zoomed away from before
    /// it is drawn, at which point spawning it is spawning what the evictor
    /// drops the same frame. So the queue is weighed against the reach as the
    /// live set is, and what falls beyond the kept sphere is forgotten unread
    /// — except a pinned system, which is wanted wherever it lies. Nothing to
    /// weigh against while the spyglass is not clearing, which is the map
    /// holding everything it has.
    fn prune(&mut self, center: DVec3, keep: f64, clears: bool) {
        if !clears {
            return;
        }
        let rows = &mut self.rows;
        self.order.retain(|address| {
            let kept = rows.get(address).is_some_and(|(system, pinned)| {
                *pinned || center.distance(DVec3::from(system.position)) <= keep
            });
            if !kept {
                rows.remove(address);
            }
            kept
        });
    }

    fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// How many systems are waiting, for the diagnostics panel to read.
    pub fn queued(&self) -> usize {
        self.order.len()
    }

    /// Take up to `budget` systems, oldest first.
    fn take(&mut self, budget: usize) -> Vec<System> {
        let n = budget.min(self.order.len());
        let mut batch = Vec::with_capacity(n);
        while batch.len() < n {
            let Some(address) = self.order.pop_front() else { break };
            if let Some((system, _)) = self.rows.remove(&address) {
                batch.push(system);
            }
        }
        batch
    }
}

/// Turn a budgeted number of queued systems into entities
///
/// First weighs the queue against the reach, dropping what a move has left
/// behind so nothing is spawned only to be evicted, then hands
/// [`SPAWN_BUDGET`] of what remains to [`spawn_systems`], so the frame's
/// structural work is bounded however wide the region that arrived.
#[allow(clippy::too_many_arguments)]
fn drain_spawns(
    mut pending: ResMut<PendingSpawns>,
    systems_query: Query<(Entity, &System)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    filtering: Filtering,
    time: Res<Time<Real>>,
    camera: Query<&OrbitCamera>,
    spyglass: Res<Spyglass>,
    bounded: Option<Res<crate::systems::bounded::LodFetch>>,
    mut commands: Commands,
) {
    // Weigh the queue against the reach before drawing any of it, the same cut
    // the evictor makes on the live set, so a region flown away from is
    // dropped unread rather than spawned and evicted in the same breath.
    // Only against the spyglass reach, and only when the spyglass is the
    // source. The bounded walk chooses its own set with no radius to weigh
    // against, and its marks sit wherever the walk reached — pruning them here
    // against the spyglass radius would drop the whole set before it spawned.
    let bounded = bounded.as_deref().is_some_and(|b| b.0);
    if !bounded && let Ok(camera) = camera.single() {
        let keep = spyglass.radius as f64 * super::EVICT_MARGIN;
        pending.prune(camera.center, keep, spyglass.clear);
    }
    if pending.is_empty() {
        return;
    }
    let Ok(grid) = grids.get(galaxy.0) else { return };
    let arrived_at = pending.arrived_at.unwrap_or_else(|| time.startup());
    let batch = pending.take(SPAWN_BUDGET);
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

/// Name and colour a raw system from the resident tables
///
/// The cells give an address and a place and nothing political. Everything a
/// [`System`] is coloured and filtered by comes from the [`Populated`] table
/// where the system is one of the dynamic set, and its name from [`Names`]. A
/// system absent from `populated` is ungoverned, which is most of the galaxy,
/// and drawn as such.
///
/// The cells carry no per-system time, so `updated_at` is stamped now rather
/// than read: Recency over individuals lands with the field step's age buckets,
/// and until then a freshly drawn system is never taken to be stale.
pub(crate) fn build_system(
    raw: &RawSystem,
    populated: &Populated,
    names: &Names,
) -> System {
    let name = names
        .get(raw.address)
        .map(|entry| entry.name.clone())
        .unwrap_or_else(|| raw.address.to_string());
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
            updated_at: Utc::now(),
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
            updated_at: Utc::now(),
        },
    }
}

/// The drawable system at an address, if the resident tables can place it
///
/// A search or a filter names a system by address; its place comes from the
/// [`Names`] table and everything political from [`Populated`]. [`None`] where
/// the names table cannot place it, which is a system the map cannot draw.
pub(crate) fn system_at(
    address: i64,
    populated: &Populated,
    names: &Names,
) -> Option<System> {
    let entry = names.get(address)?;
    let raw = RawSystem {
        address,
        position: [
            entry.position[0] as f64,
            entry.position[1] as f64,
            entry.position[2] as f64,
        ],
        magnitude: None,
        temp_bucket: None,
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
    for system in new_systems {
        // What no filter admits is dropped rather than dimmed once the dim is
        // zero, so it is never spawned in the first place: the load avoided,
        // not paid and then hidden. Left to [`super::evict`] to take off what
        // already stands.
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

            let mut spawned = commands.spawn((
                placement(&system, grid),
                system,
                // What the map draws as a star, and no more than a marker:
                // the field paints the mark from this entity's position and
                // the size `super::scale` writes onto it, so there is no
                // mesh, material or render layer to carry.
                Shell,
                // Fitted by `pointing::size_indicators` before the first
                // draw, and what the pointer is tested against.
                Indicator::default(),
                // A system does not block what lies behind it, so a name
                // drawn over one is reported as well and `pointing` can
                // weigh the two.
                Pickable { should_block_lower: false, is_hoverable: true },
                // Whether the system is drawn at all. Nothing inherits it:
                // `field::build_field` and the `labels`, `pointing` and
                // `selection` painters each read the value off the shell in
                // their own query and skip the ones hidden.
                Visibility::default(),
                // A star outside the galaxy's grid is not placed by it,
                // and would be drawn wherever its bare transform happened
                // to put it rather than where the cell says.
                ChildOf(galaxy.0),
            ));
            if excluded {
                spawned.insert(Filtered);
            }
        }
    }
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
        ColorBy::Allegiance => allegiance_hue(system),
        ColorBy::Government => government_hue(system),
        ColorBy::Security => security_hue(system),
    }
}

/// Bake the point spread every star's mark is painted through
///
/// One image for the whole sky, put up before [`super::field`]'s own startup
/// reads it. Nothing else is prepared here: a mark is painted flat in screen
/// space from a system's position and its size, so there is no per-star mesh
/// or material to build.
pub(crate) fn bake_star_psf(
    mut images: ResMut<Assets<Image>>,
    star_profile: Res<StarProfile>,
    mut commands: Commands,
) {
    let psf = images.add(star_psf(star_profile.0));
    commands.insert_resource(StarSprite { psf });
}

/// Rebake the star point spread when the profile changes
///
/// The sprite's texture is the profile's shape ([`star_psf`]); a change to the
/// profile is a change to that one image, and rewriting it in place repaints
/// every star drawn through it at once, the field's glint included.
/// Guarded on the change, since baking and re-uploading the texture is not free.
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

fn allegiance_hue(system: &System) -> Hue {
    match system.allegiance {
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

fn government_hue(system: &System) -> Hue {
    match system.government {
        Some(Government::Anarchy) => Hue::Yellow,
        // Neither is a way of governing anybody. A carrier answers to whoever
        // owns it, and a megaconstruction site to whoever is building it.
        Some(Government::Carrier | Government::Megaconstruction) => Hue::Green,
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

fn security_hue(system: &System) -> Hue {
    match system.security {
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

    /// A system at `address`, with nothing else on record
    fn system(address: i64) -> System {
        build_system(
            &RawSystem {
                address,
                position: [address as f64, 0., 0.],
                magnitude: None,
                temp_bucket: None,
            },
            &Populated::default(),
            &Names::default(),
        )
    }

    /// Which systems a batch is about, in order
    fn about(systems: &[System]) -> Vec<i64> {
        systems.iter().map(|system| system.address).collect()
    }

    /// A system at `address`, placed `x` light years out along the first axis
    fn at(address: i64, x: f64) -> System {
        let mut system = system(address);
        system.position = [x, 0., 0.];
        system
    }

    /// A system named `name` at `address`
    fn called(address: i64, name: &str) -> System {
        let mut system = system(address);
        system.name = name.to_owned();
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

        let landed = plotted_route(&hops, "10", None).unwrap();

        assert_eq!(landed.label, "SOL -> BARNARD");
        assert_eq!(landed.systems, vec![1, 2, 3]);
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
        pending.push(system(1), false, now);
        pending.push(system(2), false, now);
        pending.push(system(1), false, now);

        assert_eq!(about(&pending.take(10)), vec![1, 2]);
    }

    /// A re-queued system keeps its place and takes the later row
    #[test]
    fn a_re_queued_system_keeps_its_place_and_the_later_row() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(system(1), false, now);
        pending.push(system(2), false, now);
        let mut later = system(1);
        later.position = [9., 9., 9.];
        pending.push(later, false, now);

        let batch = pending.take(10);
        assert_eq!(about(&batch), vec![1, 2]);
        assert_eq!(batch[0].position, [9., 9., 9.]);
    }

    /// The budget bounds what one frame takes, and the rest waits its turn
    #[test]
    fn the_budget_bounds_what_a_frame_takes() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        for address in 1..=5 {
            pending.push(system(address), false, now);
        }

        assert_eq!(about(&pending.take(2)), vec![1, 2]);
        assert_eq!(about(&pending.take(2)), vec![3, 4]);
        assert_eq!(about(&pending.take(2)), vec![5]);
        assert!(pending.is_empty());
    }

    /// An empty queue takes nothing
    #[test]
    fn an_empty_queue_takes_nothing() {
        let mut pending = PendingSpawns::default();
        assert!(pending.take(10).is_empty());
        assert!(pending.is_empty());
    }

    /// The queue drops what the reach has left behind, so nothing is spawned
    /// only for the evictor to take back off the same frame
    #[test]
    fn pruning_drops_the_unpinned_the_reach_has_left() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(at(1, 5.), false, now);
        pending.push(at(2, 50.), false, now);
        pending.push(at(3, 50.), true, now);

        // radius 10 * margin 1.5 = kept within 15 ly.
        pending.prune(DVec3::ZERO, 15., true);

        // 1 is within reach, 2 is beyond it, 3 is beyond it but pinned.
        assert_eq!(about(&pending.take(10)), vec![1, 3]);
    }

    /// A system queued again as pinned is kept where it would have been dropped
    #[test]
    fn re_queuing_as_pinned_keeps_a_far_system() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(at(1, 50.), false, now);
        pending.push(at(1, 50.), true, now);

        pending.prune(DVec3::ZERO, 15., true);

        assert_eq!(about(&pending.take(10)), vec![1]);
    }

    /// Nothing is weighed against a spyglass that is not clearing, which is the
    /// map holding everything it has
    #[test]
    fn pruning_is_off_while_not_clearing() {
        let mut pending = PendingSpawns::default();
        let now = Instant::now();
        pending.push(at(2, 50.), false, now);

        pending.prune(DVec3::ZERO, 15., false);

        assert_eq!(about(&pending.take(10)), vec![2]);
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

    /// Switching the profile rebakes the one star texture in place
    ///
    /// The sprite's point spread is a shared image; changing the profile has to
    /// rewrite it so every star repaints at once, rather than leaving the sky on
    /// the shape it was baked with. The two profiles draw different textures, so
    /// the bytes must change.
    #[test]
    fn switching_the_profile_rebakes_the_star_texture() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<Image>>();
        app.insert_resource(StarProfile(ProfileKind::Moffat));
        app.add_systems(Startup, bake_star_psf);
        app.add_systems(Update, reprofile);
        app.update();

        let handle = app.world().resource::<StarSprite>().psf.clone();
        let moffat = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .expect("a baked star texture")
            .data
            .clone();

        app.world_mut().resource_mut::<StarProfile>().0 = ProfileKind::Gaussian;
        app.update();
        let gaussian = app
            .world()
            .resource::<Assets<Image>>()
            .get(&handle)
            .expect("a rebaked star texture")
            .data
            .clone();

        assert_ne!(moffat, gaussian, "the profile switch did not rebake");
    }
}
