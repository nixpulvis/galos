//! Drawing what is inside a system
//!
//! A body is drawn at the size it is, in the metres the map is laid out in,
//! placed where its orbit puts it. Nothing here is exaggerated — the shell
//! standing for the system is what carries the exaggeration, and it is drawn
//! elsewhere.
//!
//! # Light
//!
//! Real, and physical, which is what re-basing the map to metres bought. A
//! star is given a [`PointLight`] of however many lumens its size and
//! temperature come to, so a planet at an Earth's distance from a Sun sees an
//! Earth's daylight and the far side of it is dark. In light years none of
//! that could be said: bevy's lighting is in lumens and lux, and a unit that
//! large sends every one of them out of range.
//!
//! Bodies keep a little emission of their own regardless, so that a system
//! whose star is not on record is dim rather than invisible.

use super::{Contents, orbit::Orbits};
use crate::camera::OrbitCamera;
use crate::schedule::MapSet;
use crate::space;
use crate::systems::System;
use crate::systems::pointing::Indicator;
use crate::systems::route::LineStrip;
use bevy::ecs::system::SystemParam;
use bevy::light::NotShadowCaster;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;

pub fn plugin(app: &mut App) {
    app.init_resource::<Drawn>();
    app.init_resource::<Apparent>();
    app.add_systems(Startup, (init_meshes, init_materials));
    // After the rows have been taken in, so that a system's contents can be
    // drawn on the frame they land rather than the one after.
    app.add_systems(
        Update,
        draw.in_set(MapSet::Populate).after(super::fetch::collect),
    );
}

/// How large a system has to look before what is in it is drawn, in radians
///
/// About three degrees, by which point the sphere standing for the system is a
/// good part of the view and the things inside it are worth the entities.
///
/// An angle rather than a distance, so that a system reaching light hours and
/// one reaching light seconds are both drawn when they are equally worth
/// looking at. The rows arrive far earlier — that is [`super::fetch`]'s
/// business — so nothing waits on the database at this range.
const WORTH_DRAWING: f32 = 0.05;

/// And how small before it is taken away again
///
/// Lower than it took to draw, so a camera sitting on the line does not spawn
/// and despawn a system's insides every frame.
const WORTH_KEEPING: f32 = 0.02;

/// How large a system has to look before the mark standing for it starts to go
///
/// A quarter of [`WORTH_DRAWING`], which is four times the distance. The mark
/// is what says a system is there while it is too small to see, and by the
/// time it is drawn it is standing over the thing it stood in for. Four times
/// the distance is long enough that the exchange reads as one thing becoming
/// another rather than as one being swapped for the other.
const WORTH_MARKING: f32 = 0.0125;

/// How large the system the map is holding looks, in radians
///
/// Its own reach over how far off it is, which is the one question deciding
/// whether what is inside it is drawn. Published because the mark standing for
/// that system answers the same question from the other side: a shell has to
/// be gone by the time the system itself is drawn, and a ring around a system
/// the camera is standing inside is a ring around the view.
///
/// One system at a time, as the drawing is, and nothing at all until there is
/// one in hand whose reach the map can say.
#[derive(Resource, Default)]
pub struct Apparent(Option<(Entity, f32)>);

impl Apparent {
    /// How much of the mark standing for `system` is left, from one to nothing
    ///
    /// The whole of it for every system but the one being held, none of it
    /// once that one is drawn, and the way between over [`WORTH_MARKING`] to
    /// [`WORTH_DRAWING`].
    pub fn standing(&self, system: Entity) -> f32 {
        let Some((held, seen)) = self.0 else { return 1. };
        if held != system {
            return 1.;
        }

        let through = (seen - WORTH_MARKING) / (WORTH_DRAWING - WORTH_MARKING);
        1. - through.clamp(0., 1.)
    }
}

/// How far past the system a star's light is allowed to reach
///
/// As a multiple of how far the system reaches. Bevy eases the last of a
/// light away as its range is approached, so a range set at the outermost
/// orbit would draw the body standing there darker than it is; four times over
/// leaves that worth well under a percent anywhere anything stands.
///
/// A range at all, rather than the `f32::MAX` this was, because the range is
/// also the bounding sphere the renderer sorts lights into clusters by. That
/// sphere is projected to find which clusters it covers, and a radius of
/// `f32::MAX` puts infinities through the projection: which clusters the light
/// lands in then depends on where the camera happens to be pointing, and the
/// light comes and goes as it moves.
const LIGHT_REACH: f32 = 4.;

/// How many points an orbit is drawn with
///
/// Enough that the roundest orbit does not read as a polygon at the size an
/// orbit is ever looked at, and few enough that a system of a hundred bodies
/// is a few thousand vertices rather than a mesh worth thinking about.
const ORBIT_POINTS: usize = 128;

/// Which system's insides are drawn, if any
#[derive(Resource, Default)]
struct Drawn(Option<i64>);

/// Anything drawn because it is inside a system
///
/// One marker over stars, bodies and orbit lines alike, since what they have
/// in common is when they go away: all of them at once, when the camera
/// leaves.
#[derive(Component)]
pub struct Inside;

/// A star, a planet or a moon, drawn where its orbit puts it
///
/// Carries the row it was drawn from, so that whatever the pointer lands on
/// can say what it is without going back to the database.
#[derive(Component)]
pub struct Body {
    /// Which system it is in
    ///
    /// A body is a child of its system on the map, so this says nothing the
    /// tree does not. It is here because what picks a body out holds it as a
    /// value and has to find its way back: an id alone names a different body
    /// in every system, and one system's contents are drawn at a time.
    pub address: i64,
    /// What it is called
    pub name: String,
    /// Which of the system's numbering it is
    pub id: i16,
    /// What kind of thing it is, as the journal spells it
    pub class: String,
    /// How far across it is, in metres
    pub radius: f32,
}

/// A [`Body`] that is a star rather than something going round one
///
/// The two are drawn from different tables and lit from opposite ends, and
/// they are the same thing to everything that aims at one, so what tells them
/// apart is a mark rather than two components.
#[derive(Component)]
pub struct Star;

/// The sphere a body is drawn with
///
/// Its own rather than [`crate::systems::spawn::SystemMesh`], which is an
/// icosahedron barely smoothed. That is all a mark a few pixels across ever
/// needed; a planet filling the view wants to be round.
#[derive(Resource)]
struct BodyMesh(Handle<Mesh>);

/// What a star is drawn in, by the colour its class comes to
#[derive(Resource)]
struct StarMaterials(Vec<Handle<StandardMaterial>>);

/// What a body is drawn in, by the surface its class comes to
#[derive(Resource)]
struct BodyMaterials(Vec<Handle<StandardMaterial>>);

/// What an orbit's line is drawn in
#[derive(Resource)]
struct OrbitMaterial(Handle<StandardMaterial>);

/// The colours a star is drawn in
///
/// By temperature, which is what a star's class is a shorthand for. The
/// remnants and the oddities share one colour rather than being guessed at.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Glow {
    Blue,
    White,
    Yellow,
    Orange,
    Red,
    Odd,
}

impl Glow {
    const ALL: [Glow; 6] = [
        Glow::Blue,
        Glow::White,
        Glow::Yellow,
        Glow::Orange,
        Glow::Red,
        Glow::Odd,
    ];

    /// What the glow is painted in
    const fn color(self) -> Color {
        match self {
            Glow::Blue => Color::srgb(0.7, 0.8, 1.),
            Glow::White => Color::srgb(0.95, 0.95, 1.),
            Glow::Yellow => Color::srgb(1., 0.95, 0.75),
            Glow::Orange => Color::srgb(1., 0.75, 0.45),
            Glow::Red => Color::srgb(1., 0.5, 0.35),
            Glow::Odd => Color::srgb(0.6, 0.55, 0.7),
        }
    }

    /// Which colour a class of star comes to
    ///
    /// The first letter carries it for the ordinary sequence, hottest to
    /// coolest. What is left — dwarfs, neutron stars, holes, the carbon and
    /// Wolf-Rayet families — is not on that sequence at all, and is drawn as
    /// itself rather than as whatever letter it happens to start with.
    fn of(class: &str) -> Glow {
        let class = class.trim().to_uppercase();
        match class.as_str() {
            _ if class.starts_with("D") => Glow::White,
            _ if class.starts_with("N") || class.starts_with("H") => Glow::Odd,
            _ if class.starts_with("W") || class.starts_with("C") => Glow::Odd,
            _ if class.starts_with("SUPERMASSIVE") => Glow::Odd,
            _ if class.starts_with("O") || class.starts_with("B") => Glow::Blue,
            _ if class.starts_with("A") || class.starts_with("F") => {
                Glow::White
            }
            _ if class.starts_with("G") => Glow::Yellow,
            _ if class.starts_with("K") => Glow::Orange,
            _ if class.starts_with("M")
                || class.starts_with("L")
                || class.starts_with("T")
                || class.starts_with("Y") =>
            {
                Glow::Red
            }
            _ => Glow::Odd,
        }
    }
}

/// The colours a body is drawn in
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Surface {
    Earthlike,
    Water,
    Ammonia,
    Gas,
    Icy,
    Rocky,
    Metal,
    Unknown,
}

impl Surface {
    const ALL: [Surface; 8] = [
        Surface::Earthlike,
        Surface::Water,
        Surface::Ammonia,
        Surface::Gas,
        Surface::Icy,
        Surface::Rocky,
        Surface::Metal,
        Surface::Unknown,
    ];

    /// What the surface is painted in
    const fn color(self) -> Color {
        match self {
            Surface::Earthlike => Color::srgb(0.25, 0.5, 0.3),
            Surface::Water => Color::srgb(0.2, 0.4, 0.7),
            Surface::Ammonia => Color::srgb(0.7, 0.6, 0.35),
            Surface::Gas => Color::srgb(0.75, 0.65, 0.5),
            Surface::Icy => Color::srgb(0.8, 0.85, 0.9),
            Surface::Rocky => Color::srgb(0.45, 0.4, 0.35),
            Surface::Metal => Color::srgb(0.5, 0.45, 0.4),
            Surface::Unknown => Color::srgb(0.35, 0.35, 0.35),
        }
    }

    /// Which surface a class of body comes to
    ///
    /// Matched on what the phrase contains rather than on the whole of it. The
    /// journal spells a class out in words — "Sudarsky class III gas giant",
    /// "High metal content body" — and there are more of them than are worth
    /// listing, most differing in ways nothing here draws.
    ///
    /// Order matters, and in two places. A gas giant is named for what lives
    /// in it — "Gas giant with ammonia based life" — so it has to be caught
    /// before ammonia is, or every one of them comes out an ammonia world. And
    /// a "rocky ice body" contains both its materials, of which the ice is the
    /// one that shows.
    fn of(class: &str) -> Surface {
        let class = class.to_lowercase();
        if class.contains("earthlike") {
            Surface::Earthlike
        } else if class.contains("gas giant") {
            Surface::Gas
        } else if class.contains("water world") || class.contains("water giant")
        {
            Surface::Water
        } else if class.contains("ammonia") {
            Surface::Ammonia
        } else if class.contains("rocky ice") {
            Surface::Icy
        } else if class.contains("icy") || class.contains("ice") {
            Surface::Icy
        } else if class.contains("metal") {
            Surface::Metal
        } else if class.contains("rocky") {
            Surface::Rocky
        } else {
            Surface::Unknown
        }
    }
}

/// The Stefan-Boltzmann constant, in watts per square metre per kelvin to the
/// fourth
const STEFAN_BOLTZMANN: f64 = 5.670374419e-8;

/// How many lumens a watt of starlight is worth
///
/// Sunlight comes to about this. A hotter star puts more of itself where the
/// eye cannot see and a cooler one likewise, so taking one figure for all of
/// them overstates the ends of the range — by enough to matter to a
/// photometrist and not to a map.
const EFFICACY: f64 = 93.;

/// How much light a star of this size and heat gives off, in lumens
///
/// Stefan-Boltzmann: the power leaving a sphere goes as its area and as the
/// fourth power of its temperature. Every term is on record — `radius` in
/// metres and `temperature` in kelvin — so this is the star's real
/// output rather than anything chosen to look right.
fn lumens(radius: f32, temperature: f32) -> f32 {
    let (r, t) = (radius.max(0.) as f64, temperature.max(0.) as f64);
    let watts =
        4. * std::f64::consts::PI * r * r * STEFAN_BOLTZMANN * t.powi(4);
    (watts * EFFICACY) as f32
}

fn init_meshes(mut assets: ResMut<Assets<Mesh>>, mut commands: Commands) {
    // Five subdivisions is about five thousand faces, which is nothing to
    // draw a handful of and is what a body filling the screen wants: the
    // silhouette holds, and so does the terminator, which crosses the whole
    // face of a body and takes its shape from where the vertices fall.
    let handle = assets.add(Sphere::new(1.).mesh().ico(5).unwrap());
    commands.insert_resource(BodyMesh(handle));
}

fn init_materials(
    mut assets: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    commands.insert_resource(StarMaterials(
        Glow::ALL
            .into_iter()
            .map(|glow| {
                assets.add(StandardMaterial {
                    base_color: glow.color(),
                    // A star is the light rather than a thing lit by it.
                    emissive: LinearRgba::from(glow.color()) * 4000.,
                    ..default()
                })
            })
            .collect(),
    ));

    commands.insert_resource(BodyMaterials(
        Surface::ALL
            .into_iter()
            .map(|surface| {
                assets.add(StandardMaterial {
                    base_color: surface.color(),
                    perceptual_roughness: 0.9,
                    // A trace of its own, so that a body whose star is not on
                    // record is dim rather than invisible.
                    emissive: LinearRgba::from(surface.color()) * 0.02,
                    ..default()
                })
            })
            .collect(),
    ));

    commands.insert_resource(OrbitMaterial(assets.add(StandardMaterial {
        base_color: Color::srgba(0.5, 0.6, 0.75, 0.25),
        alpha_mode: AlphaMode::Blend,
        unlit: true,
        ..default()
    })));
}

/// Put a system's insides on the map, and take them off again
///
/// One system at a time, which is however many [`Contents`] holds. The `Grid`
/// a system wears goes on with its contents and comes off with them: every
/// grid in the world is walked every frame, so one that holds nothing is a
/// cost with nothing to show for it.
#[allow(clippy::too_many_arguments)]
fn draw(
    camera: Query<(Entity, &OrbitCamera)>,
    galaxy: Res<crate::space::Galaxy>,
    systems: Query<(Entity, &System)>,
    inside: Query<Entity, With<Inside>>,
    contents: Res<Contents>,
    mesh: Res<BodyMesh>,
    stars: Res<StarMaterials>,
    bodies: Res<BodyMaterials>,
    orbit_material: Res<OrbitMaterial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drawn: ResMut<Drawn>,
    mut seen_as: ResMut<Apparent>,
    mut commands: Commands,
) {
    let Ok((eye_entity, eye)) = camera.single().map(|(e, c)| (e, c.eye)) else {
        seen_as.0 = None;
        return;
    };

    // How large the system being held looks from here, which is the one
    // question deciding whether any of this is worth drawing.
    let apparent =
        contents.of().zip(contents.extent()).and_then(|(address, extent)| {
            let system = systems.iter().find(|(_, s)| s.address == address)?;
            let away = space::metres(eye - DVec3::from(system.1.position))
                .length() as f32;
            Some((address, system.0, extent / away.max(1.)))
        });

    // Said before anything is decided from it, since the marks standing for
    // this system answer it as well and one of the ways below is to leave
    // everything as it is.
    seen_as.0 = apparent.map(|(_, entity, seen)| (entity, seen));

    let wanted = match (drawn.0, apparent) {
        // Nothing held, or too small to bother with.
        (_, None) => None,
        // Already drawn, and still worth keeping.
        (Some(shown), Some((address, _, seen)))
            if shown == address && seen >= WORTH_KEEPING =>
        {
            return;
        }
        (_, Some((address, entity, seen))) if seen >= WORTH_DRAWING => {
            Some((address, entity))
        }
        (Some(_), _) => None,
        _ => return,
    };

    // The camera first, then the contents, then the grid they were all
    // placed in: a `CellCoord` under an entity that is no longer a grid has
    // nothing to be measured against.
    if let Some(shown) = drawn.0.take() {
        commands.entity(eye_entity).insert(ChildOf(galaxy.0));
        for entity in &inside {
            commands.entity(entity).despawn();
        }
        if let Some((entity, _)) =
            systems.iter().find(|(_, s)| s.address == shown)
        {
            commands.entity(entity).remove::<Grid>();
        }
    }

    let Some((address, entity)) = wanted else { return };

    let grid = space::system_grid();
    let orbits = contents.orbits();
    let mut commands = commands.entity(entity);
    commands.insert(grid.clone());
    // Down into the system with them.
    //
    // Everything is drawn relative to the cell the floating origin stands in
    // rather than to the origin itself, so what a float has left over inside
    // that cell is the precision everything near the camera is drawn with. A
    // galaxy cell is `2^53` metres and the camera can stand anywhere in one,
    // which leaves about five hundred thousand kilometres: enough to shred an
    // orbit into a polygon and a name into scribble. A system's cells are a
    // metre, so descending leaves nothing over and everything inside is drawn
    // as exactly as a float can hold it.
    commands.add_child(eye_entity);

    // Everything is placed short of the arrival star, so the star lands at the
    // middle of the grid and the system's own position is where its star is.
    // The camera is sent to that position and zooms towards it, and in a
    // system whose stars go round a point between them it was arriving at the
    // point: empty sky with the star it came for ten billion kilometres off to
    // one side.
    let middle = contents.middle(&orbits, 0.);

    // How far a star has to light, which is out to the far side of the
    // outermost thing going round it.
    let reach = contents.extent().unwrap_or_default();
    for star in contents.stars() {
        let place = orbits.place(star.id, 0.) - middle;
        commands
            .with_child(drawn_star(star, place, reach, &grid, &mesh, &stars));
    }
    for body in contents.bodies() {
        let place = orbits.place(body.id, 0.) - middle;
        commands.with_child(drawn_body(body, place, &grid, &mesh, &bodies));
    }

    // One line per thing that goes round something, hung off whatever it goes
    // round so its own vertices carry only the size of the orbit.
    let paths = contents
        .stars()
        .iter()
        .map(|s| (s.id, s.parent_id()))
        .chain(contents.bodies().iter().map(|b| (b.id, b.parent_id())));
    for (id, parent) in paths {
        if let Some(line) = drawn_orbit(
            id,
            parent,
            middle,
            &orbits,
            &grid,
            &mut meshes,
            &orbit_material,
        ) {
            commands.with_child(line);
        }
    }

    debug!("drew what is inside {address}");
    drawn.0 = Some(address);
}

/// Where something inside a system sits, as that system's grid wants it
fn placed(place: DVec3, grid: &Grid) -> (CellCoord, Vec3) {
    grid.translation_to_grid(place)
}

/// Where the things inside a system stand, out in the galaxy
///
/// A body is placed in its system's own grid, in metres from that system's
/// centre, so saying where one is in the galaxy means going up to the system
/// holding it. Two places ask it, the row the bar draws for a body picked out
/// and the double click that flies to one, so it is answered in one.
///
/// Read from the grid rather than from a body's [`GlobalTransform`], which is
/// measured from the camera in a float and has tens of kilometres of slack out
/// at the edge of a wide system.
#[derive(SystemParam)]
pub struct Placed<'w, 's> {
    inside: Query<
        'w,
        's,
        (&'static ChildOf, &'static CellCoord, &'static Transform),
        With<Body>,
    >,
    systems: Query<'w, 's, (&'static System, &'static Grid)>,
}

impl Placed<'_, '_> {
    /// Where `body` stands, in light years
    ///
    /// Nothing for anything that is not a body drawn inside a system on the
    /// map, which is the only thing this can answer about.
    pub fn of(&self, body: Entity) -> Option<DVec3> {
        let (child_of, cell, at) = self.inside.get(body).ok()?;
        let (system, grid) = self.systems.get(child_of.parent()).ok()?;
        let metres = cell.as_dvec3(grid) + at.translation.as_dvec3();

        Some(system.position() + space::light_years(metres))
    }
}

/// A star, drawn at its own size and lighting what is around it
fn drawn_star(
    star: &DbStar,
    place: DVec3,
    reach: f32,
    grid: &Grid,
    mesh: &BodyMesh,
    materials: &StarMaterials,
) -> impl Bundle {
    let (cell, offset) = placed(place, grid);
    (
        Body {
            address: star.system_address,
            name: star.name.clone(),
            id: star.id,
            class: star.star_class.clone(),
            radius: star.radius,
        },
        Star,
        Inside,
        // Fitted by `pointing::size_bodies` before the first draw.
        Indicator::default(),
        cell,
        Transform::from_translation(offset)
            .with_scale(Vec3::splat(star.radius.max(1.))),
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(
            materials.0[Glow::of(&star.star_class) as usize].clone(),
        ),
        NotShadowCaster,
        // A body does not block what lies behind it, so everything under the
        // pointer is reported and `pointing` weighs them by which is nearer.
        Pickable { should_block_lower: false, is_hoverable: true },
        // Placed on the star rather than beside it, and given the star's real
        // output. Shadows are off: a shadow map spanning a system would be
        // all of one texel, and a star lights from inside its own mesh.
        children![(
            PointLight {
                intensity: lumens(star.radius, star.temperature),
                // Out past everything in the system. Never shorter than the
                // star itself, so a system nobody has recorded anything else
                // about still has a light with a size to it.
                range: reach.max(star.radius) * LIGHT_REACH,
                shadow_maps_enabled: false,
                ..default()
            },
            // Cancels the scale the mesh is drawn at, so the light sits at a
            // point rather than being stretched with the sphere.
            Transform::default(),
        )],
    )
}

/// A planet or a moon, drawn at its own size
fn drawn_body(
    body: &DbBody,
    place: DVec3,
    grid: &Grid,
    mesh: &BodyMesh,
    materials: &BodyMaterials,
) -> impl Bundle {
    let (cell, offset) = placed(place, grid);
    (
        Body {
            address: body.system_address,
            name: body.name.clone(),
            id: body.id,
            class: body.planet_class.clone(),
            radius: body.radius,
        },
        Inside,
        // Fitted by `pointing::size_bodies` before the first draw.
        Indicator::default(),
        cell,
        Transform::from_translation(offset)
            .with_scale(Vec3::splat(body.radius.max(1.))),
        Mesh3d(mesh.0.clone()),
        MeshMaterial3d(
            materials.0[Surface::of(&body.planet_class) as usize].clone(),
        ),
        // As a star: what lies behind is reported too, and settled by depth.
        Pickable { should_block_lower: false, is_hoverable: true },
    )
}

/// The line one thing traces about whatever it goes round
///
/// Hung off the parent's own place, so its vertices hold the size of the orbit
/// rather than the distance out to it — the same reason a route's line is hung
/// off its midpoint.
///
/// Nothing for something that does not go round anything, which is what a
/// primary star comes back as.
#[allow(clippy::too_many_arguments)]
fn drawn_orbit(
    id: i16,
    parent: Option<i16>,
    middle: DVec3,
    orbits: &Orbits,
    grid: &Grid,
    meshes: &mut Assets<Mesh>,
    material: &OrbitMaterial,
) -> Option<impl Bundle> {
    let path = orbits.path(id, ORBIT_POINTS)?;
    let about = parent.map_or(DVec3::ZERO, |parent| orbits.place(parent, 0.));
    let (cell, offset) = placed(about - middle, grid);

    let points = path.into_iter().map(|p| p.as_vec3()).collect();
    Some((
        Inside,
        cell,
        Transform::from_translation(offset),
        Mesh3d(meshes.add(LineStrip { points })),
        MeshMaterial3d(material.0.clone()),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the one body in `app` says it stands
    #[derive(Resource, Default)]
    struct Stood(Option<DVec3>);

    fn ask(
        placed: Placed,
        bodies: Query<Entity, With<Body>>,
        mut stood: ResMut<Stood>,
    ) {
        stood.0 = bodies.single().ok().and_then(|body| placed.of(body));
    }

    /// Where a body `out` metres from the middle of a system `away` light
    /// years off is found to stand
    ///
    /// Placed by the same call that places one on the map, so this measures
    /// the round trip rather than a rearrangement of the same arithmetic.
    fn stands(away: f64, out: DVec3) -> DVec3 {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins);
        app.init_resource::<Stood>();
        app.add_systems(Update, ask);

        let grid = space::system_grid();
        let (cell, offset) = placed(out, &grid);
        let system = app
            .world_mut()
            .spawn((crate::systems::tests::at(1, away), grid))
            .id();
        app.world_mut().spawn((
            Body {
                address: 1,
                name: String::new(),
                id: 1,
                class: String::new(),
                radius: 1e6,
            },
            cell,
            Transform::from_translation(offset),
            ChildOf(system),
        ));

        app.update();
        app.world().resource::<Stood>().0.expect("the body stands somewhere")
    }

    /// One of the systems on the map
    fn on_the_map(which: u32) -> Entity {
        Entity::from_raw_u32(which).expect("a system")
    }

    /// How much of the mark for the held system is left, at `seen` radians
    fn standing(seen: f32) -> f32 {
        Apparent(Some((on_the_map(1), seen))).standing(on_the_map(1))
    }

    /// A mark is gone by the time what it stands for is drawn
    ///
    /// The exchange this is for. A shell drawn over the bodies it stood in
    /// for is a lit sphere around the camera, and one that vanished before
    /// they arrived would leave a gap with nothing in it.
    #[test]
    fn a_mark_is_gone_once_the_system_is_drawn() {
        assert_eq!(standing(WORTH_DRAWING), 0.);
        assert_eq!(standing(WORTH_DRAWING * 2.), 0.);
    }

    /// And whole until well before then
    #[test]
    fn a_mark_stands_whole_until_the_camera_is_close() {
        assert_eq!(standing(WORTH_MARKING), 1.);
        assert_eq!(standing(WORTH_MARKING / 2.), 1.);
        assert_eq!(standing(0.), 1.);
    }

    /// Going out the whole way in between, without stepping
    #[test]
    fn a_mark_goes_out_over_the_way_between() {
        let mut before = 1.;
        for step in 1..=100 {
            let seen = WORTH_MARKING
                + (WORTH_DRAWING - WORTH_MARKING) * step as f32 / 100.;
            let left = standing(seen);

            assert!(left < before, "{seen} left {left}, against {before}");
            assert!((0. ..1.).contains(&left), "{seen} left {left}");
            before = left;
        }
    }

    /// Every other system's mark stands whole
    ///
    /// Only one system is ever being closed on, and the rest of the sky is
    /// not fading out because the camera is flying into one of them.
    #[test]
    fn a_mark_for_some_other_system_stands_whole() {
        let held = Apparent(Some((on_the_map(1), WORTH_DRAWING)));

        assert_eq!(held.standing(on_the_map(2)), 1.);
        assert_eq!(Apparent::default().standing(on_the_map(1)), 1.);
    }

    /// A body is found where the system holding it put it
    ///
    /// What the double click that flies to one asks, and what the bar's row
    /// for one asks. A body carries no galactic position of its own: it is
    /// placed in metres from the middle of its system, and this is that
    /// spoken back into the light years everything outside a system talks in.
    #[test]
    fn a_body_stands_where_its_system_put_it() {
        // A light second out, at a system ten light years off.
        let second = 2.99792458e8;
        let at = stands(10., DVec3::new(second, 0., 0.));

        let expected = DVec3::new(10. + second / space::LIGHT_YEAR, 0., 0.);
        assert!(
            at.distance(expected) * space::LIGHT_YEAR < 1.,
            "the body stood at {at}, not {expected}"
        );
    }

    /// And out at the rim, where a light second is nothing beside the distance
    ///
    /// The whole reason a body is measured from its system rather than from
    /// the galactic centre. Forty thousand light years leaves a double eleven
    /// digits to its right, and a light second asks for eight of them.
    #[test]
    fn a_body_stands_apart_from_its_system_out_at_the_rim() {
        let second = 2.99792458e8;
        let middle = stands(40_000., DVec3::ZERO);
        let out = stands(40_000., DVec3::new(0., second, 0.));

        let apart = middle.distance(out) * space::LIGHT_YEAR;
        assert!(
            (apart - second).abs() < second * 1e-3,
            "a body a light second out stood {apart}m from the middle"
        );
    }

    /// The sun comes out at the light the sun gives off
    ///
    /// A little over three and a half times ten to the twenty-eighth lumens,
    /// which is the figure this is checked against everywhere else. Getting it
    /// from a radius and a temperature rather than choosing it is the whole
    /// point: every star on record then lights its own system correctly
    /// without anything being tuned.
    #[test]
    fn a_sun_gives_off_a_suns_worth_of_light() {
        let sol = lumens(6.957e8, 5772.);

        assert!(
            (sol - 3.6e28).abs() < 0.2e28,
            "the sun came to {sol} lumens, not about 3.6e28"
        );
    }

    /// A cooler star of the same size gives off far less
    ///
    /// The fourth power is most of what decides a star's output, so halving
    /// the temperature should take all but a sixteenth of the light.
    #[test]
    fn heat_counts_far_more_than_size() {
        let hot = lumens(1e9, 6000.);
        let cool = lumens(1e9, 3000.);

        assert!(
            (hot / cool - 16.).abs() < 0.1,
            "halving the heat left {} of the light, not a sixteenth",
            cool / hot
        );
    }

    /// A star with nothing on record does not light anything, and does not
    /// come out as a nonsense
    #[test]
    fn a_star_with_nothing_recorded_gives_off_nothing() {
        assert_eq!(lumens(0., 0.), 0.);
        assert_eq!(lumens(-1., -1.), 0.);
    }

    /// The ordinary sequence runs blue to red the way it should
    #[test]
    fn the_star_sequence_runs_hot_to_cold() {
        assert_eq!(Glow::of("O"), Glow::Blue);
        assert_eq!(Glow::of("B"), Glow::Blue);
        assert_eq!(Glow::of("A"), Glow::White);
        assert_eq!(Glow::of("G"), Glow::Yellow);
        assert_eq!(Glow::of("K"), Glow::Orange);
        assert_eq!(Glow::of("M"), Glow::Red);
    }

    /// What is not on the sequence is drawn as itself
    ///
    /// A neutron star begins with the same letter as nothing in particular,
    /// and a Wolf-Rayet with the same one as a white dwarf would if the
    /// dwarfs were not caught first. None of them is a main sequence star and
    /// none should be painted as one.
    #[test]
    fn what_is_not_a_main_sequence_star_is_not_drawn_as_one() {
        assert_eq!(Glow::of("N"), Glow::Odd);
        assert_eq!(Glow::of("H"), Glow::Odd);
        assert_eq!(Glow::of("WC"), Glow::Odd);
        assert_eq!(Glow::of("CJ"), Glow::Odd);
        assert_eq!(Glow::of("SupermassiveBlackHole"), Glow::Odd);
    }

    /// A white dwarf is white, whatever letter follows the D
    #[test]
    fn every_white_dwarf_is_a_white_dwarf() {
        for class in ["D", "DA", "DAB", "DQ", "DCV"] {
            assert_eq!(Glow::of(class), Glow::White, "{class} came out wrong");
        }
    }

    /// A class nobody has written down is still drawn
    #[test]
    fn an_unheard_of_star_is_still_given_a_colour() {
        assert_eq!(Glow::of(""), Glow::Odd);
        assert_eq!(Glow::of("Quite unlike anything"), Glow::Odd);
    }

    /// The classes the journal actually writes come out as themselves
    #[test]
    fn a_body_is_drawn_as_what_it_is() {
        assert_eq!(Surface::of("Earthlike body"), Surface::Earthlike);
        assert_eq!(Surface::of("Water world"), Surface::Water);
        assert_eq!(Surface::of("Ammonia world"), Surface::Ammonia);
        assert_eq!(Surface::of("Sudarsky class III gas giant"), Surface::Gas);
        assert_eq!(Surface::of("Icy body"), Surface::Icy);
        assert_eq!(Surface::of("Rocky body"), Surface::Rocky);
        assert_eq!(Surface::of("High metal content body"), Surface::Metal);
    }

    /// A rocky ice body is ice rather than rock
    ///
    /// The one place the order of the matching shows: both words are in the
    /// phrase, and the ice is what it looks like.
    #[test]
    fn a_rocky_ice_body_reads_as_ice() {
        assert_eq!(Surface::of("Rocky ice body"), Surface::Icy);
    }

    /// A gas giant with something living in it is still a gas giant
    #[test]
    fn a_gas_giant_with_life_is_still_a_gas_giant() {
        assert_eq!(
            Surface::of("Gas giant with water based life"),
            Surface::Gas
        );
        assert_eq!(
            Surface::of("Gas giant with ammonia based life"),
            Surface::Gas
        );
    }

    /// A class nobody has written down is still drawn
    #[test]
    fn an_unheard_of_body_is_still_given_a_surface() {
        assert_eq!(Surface::of(""), Surface::Unknown);
        assert_eq!(Surface::of("Something else entirely"), Surface::Unknown);
    }

    /// It takes more to draw a system's insides than to keep them
    ///
    /// Which is what stops a camera sitting on the line from spawning and
    /// despawning everything in a system every frame.
    #[test]
    fn drawing_asks_more_than_keeping() {
        assert!(WORTH_DRAWING > WORTH_KEEPING);
    }
}
