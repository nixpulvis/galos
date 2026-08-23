use crate::camera::MoveCamera;
use crate::schedule::MapSet;
use crate::search::Plot;
use crate::space::Galaxy;
use crate::systems::bodies::spawn::{Body, Places, Strength};
use crate::systems::{
    System,
    fetch::FetchIndex,
    fetch::FetchTasks,
    filter::{DimTo, Filtered, Filters},
    pointing::{DRAG_THRESHOLD, DragDistance, Indicator, PointedAt},
    roundness::Roundness,
    route::spawn::{framing, spawn_route},
    route::{self, PlottedRoute, Route},
    selection::{Picked, PickedBody, Selection},
    system_to_vec,
};
use crate::ui::{Gesture, PressOwner};
use bevy::camera::visibility::{RenderLayers, ViewVisibility};
use bevy::diagnostic::FrameCount;
use bevy::light::NotShadowCaster;
use bevy::math::DVec3;
use bevy::picking::pointer::PointerMap;
use bevy::prelude::*;
use bevy::tasks::block_on;
use bevy::tasks::futures_lite::future;
use big_space::prelude::*;
use chrono::{DateTime, Utc};
use elite_journal::{Allegiance, Government, system::Security};
use galos_db::systems::System as DbSystem;
use std::{
    collections::{HashMap, HashSet},
    ops::Deref,
    time::Instant,
};

pub fn plugin(app: &mut App) {
    app.insert_resource(ColorBy::Allegiance);
    app.insert_resource(ShowNames(true));

    app.add_systems(Startup, init_materials);
    app.add_systems(Update, spawn.in_set(MapSet::Populate));
    app.add_systems(Update, update.in_set(MapSet::Populate).before(spawn));
    app.add_systems(Update, redim.in_set(MapSet::Populate));
    // Reads where the camera is standing, which `Camera` settles, and the sets
    // already say that comes first.
    app.add_systems(Update, shells.in_set(MapSet::Present));

    app.add_observer(select_on_click);
    // Answers what is pointed at this frame, which `point_at` decides.
    app.add_systems(
        Update,
        fly_on_double_click
            .in_set(MapSet::Present)
            .after(super::pointing::point_at),
    );
}

/// What a star is drawn in, at full strength and dimmed
///
/// Two sets of the same colors rather than one recolored per star, because
/// the color lives on a shared asset. A star moves between the sets by
/// swapping which handle it points at, which repaints only that star, and the
/// dim set is recolored in place when [`DimTo`] moves, which is meant to
/// repaint every dimmed star at once.
#[derive(Resource)]
pub struct SystemMaterials {
    /// One per color, indexed as [`hue`] answers
    bright: Vec<Handle<StandardMaterial>>,
    /// The same colors, at whatever [`DimTo`] is asking
    dim: Vec<Handle<StandardMaterial>>,
    /// The same colors again, in every step of the fade a shell goes out
    /// through
    ///
    /// Laid out as [`Hue::ALL`] by step, so a shell part way out follows its
    /// own hue and its own strength to a handle and nothing has to be assigned
    /// or repainted. Which is what lets two of them be part way at once: only
    /// the held system's mark goes out, but the one just let go of is still
    /// coming back while the next is going, and a single handle repainted per
    /// frame would draw both at whichever strength was written last.
    fading: Vec<Handle<StandardMaterial>>,
}

impl SystemMaterials {
    /// The handle for `hue`, at the strength `dimmed` asks for
    ///
    /// Lent rather than handed over. This is asked of every shell every frame
    /// and the answer nearly always matches what the shell already points at,
    /// so a handle taken by value would be an atomic pair per star per frame
    /// spent on a comparison.
    fn get(&self, hue: Hue, dimmed: bool) -> &Handle<StandardMaterial> {
        let set = if dimmed { &self.dim } else { &self.bright };
        &set[hue as usize]
    }

    /// And the handle for `hue` at `strength` of the way out
    ///
    /// Stepped to [`FADE_STEPS`], the whole fade being painted once rather
    /// than a handle repainted per shell per frame.
    fn going(&self, hue: Hue, strength: f32) -> &Handle<StandardMaterial> {
        let step =
            (strength.clamp(0., 1.) * FADE_STEPS as f32).round() as usize;

        &self.fading[hue as usize * (FADE_STEPS + 1) + step]
    }
}

/// How many steps a shell is drawn going out in
///
/// The strength runs the emission, a shell being opaque now and its fill left
/// where it is, and a mark does not always go out at the pace
/// [`super::bodies::spawn::GOES_OUT_IN`] bounds: a camera coming in slowly
/// leaves the distance in charge, and the fade can take seconds. So the steps
/// are cut fine enough not to be seen at that pace either, under a percent of
/// the strength apiece.
const FADE_STEPS: usize = 128;

/// How bright a shell's glow is emitted, at full strength
///
/// Full. A shell draws opaque and without bloom now, so its colour is the
/// emission itself rather than a haze spread around a white-hot core, and the
/// emission has to carry the hue on its own.
///
/// One, so a resting mark is emitted at the colour it was named in: the
/// palette runs each channel from nothing to one (see [`Hue::color`]), and
/// [`crate::camera::shells_view`] draws the shells past the filmic curve, so
/// an emission of one reaches the screen as that colour at full and none of
/// it clips or washes. Lower would only dim it towards black; the fade takes
/// a mark out that way, but a mark standing does so at full.
const SHELL_GLOW: f32 = 1.;

/// The colors a star may be drawn in
///
/// Named rather than numbered, so that a scheme below says which color it
/// means. The two material sets are laid out in [`Hue::ALL`] order and
/// indexed by the hue itself, so there is one list of colors rather than a
/// list and a set of numbers agreeing with it.
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
    /// Every hue, in the order the material sets hold them
    const ALL: [Hue; 8] = [
        Hue::Green,
        Hue::Cyan,
        Hue::Red,
        Hue::Orange,
        Hue::Yellow,
        Hue::Blue,
        Hue::Magenta,
        Hue::Grey,
    ];

    /// What the hue is painted in
    ///
    /// Alpha is part of it: a star is drawn as a translucent ball with a glow
    /// over it, and the grey a system with nothing on record comes out is
    /// fainter than the rest so that an unknown system does not read as a
    /// finding.
    const fn color(self) -> Color {
        match self {
            Hue::Green => Color::srgba(0., 1., 0., 0.4),
            Hue::Cyan => Color::srgba(0., 1., 1., 0.4),
            Hue::Red => Color::srgba(1., 0., 0., 0.4),
            Hue::Orange => Color::srgba(1., 0.5, 0., 0.4),
            Hue::Yellow => Color::srgba(1., 1., 0., 0.4),
            Hue::Blue => Color::srgba(0., 0., 1., 0.4),
            Hue::Magenta => Color::srgba(1., 0., 1., 0.4),
            Hue::Grey => Color::srgba(0.15, 0.15, 0.15, 0.3),
        }
    }
}

/// How a star is painted in `color`, at `strength` of full, blended or not
///
/// A resting mark is drawn opaque, so a wide field of them costs only the
/// nearest at each pixel and there is nothing to sort. A mark on its way out
/// is drawn blended instead: fading an opaque disc leaves it standing dark
/// over the contents drawn in its place, the same disc reading one way against
/// empty space and another over a lit system, so a mark goes out and comes
/// back looking unlike itself. Blended, it crosses with what is behind it the
/// same both ways. Only the held system ever goes out, so at most one mark is
/// ever the blended kind and the field pays nothing for it.
///
/// The glow and the coverage both follow `strength`, the glow being most of
/// what a mark reads as and the coverage what lets the contents through.
fn star_material(
    color: Color,
    strength: f32,
    mode: AlphaMode,
) -> StandardMaterial {
    let coverage = match mode {
        AlphaMode::Blend => strength,
        _ => 1.,
    };
    StandardMaterial {
        base_color: color.with_alpha(coverage),
        alpha_mode: mode,
        emissive: LinearRgba::from(color.with_alpha(1.))
            * SHELL_GLOW
            * strength,
        ..default()
    }
}

// pub struct SystemMaterials(pub HashMap<String, Handle<StandardMaterial>>);

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
/// than as a field of dots, and how many of them are drawn is answered by
/// [`crate::systems::labels::NameRadius`] and by the room each is given
/// rather than by having them off.
#[derive(Resource)]
pub struct ShowNames(pub bool);

/// A whole system, drawn as one thing
///
/// From far enough away nothing in a system can be told apart from anything
/// else in it, so what is drawn is a single sphere standing for the lot. Up
/// close the same sphere is the edge of what the system takes up, and its
/// contents are drawn inside it.
///
/// Not a star. A system is a place, a star is a thing in it, and there may be
/// several; those are read from the `stars` table and drawn within this.
///
/// [`super::scale`] writes a size onto this entity rather than onto the
/// system, because a shell is drawn far larger than the system is so as to
/// stay visible from light years away. Scale is inherited, so anything sharing
/// an entity with it would be stretched by the same exaggeration; keeping the
/// shell on a child of its own leaves the system's transform meaning what it
/// says, and lets labels and bodies sit at their true size.
#[derive(Component)]
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

/// Polls the tasks in `FetchTasks` and spawns entities for each of the
/// resulting star systems
pub fn spawn(
    systems_query: Query<(Entity, &System)>,
    route_query: Query<(Entity, &Route)>,
    galaxy: Res<Galaxy>,
    grids: Query<&Grid>,
    color_by: Res<ColorBy>,
    filters: Res<Filters>,
    roundness: Res<Roundness>,
    materials: Res<SystemMaterials>,
    time: Res<Time<Real>>,
    mut mesh_assets: ResMut<Assets<Mesh>>,
    mut material_assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut plotted: MessageWriter<route::PlottedRoute>,
    mut tasks: ResMut<FetchTasks>,
    mut plot: ResMut<Plot>,
) {
    let Ok(grid) = grids.get(galaxy.0) else { return };

    // Every row that arrived this frame, and when the last of them was asked
    // for. Put together first and handed over once, for the reason given at
    // [`one_per_system`].
    //
    // One time for however many queries landed together, since what it is for
    // is the line the spawn is logged under. Nothing is stamped with it and
    // nothing measures how stale a row is by it, so the latest of them stands
    // for the batch rather than each row having to carry its own.
    let mut arrived: Vec<DbSystem> = Vec::new();
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
            if let FetchIndex::Route(start, end, range) = index {
                // A route is a line between systems, so one system is
                // no route. Coming back with nothing is how the
                // database says it could not get from one end to the
                // other in jumps that long, and nothing drawn is the
                // same nothing as a route still being worked out.
                //
                // Only ever an answer to a route still being waited on.
                // A name that resolved to nothing is already said, and
                // said more exactly than this could: the route was
                // fetched anyway, and it comes back empty for the same
                // reason, so without this the better answer is talked
                // over a moment after it arrives.
                if *plot == Plot::Working {
                    *plot = if new_systems.len() < 2 {
                        Plot::Failed(format!(
                            "No route from {start} to {end} at {range} Ly"
                        ))
                    } else {
                        Plot::Nothing
                    };
                }

                // Said rather than acted on. What a route does to the map is
                // `route::plotted`'s business; this is the one place its
                // systems are in hand, so it is the one place that can say
                // what they are.
                //
                // The line is drawn from the same value, so that what it
                // carries and what the row in the bar holds are one filter
                // and closing the row finds the line.
                if let Some(landed) = plotted_route(&new_systems, range) {
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

            // TODO: Pass FetchIndex along. I'd like to have index.marker() or
            // similar so I can mark entities with some info about where they
            // were fetched from.
            //
            arrived_at = arrived_at.max(*fetched_at);
            arrived.extend(new_systems);
        }
        retain
    });

    for (index, at) in answered {
        tasks.surveyed(index, at);
    }

    let arrived = one_per_system(arrived);
    if !arrived.is_empty() {
        spawn_systems(
            &arrived,
            &systems_query,
            &galaxy,
            grid,
            &color_by,
            &filters,
            &mut commands,
            &roundness,
            &materials,
            &time,
            &arrived_at,
        );
    }

    // TODO(#43): despawn stuff...
}

/// What a route that has landed amounts to, if it amounts to a route
///
/// Nothing where fewer than two systems came back, a line between one system
/// being no line, and nothing where none of them has a position on record and
/// there is nowhere to put it.
///
/// `range` comes off the key the route was fetched under, that being where
/// what the user asked for is still written down. The rows that came back say
/// which systems the ship passes through and nothing about how far it reaches.
fn plotted_route(systems: &[DbSystem], range: &str) -> Option<PlottedRoute> {
    let (first, last) = (systems.first()?, systems.last()?);
    if systems.len() < 2 {
        return None;
    }

    let places: Vec<_> = systems.iter().filter_map(system_to_vec).collect();
    let (middle, extent) = framing(&places)?;

    Some(PlottedRoute {
        label: format!("{} -> {}", first.name, last.name),
        // In the order they are travelled, which is the order the route came
        // back in and the order its panel lists.
        systems: systems.iter().map(|system| system.address).collect(),
        middle,
        extent,
        range: range.to_owned(),
    })
}

/// The rows that arrived, with each system named once
///
/// [`spawn_systems`] finds what is already on the map by asking the world, and
/// the world does not yet hold what a command spawned a moment ago: commands
/// wait for the next sync point. So two answers arriving in one frame about
/// one system would each find nothing there and each spawn it, leaving two
/// stars on top of each other for as long as the map holds them, which is for
/// good. The map fetches by region and by name at once, and a system searched
/// for and flown to is exactly the system the region around it is bringing in,
/// so the two answers are not rare.
///
/// Whichever answer came first is the one kept. Two of them in one frame are
/// one system as two queries a few milliseconds apart saw it, which is the
/// same system.
fn one_per_system(arrived: Vec<DbSystem>) -> Vec<DbSystem> {
    let mut named = HashSet::with_capacity(arrived.len());
    arrived.into_iter().filter(|system| named.insert(system.address)).collect()
}

/// Create or refresh the entities for each row fetched
///
/// A [`System`] carries the database row and the grid placement, is what the
/// rest of the map addresses, and is itself drawn as the [`Shell`] standing
/// for it. Labels hang off it alongside and are drawn far smaller, dividing
/// the shell's scale back out; see [`super::labels::face_camera`].
///
/// A row already on the map has its [`System`] replaced rather than being
/// respawned, which [`update`] then acts on.
///
/// The filters are asked here rather than left to [`filter::mark`], so that a
/// system arrives already marked and already drawn at the strength it should
/// be. A mark applied by a command lands at the next sync point, by which
/// time the star has been drawn once at full strength.
pub fn spawn_systems(
    db_systems: &[DbSystem],
    systems: &Query<(Entity, &System)>,
    galaxy: &Res<Galaxy>,
    grid: &Grid,
    color_by: &Res<ColorBy>,
    filters: &Res<Filters>,
    commands: &mut Commands,
    roundness: &Res<Roundness>,
    materials: &Res<SystemMaterials>,
    time: &Res<Time<Real>>,
    fetched_at: &Instant,
) {
    let mut existing_systems: HashMap<i64, Entity> = systems
        .iter()
        .map(|(entity, system)| (system.address, entity))
        .collect();

    for db_system in db_systems {
        let Ok(system) = System::try_from(db_system) else {
            debug!("skipping {}, no position on record", db_system.address);
            continue;
        };

        if let Some(enitity) = existing_systems.remove(&db_system.address) {
            debug!(
                "updating {} @ {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            commands.entity(enitity).insert(system);
        } else {
            debug!(
                "spawning {} {:?}",
                db_system.address,
                fetched_at.duration_since(time.startup())
            );

            // Asked here as well as in `filter::mark`, since a mark applied
            // by a command lands at the next sync point and the star would
            // be drawn once at full strength before it arrived.
            let excluded = !filters.admit(&system, Utc::now());
            let drawn = star(&system, color_by, roundness, materials, excluded);
            let mut spawned = commands.spawn((
                placement(&system, grid),
                system,
                // Fitted by `pointing::size_indicators` before the first
                // draw, and what the pointer is tested against.
                Indicator::default(),
                // A system does not block what lies behind it, so a name
                // drawn over one is reported as well and `pointing` can
                // weigh the two.
                Pickable { should_block_lower: false, is_hoverable: true },
                // The star is what is shown or hidden; the mesh and any
                // labels inherit that from it.
                Visibility::default(),
                // A star outside the galaxy's grid is not placed by it,
                // and would be drawn wherever its bare transform happened
                // to put it rather than where the cell says.
                ChildOf(galaxy.0),
            ));
            if excluded {
                spawned.insert(Filtered);
            }
            spawned.insert(drawn);
        }
    }
}

/// Carry a changed row onto where its star is drawn
///
/// A row fetched again is written over the one already there, and the
/// position it carries is free to differ from the one it replaces. What a
/// star is drawn *in* follows the row as well, and [`shells`] settles that.
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

/// Decide how each shell is drawn, and whether it is drawn at all
///
/// Three things decide it and one of them has to write the material, so all
/// three are settled here: the color scheme says which hue, the filters say
/// how faintly, and how near the camera has come says how much of the shell is
/// left at all.
///
/// A shell is a mark standing in for a system too small to see. It fades out
/// as the camera closes and is gone by the time what is inside the system is
/// drawn, which is what the camera came for. Past there the shell is a lit
/// sphere around the camera, drawn from the galaxy's scale where a float has
/// nothing left over at the size it is; and for a system that is one star and
/// nothing else it sits all but exactly on that star's surface.
///
/// A shell on its way out points at a handle painted for that step of the fade,
/// and every other shell at the shared handle for its hue. So a fade repaints
/// nothing, and any number of shells may be fading at once.
///
/// Decided afresh each frame and written only where it differs, as
/// [`super::labels::tint_marked_names`] is. A mark is applied by a command and
/// so lands a frame after the filter that asked for it, which leaves nothing
/// that both runs after the mark and can still see what changed.
pub(super) fn shells(
    color_by: Res<ColorBy>,
    dim: Res<DimTo>,
    materials: Res<SystemMaterials>,
    mut shells: Query<
        (
            &System,
            &Strength,
            Has<Filtered>,
            &mut MeshMaterial3d<StandardMaterial>,
            &ViewVisibility,
        ),
        With<Shell>,
    >,
) {
    for (system, mark, filtered, mut material, visible) in &mut shells {
        // Off the frame a shell is not drawn, so which handle it points at is
        // not settled. It is settled again the frame it comes back on.
        if !visible.get() {
            continue;
        }
        // How much of the mark is left. At nothing the fading material is
        // fully transparent, which is what takes a shell off screen now that
        // it shares an entity with its system: hiding the entity would take
        // the system's bodies and labels with it. Only the held system ever
        // fades, so only ever one shell is the transparent kind.
        let standing = mark.0;
        let hue = hue(system, &color_by);
        let wanted = if standing < 1. {
            // Dimmed by the filters first and by the fade after, so a shell
            // the filters were drawing faintly does not come back to full
            // strength on its way out.
            let strength = if filtered { dim.0 } else { 1. };
            materials.going(hue, strength * standing)
        } else {
            materials.get(hue, filtered)
        };
        if material.0 != *wanted {
            material.0 = wanted.clone();
        }
    }
}

/// Repaint the dimmed colors when the slider moves
///
/// The handles stay as they are, so nothing has to be told which material it
/// is pointing at. Recoloring a shared asset repaints everything drawn in
/// it, which here is every star the filters exclude, and is the point.
fn redim(
    dim: Res<DimTo>,
    materials: Res<SystemMaterials>,
    mut assets: ResMut<Assets<StandardMaterial>>,
) {
    if !dim.is_changed() {
        return;
    }

    for (handle, hue) in materials.dim.iter().zip(Hue::ALL) {
        if let Some(mut material) = assets.get_mut(handle) {
            *material = star_material(hue.color(), dim.0, AlphaMode::Opaque);
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

/// The shell a system is drawn as
///
/// Inserted onto the [`System`] entity itself rather than hung off it, so the
/// system is drawn as its own shell. [`super::scale`] writes a size onto that
/// entity each frame; the size is an exaggeration far larger than a metre, and
/// the labels alongside divide it back out (see [`super::labels::face_camera`]).
///
/// Nothing aims at it. What answers the pointer is the system itself, over
/// the mark [`super::pointing::Indicator`] holds, so a system is as easy to
/// hit as the ring says it is however small the shell is drawn.
fn star(
    system: &System,
    color_by: &Res<ColorBy>,
    roundness: &Res<Roundness>,
    materials: &Res<SystemMaterials>,
    dimmed: bool,
) -> impl Bundle {
    (
        Shell,
        // Fitted by `super::scale` before the first draw, as the size is.
        Mesh3d(roundness.coarsest()),
        MeshMaterial3d(materials.get(hue(system, color_by), dimmed).clone()),
        NotShadowCaster,
        // Drawn on its own layer by a camera without bloom, so a wide field of
        // shells is opaque and the nearest covers the rest while the bodies
        // keep the glow. See [`crate::camera::SHELLS_LAYER`].
        RenderLayers::layer(crate::camera::SHELLS_LAYER),
    )
}

/// Which color a star is drawn in
fn hue(system: &System, color_by: &Res<ColorBy>) -> Hue {
    match color_by.deref() {
        ColorBy::Allegiance => allegiance_hue(system),
        ColorBy::Government => government_hue(system),
        ColorBy::Security => security_hue(system),
    }
}

fn init_materials(
    mut assets: ResMut<Assets<StandardMaterial>>,
    dim: Res<DimTo>,
    mut commands: Commands,
) {
    let mut set = |strength: f32| {
        Hue::ALL
            .into_iter()
            .map(|hue| {
                assets.add(star_material(
                    hue.color(),
                    strength,
                    AlphaMode::Opaque,
                ))
            })
            .collect()
    };
    let bright = set(1.);
    let dim = set(dim.0);
    let mut fading = Vec::with_capacity(Hue::ALL.len() * (FADE_STEPS + 1));
    for hue in Hue::ALL {
        for step in 0..=FADE_STEPS {
            let strength = step as f32 / FADE_STEPS as f32;
            fading.push(assets.add(star_material(
                hue.color(),
                strength,
                AlphaMode::Blend,
            )));
        }
    }

    commands.insert_resource(SystemMaterials { bright, dim, fading });
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

/// A system the database has no coordinates for
///
/// The map is a map. A system it cannot place is not something it can draw.
pub struct Unplaceable;

impl TryFrom<&DbSystem> for System {
    type Error = Unplaceable;

    fn try_from(system: &DbSystem) -> Result<System, Unplaceable> {
        let position = system.position.ok_or(Unplaceable)?;
        let pos = [position.x, position.y, position.z];

        Ok(System {
            address: system.address,
            position: pos,
            name: system.name.clone(),
            population: system.population,
            allegiance: system.allegiance,
            government: system.government,
            security: system.security,
            economies: system.economies,
            factions: system.factions.clone(),
            body_count: system.body_count,
            non_body_count: system.non_body_count,
            reach: system.reach,
            updated_at: system.updated_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A row for the system at `address`, with nothing else on record
    fn row(address: i64) -> DbSystem {
        DbSystem {
            address,
            name: format!("System {address}"),
            position: None,
            population: 0,
            security: None,
            government: None,
            allegiance: None,
            economies: None,
            factions: Vec::new(),
            body_count: None,
            non_body_count: None,
            reach: None,
            updated_at: chrono::DateTime::UNIX_EPOCH,
            updated_by: String::new(),
        }
    }

    /// Which systems a set of rows is about, in order
    fn about(rows: &[DbSystem]) -> Vec<i64> {
        rows.iter().map(|system| system.address).collect()
    }

    /// Two answers about one system leave one row for it
    ///
    /// Which is what keeps two stars from being spawned on top of each other.
    /// The map cannot see what it spawned a moment ago, so it has to be told
    /// about a system once.
    #[test]
    fn a_system_answered_for_twice_is_named_once() {
        let arrived = vec![row(1), row(2), row(1)];

        assert_eq!(about(&one_per_system(arrived)), vec![1, 2]);
    }

    /// The answer that came first is the one kept
    #[test]
    fn the_first_answer_about_a_system_is_the_one_kept() {
        let mut second = row(1);
        second.name = "Renamed".to_owned();
        let arrived = vec![row(1), second];

        let kept = one_per_system(arrived);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].name, "System 1");
    }

    /// Rows about different systems are all kept, in the order they arrived
    #[test]
    fn every_system_answered_for_once_is_kept() {
        let arrived = vec![row(3), row(1), row(2)];

        assert_eq!(about(&one_per_system(arrived)), vec![3, 1, 2]);
    }

    /// Nothing arriving is nothing to spawn
    #[test]
    fn nothing_arriving_names_nothing() {
        assert!(one_per_system(Vec::new()).is_empty());
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

    /// How many shells were repainted
    #[derive(Resource, Default)]
    struct Repaints(usize);

    fn count_repaints(
        mut repaints: ResMut<Repaints>,
        shells: Query<
            (),
            (Changed<MeshMaterial3d<StandardMaterial>>, With<Shell>),
        >,
    ) {
        repaints.0 += shells.iter().count();
    }

    /// A world holding the colors and whatever shells are painted out of them
    ///
    /// Nothing keeps the marks up in it, so every shell stands whole and takes
    /// the shared handle for its hue rather than a step of the fade.
    fn painted() -> App {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Assets<StandardMaterial>>();
        app.init_resource::<DimTo>();
        app.init_resource::<Repaints>();
        app.insert_resource(ColorBy::Allegiance);
        app.add_systems(Startup, init_materials);
        app.add_systems(Update, (shells, count_repaints).chain());
        app
    }

    /// A system with a shell standing around it
    fn shelled(app: &mut App) -> Entity {
        app.world_mut()
            .spawn((
                crate::systems::tests::system(1),
                Shell,
                Strength::default(),
                MeshMaterial3d::<StandardMaterial>::default(),
                ViewVisibility::VISIBLE,
            ))
            .id()
    }

    /// How many shells have been repainted so far
    fn repaints(app: &App) -> usize {
        app.world().resource::<Repaints>().0
    }

    /// A frame that changes nothing leaves a shell's paint alone
    ///
    /// What the color is decided afresh each frame and written only where it
    /// differs is for. Every shell in the sky is asked this every frame, and
    /// the answer nearly always matches the handle it already points at.
    #[test]
    fn a_resting_frame_leaves_a_shell_painted_as_it_was() {
        let mut app = painted();
        shelled(&mut app);

        // The shell arriving is a change of its own, and the frame after it is
        // the first that could be said to be resting.
        app.update();
        app.update();
        let settled = repaints(&app);

        app.update();
        assert_eq!(
            repaints(&app),
            settled,
            "repainted a shell that had not moved"
        );
    }

    /// And one whose system falls out of the filters is painted again
    ///
    /// Which is what the answer is worked out for. A guard that held through a
    /// filter would leave the whole sky at full strength.
    #[test]
    fn a_filtered_shell_is_painted_again() {
        let mut app = painted();
        let system = shelled(&mut app);

        app.update();
        app.update();
        let settled = repaints(&app);

        app.world_mut().entity_mut(system).insert(Filtered);
        app.update();

        assert!(
            repaints(&app) > settled,
            "left a dimmed shell at full strength"
        );
    }
}
