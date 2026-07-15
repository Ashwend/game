//! Recipe value types and id interning: [`RecipeStation`], [`RecipeCategory`],
//! [`CraftingInput`], [`RecipeDefinition`], the [`RecipeId`] interner, and the
//! queue-length cap. The `REGISTERED_RECIPES` slice and its lookups live in
//! [`super::registry`].

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use crate::items::DeployableKind;

use super::registry::REGISTERED_RECIPES;

/// Crafting station a recipe needs to be in range of. Cheap value type so
/// it lives next to the recipe definition without a registry lookup. The
/// server resolves "in range" by walking the local player's nearby placed
/// deployables; client UIs surface the requirement so the player knows
/// what to build first.
///
/// Furnace-style "machine inside an entity" operations are intentionally
/// not represented here, smelting happens inside the furnace's own UI,
/// not via the recipe registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeStation {
    /// Hand-craftable, no nearby structure required.
    None,
    /// A `DeployableKind::Workbench { tier }` with `tier >= min_tier`
    /// must be within its `station_radius` of the player.
    Workbench { min_tier: u8 },
}

impl RecipeStation {
    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "Hand craft",
            Self::Workbench { .. } => "Workbench",
        }
    }

    /// True when `kind` (a placed deployable) satisfies this requirement.
    /// Workbench tier 2 also satisfies a Workbench tier 1 requirement,
    /// same as how higher-tier tools satisfy lower-tier gather rules.
    pub fn satisfied_by(self, kind: DeployableKind) -> bool {
        match (self, kind) {
            (Self::None, _) => true,
            (Self::Workbench { min_tier }, DeployableKind::Workbench { tier }) => tier >= min_tier,
            _ => false,
        }
    }
}

/// Interned identifier shared between protocol messages, server state, and
/// the UI. Same `Arc<str>` story as [`crate::items::ItemId`], clones are a
/// refcount bump and deserialized ids reuse the cached `Arc` on hits.
pub type RecipeId = Arc<str>;

/// Cap on a player's queue length. Picked high enough that early players
/// won't bump into it and low enough that a malicious client can't flood
/// the server with unbounded refund bookkeeping.
pub const MAX_CRAFTING_QUEUE_LEN: usize = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RecipeCategory {
    /// Raw-material refinements (e.g. fiber → twine, iron ore → bar).
    Materials,
    /// Equipable gather tools (hatchet, pickaxe, hammer).
    Tools,
    /// Placeable structures (workbench, furnace, …).
    Building,
    /// Catch-all so the enum doesn't need a protocol bump for one-offs.
    Misc,
    /// Dedicated melee (and later ranged) weapons. Appended last: the category
    /// only drives the browser filter chip, but keeping the enum append-only
    /// matches the save/wire discipline the other positional enums follow.
    Weapons,
    /// Worn armor sets (padded, lamellar, iron). Appended after `Weapons` for
    /// the same append-only reason: it only drives the browser filter chip.
    Armor,
    /// Blackpowder explosives (bomb, keg, satchel). Appended after
    /// `Armor` for the same append-only reason: it only drives the browser
    /// filter chip.
    Explosives,
    /// Healing consumables (the bandage). Appended after `Explosives` for the
    /// same append-only reason: it only drives the browser filter chip.
    Consumables,
}

impl RecipeCategory {
    pub const ALL: &'static [Self] = &[
        Self::Materials,
        Self::Tools,
        Self::Building,
        Self::Misc,
        Self::Weapons,
        Self::Armor,
        Self::Explosives,
        Self::Consumables,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Materials => "Materials",
            Self::Tools => "Tools",
            Self::Building => "Building",
            Self::Misc => "Misc",
            Self::Weapons => "Weapons",
            Self::Armor => "Armor",
            Self::Explosives => "Explosives",
            Self::Consumables => "Consumables",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CraftingInput {
    pub item_id: &'static str,
    pub quantity: u16,
}

impl CraftingInput {
    pub const fn new(item_id: &'static str, quantity: u16) -> Self {
        Self { item_id, quantity }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RecipeDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub category: RecipeCategory,
    pub inputs: &'static [CraftingInput],
    pub output_item: &'static str,
    pub output_quantity: u16,
    /// How long one unit of the output takes to craft, in seconds. Server
    /// converts to ticks via [`crate::protocol::SERVER_TICK_RATE_HZ`].
    pub craft_seconds: f32,
    /// Progression tier the recipe belongs to. Used only for sorting in
    /// the recipe browser today (higher tier surfaces first). Keep low
    /// numbers for raw refinements and bump it as the tech tree grows:
    /// `0` = primitive material processing (e.g. plant twine), `1` =
    /// stone-age tools, `2` = iron, and so on.
    pub tier: u8,
    /// Crafting station the player must stand next to. Defaults to
    /// `RecipeStation::None` for hand-craftable recipes.
    pub station: RecipeStation,
}

/// Intern a recipe id so identical strings produced by the deserializer share
/// a single `Arc<str>`. Mirrors [`crate::items::intern_item_id`].
pub fn intern_recipe_id(id: &str) -> RecipeId {
    let registry = interned_registry();
    if let Some(cached) = registry.read().ok().and_then(|map| map.get(id).cloned()) {
        return cached;
    }
    let fresh: Arc<str> = Arc::from(id);
    if let Ok(mut map) = registry.write() {
        if let Some(cached) = map.get(id).cloned() {
            return cached;
        }
        map.insert(Box::from(id), fresh.clone());
    }
    fresh
}

fn interned_registry() -> &'static RwLock<HashMap<Box<str>, Arc<str>>> {
    static REGISTRY: OnceLock<RwLock<HashMap<Box<str>, Arc<str>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut map = HashMap::with_capacity(REGISTERED_RECIPES.len());
        for recipe in REGISTERED_RECIPES {
            let arc: Arc<str> = Arc::from(recipe.id);
            map.insert(Box::from(recipe.id), arc);
        }
        RwLock::new(map)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crafting::PLANT_TWINE_RECIPE_ID;

    #[test]
    fn intern_returns_same_arc_for_same_id() {
        let a = intern_recipe_id(PLANT_TWINE_RECIPE_ID);
        let b = intern_recipe_id(PLANT_TWINE_RECIPE_ID);
        assert!(Arc::ptr_eq(&a, &b));
    }
}
