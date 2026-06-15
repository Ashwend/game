use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use crate::protocol::{DroppedWorldItem, ItemStack, Vec3Net};

pub const WOOD_ID: &str = "wood";
pub const STONE_ID: &str = "stone";
pub const COAL_ID: &str = "coal";
pub const IRON_ORE_ID: &str = "iron_ore";
pub const IRON_BAR_ID: &str = "iron_bar";
pub const SULFUR_ORE_ID: &str = "sulfur_ore";
pub const FIBER_ID: &str = "fiber";
pub const PLANT_TWINE_ID: &str = "plant_twine";
/// Refined wood. Raw `wood` worked into a clean structural billet at a
/// workbench, the handle stock for tier-2 tools and the building block for
/// later construction.
pub const HEWN_LOG_ID: &str = "hewn_log";
pub const BASIC_HATCHET_ID: &str = "wood_stone_hatchet";
pub const BASIC_PICKAXE_ID: &str = "wood_stone_pickaxe";
pub const IRON_HATCHET_ID: &str = "iron_hatchet";
pub const IRON_PICKAXE_ID: &str = "iron_pickaxe";
pub const WORKBENCH_T1_ID: &str = "workbench_t1";
pub const CRUDE_FURNACE_ID: &str = "crude_furnace";
/// Holdable blueprint that drives the building system: hold right click to
/// pick a piece, left click to place its ghost.
pub const BUILDING_PLAN_ID: &str = "building_plan";
/// Construction hammer: left click repairs building blocks, hold right
/// click for the upgrade / demolish wheel.
pub const HAMMER_ID: &str = "hammer";
/// Code-locked door deployable that mounts only in doorway openings.
pub const HEWN_LOG_DOOR_ID: &str = "hewn_log_door";
/// Respawn-anchor deployable crafted from plant fiber.
pub const SLEEPING_BAG_ID: &str = "sleeping_bag";
/// Placeable item containers; small is hand-craftable, large needs a
/// workbench.
pub const STORAGE_BOX_SMALL_ID: &str = "storage_box_small";
pub const STORAGE_BOX_LARGE_ID: &str = "storage_box_large";
/// Placeable light source crafted from wood + coal. Burns ~8 hours, then
/// goes dark. Mounts on the ground or the side of a wall.
pub const TORCH_ID: &str = "torch";

/// Identifier shared between `ItemStack`, `ItemMerged`, and item definitions.
/// Backed by `Arc<str>` so clones are a refcount bump instead of a heap copy.
/// Known IDs are interned to a single allocation at startup; deserialized IDs
/// are looked up against the registry and reuse the cached `Arc` on hits.
pub type ItemId = Arc<str>;

/// Returns the interned `Arc<str>` for `id`. Compile-time constants from
/// `REGISTERED_ITEMS` resolve without allocating on hits via an O(1) hash
/// lookup; unknown ids fall through to a fresh `Arc` and are cached so
/// subsequent hits also avoid allocating. Stays open to runtime-loaded items.
pub fn intern_item_id(id: &str) -> ItemId {
    let registry = interned_registry();
    if let Some(cached) = registry.read().ok().and_then(|map| map.get(id).cloned()) {
        return cached;
    }
    // Allocate outside the write lock so a contended path doesn't hold the
    // lock through the heap allocation. The double-insert window is harmless:
    // both inserts produce the same Arc<str> contents, and the second simply
    // overwrites with an Arc that hashes/compares equal.
    let fresh: Arc<str> = Arc::from(id);
    if let Ok(mut map) = registry.write() {
        // Re-check after taking the write lock, another caller may have
        // inserted between our read miss and now. Returning the cached value
        // keeps the registry's "one Arc per id" invariant in lockstep with
        // anything that already grabbed the earlier-inserted Arc.
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
        let mut map = HashMap::with_capacity(REGISTERED_ITEMS.len());
        for definition in REGISTERED_ITEMS {
            let arc: Arc<str> = Arc::from(definition.id);
            map.insert(Box::from(definition.id), arc);
        }
        RwLock::new(map)
    })
}

pub const PICKUP_RANGE: f32 = 3.4;
const PICKUP_RAY_RADIUS: f32 = 0.58;
const PICKUP_ANCHOR_HEIGHT: f32 = 0.28;

/// First-person *animation archetype* for a held item. Drives the swing
/// pose and the tool-swap lift cadence, not the mesh. Iron and stone tools
/// of the same kind share an archetype (an iron hatchet swings exactly like
/// a stone one); only their [`HeldMesh`] differs. Keeping this coarse means
/// adding a new tool material never touches the pose curves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemModel {
    Bag,
    Hatchet,
    Pickaxe,
    /// Deployable items render as the bag silhouette in the held-item
    /// slot, the actual structure mesh is what gets placed in the world.
    Deployable,
}

/// Which first-person *mesh* the registry tells the renderer to put in the
/// player's hand. Decoupled from [`ItemModel`] so a tool's look (stone vs
/// iron head) is independent of how it animates. Raw materials and
/// deployables-in-hand fall back to the generic bag silhouette. Adding a
/// new tool material is a new variant here plus one mesh handle, no pose or
/// gameplay code changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeldMesh {
    Bag,
    StoneHatchet,
    IronHatchet,
    StonePickaxe,
    IronPickaxe,
    /// Procedural construction hammer (head + haft). A candidate for the
    /// authored-glb pipeline later; the procedural stand-in keeps the
    /// registry total.
    Hammer,
    /// Rolled-up building plan scroll.
    BuildingPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolKind {
    /// No tool equipped. Synthesized via [`HANDS_TOOL`] when the active
    /// actionbar slot has no tool. Crude pickup nodes carry a
    /// `ToolRequirement` of `Hands` to mark themselves as
    /// E-pickup-only, no tool (including bare hands) can gather them
    /// by swinging. See [`crate::resources::ToolRequirement::allows`].
    Hands,
    Axe,
    Pickaxe,
    /// Construction hammer. Never gathers and never damages; its swing
    /// repairs building blocks and its held-right-click wheel upgrades or
    /// demolishes them.
    Hammer,
}

impl ToolKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Hands => "Bare hands",
            Self::Axe => "Hatchet",
            Self::Pickaxe => "Pickaxe",
            Self::Hammer => "Hammer",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolProfile {
    pub kind: ToolKind,
    pub tier: u8,
    pub gather_amount: u16,
    pub cooldown_ticks: u64,
    /// Impacts the tool survives before breaking. Only swings that
    /// connect with something (gather payout, player hit, structure
    /// hit) consume durability; whiffs are free. `None` means the tool
    /// never wears (bare hands).
    pub max_durability: Option<u32>,
    /// Raw per-swing PvP damage before armor. `0` means the tool can't
    /// damage players at all (bare hands); the combat path rejects the
    /// swing instead of landing a zero-damage hit.
    pub player_damage: u32,
}

/// Synthesized tool profile used when no actionbar item is held. The
/// server substitutes this in when the active stack carries no tool
/// definition so the gather pipeline always has a `ToolProfile` to read.
/// It's never accepted as a valid gather tool, crude nodes are E-pickup
/// only and the tool-required nodes reject Hands explicitly, but it
/// keeps the cooldown/payout math uniform across the gather path.
pub const HANDS_TOOL: ToolProfile = ToolProfile {
    kind: ToolKind::Hands,
    tier: 0,
    gather_amount: 1,
    cooldown_ticks: 10,
    max_durability: None,
    player_damage: 0,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemTint {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl ItemTint {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// What kind of structure a deployable item places. The tier travels with
/// the kind so a single `RecipeStation::Workbench { min_tier }` check can
/// match any equal-or-higher workbench in range, same idea behind tool
/// tiers (`ToolProfile`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DeployableKind {
    Workbench {
        tier: u8,
    },
    Furnace {
        tier: u8,
    },
    /// A base-building block placed via the building plan. The tier is
    /// mutable server-side (hammer upgrades); a tier change respawns the
    /// mirror entity since `Deployable` identity is immutable post-spawn.
    Building {
        piece: crate::building::BuildingPiece,
        tier: crate::building::BuildingTier,
    },
    /// Code-locked door mounted in a doorway opening. The hinge side and
    /// swing direction are fully captured by the entity's yaw (flipping a
    /// door during placement rotates it half a turn), so the kind carries
    /// no fields.
    Door,
    /// Respawn-anchor sleeping bag.
    SleepingBag,
    /// Placeable item container. `tier` 1 is the small box, 2 the large
    /// one; slot counts live in [`crate::game_balance`] and resolve via
    /// `crate::server` storage helpers.
    StorageBox {
        tier: u8,
    },
    /// Light source. `wall` records how it was placed (and is immutable
    /// after): `false` stands upright on a surface, `true` mounts on the
    /// side of a wall (the client tilts it out from the wall along the
    /// stored yaw). Carrying the mount in the kind keeps the orientation
    /// replicated for free via the immutable `Deployable` component.
    Torch {
        wall: bool,
    },
}

impl DeployableKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workbench { .. } => "Workbench",
            Self::Furnace { .. } => "Furnace",
            Self::Building { piece, .. } => piece.label(),
            Self::Door => "Hewn Log Door",
            Self::SleepingBag => "Sleeping Bag",
            Self::StorageBox { tier } => {
                if tier >= 2 {
                    "Large Storage Box"
                } else {
                    "Storage Box"
                }
            }
            Self::Torch { .. } => "Torch",
        }
    }

    /// Source of truth for what the structure is built from. The damage
    /// path uses this for the tool-vs-material multiplier and the
    /// client uses it to pick the swing surface (audio/visual chip).
    /// Building blocks change material as they're upgraded, which is the
    /// entire raid-balance lever: see the building arms in
    /// [`tool_effectiveness_pct`].
    pub const fn material(self) -> DestructibleMaterial {
        match self {
            Self::Workbench { .. } => DestructibleMaterial::Wood,
            Self::Furnace { .. } => DestructibleMaterial::Stone,
            Self::Building { tier, .. } => match tier {
                crate::building::BuildingTier::Sticks => DestructibleMaterial::Sticks,
                crate::building::BuildingTier::HewnWood => DestructibleMaterial::WoodBuilding,
                crate::building::BuildingTier::Stone => DestructibleMaterial::StoneBuilding,
            },
            Self::Door => DestructibleMaterial::WoodBuilding,
            Self::SleepingBag => DestructibleMaterial::Cloth,
            Self::StorageBox { .. } => DestructibleMaterial::Wood,
            Self::Torch { .. } => DestructibleMaterial::Wood,
        }
    }

    /// True for the entity kinds anyone may damage, regardless of who
    /// placed them. Raid targets (building blocks, doors, sleeping bags)
    /// must be damageable by non-owners or raiding can't exist; utility
    /// stations (workbench, furnace) keep the owner-only damage gate so
    /// griefers can't idly chew through someone's crafting corner.
    pub const fn raidable(self) -> bool {
        matches!(self, Self::Building { .. } | Self::Door | Self::SleepingBag)
    }
}

/// What a destructible thing is made of, for the tool-vs-material matchup
/// system. The taxonomy is deliberately coarse: wood vs stone is enough to
/// express "hatchet eats workbenches, pickaxe eats furnaces" today. New
/// materials (metal, concrete, …) slot in here as the world gains them, and
/// the single [`tool_effectiveness_pct`] table below is where their matchups
/// are declared, no per-entity special-casing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestructibleMaterial {
    Wood,
    Stone,
    /// Sticks-tier building blocks. Deliberately fragile: any proper tool
    /// tears through in a few swings.
    Sticks,
    /// Wood-tier building blocks and doors. Raidable with tools but
    /// slowly, the soft side of a base.
    WoodBuilding,
    /// Stone-tier building blocks. Immune to every tool; raiding stone
    /// waits for future siege equipment.
    StoneBuilding,
    /// Sleeping bags. Tears in a couple of hits.
    Cloth,
}

/// Central tool-vs-material effectiveness table, expressed as a percentage
/// multiplier so the server stays on integer math. This is the one place
/// that answers "how well does tool X bite material Y": matched tool ≈ 1.5×,
/// mismatched proper tool ≈ 0.5×. Every destructible-entity damage path
/// (deployables today, more later) reads through here rather than branching
/// on entity type, so balancing a matchup is a single-line edit and adding a
/// material is a single new arm. Bare hands never reach this code path
/// (they're rejected upstream); the catch-all keeps the math total.
pub fn tool_effectiveness_pct(tool: ToolKind, material: DestructibleMaterial) -> u32 {
    match (tool, material) {
        (ToolKind::Axe, DestructibleMaterial::Wood) => 150,
        (ToolKind::Pickaxe, DestructibleMaterial::Stone) => 150,
        (ToolKind::Axe, DestructibleMaterial::Stone) => 50,
        (ToolKind::Pickaxe, DestructibleMaterial::Wood) => 50,
        // Building materials, the raid-balance table. Sticks shred under
        // any proper tool; wood-tier buildings take a trickle (slow but
        // real raids); stone-tier buildings are immune to tools entirely.
        (ToolKind::Axe, DestructibleMaterial::Sticks) => 300,
        (ToolKind::Pickaxe, DestructibleMaterial::Sticks) => 200,
        (ToolKind::Axe, DestructibleMaterial::WoodBuilding) => 15,
        (ToolKind::Pickaxe, DestructibleMaterial::WoodBuilding) => 5,
        (_, DestructibleMaterial::StoneBuilding) => 0,
        // Sleeping bags tear under anything with an edge.
        (ToolKind::Axe | ToolKind::Pickaxe, DestructibleMaterial::Cloth) => 300,
        // The hammer builds, it never breaks. Repair/upgrade/demolish all
        // ride their own commands, so zero here closes the "hammer as a
        // free raid tool" hole outright.
        (ToolKind::Hammer, _) => 0,
        // Hands shouldn't reach here, but if they do treat them as
        // worst-case mismatched so they make minimal dents.
        (ToolKind::Hands, _) => 50,
    }
}

/// Footprint + health profile for items that drop into the world as
/// placed structures. Lives on `ItemDefinition` so item-aware UIs (action
/// bar, inventory tooltip) can show "placeable" affordances without a
/// separate registry, mirroring `ToolProfile`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeployableProfile {
    pub kind: DeployableKind,
    /// Spawn HP for the placed structure. Persisted in the world save.
    pub max_health: u32,
    /// Horizontal half-extent of the structure's AABB collider. The
    /// vertical extent is taken from `collider_half_height` and the
    /// collider is anchored on the ground.
    pub collider_half_width: f32,
    pub collider_half_height: f32,
    /// Range, in metres, within which a `RecipeStation` of this kind +
    /// tier is considered "in reach" for a player who placed it.
    /// `0.0` means the deployable does not act as a crafting station.
    pub station_radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ItemDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub stack_size: u16,
    pub equipable: bool,
    /// First-person animation archetype (swing pose + swap cadence).
    pub model: ItemModel,
    /// First-person mesh the renderer puts in hand. Independent of `model`
    /// so same-archetype tools (stone vs iron) can look different.
    pub held_mesh: HeldMesh,
    pub tint: ItemTint,
    pub tool: Option<ToolProfile>,
    pub deployable: Option<DeployableProfile>,
}

impl ItemDefinition {
    pub fn effective_stack_size(self) -> u16 {
        // Tools carry per-item durability, so two of them can never share a
        // slot, they're always a stack of one regardless of `stack_size`.
        // Everything else (raw materials, and placeable deployables like the
        // torch) stacks up to its declared `stack_size`.
        if self.tool.is_some() {
            1
        } else {
            self.stack_size.max(1)
        }
    }
}

pub const REGISTERED_ITEMS: &[ItemDefinition] = &[
    ItemDefinition {
        id: WOOD_ID,
        name: "Wood",
        description: "A common building material gathered from trees.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(139, 95, 56),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: STONE_ID,
        name: "Stone",
        description: "A rough stone material used for primitive tools.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(122, 128, 126),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: COAL_ID,
        name: "Coal",
        description: "A fuel-rich mineral gathered from coal nodes.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(42, 45, 48),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_ORE_ID,
        name: "Iron Ore",
        description: "Raw iron-bearing rock ready for later smelting systems.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(155, 120, 94),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: SULFUR_ORE_ID,
        name: "Sulfur Ore",
        description: "A yellow mineral gathered from sulfur nodes.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(218, 189, 73),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: FIBER_ID,
        name: "Plant Fiber",
        description: "Coarse fibers pulled from grass tufts. Used for crude bindings.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(168, 184, 96),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: PLANT_TWINE_ID,
        name: "Plant Twine",
        description: "Twisted plant fibers. The binding that holds primitive tools together.",
        stack_size: 200,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(196, 176, 110),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_BAR_ID,
        name: "Iron Bar",
        description: "Refined iron, smelted from raw ore in a furnace.",
        stack_size: 100,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(196, 198, 204),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: HEWN_LOG_ID,
        name: "Hewn Log",
        description: "Raw wood squared and worked into a clean structural billet. \
                      Handle stock for iron tools and a staple of later building.",
        stack_size: 100,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(120, 82, 48),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: BASIC_HATCHET_ID,
        name: "Stone Hatchet",
        description: "A basic wood-and-stone axe for gathering trees.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Hatchet,
        held_mesh: HeldMesh::StoneHatchet,
        tint: ItemTint::new(148, 122, 82),
        tool: Some(ToolProfile {
            kind: ToolKind::Axe,
            tier: 1,
            gather_amount: 6,
            cooldown_ticks: 6,
            max_durability: Some(crate::game_balance::STONE_TOOL_DURABILITY),
            player_damage: crate::game_balance::STONE_HATCHET_PVP_DAMAGE,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: BASIC_PICKAXE_ID,
        name: "Stone Pickaxe",
        description: "A basic wood-and-stone pickaxe for gathering ore nodes.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Pickaxe,
        held_mesh: HeldMesh::StonePickaxe,
        tint: ItemTint::new(134, 128, 112),
        tool: Some(ToolProfile {
            kind: ToolKind::Pickaxe,
            tier: 1,
            gather_amount: 6,
            cooldown_ticks: 6,
            max_durability: Some(crate::game_balance::STONE_TOOL_DURABILITY),
            player_damage: crate::game_balance::STONE_PICKAXE_PVP_DAMAGE,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: IRON_HATCHET_ID,
        name: "Iron Hatchet",
        description: "A forged iron axe head on a hewn handle. Bites twice as \
                      deep into wood as the stone hatchet.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Hatchet,
        held_mesh: HeldMesh::IronHatchet,
        tint: ItemTint::new(176, 180, 188),
        tool: Some(ToolProfile {
            kind: ToolKind::Axe,
            tier: 2,
            // Twice the stone hatchet's yield per swing. Cadence is gated by
            // the swing animation, not this cooldown (see gather.rs), so the
            // tier upgrade is felt as bigger payouts, not faster swings.
            gather_amount: 12,
            cooldown_ticks: 5,
            max_durability: Some(crate::game_balance::IRON_TOOL_DURABILITY),
            player_damage: crate::game_balance::IRON_HATCHET_PVP_DAMAGE,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: IRON_PICKAXE_ID,
        name: "Iron Pickaxe",
        description: "A forged iron head on a hewn handle. Tears ore and stone \
                      loose twice as fast as the stone pickaxe.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Pickaxe,
        held_mesh: HeldMesh::IronPickaxe,
        tint: ItemTint::new(170, 174, 182),
        tool: Some(ToolProfile {
            kind: ToolKind::Pickaxe,
            tier: 2,
            gather_amount: 12,
            cooldown_ticks: 5,
            max_durability: Some(crate::game_balance::IRON_TOOL_DURABILITY),
            player_damage: crate::game_balance::IRON_PICKAXE_PVP_DAMAGE,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: WORKBENCH_T1_ID,
        name: "Workbench lvl 1",
        description: "A sturdy table for assembling tier-1 crafted goods. \
                      Place it in the world and craft within ~5m of it.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(136, 96, 56),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Workbench { tier: 1 },
            max_health: 500,
            collider_half_width: 0.55,
            collider_half_height: 0.45,
            station_radius: 5.0,
        }),
    },
    ItemDefinition {
        id: CRUDE_FURNACE_ID,
        name: "Furnace",
        description: "A stone furnace for smelting ore into bars. \
                      Requires a workbench nearby to build.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(102, 92, 84),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Furnace { tier: 1 },
            max_health: 800,
            collider_half_width: 0.50,
            collider_half_height: 0.60,
            station_radius: 5.0,
        }),
    },
    ItemDefinition {
        id: BUILDING_PLAN_ID,
        name: "Building Plan",
        description: "Sketched construction lines on rough parchment. Hold \
                      right click to choose a piece, left click to place it.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::BuildingPlan,
        tint: ItemTint::new(204, 188, 150),
        tool: None,
        deployable: None,
    },
    ItemDefinition {
        id: HAMMER_ID,
        name: "Hammer",
        description: "A heavy construction mallet. Swing at your buildings \
                      to repair them; hold right click for upgrades.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Hatchet,
        held_mesh: HeldMesh::Hammer,
        tint: ItemTint::new(140, 110, 78),
        tool: Some(ToolProfile {
            kind: ToolKind::Hammer,
            tier: 1,
            // Hammers never gather; the profile exists so the swing
            // pipeline (cadence, durability) treats it like a tool.
            gather_amount: 0,
            cooldown_ticks: 8,
            max_durability: Some(crate::game_balance::HAMMER_DURABILITY),
            player_damage: 0,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: HEWN_LOG_DOOR_ID,
        name: "Hewn Log Door",
        description: "A heavy door of squared logs with a settable code \
                      lock. Mounts only in a doorway opening.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(112, 78, 46),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Door,
            max_health: crate::game_balance::DOOR_MAX_HP,
            collider_half_width: 0.55,
            collider_half_height: 1.1,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: SLEEPING_BAG_ID,
        name: "Sleeping Bag",
        description: "A bedroll of woven plant fiber. Place it to set a \
                      respawn point; hold E on it to rename.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(96, 122, 92),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::SleepingBag,
            max_health: crate::game_balance::SLEEPING_BAG_MAX_HP,
            collider_half_width: 0.8,
            collider_half_height: 0.12,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: TORCH_ID,
        name: "Torch",
        description: "Pitch-soaked wood that burns for hours. Place it on \
                      the ground or mount it on a wall to light the dark.",
        stack_size: 10,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(178, 116, 58),
        tool: None,
        deployable: Some(DeployableProfile {
            // `wall` is the placement default; the server overwrites it from
            // the placement command (floor vs wall mount).
            kind: DeployableKind::Torch { wall: false },
            max_health: crate::game_balance::TORCH_MAX_HP,
            collider_half_width: 0.1,
            collider_half_height: 0.3,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: STORAGE_BOX_SMALL_ID,
        name: "Storage Box",
        description: "A small wooden chest. Place it down and press E to \
                      stash items inside; anyone who finds it can open it.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(140, 100, 58),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::StorageBox { tier: 1 },
            max_health: crate::game_balance::STORAGE_BOX_SMALL_HP,
            collider_half_width: 0.5,
            collider_half_height: 0.35,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: STORAGE_BOX_LARGE_ID,
        name: "Large Storage Box",
        description: "A long banded chest with more than twice the room of \
                      the small box. Press E on the placed box to open it.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(150, 104, 56),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::StorageBox { tier: 2 },
            max_health: crate::game_balance::STORAGE_BOX_LARGE_HP,
            collider_half_width: 0.75,
            collider_half_height: 0.42,
            station_radius: 0.0,
        }),
    },
    // Hidden definitions for placed building blocks. Never craftable and
    // never in an inventory; they exist so `DeployedEntity::item_id`
    // resolves through the registry (saves, mirror views, colliders).
    // Their profile kind carries the spawn tier; the live tier lives on
    // the entity and changes with hammer upgrades.
    ItemDefinition {
        id: crate::building::BUILDING_FOUNDATION_ITEM_ID,
        name: "Foundation",
        description: "A structural platform. The base of every building.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::Foundation,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::FOUNDATION_HEIGHT_M / 2.0,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: crate::building::BUILDING_WALL_ITEM_ID,
        name: "Wall",
        description: "A solid building wall.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::Wall,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::WALL_HEIGHT_M / 2.0,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: crate::building::BUILDING_WINDOW_WALL_ITEM_ID,
        name: "Window Wall",
        description: "A building wall with a window opening.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::WindowWall,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::WALL_HEIGHT_M / 2.0,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: crate::building::BUILDING_DOORWAY_ITEM_ID,
        name: "Doorway",
        description: "A building wall with a door-sized opening.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::Doorway,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::WALL_HEIGHT_M / 2.0,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: crate::building::BUILDING_CEILING_ITEM_ID,
        name: "Ceiling",
        description: "A structural slab roofing a walled storey; the floor \
                      of the storey above.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::Ceiling,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::CEILING_THICKNESS_M / 2.0,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: crate::building::BUILDING_STAIRS_ITEM_ID,
        name: "Stairs",
        description: "A flight of stairs spanning one cell, rising a full \
                      storey to the floor above.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece: crate::building::BuildingPiece::Stairs,
                tier: crate::building::BuildingTier::Sticks,
            },
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height: crate::building::STAIR_RISE_M / 2.0,
            station_radius: 0.0,
        }),
    },
];

/// Build-once `id → definition` lookup over [`REGISTERED_ITEMS`]. The slice
/// itself is the source of truth; this index just gives `item_definition` an
/// O(1) hit instead of a linear scan, which matters once gather/pickup hits
/// run every swing.
fn item_definitions_by_id() -> &'static HashMap<&'static str, &'static ItemDefinition> {
    static INDEX: OnceLock<HashMap<&'static str, &'static ItemDefinition>> = OnceLock::new();
    INDEX.get_or_init(|| {
        REGISTERED_ITEMS
            .iter()
            .map(|definition| (definition.id, definition))
            .collect()
    })
}

pub fn item_definition(item_id: &str) -> Option<&'static ItemDefinition> {
    item_definitions_by_id().get(item_id).copied()
}

pub fn stack_limit(item_id: &str) -> Option<u16> {
    item_definition(item_id).map(|definition| definition.effective_stack_size())
}

pub fn normalize_stack(stack: &ItemStack) -> Option<ItemStack> {
    let limit = stack_limit(&stack.item_id)?;
    // Clone rather than rebuild via `ItemStack::new`: rebuilding would
    // reset a worn tool's remaining durability back to factory-fresh.
    let mut normalized = stack.clone();
    normalized.quantity = stack.quantity.clamp(1, limit);
    Some(normalized)
}

pub fn look_forward(yaw: f32, pitch: f32) -> Vec3Net {
    let pitch_cos = pitch.cos();
    Vec3Net::new(-yaw.sin() * pitch_cos, pitch.sin(), -yaw.cos() * pitch_cos).normalize_or_zero()
}

pub fn pickup_anchor(item: &DroppedWorldItem) -> Vec3Net {
    pickup_anchor_from_position(item.position)
}

pub fn pickup_anchor_from_position(position: Vec3Net) -> Vec3Net {
    position.plus(Vec3Net::new(0.0, PICKUP_ANCHOR_HEIGHT, 0.0))
}

pub fn pickup_score(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> Option<f32> {
    pickup_score_at_position(eye, yaw, pitch, item.position)
}

/// Projection-along-ray distance from the eye to the pickup anchor at
/// `position`. `None` when the point is outside the swept pickup
/// cylinder. Same math as [`pickup_score`] but reads the position
/// directly so callers iterating replicated `DroppedItemTransform`
/// don't need to materialise a `DroppedWorldItem`.
pub fn pickup_score_at_position(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    position: Vec3Net,
) -> Option<f32> {
    let anchor = pickup_anchor_from_position(position);
    let to_item = anchor.minus(eye);
    // Cheap distance cull before the trig in `look_forward`. Anything outside
    // the swept cylinder is unreachable; the bound stays conservative so it
    // never rejects a candidate the ray test would have accepted.
    let max_reach_sq = (PICKUP_RANGE + PICKUP_RAY_RADIUS).powi(2);
    if to_item.length_squared() > max_reach_sq {
        return None;
    }

    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let projection = to_item.dot(forward);
    if !(0.0..=PICKUP_RANGE).contains(&projection) {
        return None;
    }

    let closest = eye.plus(forward.scale(projection));
    let lateral = anchor.minus(closest);
    if lateral.length_squared() > PICKUP_RAY_RADIUS * PICKUP_RAY_RADIUS {
        return None;
    }

    Some(projection)
}

pub fn can_pick_up(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> bool {
    pickup_score(eye, yaw, pitch, item).is_some()
}

/// Lenient, distance-only reach test the *server* uses to accept a pickup,
/// instead of re-running the strict view-ray [`can_pick_up`]. The client
/// already chose this exact item with the view ray and only sends a command
/// for a target it accepted; by the time that command arrives the player has
/// usually moved or turned, so the strict cone test would reject a legitimate
/// pickup and force a visible client rollback. `slack` is the extra reach
/// beyond [`PICKUP_RANGE`] that absorbs the movement-prediction delta (see
/// `PICKUP_SERVER_REACH_SLACK_M` in `game_balance`). Look direction is
/// intentionally ignored here.
pub fn within_pickup_reach(eye: Vec3Net, item_position: Vec3Net, slack: f32) -> bool {
    let anchor = pickup_anchor_from_position(item_position);
    let reach = PICKUP_RANGE + slack.max(0.0);
    anchor.minus(eye).length_squared() <= reach * reach
}

pub fn best_pickup_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    items: impl Iterator<Item = &'a DroppedWorldItem>,
) -> Option<&'a DroppedWorldItem> {
    items
        .filter_map(|item| pickup_score(eye, yaw, pitch, item).map(|score| (score, item)))
        .min_by(|(a, _), (b, _)| a.total_cmp(b))
        .map(|(_, item)| item)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{DroppedWorldItem, ItemStack, QuatNet};

    #[test]
    fn tools_force_stack_one_but_deployables_stack() {
        // Tools carry per-item durability, so they never share a slot.
        assert_eq!(stack_limit(BASIC_HATCHET_ID), Some(1));
        assert_eq!(
            normalize_stack(&ItemStack::new(BASIC_HATCHET_ID, 40)),
            Some(ItemStack::new(BASIC_HATCHET_ID, 1))
        );
        // Raw materials stack high.
        assert_eq!(stack_limit(COAL_ID), Some(200));
        // The torch is an equipable deployable but not a tool, so it stacks up
        // to its declared limit rather than being forced to one.
        assert_eq!(stack_limit(TORCH_ID), Some(10));
    }

    #[test]
    fn tool_material_multiplier_favours_matched_pairings() {
        // Matched: hatchet→wood and pickaxe→stone hit hardest.
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Axe, DestructibleMaterial::Wood),
            150
        );
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Pickaxe, DestructibleMaterial::Stone),
            150
        );
        // Mismatched proper tools still chip but at a third of the
        // matched rate (50 / 150 = 1/3).
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Axe, DestructibleMaterial::Stone),
            50
        );
        assert_eq!(
            tool_effectiveness_pct(ToolKind::Pickaxe, DestructibleMaterial::Wood),
            50
        );
    }

    #[test]
    fn deployable_kind_material_matches_visual_intent() {
        assert_eq!(
            DeployableKind::Workbench { tier: 1 }.material(),
            DestructibleMaterial::Wood
        );
        assert_eq!(
            DeployableKind::Furnace { tier: 1 }.material(),
            DestructibleMaterial::Stone
        );
    }

    #[test]
    fn pickup_target_uses_view_ray_and_range() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(COAL_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        assert!(can_pick_up(eye, 0.0, -0.16, &item));
        assert!(!can_pick_up(eye, std::f32::consts::PI, -0.16, &item));
    }

    #[test]
    fn server_pickup_reach_is_lenient_and_distance_only() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(COAL_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        // Looking the other way fails the strict client test (used for
        // highlighting) but the server's distance-only check still accepts it,
        // so a player who turned away after pressing E isn't rolled back.
        assert!(!can_pick_up(eye, std::f32::consts::PI, 0.0, &item));
        assert!(within_pickup_reach(eye, item.position, 1.5));

        // Beyond PICKUP_RANGE + slack it's still rejected, the leniency is
        // bounded, not unlimited reach.
        let far = Vec3Net::new(0.0, 0.6, -10.0);
        assert!(!within_pickup_reach(far, item.position, 1.5));
    }
}
