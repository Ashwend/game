//! Deployable taxonomy: the [`DoorVariant`] and [`DeployableKind`] spawn
//! identities, and the [`DeployableProfile`] footprint/health shape carried
//! on `ItemDefinition` for placeable items.

use super::explosives::ExplosiveKind;
use super::ids::{HEWN_LOG_DOOR_ID, IRON_DOOR_ID};
use super::materials::DestructibleMaterial;

/// Which door model a [`DeployableKind::Door`] is. The variant is immutable
/// spawn identity: it travels on the replicated `Deployable` component and
/// in the save, and is the single lookup for the door's item id, HP, raid
/// material, and display name. Adding a new door is one arm here plus a
/// recipe and a model, nothing in the damage/placement/persistence paths
/// changes. All accessors are `const fn` because `DeployableKind::material`
/// and `label` are const and defer to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum DoorVariant {
    /// Squared-log wood door. The soft side of a base: raidable with tools
    /// but slowly (`WoodBuilding` material).
    HewnLog,
    /// Forged iron door. Tools do nothing to it (`MetalBuilding` material);
    /// it only falls to explosives, and carries double the wood door's HP.
    Iron,
}

impl DoorVariant {
    /// Inventory item id that places this door.
    pub const fn item_id(self) -> &'static str {
        match self {
            Self::HewnLog => HEWN_LOG_DOOR_ID,
            Self::Iron => IRON_DOOR_ID,
        }
    }

    /// Spawn HP for a freshly hung door of this variant.
    pub const fn max_hp(self) -> u32 {
        match self {
            Self::HewnLog => crate::game_balance::DOOR_MAX_HP,
            Self::Iron => crate::game_balance::IRON_DOOR_MAX_HP,
        }
    }

    /// Raid material, the lever the tool-vs-material table reads. Wood doors
    /// chip under tools; iron doors are tool-immune.
    pub const fn material(self) -> DestructibleMaterial {
        match self {
            Self::HewnLog => DestructibleMaterial::WoodBuilding,
            Self::Iron => DestructibleMaterial::MetalBuilding,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::HewnLog => "Hewn Log Door",
            Self::Iron => "Iron Door",
        }
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
    /// door during placement rotates it half a turn). `variant` selects the
    /// material (wood vs iron), which drives the model, HP, and raid
    /// resistance; it is immutable spawn identity.
    Door {
        variant: DoorVariant,
    },
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
    /// Base-ownership claim object (a "Tool Cupboard"). Placed on a
    /// building platform; while it stands it projects building privilege
    /// over the connected base + a margin ring, so only authorized
    /// players can build there. Carries no fields: the owner lives on the
    /// entity and the authorized list lives in the server-side
    /// [`crate::server`] cupboard sub-state.
    ToolCupboard,
    /// World-spawned ruin loot cache. Placed only by world generation (not
    /// craftable, not player-placeable), anyone may open it, and it refills
    /// its loot on a timer. It is indestructible (no damage path touches it).
    /// Appended LAST so the postcard variant index of every existing kind is
    /// unchanged: old saves that predate the cache stay loadable up to the
    /// version gate (see docs/worlds-and-saves.md, save format v19).
    RuinCache,
    /// A placed blackpowder charge (keg or satchel; the thrown
    /// bomb never becomes one). The `kind` is immutable spawn identity: it
    /// selects the model, the blast profile, and the effectiveness matrix
    /// column. An armed charge carries its countdown in the entity's `fuse`
    /// sub-state (server-only), detonates on zero, and can be FIZZLED by taking
    /// it to 0 HP through the normal deployable damage path. Appended LAST (after
    /// `RuinCache`) so every existing kind's postcard variant index is unchanged;
    /// the append bumps the save format to v20 (see docs/worlds-and-saves.md).
    Explosive {
        kind: ExplosiveKind,
    },
}

impl DeployableKind {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Workbench { .. } => "Workbench",
            Self::Furnace { .. } => "Furnace",
            Self::Building { piece, .. } => piece.label(),
            Self::Door { variant } => variant.label(),
            Self::SleepingBag => "Sleeping Bag",
            Self::StorageBox { tier } => {
                if tier >= 2 {
                    "Large Storage Box"
                } else {
                    "Storage Box"
                }
            }
            Self::Torch { .. } => "Torch",
            Self::ToolCupboard => "Tool Cupboard",
            Self::RuinCache => "Ruin Cache",
            Self::Explosive { kind } => kind.label(),
        }
    }

    /// Source of truth for what the structure is built from. The damage
    /// path uses this for the tool-vs-material multiplier and the
    /// client uses it to pick the swing surface (audio/visual chip).
    /// Building blocks change material as they're upgraded, which is the
    /// entire raid-balance lever: see the building arms in
    /// [`super::tool_effectiveness_pct`].
    pub const fn material(self) -> DestructibleMaterial {
        match self {
            Self::Workbench { .. } => DestructibleMaterial::Wood,
            Self::Furnace { .. } => DestructibleMaterial::Stone,
            Self::Building { tier, .. } => match tier {
                crate::building::BuildingTier::Sticks => DestructibleMaterial::Sticks,
                crate::building::BuildingTier::HewnWood => DestructibleMaterial::WoodBuilding,
                crate::building::BuildingTier::Stone => DestructibleMaterial::StoneBuilding,
            },
            Self::Door { variant } => variant.material(),
            Self::SleepingBag => DestructibleMaterial::Cloth,
            Self::StorageBox { .. } => DestructibleMaterial::Wood,
            Self::Torch { .. } => DestructibleMaterial::Wood,
            // Raidable soft-wood band: destroying it lifts the base's
            // building privilege, so it has to be a reachable raid goal.
            Self::ToolCupboard => DestructibleMaterial::WoodBuilding,
            // Indestructible in practice (the damage path rejects it before
            // reading this), so the material is only nominal. Stone reads
            // right for a weathered strongbox.
            Self::RuinCache => DestructibleMaterial::Stone,
            // A placed charge is deliberately fragile: cloth means any tool or
            // projectile shreds it in a couple of hits, which is exactly the
            // defender's fizzle counterplay. The material only gates how easy the
            // charge is to knock out; the blast it deals is the separate
            // `explosive_effectiveness_pct` matrix keyed on the target's material.
            Self::Explosive { .. } => DestructibleMaterial::Cloth,
        }
    }

    /// True for the entity kinds anyone may damage, regardless of who
    /// placed them. Raid targets (building blocks, doors, sleeping bags)
    /// must be damageable by non-owners or raiding can't exist; utility
    /// stations (workbench, furnace) keep the owner-only damage gate so
    /// griefers can't idly chew through someone's crafting corner.
    pub const fn raidable(self) -> bool {
        matches!(
            self,
            Self::Building { .. }
                | Self::Door { .. }
                | Self::SleepingBag
                | Self::ToolCupboard
                // A placed charge must be damageable by any defender so they can
                // shoot/hit it to fizzle it, regardless of who armed it. Without
                // this it would fall under the owner-only damage gate and a
                // defender could never disarm an attacker's charge.
                | Self::Explosive { .. }
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::{HEWN_LOG_DOOR_ID, IRON_DOOR_ID, item_definition};

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
        // The wood door chips under tools (WoodBuilding); the iron door is
        // metal, which every tool does 0 to.
        assert_eq!(
            DeployableKind::Door {
                variant: DoorVariant::HewnLog
            }
            .material(),
            DestructibleMaterial::WoodBuilding
        );
        assert_eq!(
            DeployableKind::Door {
                variant: DoorVariant::Iron
            }
            .material(),
            DestructibleMaterial::MetalBuilding
        );
    }

    #[test]
    fn door_variants_resolve_their_item_and_hp() {
        assert_eq!(DoorVariant::HewnLog.item_id(), HEWN_LOG_DOOR_ID);
        assert_eq!(DoorVariant::Iron.item_id(), IRON_DOOR_ID);
        // The iron door carries double the wood door's HP.
        assert_eq!(
            DoorVariant::Iron.max_hp(),
            DoorVariant::HewnLog.max_hp() * 2
        );
        // The registry item HP agrees with the variant.
        assert_eq!(
            item_definition(IRON_DOOR_ID)
                .and_then(|d| d.deployable)
                .map(|p| p.max_health),
            Some(DoorVariant::Iron.max_hp())
        );
    }
}
