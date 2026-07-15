//! Shared mesh/material handles for every placeable structure, plus the tier /
//! variant lookup helpers on [`DeployableVisualAssets`]. Built once by
//! `setup_scene` (see `assets.rs`); consumed by the deployable mirror spawn,
//! the placement ghost, and the world-scene ruin spawns.

use bevy::prelude::*;

use super::toon::ToonMaterial;

#[derive(Resource, Clone)]
pub(crate) struct DeployableVisualAssets {
    pub(crate) workbench_mesh: Handle<Mesh>,
    /// Tier-2 workbench model (heavier bench + anvil + vice + bolted frame +
    /// lower-shelf clutter), swapped in by [`Self::workbench_mesh`] when a placed
    /// bench is upgraded. Same footprint + ground origin as tier 1, so the
    /// placement collider is unchanged. Source: `art/deployables/build_deployables.py`.
    pub(crate) workbench_t2_mesh: Handle<Mesh>,
    pub(crate) furnace_mesh: Handle<Mesh>,
    /// Building piece meshes indexed `[piece][tier]` via
    /// [`Self::building_mesh`]. Authored Blender glbs built from the same box
    /// layout as the collision grid, so the silhouette agrees with what
    /// blocks movement. Source: `art/building/build_pieces.py`.
    pub(crate) building_meshes: [[Handle<Mesh>; 3]; 6],
    /// Textured tier materials (twig / hewn timber / coursed stone) indexed by
    /// [`crate::building::BuildingTier`], applied to every building piece of
    /// that tier (the glb COLOR_0 multiplies them).
    pub(crate) building_materials: [Handle<StandardMaterial>; 3],
    /// Authored door panel glbs per variant (hinge at origin, spans +X),
    /// spawned as an animated child of the door root entity. UV-unwrapped +
    /// COLOR_0, paired with the textured `*_door_material` below. Sources:
    /// `art/building/build_door.py`.
    pub(crate) hewn_door_panel_mesh: Handle<Mesh>,
    pub(crate) iron_door_panel_mesh: Handle<Mesh>,
    /// Textured door materials (base-white + plank/plate texture, COLOR_0
    /// tints the frame/braces/straps), one per variant.
    pub(crate) hewn_door_material: Handle<StandardMaterial>,
    pub(crate) iron_door_material: Handle<StandardMaterial>,
    /// Door placement ghost: closed panel + swing-arc indicator (procedural,
    /// shared by both variants; the ghost is a translucent preview).
    pub(crate) door_ghost_mesh: Handle<Mesh>,
    pub(crate) sleeping_bag_mesh: Handle<Mesh>,
    /// Authored storage box models (Blender glbs, vertex-coloured like
    /// the workbench/furnace).
    pub(crate) storage_box_small_mesh: Handle<Mesh>,
    pub(crate) storage_box_large_mesh: Handle<Mesh>,
    /// Procedural torch haft + head (origin at the base so it mounts on the
    /// ground or tilts off a wall about its foot).
    pub(crate) torch_mesh: Handle<Mesh>,
    /// Placed-charge body meshes (primitive 0 of each explosive glb): the staved
    /// keg barrel, the strapped satchel pack, and the cloth bomb. Sources:
    /// `assets/items/{powder_keg,satchel_charge,powder_bomb}/model.glb`.
    pub(crate) charge_keg_mesh: Handle<Mesh>,
    pub(crate) charge_satchel_mesh: Handle<Mesh>,
    pub(crate) charge_bomb_mesh: Handle<Mesh>,
    /// Authored Tool Cupboard model (Blender glb, vertex-coloured like the
    /// workbench/furnace; origin at the base so it sits on a foundation).
    pub(crate) tool_cupboard_mesh: Handle<Mesh>,
    /// The salvage chest spawned inside burnt-house ruins (Blender glb under
    /// `assets/ruins/`, vertex-coloured for cel identity: charred wood body
    /// with near-black iron bands). Rendered on the toon wood material.
    /// Source: `art/ruins/build_ruins.py`.
    pub(crate) ruin_cache_mesh: Handle<Mesh>,
    /// Burnt-house shell meshes, one `(timber, masonry)` primitive pair per
    /// [`crate::world::RuinPrefab`], indexed by `RuinPrefab::index()`. Prim 0
    /// is the charred timber (toon wood material), prim 1 the stone plinth +
    /// rubble (toon stone material). Sources:
    /// `assets/ruins/burnt_{cottage,farmhouse,shed,barn}.glb`, built by
    /// `art/ruins/build_ruins.py`.
    pub(crate) ruin_house_meshes: [(Handle<Mesh>, Handle<Mesh>); 4],
    /// Cel-shaded wood material (hand-painted plank line-art, UV-mapped) for the
    /// wooden deployables: workbench, storage boxes, tool cupboard, torch.
    pub(crate) toon_wood_material: Handle<ToonMaterial>,
    /// Cel-shaded stone material (hand-painted cobble line-art, UV-mapped) for the
    /// crude furnace.
    pub(crate) toon_stone_material: Handle<ToonMaterial>,
    /// Cel-shaded fabric material (woven-quilt line-art, UV-mapped) for the
    /// sleeping bag bedroll. See [Toon / cel shading](../../../docs/toon-shading.md).
    pub(crate) toon_fabric_material: Handle<ToonMaterial>,
    /// Woven-cloth cel material for placed cloth-bodied charges (the powder bomb
    /// and the satchel), the shared fabric tile tinted by each glb's COLOR_0.
    pub(crate) charge_cloth_material: Handle<ToonMaterial>,
    /// Semi-transparent green tint used by the placement ghost when the
    /// slot is valid. Mirrors the convention from popular survival games
    ///, green means "click to place", we pair it with a slight pulse.
    pub(crate) ghost_valid_material: Handle<StandardMaterial>,
    /// Red variant for invalid placement (out of reach, overlapping).
    pub(crate) ghost_invalid_material: Handle<StandardMaterial>,
    /// Punchier green/red ghost variants for the SMALL placed charges (keg /
    /// satchel): a knee-high prop at arm's reach vanishes into tall grass at
    /// the building ghost's subtle alpha, so the charge preview runs a higher
    /// alpha and a stronger emissive to stay legible.
    pub(crate) ghost_valid_charge_material: Handle<StandardMaterial>,
    pub(crate) ghost_invalid_charge_material: Handle<StandardMaterial>,
}

impl DeployableVisualAssets {
    pub(crate) fn building_mesh(
        &self,
        piece: crate::building::BuildingPiece,
        tier: crate::building::BuildingTier,
    ) -> Handle<Mesh> {
        use crate::building::{BuildingPiece, BuildingTier};
        let piece_index = match piece {
            BuildingPiece::Foundation => 0,
            BuildingPiece::Wall => 1,
            BuildingPiece::WindowWall => 2,
            BuildingPiece::Doorway => 3,
            BuildingPiece::Ceiling => 4,
            BuildingPiece::Stairs => 5,
        };
        let tier_index = match tier {
            BuildingTier::Sticks => 0,
            BuildingTier::HewnWood => 1,
            BuildingTier::Stone => 2,
        };
        self.building_meshes[piece_index][tier_index].clone()
    }

    /// Textured material for a building tier.
    pub(crate) fn building_material(
        &self,
        tier: crate::building::BuildingTier,
    ) -> Handle<StandardMaterial> {
        use crate::building::BuildingTier;
        let index = match tier {
            BuildingTier::Sticks => 0,
            BuildingTier::HewnWood => 1,
            BuildingTier::Stone => 2,
        };
        self.building_materials[index].clone()
    }

    /// Storage box mesh for a tier (1 = small, 2+ = large).
    pub(crate) fn storage_box_mesh(&self, tier: u8) -> Handle<Mesh> {
        if tier >= 2 {
            self.storage_box_large_mesh.clone()
        } else {
            self.storage_box_small_mesh.clone()
        }
    }

    /// Workbench mesh for a tier: the heavier anvil-and-vice bench at tier 2, the
    /// plank bench at tier 1. Centralised here so the upgrade's mirror respawn and
    /// the placement ghost both pick the right model with no scattered `if tier`.
    pub(crate) fn workbench_mesh(&self, tier: u8) -> Handle<Mesh> {
        match tier {
            2 => self.workbench_t2_mesh.clone(),
            _ => self.workbench_mesh.clone(),
        }
    }

    /// Authored panel mesh for a door variant.
    pub(crate) fn door_panel_mesh(&self, variant: crate::items::DoorVariant) -> Handle<Mesh> {
        match variant {
            crate::items::DoorVariant::HewnLog => self.hewn_door_panel_mesh.clone(),
            crate::items::DoorVariant::Iron => self.iron_door_panel_mesh.clone(),
        }
    }

    /// Textured material for a door variant.
    pub(crate) fn door_material(
        &self,
        variant: crate::items::DoorVariant,
    ) -> Handle<StandardMaterial> {
        match variant {
            crate::items::DoorVariant::HewnLog => self.hewn_door_material.clone(),
            crate::items::DoorVariant::Iron => self.iron_door_material.clone(),
        }
    }
}
