//! One system as `galaxy.json` gives it, and the scans it stands for.
//!
//! The full dump is the brief one with everything in the system attached:
//! its standing in the background simulation, and a [`Body`] for every
//! star, planet and barycentre anybody has looked at. [`System::scans`]
//! turns those into the [`Entry`] values the rest of the ecosystem is fed,
//! so a dumped system and a journalled one arrive as the same shape.
//!
//! The two vocabularies differ in their units and in their spellings, and
//! both differences are settled here. The units are the constants below.
//! The spellings are [`planet_class`], [`atmosphere_of`] and
//! [`volcanism_of`] for planets, and [`crate::class_of`] for stars.
//!
//! The dump carries more than the journal has anywhere to put: a body's
//! `rings`, `belts`, `signals`, `reserveLevel`, `atmosphereComposition`,
//! `timestamps` and `stations`, and a system's `stations`, `factions`,
//! `powers` and `thargoidWar`. Those are read past.

use crate::class_of;
use chrono::{DateTime, Utc};
use elite_journal::body::Body as Planet;
use elite_journal::body::{
    AtmosphereType, BodyType, Composition, Discovery, Material, Orbit, Spin,
    Star, StarClass, Surface,
};
use elite_journal::de::null_is_none;
use elite_journal::entry::incremental::exploration::{
    Scan, ScanBaryCentre, ScanTarget,
};
use elite_journal::entry::{Entry, Event};
use elite_journal::system::{Coordinate, Economy, Security};
use elite_journal::{Allegiance, Government};
use serde::Deserialize;
use std::collections::BTreeMap as Map;

/// Metres in a kilometre: a planet's radius is kilometres in the dump and
/// metres in the journal.
const KM: f64 = 1_000.;

/// Metres in a solar radius, as the game reckons one: a star's radius is a
/// ratio of Sol's in the dump and metres in the journal.
const SOLAR_RADIUS: f64 = 695_500_000.;

/// Metres per second squared in a gravity, as the game reckons one:
/// gravity is a ratio of Earth's in the dump and an acceleration in the
/// journal.
const GRAVITY: f64 = 9.807;

/// Pascals in an atmosphere: surface pressure is atmospheres in the dump
/// and pascals in the journal.
const ATMOSPHERE: f64 = 101_325.;

/// Seconds in a day: an orbital or rotational period is days in the dump
/// and seconds in the journal.
const DAY: f64 = 86_400.;

/// Metres in an astronomical unit: a semi-major axis is astronomical units
/// in the dump and metres in the journal. The published schema says
/// kilometres and the file does not agree with it.
const AU: f64 = 149_597_870_700.;

/// The fraction a percentage stands for: a solid composition is
/// percentages in the dump and fractions in the journal. A body's
/// materials are percentages in both.
const PERCENT: f64 = 0.01;

/// A system as a dump gives it: where it is, how it is run, and what is
/// in it.
///
/// **One type for both files.** `systems.json` fills in the address, the
/// name, the place, the time and the prose for the star at the middle;
/// `galaxy.json` fills in the standing as well and hangs a [`Body`] on
/// the system for every star, planet and barycentre anybody has looked
/// at. Everything the brief file leaves out is what the schema itself
/// leaves out of `required`, so it is [`None`] or empty here and no
/// caller has to know which file it came from.
///
/// The dump's own field names are kept. The one spelling that differs
/// between the files is the system's own time — `updateTime` in the
/// brief file and `date` in the full one — which is an alias and not a
/// second field: both mean when anything in the system was last heard
/// about.
///
/// Only what has a home downstream is read. The schema declares
/// `additionalProperties: false`, so a field this does not want -- a
/// system's `stations`, `factions`, `powers`, `thargoidWar`, the brief
/// file's `needsPermit` -- is passed over.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct System {
    /// The game's own 64-bit address, which every other source keys by too.
    pub id64: i64,
    pub name: String,
    pub coords: Coordinate,
    /// When anything in the system was last heard about, in UTC.
    #[serde(alias = "date")]
    pub update_time: DateTime<Utc>,
    /// The class of the star at the middle, as the brief file's prose:
    /// `"M (Red dwarf) Star"`. The full file says the same thing by
    /// flagging the body instead, which is what [`System::arrival`]
    /// reads, and [`System::class`] answers off whichever is there.
    pub main_star: Option<String>,
    pub population: Option<u64>,

    // `Anarchy`, `None` and `""` all mean "no reading" here — see
    // `Nullable` — and the enums Postgres holds have no label for them.
    #[serde(deserialize_with = "null_is_none", default)]
    pub allegiance: Option<Allegiance>,
    #[serde(deserialize_with = "null_is_none", default)]
    pub government: Option<Government>,
    #[serde(deserialize_with = "null_is_none", default)]
    pub security: Option<Security>,
    #[serde(deserialize_with = "null_is_none", default)]
    pub primary_economy: Option<Economy>,
    #[serde(deserialize_with = "null_is_none", default)]
    pub secondary_economy: Option<Economy>,
    /// How many bodies the system holds, which is not how many
    /// [`System::bodies`] lists: the count comes off a discovery scan and
    /// the list off what has since been looked at.
    pub body_count: Option<i32>,
    /// Every body anybody has reported, in the dump's order. Empty of a
    /// brief file, which lists none.
    #[serde(default)]
    pub bodies: Vec<Body>,
}

/// Which of the three kinds of thing a [`Body`] is.
///
/// A barycentre is the centre of mass a close pair goes round, and is a
/// body in the numbering without being an object.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub enum Kind {
    Star,
    Planet,
    Barycentre,

    /// A kind the schema does not list, which means the dump's format
    /// moved. Kept rather than refused, so one new kind does not stop a
    /// system's other bodies being read.
    #[serde(untagged)]
    Unknown(String),
}

/// A body in the full dump, as it stands.
///
/// One shape for all three kinds, because the dump has one: a star's
/// figures and a planet's are fields of the same object, and which are
/// filled says what was looked at. [`System::scans`] is where this becomes
/// a star, a planet or a barycentre.
///
/// Everything the schema leaves out of a body's `required` is
/// [`Option`], which is everything but the name, the id and the time. A
/// body listed and never scanned carries no more than those.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Body {
    /// The body's number within its system, which is what the journal
    /// keys by. Not the dump's `id64`, which is the body's own galactic
    /// address and has no home downstream.
    pub body_id: i16,
    pub name: String,
    #[serde(rename = "type")]
    pub ty: Option<Kind>,
    /// What kind of star or planet, as the dump's prose: `"M (Red dwarf)
    /// Star"`, `"Rocky body"`.
    pub sub_type: Option<String>,
    /// What it goes round, nearest first, each named by kind and number.
    #[serde(default)]
    pub parents: Vec<Map<String, i16>>,
    /// Light seconds from the system's arrival point. The published
    /// schema says kilometres and the file does not agree with it.
    pub distance_to_arrival: Option<f64>,
    /// When this body was last heard about, in UTC, which is what its
    /// scan is stamped with.
    pub update_time: DateTime<Utc>,

    pub is_landable: Option<bool>,
    pub gravity: Option<f64>,
    pub earth_masses: Option<f64>,
    pub radius: Option<f64>,
    pub surface_temperature: Option<f64>,
    pub surface_pressure: Option<f64>,
    pub volcanism_type: Option<String>,
    pub atmosphere_type: Option<String>,
    /// What the body is made of, as percentages. Present only where there
    /// is something to stand on, and so what says there is a
    /// [`Surface`] here at all.
    pub solid_composition: Option<Composition>,
    pub terraforming_state: Option<String>,
    /// What can be picked up off it, by percentage.
    pub materials: Option<Map<String, f64>>,

    pub rotational_period: Option<f64>,
    pub rotational_period_tidally_locked: Option<bool>,
    pub axial_tilt: Option<f64>,

    pub orbital_period: Option<f64>,
    pub semi_major_axis: Option<f64>,
    pub orbital_eccentricity: Option<f64>,
    pub orbital_inclination: Option<f64>,
    pub arg_of_periapsis: Option<f64>,
    pub mean_anomaly: Option<f64>,
    pub ascending_node: Option<f64>,

    pub age: Option<i32>,
    /// The spectral letter and subclass together: `"M1"`, `"WNC0"`. The
    /// letter alone does not say a giant from a dwarf, which is why the
    /// class comes off [`Body::sub_type`] and only the number comes off
    /// this.
    pub spectral_class: Option<String>,
    pub luminosity: Option<String>,
    pub absolute_magnitude: Option<f64>,
    pub solar_masses: Option<f64>,
    pub solar_radius: Option<f64>,
    pub main_star: Option<bool>,
}

impl System {
    /// The body a ship arrives at, where the file lists bodies and one
    /// of them is flagged.
    ///
    /// The flag is the dump's own — `mainStar: true` on the body — and
    /// not a rule of ours about which body is first or nearest. A brief
    /// file lists no bodies, so this is [`None`] there and
    /// [`System::main_star`] is what it said instead.
    pub fn arrival(&self) -> Option<&Body> {
        self.bodies.iter().find(|body| body.main_star == Some(true))
    }

    /// What is at the middle of this system, in the game's vocabulary, or
    /// [`None`] where there is no star there or nobody has looked.
    ///
    /// The same answer from either file: the brief one's prose where
    /// there is prose, and the arrival body's `subType` — the same
    /// vocabulary, a subset of the same published list — where there are
    /// bodies. See [`crate::star`].
    pub fn class(&self) -> Option<StarClass> {
        let prose = match &self.main_star {
            Some(prose) => prose.as_str(),
            None => self.arrival()?.sub_type.as_deref()?,
        };
        class_of(prose)
    }

    /// One scan per body the dump gives a home to, in dump order.
    ///
    /// A star and a planet come back as [`Event::Scan`] and a barycentre
    /// as [`Event::ScanBaryCentre`], each stamped with that body's own
    /// [`Body::update_time`]. A body the dump lists without the figures
    /// its kind is made of -- one counted by a beacon and never looked at
    /// -- is left out rather than filled in, so what comes back is
    /// shorter than [`System::bodies`] by however many of those there
    /// were, and by however many stars were a second record of one
    /// already listed.
    pub fn scans(&self) -> Vec<Entry<Event>> {
        let twins = self.twins();
        self.bodies
            .iter()
            .enumerate()
            .filter(|(at, _)| !twins.contains(at))
            .filter_map(|(_, body)| self.scan(body))
            .collect()
    }

    /// Which of [`System::bodies`] are a second record of a star already
    /// listed, by position in the list.
    ///
    /// Two star bodies of one system sharing a `name` are one star: the
    /// game names a system's bodies uniquely, and the dump carries stars
    /// listed twice under one name, agreeing to six digits on magnitude
    /// and temperature and differing only in `bodyId` and
    /// `distanceToArrival`. Kept both, a system's light is counted twice
    /// and it is published `2.5 * log10(2)` = 0.75 magnitudes too bright.
    ///
    /// The one kept is the nearer of the two to the arrival point, which
    /// is the arrival star a system's class is read off. Stars only: no
    /// two planets of one system in the dump share a name, and a rule
    /// wider than the evidence for it would drop a body that is real.
    fn twins(&self) -> Vec<usize> {
        let stars = || {
            self.bodies
                .iter()
                .enumerate()
                .filter(|(_, body)| matches!(body.ty, Some(Kind::Star)))
        };
        // What almost every system is, and nothing is allocated for it.
        if stars().take(2).count() < 2 {
            return Vec::new();
        }
        let mut standing: Vec<(&str, usize)> = Vec::new();
        let mut twins = Vec::new();
        for (at, body) in stars() {
            match standing.iter_mut().find(|(name, _)| *name == body.name) {
                Some((_, stood)) => {
                    if nearer(body, &self.bodies[*stood]) {
                        twins.push(*stood);
                        *stood = at;
                    } else {
                        twins.push(at);
                    }
                }
                None => standing.push((&body.name, at)),
            }
        }
        twins
    }

    /// The one scan a body stands for, or [`None`] where it stands for
    /// none.
    fn scan(&self, body: &Body) -> Option<Entry<Event>> {
        let event = match body.ty.as_ref()? {
            Kind::Star => Event::Scan(self.of(ScanTarget::Star(body.star()?))),
            Kind::Planet => {
                Event::Scan(self.of(ScanTarget::Body(body.planet()?)))
            }
            Kind::Barycentre => Event::ScanBaryCentre(ScanBaryCentre {
                star_system: self.name.clone(),
                star_pos: Some(self.coords),
                system_address: self.id64,
                body_id: body.body_id,
                orbit: body.orbit(),
            }),
            Kind::Unknown(_) => return None,
        };
        Some(Entry {
            timestamp: body.update_time,
            event,
            horizons: false,
            odyssey: false,
        })
    }

    /// A scan of this system, around whatever was looked at.
    ///
    /// No `scan_type`: the dump is what a thousand commanders' scans were
    /// merged into, so how close a look any one of them took is not
    /// something it still knows.
    fn of(&self, target: ScanTarget) -> Scan {
        Scan {
            scan_type: None,
            star_system: self.name.clone(),
            star_pos: Some(self.coords),
            system_address: self.id64,
            target,
            other: serde_json::Value::Null,
        }
    }
}

impl Body {
    /// This body as a star, or [`None`] where the dump has not got one.
    fn star(&self) -> Option<Star> {
        let class = class_of(self.sub_type.as_deref()?)?;
        Some(Star {
            name: self.name.clone(),
            id: self.body_id,
            parents: self.parents.clone(),
            absolute_magnitude: self.absolute_magnitude? as f32,
            age_my: self.age?,
            distance_from_arrival_ls: self.distance_to_arrival? as f32,
            luminosity: self.luminosity.clone()?,
            star_class: class.token().to_owned(),
            stellar_mass: self.solar_masses? as f32,
            subclass: self.subclass(),
            orbit: self.orbit(),
            spin: self.spin()?,
            radius: (self.solar_radius? * SOLAR_RADIUS) as f32,
            temperature: self.surface_temperature? as f32,
            discovery: discovery(),
        })
    }

    /// This body as a planet or moon, or [`None`] where the dump has not
    /// got one.
    fn planet(&self) -> Option<Planet> {
        let class = planet_class(self.sub_type.as_deref()?)?;
        Some(Planet {
            id: self.body_id,
            name: self.name.clone(),
            ty: Some(BodyType::Planet),
            distance_from_arrival: self.distance_to_arrival.map(|ls| ls as f32),
            parents: self.parents.clone(),
            planet_class: class.to_owned(),
            tidal_lock: self.rotational_period_tidally_locked,
            mass: self.earth_masses? as f32,
            radius: (self.radius? * KM) as f32,
            gravity: (self.gravity? * GRAVITY) as f32,
            temperature: self.surface_temperature.map(|k| k as f32),
            surface: self.surface(class),
            orbit: self.orbit()?,
            spin: self.spin()?,
            discovery: discovery(),
        })
    }

    /// What there is to stand on, or [`None`] where there is nothing or
    /// nobody has said what it is.
    ///
    /// A giant has no surface whatever else the dump carries for it. For
    /// everything else the solid composition decides, being the one of
    /// [`Surface`]'s three defining figures the dump may leave out: 3.2%
    /// of the bodies with a surface go without it, and their landability
    /// and materials go with it, there being no way to say a surface of
    /// unstated composition here.
    fn surface(&self, class: &str) -> Option<Surface> {
        if is_giant(class) {
            return None;
        }
        let composition = self.solid_composition.as_ref()?;
        let fraction = PERCENT as f32;
        Some(Surface {
            atmosphere_type: atmosphere_of(self.atmosphere_type.as_deref()),
            pressure: (self.surface_pressure? * ATMOSPHERE) as f32,
            composition: Composition {
                ice: composition.ice * fraction,
                rock: composition.rock * fraction,
                metal: composition.metal * fraction,
            },
            landable: self.is_landable.unwrap_or(false),
            // The game writes the atmosphere twice, once as prose for a
            // commander to read and once as what it is made of. The dump
            // keeps only the second, so there is no prose to carry.
            atmosphere: None,
            volcanism: self.volcanism_type.as_deref().and_then(volcanism_of),
            terraform_state: match self.terraforming_state.as_deref() {
                None | Some("Not terraformable") => None,
                Some(state) => Some(state.to_owned()),
            },
            materials: self
                .materials
                .iter()
                .flatten()
                .map(|(name, percent)| Material {
                    name: name.to_lowercase(),
                    percent: *percent,
                })
                .collect(),
        })
    }

    /// The path this body takes, or [`None`] where it goes round nothing
    /// or nobody has worked out where.
    fn orbit(&self) -> Option<Orbit> {
        Some(Orbit {
            semi_major_axis: (self.semi_major_axis? * AU) as f32,
            eccentricity: self.orbital_eccentricity? as f32,
            orbital_inclination: self.orbital_inclination? as f32,
            periapsis: self.arg_of_periapsis? as f32,
            orbital_period: (self.orbital_period? * DAY) as f32,
            ascending_node: self.ascending_node.map(|deg| deg as f32),
            mean_anomaly: self.mean_anomaly.map(|deg| deg as f32),
        })
    }

    /// How this body turns, or [`None`] where the dump does not say.
    fn spin(&self) -> Option<Spin> {
        Some(Spin {
            period: (self.rotational_period? * DAY) as f32,
            tilt: self.axial_tilt? as f32,
        })
    }

    /// Where in its class a star sits, off the trailing number of the
    /// dump's `spectralClass`: `"M1"` is an M1.
    ///
    /// Zero where there is no number to read, which is what the game
    /// writes for a class it does not subdivide.
    fn subclass(&self) -> i16 {
        self.spectral_class
            .as_deref()
            .map(|class| {
                class.trim_start_matches(|c: char| !c.is_ascii_digit())
            })
            .and_then(|number| number.parse().ok())
            .unwrap_or(0)
    }
}

/// Whether `body` is the record to keep of a star `standing` is the other
/// record of: nearer to the arrival point, or the lower `bodyId` where
/// they stand the same distance off.
///
/// A body with no distance measured stands further off than one with any,
/// and two of them are as far off as each other. The tie is broken rather
/// than left to the order the dump happened to list them in, because the
/// database's own upsert breaks it the same way and two derivations that
/// agreed only where a dump is sorted would be two indexes.
fn nearer(body: &Body, standing: &Body) -> bool {
    match (body.distance_to_arrival, standing.distance_to_arrival) {
        (Some(near), Some(far)) if near != far => near < far,
        (Some(_), None) => true,
        (None, Some(_)) => false,
        _ => body.body_id < standing.body_id,
    }
}

/// What a dumped body has had done to it before now.
///
/// The dump says nothing on the subject, and the journal insists on an
/// answer. A body in the dump was reported by somebody, so it was
/// discovered; nobody says it was mapped. Read back,
/// `galos_index::merge::discovered_at` answers [`None`] for a body already
/// discovered, so an import claims none of the galaxy for itself.
const fn discovery() -> Discovery {
    Discovery { discovered: true, mapped: false }
}

/// The game's `PlanetClass` for a dump's `subType`, or [`None`] where what
/// is there is not a planet.
///
/// The two name the same eighteen bodies and spell none of them alike. An
/// unlisted value answers [`None`]: the schema is a closed `enum`, so a
/// value not in it means the dump's format moved, and a class stored under
/// a spelling the game never writes would not meet the journal's own rows
/// for the same body.
pub fn planet_class(sub_type: &str) -> Option<&'static str> {
    Some(match sub_type {
        "Ammonia world" => "Ammonia world",
        "Class I gas giant" => "Sudarsky class I gas giant",
        "Class II gas giant" => "Sudarsky class II gas giant",
        "Class III gas giant" => "Sudarsky class III gas giant",
        "Class IV gas giant" => "Sudarsky class IV gas giant",
        "Class V gas giant" => "Sudarsky class V gas giant",
        "Earth-like world" => "Earthlike body",
        "Gas giant with ammonia-based life" => {
            "Gas giant with ammonia based life"
        }
        "Gas giant with water-based life" => "Gas giant with water based life",
        "Helium gas giant" => "Helium gas giant",
        "Helium-rich gas giant" => "Helium rich gas giant",
        "High metal content world" => "High metal content body",
        "Icy body" => "Icy body",
        "Metal-rich body" => "Metal rich body",
        "Rocky Ice world" => "Rocky ice body",
        "Rocky body" => "Rocky body",
        "Water giant" => "Water giant",
        "Water world" => "Water world",
        _ => return None,
    })
}

/// Whether a class of body is one there is no standing on.
///
/// Ten of the eighteen: the five Sudarsky classes, the two helium ones,
/// the two with life in them, and the water giant, which is the only one
/// the game does not call a gas giant outright.
fn is_giant(planet_class: &str) -> bool {
    planet_class.ends_with("gas giant") || planet_class == "Water giant"
}

/// What an atmosphere is made of, out of the dump's prose for one.
///
/// The dump glues how thick the atmosphere is onto what it is made of --
/// `"Hot thick Carbon dioxide-rich"` -- and the journal keeps only the
/// second, which is what this reads. How thick it is has nowhere to go.
///
/// A value the schema does not list is kept as
/// [`AtmosphereType::Unknown`], stripped of its thickness, rather than
/// dropped.
pub fn atmosphere_of(atmosphere_type: Option<&str>) -> AtmosphereType {
    let Some(prose) = atmosphere_type else {
        return AtmosphereType::None;
    };
    let made_of = ["Hot thick ", "Hot thin ", "Hot ", "Thick ", "Thin "]
        .iter()
        .find_map(|thickness| prose.strip_prefix(thickness))
        .unwrap_or(prose);
    match made_of {
        "Ammonia" => AtmosphereType::Ammonia,
        "Ammonia and Oxygen" => AtmosphereType::AmmoniaOxygen,
        "Ammonia-rich" => AtmosphereType::AmmoniaRich,
        "Argon" => AtmosphereType::Argon,
        "Argon-rich" => AtmosphereType::ArgonRich,
        "Carbon dioxide" => AtmosphereType::CarbonDioxide,
        "Carbon dioxide-rich" => AtmosphereType::CarbonDioxideRich,
        "Helium" => AtmosphereType::Helium,
        "Metallic vapour" => AtmosphereType::MetallicVapour,
        "Methane" => AtmosphereType::Methane,
        "Methane-rich" => AtmosphereType::MethaneRich,
        "Neon" => AtmosphereType::Neon,
        "Neon-rich" => AtmosphereType::NeonRich,
        "Nitrogen" => AtmosphereType::Nitrogen,
        "No atmosphere" => AtmosphereType::None,
        "Oxygen" => AtmosphereType::Oxygen,
        "Silicate vapour" => AtmosphereType::SilicateVapour,
        "Suitable for water-based life" => AtmosphereType::EarthLike,
        "Sulphur dioxide" => AtmosphereType::SulphurDioxide,
        "Water" => AtmosphereType::Water,
        "Water-rich" => AtmosphereType::WaterRich,
        moved => AtmosphereType::Unknown(moved.to_owned()),
    }
}

/// The game's volcanism for a dump's `volcanismType`, or [`None`] where
/// there is none.
///
/// The game writes it in lower case with the word after it -- `"minor
/// metallic magma volcanism"` -- and the dump in title case without, which
/// is the whole of the difference across the twenty-one kinds either
/// names.
pub fn volcanism_of(volcanism_type: &str) -> Option<String> {
    match volcanism_type {
        "No volcanism" => None,
        kind => Some(format!("{} volcanism", kind.to_lowercase())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every planet `galaxy.schema.json` lists, off the vendored copy of
    /// it rather than a transcription — a value missed here would drop a
    /// whole family of world from every import, silently. See
    /// [`crate::schema`].
    fn planets() -> Vec<String> {
        crate::schema::sub_types("Planet")
    }

    /// Every planet the schema lists has a class the game writes, and no
    /// two share one.
    #[test]
    fn every_planet_the_schema_lists_is_mapped() {
        let planets = planets();
        assert_eq!(planets.len(), 18, "the schema's planet list moved");

        let mut classes: Vec<_> = planets
            .iter()
            .map(|planet| {
                planet_class(planet)
                    .unwrap_or_else(|| panic!("{planet} has no class"))
            })
            .collect();
        classes.sort_unstable();
        classes.dedup();
        assert_eq!(classes.len(), planets.len());
    }

    /// A star's prose is not a planet's, and neither answers the other.
    #[test]
    fn a_star_is_not_a_planet() {
        assert_eq!(planet_class("M (Red dwarf) Star"), None);
        assert_eq!(class_of("Rocky body"), None);
    }

    /// How thick an atmosphere is does not change what it is made of.
    #[test]
    fn thickness_is_read_past() {
        for prose in [
            "Carbon dioxide",
            "Hot Carbon dioxide",
            "Thin Carbon dioxide",
            "Thick Carbon dioxide",
            "Hot thin Carbon dioxide",
            "Hot thick Carbon dioxide",
        ] {
            assert_eq!(
                atmosphere_of(Some(prose)),
                AtmosphereType::CarbonDioxide,
                "{prose}",
            );
        }
    }

    /// A body with no atmosphere and a body nobody has said read alike,
    /// because the game writes one value for both.
    #[test]
    fn nothing_to_breathe_is_none() {
        assert_eq!(atmosphere_of(None), AtmosphereType::None);
        assert_eq!(atmosphere_of(Some("No atmosphere")), AtmosphereType::None);
    }

    /// No volcanism is nothing rather than a kind of volcanism.
    #[test]
    fn no_volcanism_is_nothing() {
        assert_eq!(volcanism_of("No volcanism"), None);
        assert_eq!(
            volcanism_of("Minor Metallic Magma").as_deref(),
            Some("minor metallic magma volcanism"),
        );
    }
}
