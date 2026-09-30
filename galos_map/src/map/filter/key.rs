//! How the color key groups its rows: a top tier an axis, and groups under it
//!
//! The key does not list every value flat. Allegiance leads with the three
//! powers and folds the rest under Other; government groups by the color each
//! is drawn in, seven of them sharing red; security is five values and lists
//! them. Only the allegiance split is written by hand. The government and
//! security groups are built by walking every bucket through the mapping a
//! mark is painted by ([`ColorBy::hue_of`]), so the key and the map cannot
//! disagree about which color means what.
//!
//! Model, not drawing. The Filter tab draws these as rows, the color row as
//! one chip a tier and the mini legend as one line a tier; all three read the
//! same list.

use super::mask::Mask;
use crate::map::galaxy::spawn::{ColorBy, Hue};
use elite_journal::{Allegiance, Government, system::Security};
use galos_index::read::inhabited::{Bucketed, Inhabited};

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
/// none" has no use for two gray rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: &'static str,
    pub hue: Hue,
    pub buckets: Vec<usize>,
}

impl Item {
    /// How many of `held`'s colonies this counts, summed over its buckets
    pub fn count(&self, axis: ColorBy, held: &Inhabited) -> u64 {
        let counts = axis.counts(held);
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

    /// How many of `held`'s colonies it counts
    pub fn count(&self, axis: ColorBy, held: &Inhabited) -> u64 {
        self.items().iter().map(|item| item.count(axis, held)).sum()
    }
}

/// The key's rows for `axis`, top tier first
pub fn tiers(axis: ColorBy) -> Vec<Tier> {
    match axis {
        ColorBy::Allegiance => allegiance(),
        ColorBy::Government => by_hue(axis, &GOVERNMENT_HUES),
        ColorBy::Security => items(axis).into_iter().map(Tier::Item).collect(),
    }
}

/// How many values `mask` hides along `axis`, as the color row's summary
/// counts them
pub fn hidden_values(axis: ColorBy, mask: &Mask) -> usize {
    tiers(axis)
        .iter()
        .flat_map(|tier| tier.items().to_vec())
        .filter(|item| item.hidden(axis, mask) == Hidden::All)
        .count()
}

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

/// Every value of `axis`, one item each, in bucket order, the unreported
/// bucket folded into the axis's None
fn items(axis: ColorBy) -> Vec<Item> {
    let none = axis.buckets() - 1;
    (1..axis.buckets())
        .map(|bucket| {
            let mut buckets = vec![bucket];
            if bucket == none {
                buckets.insert(0, 0);
            }
            Item {
                name: value_name(axis, bucket),
                hue: axis.hue_of(bucket),
                buckets,
            }
        })
        .collect()
}

/// `axis`'s values grouped by the color each is drawn in, in `order`
///
/// A color only one value is drawn in is that value's own row rather than a
/// group of one.
fn by_hue(axis: ColorBy, order: &[Hue]) -> Vec<Tier> {
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
                    name: hue_name(*hue),
                    hue: Some(*hue),
                    collapsible: false,
                    items: members,
                }),
            }
        })
        .collect()
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
    let find = |value: Allegiance| {
        let bucket = Allegiance::bucket(Some(value));
        all.iter()
            .find(|item| item.buckets.contains(&bucket))
            .cloned()
            .expect("every allegiance has an item")
    };
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

/// What a bucket's value is called, as a row says it
///
/// Nothing for the unreported bucket, which has no row of its own; see
/// [`Item`].
fn value_name(axis: ColorBy, bucket: usize) -> &'static str {
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
            Some(Government::None) => "None",
            None => "",
        },
        ColorBy::Security => match Security::at(bucket) {
            Some(Security::High) => "High",
            Some(Security::Medium) => "Medium",
            Some(Security::Low) => "Low",
            Some(Security::Anarchy) => "Anarchy",
            Some(Security::None) => "None",
            None => "",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
                "None"
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
        assert_eq!(hidden_values(ColorBy::Allegiance, &mask), 7);
        assert_eq!(other.hidden(ColorBy::Allegiance, &mask), Hidden::All);
        mask.set(ColorBy::Allegiance, [0], false);
        // Unreported shown, None still hidden: Unaligned is partly hidden and
        // no longer counts.
        assert_eq!(hidden_values(ColorBy::Allegiance, &mask), 6);
        assert_eq!(
            other.hidden(ColorBy::Allegiance, &mask),
            Hidden::Some { hidden: 6, of: 7 }
        );
    }
}
