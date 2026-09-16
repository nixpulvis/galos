//! Reading a `galaxy.json`, over lines cut out of the real file.
//!
//! `galaxy.json` is five systems and the array's own punctuation: a triple
//! star round a barycentre, a system nobody has looked into, a system
//! holding a star nobody has measured, a red dwarf with twelve planets and
//! a gas giant, and a populated one. Between them they carry every shape
//! the mapping has to answer for.
//!
//! `galaxy_twins.json` is the two systems that carry one star twice; see
//! [`one_star_listed_twice_is_one_star`].

use elite_journal::body::AtmosphereType;
use elite_journal::entry::incremental::exploration::ScanTarget;
use elite_journal::entry::Event;
use elite_journal::system::Security;
use elite_journal::{Allegiance, Government};
use spansh::galaxy::System;
use spansh::Lines;
use std::path::Path;

/// Every system of the five-system fixture, in the file's order.
fn fixture() -> Vec<System> {
    read("galaxy.json")
}

/// Every system of one fixture beside this test, in the file's order.
fn read(fixture: &str) -> Vec<System> {
    let path =
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/")).join(fixture);
    let mut lines = Lines::open(&path).expect("the fixture opens");
    let mut systems = Vec::new();
    loop {
        let at = lines.at() + 1;
        let Some(text) = lines.next().expect("the fixture reads") else {
            break;
        };
        systems.push(
            serde_json::from_str(text)
                .unwrap_or_else(|err| panic!("line {at}: {err}")),
        );
    }
    systems
}

fn named(systems: &[System], name: &str) -> usize {
    systems.iter().position(|system| system.name == name).expect(name)
}

/// Rough enough for a figure that went through an `f32` on the way.
fn about(measured: f32, expected: f32) {
    let off = (measured - expected).abs() / expected.abs();
    assert!(off < 1e-5, "{measured} is not {expected}");
}

#[test]
fn the_dump_reads_as_systems() {
    let systems = fixture();
    assert_eq!(systems.len(), 5, "the array's own punctuation was read too");

    let populated = &systems[named(&systems, "lam01 Orionis")];
    assert_eq!(populated.id64, 19365007);
    assert_eq!(populated.population, Some(395452));
    assert!(matches!(populated.allegiance, Some(Allegiance::Independent)));
    assert!(matches!(populated.government, Some(Government::Democracy)));
    assert_eq!(populated.bodies.len(), 15);
    // The dump's `date`, which is the system's own and not any body's.
    assert_eq!(populated.update_time.to_rfc3339(), "2026-09-06T13:19:55+00:00",);

    // A system nobody has looked into is a system with no bodies, not a
    // line that will not read.
    let empty = &systems[named(&systems, "Traikoa EG-Y g0")];
    assert!(empty.bodies.is_empty());
    assert_eq!(empty.security, None);
    assert!(empty.scans().is_empty());
}

#[test]
fn a_stars_class_is_the_games_token() {
    let systems = fixture();
    let system = &systems[named(&systems, "Phua Scrua AA-A h1")];
    let scans = system.scans();

    let Event::Scan(scan) = &scans[0].event else { panic!("not a scan") };
    let ScanTarget::Star(star) = &scan.target else { panic!("not a star") };

    // `"M (Red dwarf) Star"` and `"M7"`, which the game writes as `M` and
    // a subclass of 7.
    assert_eq!(star.star_class, "M");
    assert_eq!(star.subclass, 7);
    assert_eq!(star.luminosity, "Va");
    assert_eq!(star.age_my, 2648);
    // 0.3888 solar radii, in metres.
    about(star.radius, 270_427_392.);
    // 1.7996 days, in seconds.
    about(star.spin.period, 155_482.98);

    // The body's own time, not the system's.
    assert_eq!(scans[0].timestamp.to_rfc3339(), "2026-09-04T16:52:44+00:00");
    assert_eq!(scan.system_address, system.id64);
    assert_eq!(scan.star_system, "Phua Scrua AA-A h1");
    assert_eq!(scan.star_pos, Some(system.coords));
}

/// The full form names the star a ship arrives at, and it is the same
/// answer the brief form's `mainStar` gives for the same system.
///
/// The flag is the dump's, not a rule of ours: the body carrying
/// `mainStar: true` is the one, whatever its number or its distance.
#[test]
fn the_main_star_is_the_one_the_dump_flags() {
    let systems = fixture();

    let dwarf = &systems[named(&systems, "Phua Scrua AA-A h1")];
    let class = dwarf.class().expect("a red dwarf has a class");
    assert_eq!(class.token(), "M");
    assert_eq!(
        dwarf.main_star().map(|body| body.name.as_str()),
        Some("Phua Scrua AA-A h1"),
    );

    // A system nobody has looked into has no bodies, so no arrival star
    // and no class -- which is not the same answer as a system whose
    // middle is a planet, and the tally of a read tells the two apart.
    let empty = &systems[named(&systems, "Traikoa EG-Y g0")];
    assert!(empty.bodies.is_empty());
    assert!(empty.main_star().is_none());
    assert_eq!(empty.class(), None);
}

#[test]
fn a_planets_surface_survives() {
    let systems = fixture();
    let system = &systems[named(&systems, "Phua Scrua AA-A h1")];

    let planets: Vec<_> = system
        .scans()
        .into_iter()
        .filter_map(|entry| match entry.event {
            Event::Scan(scan) => match scan.target {
                ScanTarget::Body(body) => Some(body),
                _ => None,
            },
            _ => None,
        })
        .collect();

    let icy = planets
        .iter()
        .find(|body| body.name.ends_with("h1 4 a"))
        .expect("the moon is in the fixture");
    assert_eq!(icy.planet_class, "Icy body");
    assert_eq!(icy.tidal_lock, Some(true));
    // 0.1848 g, in metres per second squared, and 2861.5 km in metres.
    about(icy.gravity, 1.812_144);
    about(icy.radius, 2_861_543.8);

    let surface = icy.surface.as_ref().expect("an icy moon has a surface");
    assert!(surface.landable);
    // 78.14% ice, as a fraction.
    about(surface.composition.ice, 0.781_358);
    // 0.0002 atmospheres, in pascals.
    about(surface.pressure, 20.319_593);
    assert_eq!(surface.atmosphere_type, AtmosphereType::None);
    // "Not terraformable" is nothing rather than a state.
    assert_eq!(surface.terraform_state, None);
    assert_eq!(surface.materials.len(), 11);
    // Named as the game names them, which is how they meet the rows a
    // journal wrote for the same body.
    let iron = surface
        .materials
        .iter()
        .find(|material| material.name == "iron")
        .expect("iron is there");
    assert!((iron.percent - 13.625_421).abs() < 1e-5);

    // A gas giant has nothing to stand on, and the rings it carries have
    // nowhere to go.
    let giant = planets
        .iter()
        .find(|body| body.planet_class == "Sudarsky class I gas giant")
        .expect("the gas giant is in the fixture");
    assert!(giant.surface.is_none());
}

#[test]
fn a_body_that_goes_round_nothing_has_no_orbit() {
    let systems = fixture();
    let system = &systems[named(&systems, "Phua Scrua AA-A h1")];

    let Event::Scan(scan) = &system.scans()[0].event else {
        panic!("not a scan")
    };
    let ScanTarget::Star(star) = &scan.target else { panic!("not a star") };
    assert_eq!(star.name, "Phua Scrua AA-A h1");
    assert!(star.orbit.is_none(), "the primary goes round nothing");
}

#[test]
fn the_scans_are_the_bodies_the_dump_gave_a_home() {
    let systems = fixture();

    // A barycentre is a body in the numbering and not an object, and comes
    // back as the event the game writes for one.
    let triple = &systems[named(&systems, "Phua Fraae AA-A h0")];
    let scans = triple.scans();
    assert_eq!(scans.len(), triple.bodies.len());
    assert!(matches!(scans[0].event, Event::ScanBaryCentre(_)));
    assert_eq!(
        scans.iter().filter(|e| matches!(e.event, Event::Scan(_))).count(),
        3,
    );

    // A star the dump names and has never measured is left out rather than
    // filled in.
    let unmeasured = &systems[named(&systems, "Pro Thua DV-O d6-0")];
    assert_eq!(unmeasured.bodies.len(), 8);
    assert_eq!(unmeasured.scans().len(), 7);
}

#[test]
fn anarchy_and_none_read_as_no_reading() {
    let systems = fixture();

    // Postgres holds no label for either, and a row carrying one is a row
    // refused — the whole system, and its bodies behind the foreign key.
    let deep = &systems[named(&systems, "Phua Fraae AA-A h0")];
    assert_eq!(deep.security, None, "Anarchy is not a security");
    assert_eq!(deep.government, None);
    assert_eq!(deep.primary_economy, None);

    // A reading that is one still arrives.
    let inhabited = &systems[named(&systems, "lam01 Orionis")];
    assert_eq!(inhabited.security, Some(Security::Low));
    assert_eq!(inhabited.government, Some(Government::Democracy));
    assert_eq!(inhabited.allegiance, Some(Allegiance::Independent));
}

/// A star listed twice under one name is one star, and the nearer record
/// of it is the one kept
///
/// Two systems of the real dump, both in `galaxy_twins.json`: `39 Omicron
/// Ophiuchi` (`id64` 1109989001603), where the twin is bodies 1 and 2,
/// magnitude 1.858093 and 7441K apiece, at 11342.859803 ls and at the
/// arrival point; and `UCAC3 70-2386` (`id64` 5373539404656), where it is
/// bodies 1 and 2, magnitude 11.483994 and 2083K apiece, at the arrival
/// point and at 4937.943689 ls. The dump lists the nearer of the two
/// second in the first system and first in the second, so a rule reading
/// the distance and one taking whichever came first do not agree here.
///
/// Both kept, the system is published 2.5 * log10(2) = 0.75 magnitudes
/// too bright. `9 Aurigae` (`id64` 1797183768931) carries the same fault
/// over `9 Aurigae C` and is not in the fixture: its line is a megabyte
/// of stations.
#[test]
fn one_star_listed_twice_is_one_star() {
    let systems = read("galaxy_twins.json");
    assert_eq!(systems.len(), 2);

    let kept = |system: &System| -> Vec<(i16, f32)> {
        system
            .scans()
            .into_iter()
            .filter_map(|entry| match entry.event {
                Event::Scan(scan) => match scan.target {
                    ScanTarget::Star(star) => {
                        Some((star.id, star.distance_from_arrival_ls))
                    }
                    _ => None,
                },
                _ => None,
            })
            .collect()
    };

    let ophiuchi = &systems[named(&systems, "39 Omicron Ophiuchi")];
    assert_eq!(ophiuchi.bodies.len(), 28);
    assert_eq!(
        ophiuchi.scans().len(),
        27,
        "the twin was counted as a second star",
    );
    assert_eq!(
        kept(ophiuchi),
        vec![(2, 0.0)],
        "the star kept is not the one at the arrival point",
    );

    let ucac = &systems[named(&systems, "UCAC3 70-2386")];
    assert_eq!(ucac.bodies.len(), 3);
    assert_eq!(ucac.scans().len(), 2, "the twin was counted as a second star");
    assert_eq!(
        kept(ucac),
        vec![(1, 0.0)],
        "the star kept is not the one at the arrival point",
    );
}
