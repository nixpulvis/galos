//! How the color key groups its rows: a top tier an axis, and groups under it
//!
//! The key does not list every value flat. Allegiance leads with the three
//! powers and folds the rest under Other; government, economy and state
//! group by the color each is drawn in, seven governments sharing red; a
//! controlling power groups under the allegiance it answers to; security and
//! Powerplay standing are a handful of values each and list them. Only the
//! allegiance split and the two lists' orders are written by hand. The
//! groups are built by walking every bucket through the mapping a mark is
//! painted by ([`ColorBy::hue_of`]), so the key and the map cannot disagree
//! about which color means what.
//!
//! Model, not drawing. The Filter tab draws these as rows, the color row as
//! one chip a tier and the mini legend as one line a tier; all three read the
//! same list.

use super::mask::Held;
use super::mask::Mask;
use crate::map::galaxy::spawn::{ColorBy, Hue};
use elite_journal::prelude::{
    Allegiance, Economy, Government, Power, PowerplayState, Security, State,
};
use galos_index::prelude::StarKind;
use galos_index::read::inhabited::Bucketed;

/// Whether Independent stands beside the three powers rather than under
/// Other
///
/// By count it is the largest allegiance, so Other is mostly Independent. One
/// switch while that is still being weighed.
const INDEPENDENT_ON_TOP: bool = false;

/// One value of the key, which may count in several buckets
///
/// The unreported bucket is folded in with the axis's explicit None: both draw
/// gray, and a reader asked to tell "nothing on record" from "the game says
/// none" has no use for two gray rows. Security has no unreported bucket of
/// its own, nothing on record being anarchy, so its bucket zero is a value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: &'static str,
    pub hue: Hue,
    pub buckets: Vec<usize>,
}

impl Item {
    /// How many of `held`'s systems this counts, summed over its buckets
    pub fn count(&self, axis: ColorBy, held: &Held) -> u64 {
        let counts = axis.counted(held);
        self.buckets.iter().map(|bucket| u64::from(counts[*bucket])).sum()
    }

    /// How much of it `mask` hides
    pub fn hidden(&self, axis: ColorBy, mask: &Mask) -> Hidden {
        Hidden::of(self.buckets.iter().map(|bucket| mask.hides(axis, *bucket)))
    }
}

/// How much of a row the mask hides
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hidden {
    None,
    /// `hidden` of `of` items, neither none nor all
    Some {
        hidden: usize,
        of: usize,
    },
    All,
}

impl Hidden {
    fn of(flags: impl Iterator<Item = bool>) -> Hidden {
        let (mut hidden, mut of) = (0, 0);
        for flag in flags {
            of += 1;
            hidden += usize::from(flag);
        }
        match hidden {
            0 => Hidden::None,
            _ if hidden == of => Hidden::All,
            _ => Hidden::Some { hidden, of },
        }
    }
}

/// One line of the key's top tier
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Tier {
    /// A value standing on its own
    Item(Item),
    /// Several values under one header
    Group {
        name: &'static str,
        /// The one color every member is drawn in, [`None`] for a group of
        /// several colors, which is drawn as a pie of its members'
        hue: Option<Hue>,
        /// Whether the group folds away, closed until opened
        ///
        /// Other does, being the long tail under the three powers. A
        /// government's color does not: its members are what the reader came
        /// to toggle.
        collapsible: bool,
        items: Vec<Item>,
    },
}

impl Tier {
    /// What the tier is called
    pub fn name(&self) -> &'static str {
        match self {
            Tier::Item(item) => item.name,
            Tier::Group { name, .. } => name,
        }
    }

    /// The values under it: itself, for a value on its own
    pub fn items(&self) -> &[Item] {
        match self {
            Tier::Item(item) => std::slice::from_ref(item),
            Tier::Group { items, .. } => items,
        }
    }

    /// Every bucket it counts in
    pub fn buckets(&self) -> Vec<usize> {
        self.items()
            .iter()
            .flat_map(|item| item.buckets.iter().copied())
            .collect()
    }

    /// How many of its values `mask` hides, a value counting once however many
    /// buckets it holds
    pub fn hidden(&self, axis: ColorBy, mask: &Mask) -> Hidden {
        Hidden::of(
            self.items()
                .iter()
                .map(|item| item.hidden(axis, mask) == Hidden::All),
        )
    }

    /// How many of `held`'s systems it counts
    pub fn count(&self, axis: ColorBy, held: &Held) -> u64 {
        self.items().iter().map(|item| item.count(axis, held)).sum()
    }
}

/// The key's rows for `axis`, top tier first
pub fn tiers(axis: ColorBy) -> Vec<Tier> {
    match axis {
        ColorBy::Allegiance => allegiance(),
        ColorBy::Government => by_hue(axis, &GOVERNMENT_HUES, hue_name),
        ColorBy::Security => items(axis).into_iter().map(Tier::Item).collect(),
        ColorBy::Economy => by_hue(axis, &ECONOMY_HUES, hue_name),
        ColorBy::State => by_hue(axis, &STATE_HUES, hue_name),
        ColorBy::Power => by_hue(axis, &POWER_HUES, power_bloc),
        ColorBy::PowerplayState => in_order(axis, &POWERPLAY_ORDER),
        ColorBy::StarClass => by_hue(axis, &STAR_HUES, star_group),
    }
}

/// How many values `mask` hides along `axis`, as the color row's summary
/// counts them: the values the key lists for `held`, so one it leaves out is
/// not counted as hidden. See [`held_tiers`].
pub fn hidden_values(axis: ColorBy, mask: &Mask, held: Option<&Held>) -> usize {
    held_tiers(axis, held)
        .iter()
        .flat_map(|tier| tier.items().to_vec())
        .filter(|item| item.hidden(axis, mask) == Hidden::All)
        .count()
}

/// The order a star's colors stand in: the main sequence hot to cool, then
/// what cannot be scooped, and nothing on record last
const STAR_HUES: [Hue; 8] = [
    Hue::Cyan,
    Hue::Blue,
    Hue::Yellow,
    Hue::Orange,
    Hue::Red,
    Hue::Magenta,
    Hue::Green,
    Hue::Grey,
];

/// The order a government's colors stand in, which is the order the key
/// lists them
///
/// By how much of the galaxy each paints, red first: the colors themselves
/// have no order, and this one reads down from the common to the rare.
const GOVERNMENT_HUES: [Hue; 8] = [
    Hue::Red,
    Hue::Cyan,
    Hue::Blue,
    Hue::Yellow,
    Hue::Orange,
    Hue::Green,
    Hue::Magenta,
    Hue::Grey,
];

/// The order an economy's colors stand in: from the ground up, the land and
/// what is dug out of it before what is made of it and what is sold
const ECONOMY_HUES: [Hue; 8] = [
    Hue::Green,
    Hue::Orange,
    Hue::Yellow,
    Hue::Cyan,
    Hue::Blue,
    Hue::Red,
    Hue::Magenta,
    Hue::Grey,
];

/// The order a state's colors stand in: the trouble first, the good times
/// after, and no state at all last
const STATE_HUES: [Hue; 8] = [
    Hue::Red,
    Hue::Orange,
    Hue::Magenta,
    Hue::Yellow,
    Hue::Blue,
    Hue::Cyan,
    Hue::Green,
    Hue::Grey,
];

/// The order a power's colors stand in, which are its allegiance's: the
/// three superpowers as allegiance leads with them, then the independents
const POWER_HUES: [Hue; 5] =
    [Hue::Red, Hue::Cyan, Hue::Green, Hue::Yellow, Hue::Grey];

/// The order Powerplay standings are listed in: down the ladder of a hold,
/// then the fights over one
const POWERPLAY_ORDER: [PowerplayState; 10] = [
    PowerplayState::Stronghold,
    PowerplayState::HomeSystem,
    PowerplayState::Fortified,
    PowerplayState::Controlled,
    PowerplayState::Exploited,
    PowerplayState::Unoccupied,
    PowerplayState::Prepared,
    PowerplayState::InPrepareRadius,
    PowerplayState::Contested,
    PowerplayState::Turmoil,
];

/// Every value of `axis`, one item each, in bucket order
///
/// Bucket zero folded into the axis's None where it is the unreported one,
/// and a value in its own right, last, where it names one: see
/// [`galos_index::read::inhabited::Bucketed`].
fn items(axis: ColorBy) -> Vec<Item> {
    let names_zero = !value_name(axis, 0).is_empty();
    let none = axis.buckets() - 1;
    let item = |bucket: usize, buckets: Vec<usize>| Item {
        name: value_name(axis, bucket),
        hue: axis.hue_of(bucket),
        buckets,
    };
    let mut items: Vec<Item> = (1..axis.buckets())
        .map(|bucket| match !names_zero && bucket == none {
            true => item(bucket, vec![0, bucket]),
            false => item(bucket, vec![bucket]),
        })
        .collect();
    if names_zero {
        items.push(item(0, vec![0]));
    }
    items
}

/// The key's rows for `axis` as the galaxy holds it: a value no colony has
/// is left out
///
/// Some values are only ever a station's or a faction's — a carrier, an
/// engineer's base — and never a whole system's, so along government five of
/// the seventeen come to nothing, and a row that counts nothing is a toggle
/// that does nothing. A group left with one member is that member's own row,
/// and one left with none goes. `held` is what the galaxy holds, [`None`]
/// while its colonies are still being read, when every value is listed.
pub fn held_tiers(axis: ColorBy, held: Option<&Held>) -> Vec<Tier> {
    let Some(held) = held else { return tiers(axis) };
    tiers(axis)
        .into_iter()
        .filter_map(|tier| match tier {
            Tier::Item(item) => {
                (item.count(axis, held) > 0).then_some(Tier::Item(item))
            }
            Tier::Group { name, hue, collapsible, items } => {
                let mut items: Vec<Item> = items
                    .into_iter()
                    .filter(|item| item.count(axis, held) > 0)
                    .collect();
                match items.len() {
                    0 => None,
                    1 => items.pop().map(Tier::Item),
                    _ => Some(Tier::Group { name, hue, collapsible, items }),
                }
            }
        })
        .collect()
}

/// `axis`'s values grouped by the color each is drawn in, in `order`, each
/// group headed by what `name` calls its color
///
/// A color only one value is drawn in is that value's own row rather than a
/// group of one.
fn by_hue(
    axis: ColorBy,
    order: &[Hue],
    name: fn(Hue) -> &'static str,
) -> Vec<Tier> {
    let values = items(axis);
    order
        .iter()
        .filter_map(|hue| {
            let mut members: Vec<Item> = values
                .iter()
                .filter(|item| item.hue == *hue)
                .cloned()
                .collect();
            members.sort_by_key(|item| item.name);
            match members.len() {
                0 => None,
                1 => members.pop().map(Tier::Item),
                _ => Some(Tier::Group {
                    name: name(*hue),
                    hue: Some(*hue),
                    collapsible: false,
                    items: members,
                }),
            }
        })
        .collect()
}

/// `axis`'s values one row each, `order` first and the named bucket zero
/// after, as nothing on record is
fn in_order<T: Bucketed + Copy>(axis: ColorBy, order: &[T]) -> Vec<Tier> {
    let all = items(axis);
    order
        .iter()
        .map(|value| T::bucket(Some(*value)))
        .chain(std::iter::once(0))
        .map(|bucket| Tier::Item(item_of(&all, bucket)))
        .collect()
}

/// The item of `all` counting `bucket`
fn item_of(all: &[Item], bucket: usize) -> Item {
    all.iter()
        .find(|item| item.buckets.contains(&bucket))
        .cloned()
        .expect("every bucket has an item")
}

/// The three powers, and everything else under Other
fn allegiance() -> Vec<Tier> {
    let axis = ColorBy::Allegiance;
    let mut top =
        vec![Allegiance::Federation, Allegiance::Empire, Allegiance::Alliance];
    if INDEPENDENT_ON_TOP {
        top.push(Allegiance::Independent);
    }
    let tail = [
        Allegiance::Independent,
        Allegiance::PlayerPilots,
        Allegiance::PilotsFederation,
        Allegiance::FrontlineSolutions,
        Allegiance::Guardian,
        Allegiance::Thargoid,
        Allegiance::None,
    ];
    let all = items(axis);
    let find =
        |value: Allegiance| item_of(&all, Allegiance::bucket(Some(value)));
    let mut tiers: Vec<Tier> =
        top.iter().map(|value| Tier::Item(find(*value))).collect();
    tiers.push(Tier::Group {
        name: "Other",
        hue: None,
        collapsible: true,
        items: tail
            .into_iter()
            // By bucket, `Allegiance`'s own `==` being hand-written.
            .filter(|value| {
                let bucket = Allegiance::bucket(Some(*value));
                !top.iter().any(|on| Allegiance::bucket(Some(*on)) == bucket)
            })
            .map(find)
            .collect(),
    });
    tiers
}

/// What a color is called, as a government group's header says it
fn hue_name(hue: Hue) -> &'static str {
    match hue {
        Hue::Green => "Green",
        Hue::Cyan => "Cyan",
        Hue::Red => "Red",
        Hue::Orange => "Orange",
        Hue::Yellow => "Yellow",
        Hue::Blue => "Blue",
        Hue::Magenta => "Magenta",
        Hue::Grey => "Gray",
    }
}

/// What a power's color is called, as its group's header says it: the
/// allegiance every power drawn in it answers to
fn power_bloc(hue: Hue) -> &'static str {
    match hue {
        Hue::Red => "Federation",
        Hue::Cyan => "Empire",
        Hue::Green => "Alliance",
        Hue::Yellow => "Independent",
        _ => hue_name(hue),
    }
}

/// What a star's color is called, as its group's header says it: the
/// classes or the kind of thing every star drawn in it is
fn star_group(hue: Hue) -> &'static str {
    match hue {
        Hue::Cyan => "O and B",
        Hue::Blue => "A and F",
        Hue::Magenta => "Remnants",
        Hue::Green => "Other stars",
        _ => hue_name(hue),
    }
}

/// What a bucket's value is called, as a row says it
///
/// Nothing for the unreported bucket, which has no row of its own; see
/// [`Item`].
pub(crate) fn value_name(axis: ColorBy, bucket: usize) -> &'static str {
    match axis {
        ColorBy::Allegiance => match Allegiance::at(bucket) {
            Some(Allegiance::Alliance) => "Alliance",
            Some(Allegiance::Empire) => "Empire",
            Some(Allegiance::Federation) => "Federation",
            Some(Allegiance::Guardian) => "Guardian",
            Some(Allegiance::Independent) => "Independent",
            Some(Allegiance::PilotsFederation) => "Pilots Federation",
            Some(Allegiance::PlayerPilots) => "Player pilots",
            Some(Allegiance::Thargoid) => "Thargoid",
            Some(Allegiance::FrontlineSolutions) => "Frontline Solutions",
            Some(Allegiance::None) => "Unaligned",
            None => "",
        },
        ColorBy::Government => match Government::at(bucket) {
            Some(Government::Anarchy) => "Anarchy",
            Some(Government::Communism) => "Communism",
            Some(Government::Confederacy) => "Confederacy",
            Some(Government::Cooperative) => "Cooperative",
            Some(Government::Corporate) => "Corporate",
            Some(Government::Democracy) => "Democracy",
            Some(Government::Dictatorship) => "Dictatorship",
            Some(Government::Feudal) => "Feudal",
            Some(Government::Patronage) => "Patronage",
            Some(Government::Prison) => "Prison",
            Some(Government::PrisonColony) => "Prison Colony",
            Some(Government::Theocracy) => "Theocracy",
            Some(Government::Engineer) => "Engineer",
            Some(Government::Carrier) => "Carrier",
            Some(Government::Megaconstruction) => "Megaconstruction",
            Some(Government::PrivateOwnership) => "Private Ownership",
            // Not "None", which read as the same thing as Uninhabited beside
            // it: these are colonies, with no government on record.
            Some(Government::None) => "No government",
            None => "",
        },
        ColorBy::Security => match Security::at(bucket) {
            Some(Security::High) => "High",
            Some(Security::Medium) => "Medium",
            Some(Security::Low) => "Low",
            Some(Security::Anarchy) => "Anarchy",
            None => "",
        },
        ColorBy::Economy => match Economy::at(bucket) {
            Some(Economy::Agriculture) => "Agriculture",
            Some(Economy::Colony) => "Colony",
            Some(Economy::Extraction) => "Extraction",
            Some(Economy::HighTech) => "High Tech",
            Some(Economy::Industrial) => "Industrial",
            Some(Economy::Military) => "Military",
            Some(Economy::Refinery) => "Refinery",
            Some(Economy::Service) => "Service",
            Some(Economy::Terraforming) => "Terraforming",
            Some(Economy::Tourism) => "Tourism",
            Some(Economy::Carrier) => "Carrier",
            Some(Economy::Prison) => "Prison",
            Some(Economy::Rescue) => "Rescue",
            Some(Economy::PrivateEnterprise) => "Private Enterprise",
            Some(Economy::Repair) => "Repair",
            Some(Economy::Undefined) => "Undefined",
            Some(Economy::None) => "No economy",
            None => "",
        },
        ColorBy::State => match State::at(bucket) {
            Some(State::Blight) => "Blight",
            Some(State::Boom) => "Boom",
            Some(State::Bust) => "Bust",
            Some(State::CivilLiberty) => "Civil Liberty",
            Some(State::CivilUnrest) => "Civil Unrest",
            Some(State::CivilWar) => "Civil War",
            Some(State::ColdWar) => "Cold War",
            Some(State::Colonisation) => "Colonisation",
            Some(State::Drought) => "Drought",
            Some(State::Election) => "Election",
            Some(State::Expansion) => "Expansion",
            Some(State::Famine) => "Famine",
            Some(State::HistoricEvent) => "Historic Event",
            Some(State::InfrastructureFailure) => "Infrastructure Failure",
            Some(State::Investment) => "Investment",
            Some(State::Lockdown) => "Lockdown",
            Some(State::NaturalDisaster) => "Natural Disaster",
            Some(State::Outbreak) => "Outbreak",
            Some(State::PirateAttack) => "Pirate Attack",
            Some(State::PublicHoliday) => "Public Holiday",
            Some(State::Retreat) => "Retreat",
            Some(State::Revolution) => "Revolution",
            Some(State::TechnologicalLeap) => "Technological Leap",
            Some(State::Terrorism) => "Terrorism",
            Some(State::TradeWar) => "Trade War",
            Some(State::War) => "War",
            Some(State::None) => "No state",
            None => "",
        },
        // Bucket zero is named along the two Powerplay axes: no power holding
        // a system is a reading, and there is no variant of its own to fold
        // it into.
        ColorBy::Power => match Power::at(bucket) {
            Some(Power::AislingDuval) => "Aisling Duval",
            Some(Power::ArchonDelaine) => "Archon Delaine",
            Some(Power::ArissaLavignyDuval) => "Arissa Lavigny-Duval",
            Some(Power::DentonPatreus) => "Denton Patreus",
            Some(Power::EdmundMahon) => "Edmund Mahon",
            Some(Power::FeliciaWinters) => "Felicia Winters",
            Some(Power::JeromeArcher) => "Jerome Archer",
            Some(Power::LiYongRui) => "Li Yong-Rui",
            Some(Power::NakatoKaine) => "Nakato Kaine",
            Some(Power::PranavAntal) => "Pranav Antal",
            Some(Power::YuriGrom) => "Yuri Grom",
            Some(Power::ZacharyHudson) => "Zachary Hudson",
            Some(Power::ZeminaTorval) => "Zemina Torval",
            None => "No power",
        },
        ColorBy::PowerplayState => match PowerplayState::at(bucket) {
            Some(PowerplayState::InPrepareRadius) => "In Prepare Radius",
            Some(PowerplayState::Prepared) => "Prepared",
            Some(PowerplayState::Exploited) => "Exploited",
            Some(PowerplayState::Contested) => "Contested",
            Some(PowerplayState::Controlled) => "Controlled",
            Some(PowerplayState::Turmoil) => "Turmoil",
            Some(PowerplayState::HomeSystem) => "Home System",
            Some(PowerplayState::Unoccupied) => "Unoccupied",
            Some(PowerplayState::Fortified) => "Fortified",
            Some(PowerplayState::Stronghold) => "Stronghold",
            None => "Not in Powerplay",
        },
        // Bucket zero is named: nothing on record is a row of its own, every
        // system having some bucket along this axis.
        ColorBy::StarClass => match StarKind::from_code(bucket as u8) {
            StarKind::O => "Class O",
            StarKind::B => "Class B",
            StarKind::A => "Class A",
            StarKind::F => "Class F",
            StarKind::G => "Class G",
            StarKind::K => "Class K",
            StarKind::M => "Class M",
            StarKind::BrownDwarf => "Brown Dwarf",
            StarKind::WhiteDwarf => "White Dwarf",
            StarKind::Neutron => "Neutron Star",
            StarKind::BlackHole => "Black Hole",
            StarKind::Carbon => "Carbon Star",
            StarKind::WolfRayet => "Wolf-Rayet",
            StarKind::Forming => "Forming Star",
            StarKind::Other => "Unusual Star",
            StarKind::Unknown => "Unknown",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use galos_index::read::inhabited::Readings;

    /// Every bucket of every axis stands in exactly one row, so there is no
    /// color the key cannot hide and none it hides twice
    #[test]
    fn every_bucket_is_in_exactly_one_row() {
        for axis in ColorBy::ALL {
            let mut seen = vec![0; axis.buckets()];
            for tier in tiers(axis) {
                for bucket in tier.buckets() {
                    seen[bucket] += 1;
                }
            }
            assert!(
                seen.iter().all(|times| *times == 1),
                "{axis:?} placed its buckets {seen:?}",
            );
        }
    }

    /// A row's swatch is the color the map paints every one of its buckets
    #[test]
    fn a_row_is_painted_the_hue_its_buckets_are() {
        for axis in ColorBy::ALL {
            for tier in tiers(axis) {
                for item in tier.items() {
                    for bucket in &item.buckets {
                        assert_eq!(
                            item.hue,
                            axis.hue_of(*bucket),
                            "{}",
                            item.name
                        );
                    }
                }
                if let Tier::Group { hue: Some(hue), items, .. } = &tier {
                    assert!(items.iter().all(|item| item.hue == *hue));
                }
            }
        }
    }

    /// Government groups by color, and a color with one value is that
    /// value's own row
    #[test]
    fn government_groups_by_color() {
        let tiers = tiers(ColorBy::Government);
        let names: Vec<&str> = tiers.iter().map(Tier::name).collect();
        assert_eq!(
            names,
            [
                "Red",
                "Corporate",
                "Blue",
                "Anarchy",
                "Cooperative",
                "Green",
                "Engineer",
                "No government"
            ]
        );
        assert_eq!(tiers[0].items().len(), 7);
    }

    /// The summary counts values, and a value of two buckets counts once
    #[test]
    fn hidden_values_count_rows_not_buckets() {
        let mut mask = Mask::default();
        let other = tiers(ColorBy::Allegiance).pop().expect("Other");
        mask.toggle(ColorBy::Allegiance, &other.buckets());
        assert_eq!(hidden_values(ColorBy::Allegiance, &mask, None), 7);
        assert_eq!(other.hidden(ColorBy::Allegiance, &mask), Hidden::All);
        mask.set(ColorBy::Allegiance, [0], false);
        // Unreported shown, None still hidden: Unaligned is partly hidden and
        // no longer counts.
        assert_eq!(hidden_values(ColorBy::Allegiance, &mask, None), 6);
        assert_eq!(
            other.hidden(ColorBy::Allegiance, &mask),
            Hidden::Some { hidden: 6, of: 7 }
        );
    }

    /// Anarchy is security's one no-security row, and it is red
    #[test]
    fn security_has_one_row_for_no_security_and_it_is_anarchy() {
        let rows = tiers(ColorBy::Security);
        let names: Vec<&str> = rows.iter().map(Tier::name).collect();
        assert_eq!(names, ["High", "Medium", "Low", "Anarchy"]);
        assert_eq!(rows[3].items()[0].hue, Hue::Red);
    }

    /// A power is listed under the allegiance it answers to, and no power
    /// holding a system is a row of its own
    #[test]
    fn power_groups_under_its_allegiance() {
        let tiers = tiers(ColorBy::Power);
        let names: Vec<&str> = tiers.iter().map(Tier::name).collect();
        assert_eq!(
            names,
            ["Federation", "Empire", "Alliance", "Independent", "No power"]
        );
        let members: Vec<&str> =
            tiers[0].items().iter().map(|item| item.name).collect();
        assert_eq!(
            members,
            ["Felicia Winters", "Jerome Archer", "Zachary Hudson"]
        );
    }

    /// A value no colony holds is not listed, and a group it leaves with
    /// one member is that member's own row
    #[test]
    fn a_value_no_colony_holds_is_not_listed() {
        let colonies = galos_index::read::inhabited::Inhabited::of_system(
            [0.; 3],
            Readings {
                allegiance: Some(Allegiance::Federation),
                government: Some(Government::Democracy),
                security: Some(Security::High),
                ..Readings::default()
            },
        );
        let held = Held { colonies, stars: [0; StarKind::COUNT] };
        let rows = held_tiers(ColorBy::Government, Some(&held));
        assert_eq!(
            rows,
            [Tier::Item(Item {
                name: "Democracy",
                hue: Hue::Blue,
                buckets: vec![Government::bucket(Some(Government::Democracy))],
            })]
        );

        // Hiding every government counts only the one listed.
        let mut mask = Mask::default();
        mask.hide_all(ColorBy::Government);
        assert_eq!(hidden_values(ColorBy::Government, &mask, Some(&held)), 1);
    }
}
