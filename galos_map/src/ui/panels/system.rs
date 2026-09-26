//! A system, a star or a body, described: the fields a panel about one of
//! them holds

use crate::map::camera::MoveCamera;
use crate::map::filter::Filter;
use crate::map::galaxy::System;
use crate::map::index::Factions;
use crate::ui::MARGIN;
use crate::ui::panels::fields::{
    NEST, UNKNOWN, copied, dated, field, lasting, named, spanning, under,
    yes_no,
};
use bevy::math::DVec3;
use bevy::prelude::*;
use bevy_egui::egui;
use bevy_egui::egui::Ui;
use elite_journal::body::{Composition, Material, Orbit, Spin};
use galos_index::records::{
    Body as DbBody, Economies, Star as DbStar, Surface,
};
use galos_photometry::{Distance, Magnitude};

/// Everything the map knows about one system
pub(super) fn described(
    ui: &mut Ui,
    system: &System,
    names: &Factions,
    eye: Option<DVec3>,
    moved: &mut Option<MoveCamera>,
    wanted: &mut Option<Filter>,
) {
    egui::Grid::new(("system-fields", system.address)).num_columns(2).show(
        ui,
        |ui| {
            let [x, y, z] = system.position;
            copied(ui, "Position", format!("{x:.2}, {y:.2}, {z:.2}"));
            // What the realistic view sizes a star by: the magnitude the index
            // build assigned, how bright it looks from where the camera stands,
            // and its tint bucket. Unknown for a system built from a name lookup
            // rather than a payload point.
            field(
                ui,
                "Abs. magnitude",
                match system.indexed_magnitude() {
                    Some(m) => format!("{m:.1}"),
                    None => UNKNOWN.into(),
                },
            );
            if let (Some(m), Some(eye)) = (system.indexed_magnitude(), eye) {
                let away = eye.distance(DVec3::from(system.position));
                field(
                    ui,
                    "App. magnitude",
                    format!(
                        "{:.1}",
                        Magnitude(m as f64)
                            .apparent(Distance::light_years(away))
                            .0
                    ),
                );
            }
            if let Some(t) = system.indexed_temperature() {
                field(ui, "Temperature", format!("{t:.0} K"));
            }
            // Two rows rather than one, because the two counts are reported
            // separately: the all-found tally and a nav beacon give the bodies
            // alone, and only the honk ever counts the belts and rings. Either
            // can stand known while the other is not.
            field(ui, "Bodies", named(&system.body_count));
            field(ui, "Belts and rings", named(&system.non_body_count));
            field(
                ui,
                "Population",
                crate::ui::text::thousands(system.population),
            );
            field(ui, "Allegiance", named(&system.allegiance));
            field(ui, "Government", named(&system.government));
            field(ui, "Security", named(&system.security));
            economies(ui, &system.economies);
            // Unknown for the same systems the magnitude is: the moment rides
            // on the payload point, and one built off the names table has none.
            field(
                ui,
                "Updated",
                match system.updated_at {
                    Some(at) => at.format("%Y-%m-%d %H:%M UTC").to_string(),
                    None => UNKNOWN.into(),
                },
            );
        },
    );

    factions(ui, &system.factions, names, wanted);

    // Its own system rather than whatever is selected, since several panels
    // stand open at once and each one is about the system named in its title
    // bar.
    ui.add_space(MARGIN);
    ui.horizontal(|ui| {
        if ui.button("Center Camera").clicked() {
            *moved = Some(MoveCamera {
                position: Some(DVec3::from(system.position)),
                framing: None,
            });
        }
        // The name is what the user carries out of here: into the game's own
        // map, a message to somebody, a spreadsheet. Nothing on the panel is
        // otherwise selectable, and a name retyped is a name misspelled.
        if ui.button("Copy Name").clicked() {
            ui.ctx().copy_text(system.name.to_string());
        }
    });
}

/// Metres in a solar radius
///
/// What a star's size is read in. Metres are what the database holds and what
/// the map draws in, and a star written out in them is a run of digits nobody
/// counts.
const SOLAR_RADIUS: f64 = 6.957e8;

/// Metres per second squared in a gravity
///
/// The journal records what a body pulls at in metres per second squared, and
/// what anybody wants to know is how that compares to standing on Earth.
const GRAVITY: f64 = 9.80665;

/// Pascals in an atmosphere
const ATMOSPHERE: f64 = 101_325.;

/// Everything the map knows about one star
///
/// Read in the units a star is talked about in rather than the ones it is
/// stored in: suns for its size and its mass, days for its turn, and light
/// seconds for how far out it stands.
pub(super) fn star_described(
    ui: &mut Ui,
    star: &DbStar,
    clock: &mut crate::map::bodies::Clock,
    guessed: Option<&str>,
) {
    egui::Grid::new(("star-fields", star.system_address, star.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", format!("{}{}", star.star_class, star.subclass));
            field(ui, "Luminosity", star.luminosity.clone());
            field(
                ui,
                "Distance",
                format!("{:.1} Ls", star.distance_from_arrival_ls),
            );
            field(
                ui,
                "Radius",
                format!("{:.3} Sol", star.radius as f64 / SOLAR_RADIUS),
            );
            field(ui, "Mass", format!("{:.3} Sol", star.stellar_mass));
            field(ui, "Temperature", format!("{:.0} K", star.temperature));
            field(
                ui,
                "Age",
                format!(
                    "{} million years",
                    crate::ui::text::thousands(star.age_my.max(0) as u64)
                ),
            );
            field(ui, "Magnitude", format!("{:.2}", star.absolute_magnitude));
            turning(ui, &star.spin);
            circling(ui, star.orbit.as_ref(), clock, guessed);
            field(ui, "Mapped", yes_no(star.mapped));
            field(ui, "Discovered", dated(star.discovered_at));
            field(
                ui,
                "Updated",
                star.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// Everything the map knows about one body
pub(super) fn body_described(
    ui: &mut Ui,
    body: &DbBody,
    clock: &mut crate::map::bodies::Clock,
    guessed: Option<&str>,
) {
    egui::Grid::new(("body-fields", body.system_address, body.id))
        .num_columns(2)
        .show(ui, |ui| {
            field(ui, "Class", body.planet_class.clone());
            field(
                ui,
                "Distance",
                match body.distance_from_arrival {
                    Some(away) => format!("{away:.1} Ls"),
                    None => UNKNOWN.into(),
                },
            );
            field(
                ui,
                "Radius",
                format!(
                    "{} km",
                    crate::ui::text::thousands((body.radius / 1e3) as u64)
                ),
            );
            field(ui, "Mass", format!("{:.3} Earths", body.mass));
            field(
                ui,
                "Gravity",
                format!("{:.2} g", body.gravity as f64 / GRAVITY),
            );
            field(
                ui,
                "Temperature",
                match body.temperature {
                    Some(heat) => format!("{heat:.0} K"),
                    None => UNKNOWN.into(),
                },
            );
            standing(ui, &body.surface);
            field(ui, "Tidal lock", yes_no(body.tidal_lock));
            turning(ui, &body.spin);
            circling(ui, Some(&body.orbit), clock, guessed);
            field(ui, "Mapped", yes_no(body.mapped));
            field(ui, "Discovered", dated(body.discovered_at));
            field(
                ui,
                "Updated",
                body.updated_at.format("%Y-%m-%d %H:%M UTC").to_string(),
            );
        });
}

/// What a body's surface is like, where it has one
///
/// A gas giant has none, and is one line saying so rather than a header over
/// five rows of nothing.
fn standing(ui: &mut Ui, surface: &Option<Surface>) {
    let Some(surface) = surface else {
        field(ui, "Surface", "None".into());
        return;
    };

    ui.label(egui::RichText::new("Surface").strong());
    ui.end_row();
    under(ui, "Landable", yes_no(surface.landable));
    under(ui, "Atmosphere", surface.atmosphere_type.to_string());
    under(
        ui,
        "Pressure",
        format!("{:.3} atm", surface.pressure as f64 / ATMOSPHERE),
    );
    under(ui, "Volcanism", named(&surface.volcanism));
    under(ui, "Terraforming", named(&surface.terraform_state));
    if let Some(crust) = &surface.composition {
        under(ui, "Composition", made_of(crust));
    }
    prospected(ui, &surface.materials);
}

/// What a crust is made of, as the scan divides it
///
/// Three fractions summing to one, said in hundredths and richest first,
/// which is the order the body is described in rather than the order the scan
/// happens to write. A part the scan found none of is left out: a rocky body
/// is rock and metal, and a line saying it is nought parts ice says nothing.
fn made_of(crust: &Composition) -> String {
    let mut parts =
        [("rock", crust.rock), ("metal", crust.metal), ("ice", crust.ice)];
    // `total_cmp` rather than `partial_cmp`: a fraction the scan left as NaN
    // still sorts somewhere rather than panicking the sort.
    parts.sort_by(|one, other| other.1.total_cmp(&one.1));
    let said: Vec<String> = parts
        .iter()
        .filter(|(_, share)| *share > 0.)
        .map(|(part, share)| format!("{:.0}% {part}", share * 100.))
        .collect();
    if said.is_empty() { UNKNOWN.into() } else { said.join(", ") }
}

/// The raw materials a surface can be prospected for, one to a row
///
/// Richest first, since what is asked of a body is what it has most of.
/// Nothing at all where the scan listed none: a header over no rows is a
/// header over nothing.
fn prospected(ui: &mut Ui, materials: &[Material]) {
    if materials.is_empty() {
        return;
    }

    ui.label(egui::RichText::new("Materials").strong());
    ui.end_row();
    for (name, share) in richest(materials) {
        under(ui, &name, format!("{share:.1}%"));
    }
}

/// The materials of a surface, named as they are read, richest first
///
/// Split from [`prospected`] because the order and the spelling are what
/// there is to get wrong, and neither needs a `Ui` to be asked about.
fn richest(materials: &[Material]) -> Vec<(String, f64)> {
    let mut sorted: Vec<(String, f64)> = materials
        .iter()
        .map(|material| (capitalised(&material.name), material.percent))
        .collect();
    sorted.sort_by(|one, other| other.1.total_cmp(&one.1));
    sorted
}

/// `word` with its first letter upper case
///
/// The journal spells a material `iron`, and a panel writes names as names.
/// A letter at a time, since a letter's upper case may be more than one
/// letter and the rest of the word is left exactly as it was read.
fn capitalised(word: &str) -> String {
    let mut letters = word.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// How a thing turns on its own axis
fn turning(ui: &mut Ui, spin: &Spin) {
    ui.label(egui::RichText::new("Spin").strong());
    ui.end_row();
    under(ui, "Period", lasting(spin.period));
    under(ui, "Tilt", format!("{:.1}°", spin.tilt.to_degrees()));
}

/// The path a thing takes about whatever it goes round
///
/// One line saying None for the one that goes round nothing, which is the
/// star a system arrives at, as a body with no surface is one line saying the
/// same.
///
/// `guessed` names the kind of place this path is measured from where that
/// place is one the map made up: a close pair whose centre was never scanned,
/// or a star or body a chain names with no row behind it, is stood up at
/// the distance its riders report in a direction nobody measured, so
/// everything under it is drawn exactly as its own scan says about a point
/// that is a guess. The path is a reading either way, and where it puts the
/// thing is not, which is the one thing about it a panel could not otherwise
/// say.
fn circling(
    ui: &mut Ui,
    orbit: Option<&Orbit>,
    clock: &mut crate::map::bodies::Clock,
    guessed: Option<&str>,
) {
    let Some(orbit) = orbit else {
        field(ui, "Orbit", "None".into());
        return;
    };

    ui.label(egui::RichText::new("Orbit").strong());
    ui.end_row();
    under(ui, "Radius", spanning(orbit.semi_major_axis));
    under(ui, "Period", lasting(orbit.orbital_period));
    under(ui, "Eccentricity", format!("{:.4}", orbit.eccentricity));
    under(ui, "Inclination", format!("{:.1}°", orbit.orbital_inclination));
    if let Some(kind) = guessed {
        under(ui, "Place", format!("Guessed ({kind} unscanned)"));
    }
    turned(ui, orbit.orbital_period as f64, clock);
}

/// Where round its orbit this thing stands, and a slider to move it by
///
/// Geared to this orbit alone, so the whole slider is one turn of this body
/// however long that is: a system has no span that suits all of it, the slowest
/// body of one taking a median 993 times as long to come round as its fastest.
///
/// It moves the map's own moment, which is one moment for the whole galaxy
/// and not an arrangement built body by body. So dragging this stirs
/// everything else by the same span of time, which for something far slower
/// is imperceptible and for something far faster is a blur -- and either is
/// the truth about what a year of this body does to its neighbours. A panel
/// outlives the camera leaving its system, so a slider dragged out there
/// moves the moment all the same; what it does not do any more is measure
/// that moment from a zero of its own.
///
/// Nothing to drag where the period is unrecorded, there being no turn to be a
/// fraction of.
///
/// The percentage is formatted rather than rounded. Egui clamps and rounds
/// the value it is handed before anything is drawn, and reports a slider
/// nobody touched as changed when the rounding moved it -- so a rail that
/// rounded to a tenth of a percent wrote that tenth back into the clock every
/// frame the phase was anything else. With another slider under the date
/// setting the same offset, the two fought over it frame by frame and the
/// date flickered between them; a phase that rounded up to a whole 100% took
/// the map on by one of this body's turns every frame, which is what ran the
/// reading off the end of what a date can hold.
fn turned(ui: &mut Ui, period: f64, clock: &mut crate::map::bodies::Clock) {
    ui.horizontal(|ui| {
        ui.add_space(NEST);
        ui.label(egui::RichText::new("Phase").strong());
    });

    let mut through = clock.through(period) * 100.;
    let moved = ui
        .add_enabled_ui(period > 0., |ui| {
            ui.add(
                egui::Slider::new(&mut through, 0.0..=100.)
                    .custom_formatter(|through, _| format!("{through:.1}%")),
            )
        })
        .inner;
    // The turn the phase is measured in is the clock's own to keep, so
    // nothing here has to say when a drag begins or ends. See
    // [`crate::map::bodies::Clock::offset_to`].
    if moved.changed() {
        clock.offset_to(period, through / 100.);
    }
    ui.end_row();
}

/// Every faction present in the system, one to a line
///
/// A list under a heading rather than rows in the grid above, because a
/// system holds as many factions as it holds and their names run long. Set
/// against the grid's two columns each would wrap into a paragraph, and the
/// column widths the rest of the panel is laid out in would be decided by
/// whichever faction happened to have the longest name.
///
/// Sorted by name, so that the order holds still. What comes back from the
/// database is in no order at all, and a fetch that replaced the row would
/// otherwise shuffle the list under whoever was reading it.
///
/// A faction the resident [`Factions`] table has no name for is still one of
/// the factions here, so it keeps its line: a list that grew a line once the
/// name turned up would jump.
///
/// Clicking one asks the map for it: the faction becomes a filter, and
/// everything it is absent from goes dim. A system's panel is where the user
/// finds out who is here, and where else they are is the next question, so it
/// is asked from the answer rather than typed out again in the bar.
///
/// A faction still waiting on its name does not answer. What it is called is
/// half of a filter, being what its row in the bar says it is, and a row
/// naming a faction as punctuation would say nothing about what had gone dim.
fn factions(
    ui: &mut Ui,
    present: &[i32],
    names: &Factions,
    wanted: &mut Option<Filter>,
) {
    if present.is_empty() {
        return;
    }

    ui.add_space(MARGIN);
    ui.label(egui::RichText::new("Factions").strong());
    for (id, name) in listed(present, names) {
        let text = match name {
            Some(name) => egui::RichText::new(name),
            None => egui::RichText::new(UNNAMED).weak(),
        };
        let (_, answer) = crate::ui::list::line(ui, text, 0., name.is_some());

        if let Some(name) = name
            && answer.clicked()
        {
            *wanted = Some(Filter::Faction { id, name: name.to_owned() });
        }
    }
}

/// The factions of a system as their lines read, in order
///
/// Named first and among themselves by name, with whatever is still unnamed
/// held at the end. Sorting the placeholder in with the names would put it
/// above all of them, since it is punctuation, and the line would then jump
/// the length of the list the moment its name arrived.
///
/// The id rides along with the name because a line is a control: clicking one
/// asks for a filter, and a filter tests a system against the id.
fn listed<'a>(
    present: &[i32],
    names: &'a Factions,
) -> Vec<(i32, Option<&'a str>)> {
    let mut listed: Vec<(i32, Option<&str>)> =
        present.iter().map(|id| (*id, names.name(*id))).collect();
    listed.sort_unstable_by_key(|(id, name)| (name.is_none(), *name, *id));
    listed
}

/// A faction the map has yet to hear the name of
const UNNAMED: &str = "...";

/// What a system trades in
///
/// Under a header, because the halves of an economy are named for their
/// standing against each other: "Secondary" among the flat fields says what
/// it is but not what it is secondary to.
///
/// A system with nothing on record is one line saying so. There is no primary
/// to head the pair with, and two nested lines both reading `UNKNOWN` say the
/// same thing at three times the length.
fn economies(ui: &mut Ui, economies: &Option<Economies>) {
    let Some(economies) = economies else {
        field(ui, "Economy", UNKNOWN.into());
        return;
    };

    ui.label(egui::RichText::new("Economy").strong());
    ui.end_row();
    under(ui, "Primary", economies.primary.to_string());
    under(ui, "Secondary", named(&economies.secondary));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::bodies::DAY;
    use crate::map::galaxy::tests::tallied;
    use crate::testing::{context, painted, words};
    use chrono::DateTime;
    use elite_journal::body::{
        AtmosphereType, Orbit as JournalOrbit, Spin as JournalSpin,
    };
    use elite_journal::system::Economy;

    /// A registry naming each of `known`
    fn known(known: &[(i32, &str)]) -> Factions {
        Factions(
            known.iter().map(|(id, name)| (*id, name.to_string())).collect(),
        )
    }

    /// The color each piece of text `contents` painted came out in
    ///
    /// Read back off the shapes for the reason [`crate::testing::words`] is:
    /// a panel is read by what its lines say and how brightly they say it,
    /// and the widget that wrote a line is gone by the time it is painted.
    ///
    /// A color asked for when the text was laid out is the one it keeps.
    /// Text laid out without one carries a placeholder instead, and what
    /// answers that is whatever the shape was painted with.
    fn written(
        mut contents: impl FnMut(&mut Ui),
    ) -> Vec<(String, egui::Color32)> {
        let ctx = context();
        let output = ctx.run_ui(egui::RawInput::default(), |ui| contents(ui));

        fn text_of(
            shape: &egui::Shape,
            into: &mut Vec<(String, egui::Color32)>,
        ) {
            match shape {
                egui::Shape::Text(text) => {
                    let asked = text
                        .galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                        .filter(|color| *color != egui::Color32::PLACEHOLDER);
                    let color = text
                        .override_text_color
                        .or(asked)
                        .unwrap_or(text.fallback_color);
                    into.push((text.galley.text().into(), color));
                }
                egui::Shape::Vec(shapes) => {
                    for shape in shapes {
                        text_of(shape, into);
                    }
                }
                _ => {}
            }
        }

        let mut painted = Vec::new();
        for shape in &output.shapes {
            text_of(&shape.shape, &mut painted);
        }
        painted
    }

    /// What the economy of a system reads as, line by line
    ///
    /// In a grid, since that is where a panel writes its fields and what
    /// [`under`] indents against.
    fn economy(economies: Option<Economies>) -> Vec<String> {
        words(|ui| {
            egui::Grid::new("economies").num_columns(2).show(ui, |ui| {
                super::economies(ui, &economies);
            });
        })
    }

    /// What a system's own panel reads as, line by line
    fn panel(system: &System) -> Vec<String> {
        words(|ui| {
            described(ui, system, &known(&[]), None, &mut None, &mut None);
        })
    }

    /// What stands in the answer column beside `name`
    fn beside(painted: &[String], name: &str) -> String {
        let at = painted
            .iter()
            .position(|word| word == name)
            .unwrap_or_else(|| panic!("{name} was painted"));
        painted[at + 1].clone()
    }

    /// A tallied system says how much of itself there is to find
    #[test]
    fn a_tallied_system_says_what_it_holds() {
        let painted = panel(&tallied(1, Some(12), Some(3)));

        assert_eq!(beside(&painted, "Bodies"), "12");
        assert_eq!(beside(&painted, "Belts and rings"), "3");
    }

    /// A system tallied for its bodies alone still answers for its belts
    ///
    /// Only the honk counts the belts and rings, so a system counted by the
    /// all-found tally has the one and not the other. Dropping the row would
    /// read as a system with no belts rather than one nobody has honked.
    #[test]
    fn a_system_tallied_for_its_bodies_alone_says_so() {
        let painted = panel(&tallied(1, Some(12), None));

        assert_eq!(beside(&painted, "Bodies"), "12");
        assert_eq!(beside(&painted, "Belts and rings"), UNKNOWN);
    }

    /// Both halves are written out, under a header naming what they are
    #[test]
    fn an_economy_reads_as_its_two_halves() {
        let economies = Some(Economies {
            primary: Economy::Agriculture,
            secondary: Some(Economy::Extraction),
        });

        assert_eq!(
            economy(economies),
            vec![
                "Economy",
                "Primary",
                "Agriculture",
                "Secondary",
                "Extraction"
            ]
        );
    }

    /// A system with only a primary is still asked about its secondary
    ///
    /// The header promises two lines, and dropping the one that is unknown
    /// would read as a system whose secondary is something the panel is not
    /// saying.
    #[test]
    fn an_economy_missing_its_secondary_says_so() {
        let economies =
            Some(Economies { primary: Economy::Agriculture, secondary: None });

        assert_eq!(
            economy(economies),
            vec!["Economy", "Primary", "Agriculture", "Secondary", UNKNOWN]
        );
    }

    /// A system with nothing on record says it in one line
    #[test]
    fn a_system_with_no_economy_says_so_once() {
        assert_eq!(economy(None), vec!["Economy", UNKNOWN]);
    }

    /// Founders World, as the database holds it
    fn founders() -> DbBody {
        DbBody {
            system_address: 1,
            id: 14,
            parents: vec![],
            name: "Founders World".to_owned(),
            body_type: None,
            distance_from_arrival: Some(345.5563),
            updated_at: DateTime::UNIX_EPOCH,
            updated_by: String::new(),
            planet_class: "Earthlike body".to_owned(),
            tidal_lock: false,
            mass: 0.69,
            radius: 5.485766e6,
            gravity: 9.13869,
            temperature: Some(298.70755),
            surface: None,
            orbit: JournalOrbit {
                semi_major_axis: 1.0023064e11,
                eccentricity: 0.0026,
                orbital_inclination: 1.5,
                periapsis: 0.,
                orbital_period: 8.6e7,
                ascending_node: Some(0.),
                mean_anomaly: Some(0.),
            },
            spin: JournalSpin { period: 3.6254802e6, tilt: 0.373026 },
            discovered_at: None,
            mapped: true,
        }
    }

    /// What a body's panel says
    fn body_said() -> Vec<String> {
        words(|ui| {
            body_described(
                ui,
                &founders(),
                &mut crate::map::bodies::Clock::default(),
                None,
            )
        })
    }

    /// A place the map made up is owned as one, and says what was missing
    ///
    /// The body's own path is a reading and where it puts the body is not:
    /// what it is measured from was never scanned, so the map stood that up
    /// at the distance its riders report in a direction nobody measured. Not
    /// only ever a barycentre — an unscanned star or body is stood up the
    /// same way — so the word is part of the answer.
    #[test]
    fn a_guessed_place_says_what_was_never_scanned() {
        let said = |kind| {
            words(|ui| {
                body_described(
                    ui,
                    &founders(),
                    &mut crate::map::bodies::Clock::default(),
                    kind,
                )
            })
        };

        let guessed = said(Some("star"));
        assert!(
            guessed.contains(&"Guessed (star unscanned)".to_owned()),
            "{guessed:?}"
        );
        assert!(
            said(Some("barycentre"))
                .contains(&"Guessed (barycentre unscanned)".to_owned())
        );
        // And nothing at all where nothing was made up, which is the
        // ordinary case: a row that says where it is says it.
        assert!(
            !said(None).iter().any(|word| word.starts_with("Guessed")),
            "a place nobody guessed at was called a guess"
        );
    }

    /// A phase slider nobody has touched leaves the clock exactly where it is
    ///
    /// Reported: holding the slider under the date left the reading flicking
    /// between two moments, and a while later the map crashed dating one --
    /// `DateTime + TimeDelta` overflowed. Egui clamps and rounds the value a
    /// slider is handed before it draws it, and reports a slider nobody
    /// touched as changed when the rounding moved it. So a rail that rounded
    /// to a tenth of a percent wrote that tenth back into the clock every
    /// frame the phase was anything else: it fought the slider that was
    /// actually being dragged, and where the phase rounded up to a whole
    /// 100% it took the map on by one of this body's turns per frame.
    #[test]
    fn an_untouched_phase_slider_leaves_the_clock_alone() {
        let ctx = context();
        let period = 400. * DAY;
        let mut clock = crate::map::bodies::Clock::default();
        // A phase that is no round tenth of a percent of its own turn.
        clock.offset_to(period, 0.374_838_71);
        let was = clock.offset();

        for _ in 0..4 {
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                egui::Grid::new("phase").show(ui, |ui| {
                    turned(ui, period, &mut clock);
                });
            });
        }

        assert_eq!(clock.offset(), was, "an untouched slider moved the clock");
    }

    /// One drag of the phase slider, from `from` to `to` along its own rail
    ///
    /// Where the rail stands is read off what the pass painted: the slider's
    /// rail is the widest thin rectangle in it. Four passes to a drag — the
    /// press, the move, the release, and an idle one, which is what a frame
    /// nobody touches is — since egui knows where a widget stands only once
    /// it has been drawn and reports a gesture on the pass it arrives.
    fn dragged(
        period: f64,
        clock: &mut crate::map::bodies::Clock,
        drags: &[(f32, f32)],
    ) {
        let ctx = context();

        let pass = |input: egui::RawInput,
                    clock: &mut crate::map::bodies::Clock| {
            let mut widest = egui::Rect::NOTHING;
            let output = ctx.run_ui(input, |ui| {
                ui.set_width(300.);
                egui::Grid::new("phase").show(ui, |ui| {
                    turned(ui, period, clock);
                });
            });
            for shape in &output.shapes {
                if let egui::Shape::Rect(rect) = &shape.shape
                    && rect.rect.width() > widest.width()
                    && rect.rect.height() < 12.
                {
                    widest = rect.rect;
                }
            }
            widest
        };

        // Twice: an area is drawn from what it last came to, and the first
        // pass paints nothing to measure.
        pass(egui::RawInput::default(), clock);
        let rail = pass(egui::RawInput::default(), clock);

        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let at = |along: f32| {
            egui::pos2(rail.left() + rail.width() * along, rail.center().y)
        };
        let moved = |to: f32, events: Vec<egui::Event>| egui::RawInput {
            events: [vec![egui::Event::PointerMoved(at(to))], events].concat(),
            ..Default::default()
        };

        for (from, to) in drags {
            pass(moved(*from, vec![button(at(*from), true)]), clock);
            pass(moved(*to, Vec::new()), clock);
            pass(moved(*to, vec![button(at(*to), false)]), clock);
            pass(egui::RawInput::default(), clock);
        }
    }

    /// A phase dragged to the far end stays there, and drags back from it
    ///
    /// Reported as the slider being broken, and it was, in two ways that were
    /// the one fault: the handle jumped to the near end the moment a drag to
    /// the far end was let go of, and the drag after that ran the map forward
    /// when it was pulled back.
    ///
    /// The far end is a whole turn past where the body stood, which is the
    /// same place on its orbit and the beginning of the next turn. Folded
    /// back out of the offset, that reads as no phase at all -- so the
    /// handle was drawn at the near end while the map stood a turn on, and
    /// the next drag measured its fraction in the turn after the one the
    /// handle looked to be in.
    #[test]
    fn a_phase_dragged_to_its_far_end_stays_there_and_comes_back() {
        let period = 400. * DAY;
        let mut clock = crate::map::bodies::Clock::default();

        dragged(period, &mut clock, &[(0., 1.)]);

        assert_eq!(
            clock.offset(),
            period,
            "a drag end to end did not run the map on by one turn"
        );
        assert_eq!(
            clock.through(period),
            1.,
            "the handle went back to the near end when it was let go of"
        );

        // And back: the handle stands at the far end, so a drag from there to
        // the middle is half a turn back rather than half a turn on.
        dragged(period, &mut clock, &[(1., 0.5)]);

        assert!(
            (clock.offset() - period / 2.).abs() < period / 100.,
            "dragging back from the far end left the map at {:.3} turns",
            clock.offset() / period
        );
    }

    /// A body's panel reads in the units a body is talked about in
    ///
    /// Which are not the ones it is stored in. The database holds metres and
    /// metres per second squared because that is what a scan records and what
    /// the map draws with, and nobody asks how many metres across a world is.
    #[test]
    fn a_body_is_described_in_the_units_it_is_read_in() {
        let said = body_said();

        assert!(said.contains(&"Earthlike body".to_owned()), "{said:?}");
        assert!(said.contains(&"345.6 Ls".to_owned()), "{said:?}");
        assert!(said.contains(&"5,485 km".to_owned()), "{said:?}");
        assert!(said.contains(&"0.690 Earths".to_owned()), "{said:?}");
        assert!(said.contains(&"0.93 g".to_owned()), "{said:?}");
        assert!(said.contains(&"299 K".to_owned()), "{said:?}");
    }

    /// A gas giant has no surface, and says so in one line
    ///
    /// Rather than a header over five rows of nothing, as a system with no
    /// economy on record does.
    #[test]
    fn a_body_with_no_surface_says_so_once() {
        let said = body_said();

        assert!(said.contains(&"Surface".to_owned()), "{said:?}");
        assert!(said.contains(&"None".to_owned()), "{said:?}");
        assert!(!said.contains(&"Landable".to_owned()), "{said:?}");
    }

    /// A material the scan found `percent` of
    fn material(name: &str, percent: f64) -> Material {
        Material { name: name.to_owned(), percent }
    }

    /// A crust is said by its parts, richest first, leaving out what is not
    /// there
    ///
    /// The scan reports three fractions summing to one, and a rocky body
    /// reports no ice at all. A line saying it is nought parts ice says
    /// nothing, and the order the scan writes them in is not the order a body
    /// is described in.
    #[test]
    fn a_crust_is_said_richest_first_without_what_is_not_there() {
        let rocky = Composition { ice: 0., rock: 0.911156, metal: 0.088844 };
        let icy = Composition { ice: 0.7, rock: 0.2, metal: 0.1 };

        assert_eq!(made_of(&rocky), "91% rock, 9% metal");
        assert_eq!(made_of(&icy), "70% ice, 20% rock, 10% metal");
    }

    /// Materials are listed richest first, named as names
    ///
    /// What is asked of a body is what it has most of, and the journal spells
    /// a material `iron` where a panel writes Iron.
    #[test]
    fn materials_are_listed_richest_first() {
        let found = [
            material("nickel", 15.2),
            material("iron", 20.1),
            material("carbon", 12.8),
        ];

        assert_eq!(
            richest(&found),
            vec![
                ("Iron".to_owned(), 20.1),
                ("Nickel".to_owned(), 15.2),
                ("Carbon".to_owned(), 12.8),
            ]
        );
    }

    /// A surface the scan listed no materials for has no Materials section
    ///
    /// A header over no rows is a header over nothing. The surface's own rows
    /// still draw, so this is the header being withheld rather than the
    /// section failing to reach the panel at all.
    #[test]
    fn a_surface_with_nothing_scanned_lists_no_materials() {
        let said = words(|ui| {
            egui::Grid::new("surface").num_columns(2).show(ui, |ui| {
                standing(ui, &Some(bare()));
            });
        });

        assert!(said.contains(&"Landable".to_owned()), "{said:?}");
        assert!(!said.contains(&"Materials".to_owned()), "{said:?}");
    }

    /// A surface a scan reached but found no materials on
    fn bare() -> Surface {
        Surface {
            atmosphere_type: AtmosphereType::None,
            pressure: 0.,
            composition: None,
            landable: true,
            atmosphere: None,
            volcanism: None,
            terraform_state: None,
            materials: vec![],
        }
    }

    /// A body's turn and its orbit are read in days
    ///
    /// A period is recorded in seconds, and eight digits of them is a number
    /// nobody reads.
    #[test]
    fn what_turns_slowly_is_read_in_days() {
        let said = body_said();

        assert!(said.contains(&"42.0 Earth days".to_owned()), "{said:?}");
        assert!(said.contains(&"21.4°".to_owned()), "{said:?}");
    }

    /// The names down the left of a panel are written as brightly as a header
    ///
    /// Nested or flat, a name is what the column is read down, and one in the
    /// same color as the answers beside it leaves the two to be told apart
    /// by which side of the panel they fell on.
    #[test]
    fn the_names_of_fields_read_as_brightly_as_a_header() {
        let painted = written(|ui| {
            egui::Grid::new("fields").num_columns(2).show(ui, |ui| {
                field(ui, "Security", "Low".into());
                economies(
                    ui,
                    &Some(Economies {
                        primary: Economy::Military,
                        secondary: None,
                    }),
                );
            });
        });

        let color = |wanted: &str| {
            painted
                .iter()
                .find(|(text, _)| text == wanted)
                .unwrap_or_else(|| panic!("{wanted} was painted"))
                .1
        };

        assert_eq!(color("Security"), color("Economy"));
        assert_eq!(color("Primary"), color("Economy"));
        assert_ne!(color("Low"), color("Economy"));
        assert_ne!(color("Military"), color("Economy"));
    }

    /// A system with no factions lists none
    #[test]
    fn a_system_with_no_factions_lists_nothing() {
        assert!(listed(&[], &known(&[])).is_empty());
    }

    /// The list reads in name order, whatever order the ids came in
    ///
    /// What the database returns is in no order at all, and a fetch that
    /// replaced the row would otherwise shuffle the list under whoever was
    /// reading it.
    #[test]
    fn factions_are_listed_by_name() {
        let names = known(&[(1, "Zargon Front"), (2, "Alliance of Sol")]);
        let wanted =
            vec![(2, Some("Alliance of Sol")), (1, Some("Zargon Front"))];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }

    /// And so does a whole faction list, marks and placeholders included
    #[test]
    fn a_faction_list_paints_in_a_color() {
        let names = known(&[(1, "Alliance of Sol")]);
        painted(|ui| {
            // One named and one still waiting, so both arms are drawn.
            factions(ui, &[1, 2], &names, &mut None);
        });
    }

    /// A line carries the id its filter would be built from
    ///
    /// Clicking a faction asks for it, and what a filter tests against is the
    /// id, so the name alone would leave the line unable to say which faction
    /// it named.
    #[test]
    fn a_listed_faction_carries_its_id() {
        let names = known(&[(7, "Alliance of Sol")]);

        assert_eq!(listed(&[7], &names), vec![(7, Some("Alliance of Sol"))]);
    }

    /// A faction without a name keeps its line, at the end
    ///
    /// Named from the resident [`Factions`] table, which may not hold every
    /// id a system carries, and a list that grew a line once a name turned
    /// up would jump under the reader. Held at the
    /// end rather than sorted in, since the placeholder is punctuation and
    /// would otherwise sit above every name and then jump the length of the
    /// list as soon as its own arrived.
    ///
    /// It answers to nothing while it waits. A faction is asked for by name
    /// as well as by id, and a filter row naming one as punctuation would say
    /// nothing about what had gone dim.
    #[test]
    fn a_faction_not_yet_named_takes_the_last_line() {
        let names = known(&[(1, "Alliance of Sol")]);
        let wanted = vec![(1, Some("Alliance of Sol")), (2, None)];

        assert_eq!(listed(&[1, 2], &names), wanted);
        assert_eq!(listed(&[2, 1], &names), wanted);
    }
}
