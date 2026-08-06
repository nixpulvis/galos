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
use bevy::light::NotShadowCaster;
use bevy::math::DVec3;
use bevy::prelude::*;
use big_space::prelude::*;
use galos_db::bodies::Body as DbBody;
use galos_db::stars::Star as DbStar;

pub fn plugin(app: &mut App) {
    app.init_resource::<Drawn>();
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
    /// What it is called
    pub name: String,
    /// Which of the system's numbering it is
    pub id: i16,
    /// What kind of thing it is, as the journal spells it
    pub class: String,
    /// How far across it is, in metres
    pub radius: f32,
}

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
/// metres and `surface_temperature` in kelvin — so this is the star's real
/// output rather than anything chosen to look right.
fn lumens(radius: f32, temperature: f32) -> f32 {
    let (r, t) = (radius.max(0.) as f64, temperature.max(0.) as f64);
    let watts =
        4. * std::f64::consts::PI * r * r * STEFAN_BOLTZMANN * t.powi(4);
    (watts * EFFICACY) as f32
}

fn init_meshes(mut assets: ResMut<Assets<Mesh>>, mut commands: Commands) {
    // Four subdivisions is about thirteen hundred faces, which holds up to a
    // body filling the screen and is nothing to draw a handful of.
    let handle = assets.add(Sphere::new(1.).mesh().ico(4).unwrap());
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
    camera: Query<&OrbitCamera>,
    systems: Query<(Entity, &System)>,
    inside: Query<Entity, With<Inside>>,
    contents: Res<Contents>,
    mesh: Res<BodyMesh>,
    stars: Res<StarMaterials>,
    bodies: Res<BodyMaterials>,
    orbit_material: Res<OrbitMaterial>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut drawn: ResMut<Drawn>,
    mut commands: Commands,
) {
    let Ok(eye) = camera.single().map(|camera| camera.eye) else { return };

    // How large the system being held looks from here, which is the one
    // question deciding whether any of this is worth drawing.
    let apparent =
        contents.of().zip(contents.extent()).and_then(|(address, extent)| {
            let system = systems.iter().find(|(_, s)| s.address == address)?;
            let away = space::metres(eye - DVec3::from(system.1.position))
                .length() as f32;
            Some((address, system.0, extent / away.max(1.)))
        });

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

    // Contents first, then the grid they were placed in: a `CellCoord` under
    // an entity that is no longer a grid has nothing to be measured against.
    if let Some(shown) = drawn.0.take() {
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

    for star in contents.stars() {
        let place = orbits.place(star.id, 0.);
        commands.with_child(drawn_star(star, place, &grid, &mesh, &stars));
    }
    for body in contents.bodies() {
        let place = orbits.place(body.id, 0.);
        commands.with_child(drawn_body(body, place, &grid, &mesh, &bodies));
    }

    // One line per thing that goes round something, hung off whatever it goes
    // round so its own vertices carry only the size of the orbit.
    let paths = contents
        .stars()
        .iter()
        .map(|s| (s.id, s.parent_id))
        .chain(contents.bodies().iter().map(|b| (b.id, b.parent_id)));
    for (id, parent) in paths {
        if let Some(line) = drawn_orbit(
            id,
            parent,
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

/// A star, drawn at its own size and lighting what is around it
fn drawn_star(
    star: &DbStar,
    place: DVec3,
    grid: &Grid,
    mesh: &BodyMesh,
    materials: &StarMaterials,
) -> impl Bundle {
    let (cell, offset) = placed(place, grid);
    (
        Body {
            name: star.name.clone(),
            id: star.id,
            class: star.star_class.clone(),
            radius: star.radius,
        },
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
                intensity: lumens(star.radius, star.surface_temperature),
                range: f32::MAX,
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
fn drawn_orbit(
    id: i16,
    parent: Option<i16>,
    orbits: &Orbits,
    grid: &Grid,
    meshes: &mut Assets<Mesh>,
    material: &OrbitMaterial,
) -> Option<impl Bundle> {
    let path = orbits.path(id, ORBIT_POINTS)?;
    let about = parent.map_or(DVec3::ZERO, |parent| orbits.place(parent, 0.));
    let (cell, offset) = placed(about, grid);

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
