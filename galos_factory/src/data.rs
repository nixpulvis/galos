//! Static game data: items, recipes, buildings. Authored in RON files under
//! `data/`, embedded at compile time, interned to dense indices at load.
//!
//! The item set is a superset of E:D's commodities: items with `ed: true`
//! keep their EDDN internal id and join live `listings.name` directly;
//! galos-unique items (production grades, invented intermediates) price
//! purely from `base_price`.
//!
//! Definitions that reference items are generic over how they do so: RON
//! parses them as `Def<String>`, [`StaticData::parse`] resolves them once
//! into `Def<ItemId>` for the sim to use. One type per concept, no
//! parallel "raw" mirror.

use bevy::prelude::Resource;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub const ITEMS_RON: &str = include_str!("../data/items.ron");
pub const RECIPES_RON: &str = include_str!("../data/recipes.ron");
pub const BUILDINGS_RON: &str = include_str!("../data/buildings.ron");

/// Ticks between building-maintenance and station life-support charges.
pub const UPKEEP_PERIOD: u64 = 100;

/// Dense index into [`StaticData::items`].
#[derive(
    Clone,
    Copy,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
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

impl BuildingKind {
    pub const ALL: [BuildingKind; 8] = [
        BuildingKind::Extractor,
        BuildingKind::FuelScoop,
        BuildingKind::Refinery,
        BuildingKind::Assembler,
        BuildingKind::PowerPlant,
        BuildingKind::SolarArray,
        BuildingKind::Geothermal,
        BuildingKind::StorageModule,
    ];
}

/// Where a building may be installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum SiteKind {
    Surface,
    Orbital,
    Any,
}

/// Extra requirements a recipe places on its host station's environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Req<I = ItemId> {
    /// Host body must have a deposit of this item.
    Deposit(I),
    /// Station must orbit a scoopable star.
    ScoopableStar,
    /// Host body must have volcanism.
    Volcanism,
}

impl Req<String> {
    fn resolve(self, items: &Items) -> Result<Req, String> {
        Ok(match self {
            Req::Deposit(name) => Req::Deposit(items.id(&name, "deposit")?),
            Req::ScoopableStar => Req::ScoopableStar,
            Req::Volcanism => Req::Volcanism,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ItemDef {
    pub id: String,
    pub name: String,
    pub category: Category,
    pub tier: u8,
    pub base_price: u32,
    /// True when this id is a real EDDN commodity name (joins live
    /// `listings.name`); false for galos-unique production grades.
    pub ed: bool,
}

/// A production step. `I` is `String` as authored, [`ItemId`] once loaded.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecipeDef<I = ItemId> {
    pub id: String,
    pub building: BuildingKind,
    #[serde(default)]
    pub inputs: Vec<(I, u32)>,
    #[serde(default)]
    pub outputs: Vec<(I, u32)>,
    pub ticks: u32,
    /// Positive = consumes power, negative = generates.
    pub power_mw: i32,
    #[serde(default)]
    pub requires: Vec<Req<I>>,
}

impl RecipeDef<String> {
    fn resolve(self, items: &Items) -> Result<RecipeDef, String> {
        Ok(RecipeDef {
            inputs: items.pairs(self.inputs, &self.id)?,
            outputs: items.pairs(self.outputs, &self.id)?,
            requires: self
                .requires
                .into_iter()
                .map(|req| req.resolve(items))
                .collect::<Result<_, _>>()?,
            id: self.id,
            building: self.building,
            ticks: self.ticks,
            power_mw: self.power_mw,
        })
    }
}

/// What a building costs to raise and to keep standing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BuildingDef<I = ItemId> {
    pub kind: BuildingKind,
    #[serde(default)]
    pub cost: Vec<(I, u32)>,
    #[serde(default)]
    pub credits_cost: u32,
    pub site: SiteKind,
    /// Maintenance items consumed per [`UPKEEP_PERIOD`] ticks.
    #[serde(default)]
    pub upkeep: Vec<(I, u32)>,
}

impl BuildingDef<String> {
    fn resolve(self, items: &Items) -> Result<BuildingDef, String> {
        let ctx = format!("{:?}", self.kind);
        Ok(BuildingDef {
            cost: items.pairs(self.cost, &ctx)?,
            upkeep: items.pairs(self.upkeep, &ctx)?,
            kind: self.kind,
            credits_cost: self.credits_cost,
            site: self.site,
        })
    }
}

/// The interned item table, used while resolving the other definitions.
struct Items {
    by_name: HashMap<String, ItemId>,
}

impl Items {
    fn id(&self, name: &str, ctx: &str) -> Result<ItemId, String> {
        self.by_name
            .get(name)
            .copied()
            .ok_or_else(|| format!("{ctx}: unknown item `{name}`"))
    }

    fn pairs(
        &self,
        list: Vec<(String, u32)>,
        ctx: &str,
    ) -> Result<Vec<(ItemId, u32)>, String> {
        list.into_iter()
            .map(|(name, qty)| Ok((self.id(&name, ctx)?, qty)))
            .collect()
    }
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

    pub fn parse(
        items: &str,
        recipes: &str,
        buildings: &str,
    ) -> Result<Self, String> {
        let items: Vec<ItemDef> =
            ron::from_str(items).map_err(|e| format!("items.ron: {e}"))?;
        let recipes: Vec<RecipeDef<String>> =
            ron::from_str(recipes).map_err(|e| format!("recipes.ron: {e}"))?;
        let buildings: Vec<BuildingDef<String>> = ron::from_str(buildings)
            .map_err(|e| format!("buildings.ron: {e}"))?;

        let mut by_name = HashMap::new();
        for (i, item) in items.iter().enumerate() {
            if by_name.insert(item.id.clone(), ItemId(i as u16)).is_some() {
                return Err(format!("duplicate item id `{}`", item.id));
            }
        }
        let table = Items { by_name };

        let recipes: Vec<RecipeDef> = recipes
            .into_iter()
            .map(|recipe| recipe.resolve(&table))
            .collect::<Result<_, _>>()?;
        let buildings: Vec<BuildingDef> = buildings
            .into_iter()
            .map(|building| building.resolve(&table))
            .collect::<Result<_, _>>()?;

        let mut recipe_by_name = HashMap::new();
        for (i, recipe) in recipes.iter().enumerate() {
            if recipe_by_name
                .insert(recipe.id.clone(), RecipeId(i as u16))
                .is_some()
            {
                return Err(format!("duplicate recipe id `{}`", recipe.id));
            }
        }

        let data = StaticData {
            items,
            recipes,
            buildings,
            by_name: table.by_name,
            recipe_by_name,
        };
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

    pub fn recipes_for(
        &self,
        kind: BuildingKind,
    ) -> impl Iterator<Item = (RecipeId, &RecipeDef)> {
        self.recipes
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.building == kind)
            .map(|(i, r)| (RecipeId(i as u16), r))
    }

    /// Structural validation: every building kind is defined and every item
    /// is reachable from extraction. (Item references resolved during
    /// parsing, so unknown ids are already rejected.)
    fn validate(&self) -> Result<(), String> {
        for kind in BuildingKind::ALL {
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
                return Err(format!(
                    "item `{}` is not producible from extraction",
                    item.id
                ));
            }
        }
        Ok(())
    }
}
