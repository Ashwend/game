//! Item id interning, the `ItemDefinition` shape, the `REGISTERED_ITEMS`
//! source-of-truth slice, and the `id -> definition` lookup index.

use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
};

use crate::protocol::ItemStack;

use super::armor::ArmorProfile;
use super::deployables::{DeployableKind, DeployableProfile, DoorVariant};
use super::explosives::{ExplosiveDelivery, ExplosiveKind, ExplosiveProfile};
use super::ids::*;
use super::ranged::RangedProfile;
use super::tools::{ToolKind, ToolProfile};
use super::visual::{ArmorMesh, HeldMesh, ItemModel, ItemTint};
use super::weapons::WeaponProfile;
use crate::protocol::EquipmentSlot;

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
    /// Dedicated PvP weapon stats. Takes precedence over `tool` in combat
    /// resolution. `None` for every shipped item today; Phase 2 adds the first
    /// weapon rows.
    pub weapon: Option<WeaponProfile>,
    /// Ranged-weapon stats (bow, crossbow): draw/damage band, projectile speed,
    /// cooldown, and ammo id. `None` for every melee weapon and non-weapon item.
    /// A ranged weapon fires a server-simulated projectile instead of swinging,
    /// so it carries no `WeaponProfile` and no `ToolProfile`.
    pub ranged: Option<RangedProfile>,
    /// Worn-armor stats: which paperdoll slot the piece fits, its rig mesh, and
    /// its per-kind protection + durability. `None` for every non-armor item.
    pub armor: Option<ArmorProfile>,
    /// Blackpowder-explosive stats: base damage, blast radius, fuse window, and
    /// delivery (placed / wall-stuck / thrown). `None` for every non-explosive
    /// item. The four charges carry it; the effectiveness-per-material
    /// multiplier is the separate `explosive_effectiveness_pct` matrix.
    pub explosive: Option<ExplosiveProfile>,
    pub deployable: Option<DeployableProfile>,
}

impl ItemDefinition {
    /// The swing/impact archetype ([`ItemModel`]) this item animates and reads as
    /// on the wire when swung. A dedicated weapon uses its own registry `model`
    /// (Club/Spear/Sword/Mace); a gather tool uses its [`ToolKind`] archetype
    /// (which equals the registry `model` for tools too, but routing through the
    /// tool keeps the authoritative kind as the source of truth); anything else
    /// (raw materials, deployables-in-hand) resolves to [`ItemModel::Bag`], the
    /// non-combat fallback. This is the single derivation the server (stamping the
    /// peer-visible swing model) and the client (its local swing) share, so a
    /// swing's identity can never disagree across the two sides.
    pub fn swing_model(&self) -> ItemModel {
        if self.weapon.is_some() {
            self.model
        } else if let Some(tool) = self.tool {
            tool.kind.swing_model()
        } else {
            ItemModel::Bag
        }
    }

    pub fn effective_stack_size(self) -> u16 {
        // Tools, weapons (melee and ranged), and armor carry per-item
        // durability, so two of them can never share a slot, they're always a
        // stack of one regardless of `stack_size`. Everything else (raw
        // materials, ammo like arrows, and placeable deployables like the torch)
        // stacks up to its declared `stack_size`.
        if self.tool.is_some()
            || self.weapon.is_some()
            || self.ranged.is_some()
            || self.armor.is_some()
        {
            1
        } else {
            self.stack_size.max(1)
        }
    }
}

/// Construct the hidden `ItemDefinition` for a placed building block. The six
/// pieces differ only in id, display copy, the piece tag, and the collider
/// half-height (which is live: it anchors the damage nameplate via the
/// deployable overlay). Everything else is identical, so it lives here once.
///
/// `max_health` and `collider_half_width` are placeholders: the authoritative
/// HP and colliders for placed structures come from `crate::building`, not from
/// this registry entry. They are filled with representative values only so the
/// shared `DeployableProfile` shape stays valid.
const fn building_piece_item(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    piece: crate::building::BuildingPiece,
    collider_half_height: f32,
) -> ItemDefinition {
    ItemDefinition {
        id,
        name,
        description,
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(151, 116, 72),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Building {
                piece,
                tier: crate::building::BuildingTier::Sticks,
            },
            // Placeholder: real HP lives on the entity (crate::building).
            max_health: crate::game_balance::BUILDING_STICKS_WALL_HP,
            // Placeholder: real colliders come from crate::building.
            collider_half_width: crate::building::FOUNDATION_SIZE_M / 2.0,
            collider_half_height,
            station_radius: 0.0,
        }),
    }
}

/// Construct a worn-armor `ItemDefinition`. The pieces of a set differ only in
/// id, display copy, worn slot, rig mesh, and per-kind protection; the set-wide
/// values (icon tint, durability) are passed in so all three sets (padded,
/// lamellar, iron) share one builder. Armor never gathers or damages, so
/// `tool`/`weapon`/`ranged` stay `None`. Registered as an `equipable` item so it
/// can reach a hand for inspection, though it is really worn on the paperdoll;
/// the equip move validates the slot against the profile.
#[allow(clippy::too_many_arguments)]
const fn armor_item(
    id: &'static str,
    name: &'static str,
    description: &'static str,
    slot: EquipmentSlot,
    mesh: ArmorMesh,
    tint: ItemTint,
    max_durability: u32,
    melee_protection_pct: u8,
    projectile_protection_pct: u8,
    blast_protection_pct: u8,
) -> ItemDefinition {
    ItemDefinition {
        id,
        name,
        description,
        stack_size: 1,
        equipable: true,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint,
        tool: None,
        weapon: None,
        ranged: None,
        armor: Some(ArmorProfile {
            slot,
            mesh,
            melee_protection_pct,
            projectile_protection_pct,
            blast_protection_pct,
            max_durability: Some(max_durability),
        }),
        explosive: None,
        deployable: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: SULFUR_ID,
        name: "Sulfur",
        description: "Bright yellow sulfur, smelted from raw ore in a furnace. \
                      Ground with coal into blasting powder.",
        stack_size: 100,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(226, 202, 84),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: GUNPOWDER_ID,
        name: "Gunpowder",
        description: "Coarse black blasting powder, ground from coal and sulfur. \
                      The charge behind every explosive.",
        stack_size: 100,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(58, 56, 54),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: CLOTH_ID,
        name: "Cloth",
        description: "Plant fiber woven into a coarse cloth. The padding and \
                      wrapping behind primitive armor.",
        stack_size: 50,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(206, 194, 168),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: METEORITE_ID,
        name: "Meteorite",
        description: "A rare crystal that glows with a banked orange heat. \
                      Mined from slag-dark outcrops far from the world's centre.",
        stack_size: 20,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(232, 126, 52),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: ANCIENT_FITTINGS_ID,
        name: "Ancient Fittings",
        description: "Salvaged hinges, gears, and springs from older works. \
                      Too intricate to forge by hand; found, not made.",
        stack_size: 50,
        equipable: false,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(150, 142, 120),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    // melee weapons. Each carries a `WeaponProfile` (not a
    // `ToolProfile`), so it gathers nothing and combat resolves it ahead of any
    // tool through one `AttackProfile`. Every number lives in game_balance.
    // `model` is the swing archetype (Club/Spear/Sword/Mace); `held_mesh` is the
    // two-primitive haft+head glb.
    ItemDefinition {
        id: WOODEN_CLUB_ID,
        name: "Wooden Club",
        description: "A heavy knot of hardwood on a wrapped grip. The first real \
                      weapon: a fast, cheap swing that hits harder than a fist.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Club,
        held_mesh: HeldMesh::WoodenClub,
        tint: ItemTint::new(150, 108, 66),
        tool: None,
        weapon: Some(WeaponProfile {
            pvp_damage: crate::game_balance::WOODEN_CLUB_PVP_DAMAGE,
            knockback_speed: crate::game_balance::WOODEN_CLUB_KNOCKBACK_SPEED,
            reach_m: crate::game_balance::COMBAT_ATTACK_RANGE_M,
            cooldown_ticks: crate::game_balance::WOODEN_CLUB_COOLDOWN_TICKS,
            armor_pierce_pct: 0,
            max_durability: Some(crate::game_balance::STONE_TOOL_DURABILITY),
        }),
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: STONE_SPEAR_ID,
        name: "Stone Spear",
        description: "A knapped stone point lashed to a long haft. Slow, but it \
                      reaches a metre past any other melee weapon; keep enemies \
                      on the tip.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Spear,
        held_mesh: HeldMesh::StoneSpear,
        tint: ItemTint::new(150, 132, 104),
        tool: None,
        weapon: Some(WeaponProfile {
            pvp_damage: crate::game_balance::STONE_SPEAR_PVP_DAMAGE,
            knockback_speed: crate::game_balance::STONE_SPEAR_KNOCKBACK_SPEED,
            reach_m: crate::game_balance::STONE_SPEAR_REACH_M,
            cooldown_ticks: crate::game_balance::STONE_SPEAR_COOLDOWN_TICKS,
            armor_pierce_pct: 0,
            max_durability: Some(crate::game_balance::STONE_TOOL_DURABILITY),
        }),
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_SWORD_ID,
        name: "Iron Sword",
        description: "A forged iron blade on a wrapped grip. The workhorse: \
                      balanced damage on a medium swing, good in any fight.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Sword,
        held_mesh: HeldMesh::IronSword,
        tint: ItemTint::new(178, 182, 190),
        tool: None,
        weapon: Some(WeaponProfile {
            pvp_damage: crate::game_balance::IRON_SWORD_PVP_DAMAGE,
            knockback_speed: crate::game_balance::IRON_SWORD_KNOCKBACK_SPEED,
            reach_m: crate::game_balance::COMBAT_ATTACK_RANGE_M,
            cooldown_ticks: crate::game_balance::IRON_SWORD_COOLDOWN_TICKS,
            armor_pierce_pct: 0,
            max_durability: Some(crate::game_balance::IRON_TOOL_DURABILITY),
        }),
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: IRON_MACE_ID,
        name: "Iron Mace",
        description: "A brutal iron head on a heavy haft. The slowest, hardest \
                      swing in the game, with the biggest shove and enough force \
                      to punch through half of any armor. The answer to plate.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Mace,
        held_mesh: HeldMesh::IronMace,
        tint: ItemTint::new(150, 152, 158),
        tool: None,
        weapon: Some(WeaponProfile {
            pvp_damage: crate::game_balance::IRON_MACE_PVP_DAMAGE,
            knockback_speed: crate::game_balance::IRON_MACE_KNOCKBACK_SPEED,
            reach_m: crate::game_balance::COMBAT_ATTACK_RANGE_M,
            cooldown_ticks: crate::game_balance::IRON_MACE_COOLDOWN_TICKS,
            armor_pierce_pct: crate::game_balance::IRON_MACE_ARMOR_PIERCE_PCT,
            max_durability: Some(crate::game_balance::IRON_TOOL_DURABILITY),
        }),
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    // ranged weapons. Each carries a `RangedProfile` (not a
    // `WeaponProfile` or `ToolProfile`), so it gathers nothing and fires a
    // server-simulated projectile through the `ClientMessage::Ranged` path. The
    // arrow below is their shared ammo (no profile, plain stackable ammo). Every
    // number lives in game_balance. `model` is the ranged pose archetype
    // (Bow/Crossbow); `held_mesh` is the two-primitive body+detail glb.
    ItemDefinition {
        id: WOODEN_BOW_ID,
        name: "Wooden Bow",
        description: "A bent-wood bow on a wrapped grip. Hold to draw: the longer \
                      you pull, the harder the arrow hits, up to a full-draw \
                      punch. Drawing slows you down, so pick your moment.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Bow,
        held_mesh: HeldMesh::WoodenBow,
        tint: ItemTint::new(150, 112, 68),
        tool: None,
        weapon: None,
        ranged: Some(RangedProfile {
            damage_min: crate::game_balance::WOODEN_BOW_DAMAGE_MIN,
            damage_max: crate::game_balance::WOODEN_BOW_DAMAGE_MAX,
            projectile_speed_mps: crate::game_balance::WOODEN_BOW_PROJECTILE_SPEED_MPS,
            draw_ticks_to_full: crate::game_balance::WOODEN_BOW_DRAW_TICKS,
            cooldown_ticks: crate::game_balance::WOODEN_BOW_COOLDOWN_TICKS,
            ammo_item: ARROW_ID,
            knockback_speed: crate::game_balance::WOODEN_BOW_KNOCKBACK_SPEED,
            max_durability: Some(crate::game_balance::STONE_TOOL_DURABILITY),
        }),
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: CROSSBOW_ID,
        name: "Crossbow",
        description: "An iron-fitted crossbow on a hewn stock. No draw to hold: \
                      every bolt hits flat and hard, but the reload is a slow, \
                      heavy ratchet. The ambush weapon.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Crossbow,
        held_mesh: HeldMesh::Crossbow,
        tint: ItemTint::new(140, 130, 118),
        tool: None,
        weapon: None,
        ranged: Some(RangedProfile {
            damage_min: crate::game_balance::CROSSBOW_DAMAGE,
            damage_max: crate::game_balance::CROSSBOW_DAMAGE,
            projectile_speed_mps: crate::game_balance::CROSSBOW_PROJECTILE_SPEED_MPS,
            draw_ticks_to_full: crate::game_balance::CROSSBOW_DRAW_TICKS,
            cooldown_ticks: crate::game_balance::CROSSBOW_COOLDOWN_TICKS,
            ammo_item: ARROW_ID,
            knockback_speed: crate::game_balance::CROSSBOW_KNOCKBACK_SPEED,
            max_durability: Some(crate::game_balance::IRON_TOOL_DURABILITY),
        }),
        armor: None,
        explosive: None,
        deployable: None,
    },
    ItemDefinition {
        id: ARROW_ID,
        name: "Arrow",
        description: "A straight wooden shaft with a knapped stone head. Ammo for \
                      the bow and crossbow. Stacks deep, and about half of them \
                      survive a shot into the world to be picked back up.",
        stack_size: 24,
        equipable: true,
        model: ItemModel::Bag,
        held_mesh: HeldMesh::Arrow,
        tint: ItemTint::new(158, 138, 106),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: None,
    },
    // Blackpowder explosives. Each carries an `ExplosiveProfile` (base
    // damage, radius, fuse, delivery); the placed two additionally carry a
    // `DeployableProfile` so they route through the placement path as an
    // `Explosive` deployable. The thrown bomb has no deployable profile: it is
    // lit on the throw and lives its whole life in the projectile sim. All
    // numbers live in game_balance; the per-material effectiveness is the
    // separate matrix.
    ItemDefinition {
        id: POWDER_BOMB_ID,
        name: "Powder Bomb",
        description: "A cloth-wrapped handful of blasting powder, lit as it \
                      leaves your hand. It bounces and rolls, then blows. \
                      Shreds sticks huts and chips hewn wood, useless on stone.",
        stack_size: 20,
        equipable: true,
        // A thrown charge: its own wind-up-and-release lob pose, not the bag hold.
        model: ItemModel::ThrownBomb,
        held_mesh: HeldMesh::PowderBomb,
        tint: ItemTint::new(74, 66, 58),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: Some(ExplosiveProfile {
            kind: ExplosiveKind::PowderBomb,
            base_damage: crate::game_balance::POWDER_BOMB_BASE_DAMAGE,
            radius_m: crate::game_balance::POWDER_BOMB_RADIUS_M,
            fuse_ticks: crate::game_balance::POWDER_BOMB_FUSE_TICKS,
            delivery: ExplosiveDelivery::Thrown,
            max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        }),
        deployable: None,
    },
    ItemDefinition {
        id: POWDER_KEG_ID,
        name: "Powder Keg",
        description: "A staved barrel packed with powder and bound in iron. Place \
                      it against a wall and stand clear: the fuse hisses, then it \
                      breaches. Several take down a hewn-wood wall.",
        stack_size: 10,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::PowderKeg,
        tint: ItemTint::new(120, 92, 58),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: Some(ExplosiveProfile {
            kind: ExplosiveKind::PowderKeg,
            base_damage: crate::game_balance::POWDER_KEG_BASE_DAMAGE,
            radius_m: crate::game_balance::POWDER_KEG_RADIUS_M,
            fuse_ticks: crate::game_balance::POWDER_KEG_FUSE_TICKS,
            delivery: ExplosiveDelivery::Placed,
            max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        }),
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Explosive {
                kind: ExplosiveKind::PowderKeg,
            },
            max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
            collider_half_width: 0.35,
            collider_half_height: 0.45,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: SATCHEL_CHARGE_ID,
        name: "Satchel Charge",
        description: "A packed satchel of charges on a leather strap. The tier-2 \
                      breacher: real numbers against stone, and the first charge \
                      that scratches an iron door at all.",
        stack_size: 10,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::SatchelCharge,
        tint: ItemTint::new(96, 82, 64),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: Some(ExplosiveProfile {
            kind: ExplosiveKind::SatchelCharge,
            base_damage: crate::game_balance::SATCHEL_CHARGE_BASE_DAMAGE,
            radius_m: crate::game_balance::SATCHEL_CHARGE_RADIUS_M,
            fuse_ticks: crate::game_balance::SATCHEL_CHARGE_FUSE_TICKS,
            delivery: ExplosiveDelivery::Placed,
            max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
        }),
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Explosive {
                kind: ExplosiveKind::SatchelCharge,
            },
            max_health: crate::game_balance::EXPLOSIVE_CHARGE_HP,
            collider_half_width: 0.30,
            collider_half_height: 0.20,
            station_radius: 0.0,
        }),
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Door {
                variant: DoorVariant::HewnLog,
            },
            max_health: crate::game_balance::DOOR_MAX_HP,
            collider_half_width: 0.55,
            collider_half_height: 1.1,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: IRON_DOOR_ID,
        name: "Iron Door",
        description: "A forged iron door on a banded frame with a settable \
                      code lock. Tools can't scratch it, only explosives \
                      will breach it. Mounts only in a doorway opening.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(170, 174, 182),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::Door {
                variant: DoorVariant::Iron,
            },
            max_health: crate::game_balance::IRON_DOOR_MAX_HP,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
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
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::StorageBox { tier: 2 },
            max_health: crate::game_balance::STORAGE_BOX_LARGE_HP,
            collider_half_width: 0.75,
            collider_half_height: 0.42,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        id: TOOL_CUPBOARD_ID,
        name: "Tool Cupboard",
        description: "A locked cabinet that claims the base it sits on. \
                      While it stands, only authorized players can build \
                      nearby. Press E to authorize yourself; hold E for \
                      options.",
        stack_size: 1,
        equipable: true,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(120, 84, 48),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::ToolCupboard,
            max_health: crate::game_balance::TOOL_CUPBOARD_MAX_HP,
            collider_half_width: 0.46,
            collider_half_height: 0.9,
            station_radius: 0.0,
        }),
    },
    ItemDefinition {
        // World-spawned ruin loot cache. It is never in a player's inventory,
        // but it needs a registry row so `item_definition` resolves it for the
        // deployable mesh, collider, and save round-trip. `equipable: false`
        // keeps it off the action bar and out of any placeable-item UI, and it
        // has no recipe, so players can neither craft nor place it.
        id: RUIN_CACHE_ID,
        name: "Ruin Cache",
        description: "A weathered stone-and-iron strongbox left in the ruins. \
                      Press E to open it; anyone can loot it, and it slowly \
                      refills. The only source of ancient fittings.",
        stack_size: 1,
        equipable: false,
        model: ItemModel::Deployable,
        held_mesh: HeldMesh::Bag,
        tint: ItemTint::new(150, 142, 128),
        tool: None,
        weapon: None,
        ranged: None,
        armor: None,
        explosive: None,
        deployable: Some(DeployableProfile {
            kind: DeployableKind::RuinCache,
            // Collider matches the hip-height strongbox model (~1.2 x 0.88 x
            // 0.95 m, art/ruins/build_ruins.py) so E-targeting lands on it.
            max_health: crate::game_balance::RUIN_CACHE_MAX_HP,
            collider_half_width: 0.6,
            collider_half_height: 0.48,
            station_radius: 0.0,
        }),
    },
    // Padded (cloth) armor set: the starter worn-armor line. Per-piece
    // protection comes from game_balance so the columns sum to the set totals
    // (melee 12 / projectile 10 / blast 4).
    armor_item(
        PADDED_HOOD_ID,
        "Padded Helmet",
        "A quilted cloth hood. Turns a glancing blow; little more.",
        EquipmentSlot::Head,
        ArmorMesh::PaddedHood,
        ItemTint::new(196, 176, 132),
        crate::game_balance::PADDED_ARMOR_DURABILITY,
        crate::game_balance::PADDED_HEAD_MELEE_PCT,
        crate::game_balance::PADDED_HEAD_PROJECTILE_PCT,
        crate::game_balance::PADDED_HEAD_BLAST_PCT,
    ),
    armor_item(
        PADDED_TUNIC_ID,
        "Padded Chestplate",
        "A thick quilted tunic. The most protective piece of the padded set.",
        EquipmentSlot::Chest,
        ArmorMesh::PaddedTunic,
        ItemTint::new(196, 176, 132),
        crate::game_balance::PADDED_ARMOR_DURABILITY,
        crate::game_balance::PADDED_CHEST_MELEE_PCT,
        crate::game_balance::PADDED_CHEST_PROJECTILE_PCT,
        crate::game_balance::PADDED_CHEST_BLAST_PCT,
    ),
    armor_item(
        PADDED_LEGGINGS_ID,
        "Padded Leggings",
        "Quilted cloth leggings. Soaks a share of every hit to the legs.",
        EquipmentSlot::Legs,
        ArmorMesh::PaddedLeggings,
        ItemTint::new(196, 176, 132),
        crate::game_balance::PADDED_ARMOR_DURABILITY,
        crate::game_balance::PADDED_LEGS_MELEE_PCT,
        crate::game_balance::PADDED_LEGS_PROJECTILE_PCT,
        crate::game_balance::PADDED_LEGS_BLAST_PCT,
    ),
    armor_item(
        PADDED_WRAPS_ID,
        "Padded Boots",
        "Cloth wraps bound around the feet. The lightest padded piece.",
        EquipmentSlot::Feet,
        ArmorMesh::PaddedWraps,
        ItemTint::new(196, 176, 132),
        crate::game_balance::PADDED_ARMOR_DURABILITY,
        crate::game_balance::PADDED_FEET_MELEE_PCT,
        crate::game_balance::PADDED_FEET_PROJECTILE_PCT,
        crate::game_balance::PADDED_FEET_BLAST_PCT,
    ),
    // Lamellar (hewn-wood slats over cloth) armor set: the mid-tier line,
    // crafted at a workbench. Per-piece protection comes from game_balance so
    // the columns sum to the set totals (melee 24 / projectile 20 / blast 10).
    armor_item(
        LAMELLAR_HELM_ID,
        "Lamellar Helmet",
        "A slatted wood helm laced over a padded cap. Twice the cover of \
         the padded helmet.",
        EquipmentSlot::Head,
        ArmorMesh::LamellarHelm,
        ItemTint::new(150, 116, 72),
        crate::game_balance::LAMELLAR_ARMOR_DURABILITY,
        crate::game_balance::LAMELLAR_HEAD_MELEE_PCT,
        crate::game_balance::LAMELLAR_HEAD_PROJECTILE_PCT,
        crate::game_balance::LAMELLAR_HEAD_BLAST_PCT,
    ),
    armor_item(
        LAMELLAR_VEST_ID,
        "Lamellar Chestplate",
        "Rows of hewn-wood slats lashed over a cloth backing, with a slatted \
         shoulder cap. The most protective piece of the lamellar set.",
        EquipmentSlot::Chest,
        ArmorMesh::LamellarVest,
        ItemTint::new(150, 116, 72),
        crate::game_balance::LAMELLAR_ARMOR_DURABILITY,
        crate::game_balance::LAMELLAR_CHEST_MELEE_PCT,
        crate::game_balance::LAMELLAR_CHEST_PROJECTILE_PCT,
        crate::game_balance::LAMELLAR_CHEST_BLAST_PCT,
    ),
    armor_item(
        LAMELLAR_GREAVES_ID,
        "Lamellar Leggings",
        "Slatted wood greaves over padded leggings. Sheds a solid share of \
         every hit to the legs.",
        EquipmentSlot::Legs,
        ArmorMesh::LamellarGreaves,
        ItemTint::new(150, 116, 72),
        crate::game_balance::LAMELLAR_ARMOR_DURABILITY,
        crate::game_balance::LAMELLAR_LEGS_MELEE_PCT,
        crate::game_balance::LAMELLAR_LEGS_PROJECTILE_PCT,
        crate::game_balance::LAMELLAR_LEGS_BLAST_PCT,
    ),
    armor_item(
        LAMELLAR_BOOTS_ID,
        "Lamellar Boots",
        "Slatted wood boots over bound cloth. The lightest lamellar piece.",
        EquipmentSlot::Feet,
        ArmorMesh::LamellarBoots,
        ItemTint::new(150, 116, 72),
        crate::game_balance::LAMELLAR_ARMOR_DURABILITY,
        crate::game_balance::LAMELLAR_FEET_MELEE_PCT,
        crate::game_balance::LAMELLAR_FEET_PROJECTILE_PCT,
        crate::game_balance::LAMELLAR_FEET_BLAST_PCT,
    ),
    // Iron (plate over padding) armor set: the top line of the tree,
    // forged at a tier-2 workbench. Per-piece protection comes from game_balance
    // so the columns sum to the set totals (melee 40 / projectile 36 / blast 20).
    armor_item(
        IRON_HELM_ID,
        "Iron Helmet",
        "A forged iron helm over a padded cap. Turns aside all but the \
         heaviest blows.",
        EquipmentSlot::Head,
        ArmorMesh::IronHelm,
        ItemTint::new(178, 182, 190),
        crate::game_balance::IRON_ARMOR_DURABILITY,
        crate::game_balance::IRON_HEAD_MELEE_PCT,
        crate::game_balance::IRON_HEAD_PROJECTILE_PCT,
        crate::game_balance::IRON_HEAD_BLAST_PCT,
    ),
    armor_item(
        IRON_CUIRASS_ID,
        "Iron Chestplate",
        "A forged breastplate over padding, with a plate pauldron. The most \
         protective piece in the game.",
        EquipmentSlot::Chest,
        ArmorMesh::IronCuirass,
        ItemTint::new(178, 182, 190),
        crate::game_balance::IRON_ARMOR_DURABILITY,
        crate::game_balance::IRON_CHEST_MELEE_PCT,
        crate::game_balance::IRON_CHEST_PROJECTILE_PCT,
        crate::game_balance::IRON_CHEST_BLAST_PCT,
    ),
    armor_item(
        IRON_GREAVES_ID,
        "Iron Leggings",
        "Forged plate greaves over padded leggings. Soaks a heavy share of \
         every hit to the legs.",
        EquipmentSlot::Legs,
        ArmorMesh::IronGreaves,
        ItemTint::new(178, 182, 190),
        crate::game_balance::IRON_ARMOR_DURABILITY,
        crate::game_balance::IRON_LEGS_MELEE_PCT,
        crate::game_balance::IRON_LEGS_PROJECTILE_PCT,
        crate::game_balance::IRON_LEGS_BLAST_PCT,
    ),
    armor_item(
        IRON_BOOTS_ID,
        "Iron Boots",
        "Forged plate boots over padding. The lightest iron piece, and still \
         plate.",
        EquipmentSlot::Feet,
        ArmorMesh::IronBoots,
        ItemTint::new(178, 182, 190),
        crate::game_balance::IRON_ARMOR_DURABILITY,
        crate::game_balance::IRON_FEET_MELEE_PCT,
        crate::game_balance::IRON_FEET_PROJECTILE_PCT,
        crate::game_balance::IRON_FEET_BLAST_PCT,
    ),
    // Hidden definitions for placed building blocks. Never craftable and
    // never in an inventory; they exist so `DeployedEntity::item_id`
    // resolves through the registry (saves, mirror views, colliders).
    // Their profile kind carries the spawn tier; the live tier lives on
    // the entity and changes with hammer upgrades.
    building_piece_item(
        crate::building::BUILDING_FOUNDATION_ITEM_ID,
        "Foundation",
        "A structural platform. The base of every building.",
        crate::building::BuildingPiece::Foundation,
        crate::building::FOUNDATION_HEIGHT_M / 2.0,
    ),
    building_piece_item(
        crate::building::BUILDING_WALL_ITEM_ID,
        "Wall",
        "A solid building wall.",
        crate::building::BuildingPiece::Wall,
        crate::building::WALL_HEIGHT_M / 2.0,
    ),
    building_piece_item(
        crate::building::BUILDING_WINDOW_WALL_ITEM_ID,
        "Window Wall",
        "A building wall with a window opening.",
        crate::building::BuildingPiece::WindowWall,
        crate::building::WALL_HEIGHT_M / 2.0,
    ),
    building_piece_item(
        crate::building::BUILDING_DOORWAY_ITEM_ID,
        "Doorway",
        "A building wall with a door-sized opening.",
        crate::building::BuildingPiece::Doorway,
        crate::building::WALL_HEIGHT_M / 2.0,
    ),
    building_piece_item(
        crate::building::BUILDING_CEILING_ITEM_ID,
        "Ceiling",
        "A structural slab roofing a walled storey; the floor of the storey above.",
        crate::building::BuildingPiece::Ceiling,
        crate::building::CEILING_THICKNESS_M / 2.0,
    ),
    building_piece_item(
        crate::building::BUILDING_STAIRS_ITEM_ID,
        "Stairs",
        "A flight of stairs spanning one cell, rising a full storey to the floor above.",
        crate::building::BuildingPiece::Stairs,
        crate::building::STAIR_RISE_M / 2.0,
    ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::DeployableKind;

    #[test]
    fn every_armor_piece_names_its_slot_unmistakably() {
        // Owner feedback: piece names like "Cuirass", "Greaves" and "Wraps"
        // didn't tell players which slot they fill. Every worn-armor item must
        // end in its slot's one canonical noun (Helmet / Chestplate / Leggings
        // / Boots) so the slot can never be mistaken, whatever the set.
        for definition in REGISTERED_ITEMS {
            let Some(armor) = definition.armor else {
                continue;
            };
            let expected = match armor.slot {
                EquipmentSlot::Head => "Helmet",
                EquipmentSlot::Chest => "Chestplate",
                EquipmentSlot::Legs => "Leggings",
                EquipmentSlot::Feet => "Boots",
            };
            assert!(
                definition.name.ends_with(expected),
                "{} ({:?}) must be named \"... {}\", got \"{}\"",
                definition.id,
                armor.slot,
                expected,
                definition.name
            );
        }
    }

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
    fn equipable_items_map_to_the_expected_held_mesh_and_tool() {
        use crate::items::{HeldMesh, ItemModel, ToolKind};
        // The peer-visible held-item replication ships `HeldMesh`, and the
        // third-person swing reads the tool kind, both derived from these
        // definitions. Pin the mapping so a registry edit can't silently put
        // the wrong thing in a remote player's hand.
        let cases: &[(&str, ItemModel, HeldMesh, Option<ToolKind>)] = &[
            (
                BASIC_HATCHET_ID,
                ItemModel::Hatchet,
                HeldMesh::StoneHatchet,
                Some(ToolKind::Axe),
            ),
            (
                IRON_HATCHET_ID,
                ItemModel::Hatchet,
                HeldMesh::IronHatchet,
                Some(ToolKind::Axe),
            ),
            (
                BASIC_PICKAXE_ID,
                ItemModel::Pickaxe,
                HeldMesh::StonePickaxe,
                Some(ToolKind::Pickaxe),
            ),
            (
                IRON_PICKAXE_ID,
                ItemModel::Pickaxe,
                HeldMesh::IronPickaxe,
                Some(ToolKind::Pickaxe),
            ),
            (
                HAMMER_ID,
                ItemModel::Hatchet,
                HeldMesh::Hammer,
                Some(ToolKind::Hammer),
            ),
            (
                BUILDING_PLAN_ID,
                ItemModel::Bag,
                HeldMesh::BuildingPlan,
                None,
            ),
            (TORCH_ID, ItemModel::Deployable, HeldMesh::Bag, None),
        ];
        for (id, model, held_mesh, tool) in cases {
            let definition = item_definition(id).expect("registered item");
            assert!(definition.equipable, "{id} should be equipable");
            assert_eq!(definition.model, *model, "{id} model");
            assert_eq!(definition.held_mesh, *held_mesh, "{id} held mesh");
            assert_eq!(definition.tool.map(|t| t.kind), *tool, "{id} tool kind");
        }

        // Raw materials are not equipable, so they never reach a hand.
        assert!(!item_definition(WOOD_ID).unwrap().equipable);
    }

    #[test]
    fn padded_set_pieces_map_to_their_slots_and_meshes() {
        use crate::items::ArmorMesh;
        use crate::protocol::EquipmentSlot;
        // Pin the slot + mesh of each padded piece so a registry edit can't
        // silently put a helmet in the chest slot or swap a mesh.
        let cases: &[(&str, EquipmentSlot, ArmorMesh)] = &[
            (PADDED_HOOD_ID, EquipmentSlot::Head, ArmorMesh::PaddedHood),
            (
                PADDED_TUNIC_ID,
                EquipmentSlot::Chest,
                ArmorMesh::PaddedTunic,
            ),
            (
                PADDED_LEGGINGS_ID,
                EquipmentSlot::Legs,
                ArmorMesh::PaddedLeggings,
            ),
            (PADDED_WRAPS_ID, EquipmentSlot::Feet, ArmorMesh::PaddedWraps),
        ];
        for (id, slot, mesh) in cases {
            let definition = item_definition(id).expect("registered padded piece");
            let profile = definition.armor.expect("padded piece has an armor profile");
            assert_eq!(profile.slot, *slot, "{id} slot");
            assert_eq!(profile.mesh, *mesh, "{id} mesh");
            // Armor is a stack of one and carries durability.
            assert_eq!(stack_limit(id), Some(1), "{id} must not stack");
            assert!(profile.max_durability.is_some(), "{id} should wear");
        }
    }

    #[test]
    fn every_armor_mesh_has_exactly_one_registered_piece() {
        use crate::items::ArmorMesh;
        // Each ArmorMesh selector must be produced by exactly one registered
        // armor item, so a variant added without a registry row (or duplicated)
        // fails here rather than shipping an unrenderable or ambiguous mesh.
        for &mesh in ArmorMesh::ALL {
            let count = REGISTERED_ITEMS
                .iter()
                .filter_map(|definition| definition.armor)
                .filter(|profile| profile.mesh == mesh)
                .count();
            assert_eq!(
                count, 1,
                "{mesh:?} should map to exactly one registered piece"
            );
        }
    }

    #[test]
    fn registered_item_ids_are_unique() {
        // `item_definitions_by_id` builds its lookup with `.collect()` into a
        // HashMap, so a duplicate id would silently overwrite the earlier
        // entry instead of failing. Mirror the crafting registry's
        // duplicate-id guard so that footgun is caught at test time.
        let mut seen = std::collections::HashSet::new();
        for definition in REGISTERED_ITEMS {
            assert!(
                seen.insert(definition.id),
                "duplicate item id in REGISTERED_ITEMS: {}",
                definition.id
            );
        }
    }

    #[test]
    fn every_building_piece_has_a_matching_item_definition() {
        // `building_item_id` and the hidden building-block `ItemDefinition`s
        // are two hand-maintained tables that must agree: every piece must
        // resolve to a definition whose deployable profile is a `Building`
        // for that same piece. Without this, a new `BuildingPiece` variant
        // (or a renamed id constant) compiles fine and only fails at runtime
        // the first time a player places that piece.
        use crate::building::{BuildingPiece, building_item_id};
        for piece in BuildingPiece::ALL {
            let id = building_item_id(piece);
            let definition =
                item_definition(id).unwrap_or_else(|| panic!("no ItemDefinition for {id}"));
            let profile = definition
                .deployable
                .unwrap_or_else(|| panic!("{id} has no deployable profile"));
            match profile.kind {
                DeployableKind::Building {
                    piece: resolved, ..
                } => assert_eq!(
                    resolved, piece,
                    "{id} resolves to a Building profile for the wrong piece"
                ),
                other => panic!("{id} resolves to {other:?}, expected a Building piece"),
            }
        }
    }
}
