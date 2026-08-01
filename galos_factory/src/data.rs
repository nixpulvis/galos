//! Static game data: items, recipes, buildings. Authored in RON files under
//! `data/`, embedded at compile time, interned to dense indices at load.
//!
//! The item set is a superset of E:D's commodities: items with `ed: true`
//! keep their EDDN internal id and join live `listings.name` directly;
//! galos-unique items (production grades, invented intermediates) price
//! purely from `base_price`.

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const ITEMS_RON: &str = include_str!("../data/items.ron");
pub const RECIPES_RON: &str = include_str!("../data/recipes.ron");
pub const BUILDINGS_RON: &str = include_str!("../data/buildings.ron");

/// Dense index into [`StaticData::items`].
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct ItemId(pub u16);

/// Dense index into [`StaticData::recipes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecipeId(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Category {
    Mineral,
    Metal,
    Chemical,
    Food,
    Component,
    Machinery,
    Tech,
    Consumer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuildingKind {
    Extractor,
    FuelScoop,
    Refinery,
    Assembler,
    PowerPlant,
    SolarArray,
    Geothermal,
    StorageModule,
}

/// Where a building may be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SiteKind {
    Surface,
    Orbital,
    Any,
}

/// Extra requirements a recipe places on its host station's environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Req {
    /// Host body must have a deposit of this item (by string id in RON,
    /// interned to the deposit item at load).
    Deposit(String),
    /// Station must orbit a scoopable star.
    ScoopableStar,
    /// Host body must have volcanism.
    Volcanism,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ItemDefRaw {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub tier: u8,
    pub base_price: u32,
    /// True when this id is a real EDDN commodity name.
    pub ed: bool,
}

#[derive(Clone, Debug, Deserialize)]
pub struct RecipeDefRaw {
    pub id: String,
    pub building: BuildingKind,
    #[serde(default)]
    pub inputs: Vec<(String, u32)>,
    #[serde(default)]
    pub outputs: Vec<(String, u32)>,
    pub ticks: u32,
    /// Positive = consumes power, negative = generates.
    pub power_mw: i32,
    #[serde(default)]
    pub requires: Vec<Req>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BuildingDefRaw {
    pub kind: BuildingKind,
    #[serde(default)]
    pub cost: Vec<(String, u32)>,
    #[serde(default)]
    pub credits_cost: u32,
    pub site: SiteKind,
    /// Maintenance items consumed per [`UPKEEP_PERIOD`] ticks.
    #[serde(default)]
    pub upkeep: Vec<(String, u32)>,
}

/// Ticks between building-maintenance and station life-support charges.
pub const UPKEEP_PERIOD: u64 = 100;

#[derive(Clone, Debug)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub tier: u8,
    pub base_price: u32,
    pub ed: bool,
}

#[derive(Clone, Debug)]
pub struct RecipeDef {
    pub id: String,
    pub building: BuildingKind,
    pub inputs: Vec<(ItemId, u32)>,
    pub outputs: Vec<(ItemId, u32)>,
    pub ticks: u32,
    pub power_mw: i32,
    pub requires: Vec<Req>,
}

#[derive(Clone, Debug)]
pub struct BuildingDef {
    pub kind: BuildingKind,
    pub cost: Vec<(ItemId, u32)>,
    pub credits_cost: u32,
    pub site: SiteKind,
    pub upkeep: Vec<(ItemId, u32)>,
}

#[derive(Resource, Clone, Debug)]
pub struct StaticData {
    pub items: Vec<ItemDef>,
    pub recipes: Vec<RecipeDef>,
    pub buildings: Vec<BuildingDef>,
    by_name: HashMap<String, ItemId>,
    recipe_by_name: HashMap<String, RecipeId>,
}

impl StaticData {
    pub fn load() -> Result<Self, String> {
        Self::parse(ITEMS_RON, RECIPES_RON, BUILDINGS_RON)
    }

    pub fn parse(items: &str, recipes: &str, buildings: &str) -> Result<Self, String> {
        let items: Vec<ItemDefRaw> =
            ron::from_str(items).map_err(|e| format!("items.ron: {e}"))?;
        let recipes: Vec<RecipeDefRaw> =
            ron::from_str(recipes).map_err(|e| format!("recipes.ron: {e}"))?;
        let buildings: Vec<BuildingDefRaw> =
            ron::from_str(buildings).map_err(|e| format!("buildings.ron: {e}"))?;

        let mut by_name = HashMap::new();
        let items: Vec<ItemDef> = items
            .into_iter()
            .map(|raw| ItemDef {
                id: raw.id,
                name: raw.name,
                category: raw.category,
                tier: raw.tier,
                base_price: raw.base_price,
                ed: raw.ed,
            })
            .collect();
        for (i, item) in items.iter().enumerate() {
            if by_name.insert(item.id.clone(), ItemId(i as u16)).is_some() {
                return Err(format!("duplicate item id `{}`", item.id));
            }
        }

        let intern = |list: &[(String, u32)], ctx: &str| -> Result<Vec<(ItemId, u32)>, String> {
            list.iter()
                .map(|(name, n)| {
                    by_name
                        .get(name)
                        .copied()
                        .map(|id| (id, *n))
                        .ok_or_else(|| format!("{ctx}: unknown item `{name}`"))
                })
                .collect()
        };

        let mut recipe_by_name = HashMap::new();
        let recipes: Vec<RecipeDef> = recipes
            .into_iter()
            .map(|raw| {
                Ok(RecipeDef {
                    inputs: intern(&raw.inputs, &raw.id)?,
                    outputs: intern(&raw.outputs, &raw.id)?,
                    id: raw.id,
                    building: raw.building,
                    ticks: raw.ticks,
                    power_mw: raw.power_mw,
                    requires: raw.requires,
                })
            })
            .collect::<Result<_, String>>()?;
        for (i, recipe) in recipes.iter().enumerate() {
            if recipe_by_name.insert(recipe.id.clone(), RecipeId(i as u16)).is_some() {
                return Err(format!("duplicate recipe id `{}`", recipe.id));
            }
        }

        let buildings: Vec<BuildingDef> = buildings
            .into_iter()
            .map(|raw| {
                Ok(BuildingDef {
                    cost: intern(&raw.cost, "building cost")?,
                    upkeep: intern(&raw.upkeep, "building upkeep")?,
                    kind: raw.kind,
                    credits_cost: raw.credits_cost,
                    site: raw.site,
                })
            })
            .collect::<Result<_, String>>()?;

        let data = StaticData { items, recipes, buildings, by_name, recipe_by_name };
        data.validate()?;
        Ok(data)
    }

    pub fn item(&self, id: ItemId) -> &ItemDef {
        &self.items[id.0 as usize]
    }

    pub fn recipe(&self, id: RecipeId) -> &RecipeDef {
        &self.recipes[id.0 as usize]
    }

    pub fn item_by_name(&self, name: &str) -> Option<ItemId> {
        self.by_name.get(name).copied()
    }

    pub fn recipe_by_name(&self, name: &str) -> Option<RecipeId> {
        self.recipe_by_name.get(name).copied()
    }

    pub fn building(&self, kind: BuildingKind) -> &BuildingDef {
        self.buildings
            .iter()
            .find(|b| b.kind == kind)
            .expect("validated: every kind has a def")
    }

    pub fn recipes_for(&self, kind: BuildingKind) -> impl Iterator<Item = (RecipeId, &RecipeDef)> {
        self.recipes
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.building == kind)
            .map(|(i, r)| (RecipeId(i as u16), r))
    }

    /// Structural validation: every reference resolves, every non-raw item is
    /// producible, everything is reachable from extraction.
    fn validate(&self) -> Result<(), String> {
        for recipe in &self.recipes {
            for req in &recipe.requires {
                if let Req::Deposit(name) = req {
                    if self.item_by_name(name).is_none() {
                        return Err(format!("{}: unknown deposit item `{name}`", recipe.id));
                    }
                }
            }
        }

        for kind in [
            BuildingKind::Extractor,
            BuildingKind::FuelScoop,
            BuildingKind::Refinery,
            BuildingKind::Assembler,
            BuildingKind::PowerPlant,
            BuildingKind::SolarArray,
            BuildingKind::Geothermal,
            BuildingKind::StorageModule,
        ] {
            if !self.buildings.iter().any(|b| b.kind == kind) {
                return Err(format!("no building def for {kind:?}"));
            }
        }

        // Reachability: start from extracted items, close over recipes.
        let mut producible: Vec<bool> = vec![false; self.items.len()];
        for recipe in &self.recipes {
            if recipe.inputs.is_empty() {
                for (out, _) in &recipe.outputs {
                    producible[out.0 as usize] = true;
                }
            }
        }
        loop {
            let mut changed = false;
            for recipe in &self.recipes {
                if recipe.inputs.iter().all(|(i, _)| producible[i.0 as usize]) {
                    for (out, _) in &recipe.outputs {
                        if !producible[out.0 as usize] {
                            producible[out.0 as usize] = true;
                            changed = true;
                        }
                    }
                }
            }
            if !changed {
                break;
            }
        }
        for (i, item) in self.items.iter().enumerate() {
            if !producible[i] {
                return Err(format!("item `{}` is not producible from extraction", item.id));
            }
        }
        Ok(())
    }
}
