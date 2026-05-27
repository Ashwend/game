use crate::{
    items::{
        COAL_ID, FIBER_ID, IRON_ORE_ID, STONE_ID, SULFUR_ORE_ID, ToolKind, ToolProfile, WOOD_ID,
        look_forward,
    },
    protocol::{ItemStack, ResourceNodeState, Vec3Net},
    world::{WorldBlock, WorldResourceNodeSpawn},
};

pub const COAL_NODE_ID: &str = "coal_node";
pub const IRON_NODE_ID: &str = "iron_node";
pub const SULFUR_NODE_ID: &str = "sulfur_node";
/// Mineable bare-rock vein. Visually a chunkless ore pile (all stone),
/// pickaxe-required, more frequent than coal/iron/sulfur veins. Bridges
/// the "1 stone from hand-pickup" → "you need a steady stone supply"
/// gap before the player has access to an ore vein.
pub const STONE_NODE_ID: &str = "stone_node";
// Tree IDs: the un-suffixed names (`pine_tree`, `birch_tree`) are the
// medium variants. Old saves that referenced these IDs before size
// variants existed continue to load as medium without migration.
pub const PINE_TREE_SMALL_NODE_ID: &str = "pine_tree_small";
pub const PINE_TREE_NODE_ID: &str = "pine_tree";
pub const PINE_TREE_LARGE_NODE_ID: &str = "pine_tree_large";
pub const BIRCH_TREE_SMALL_NODE_ID: &str = "birch_tree_small";
pub const BIRCH_TREE_NODE_ID: &str = "birch_tree";
pub const BIRCH_TREE_LARGE_NODE_ID: &str = "birch_tree_large";
/// Hand-harvestable starter materials so a fresh player can craft their
/// first crude tools without already owning a tool.
pub const SURFACE_STONE_NODE_ID: &str = "surface_stone";
pub const BRANCH_PILE_NODE_ID: &str = "branch_pile";
pub const HAY_GRASS_NODE_ID: &str = "hay_grass";

pub const RESOURCE_GATHER_RANGE: f32 = 3.75;
const DEFAULT_RESOURCE_RAY_RADIUS: f32 = 0.7;
// Loose upper bound used only for the cheap distance cull in
// `resource_node_score`. Must be >= any definition's `ray_radius`; correctness
// of the actual ray test does not depend on it.
const MAX_RESOURCE_RAY_RADIUS: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceNodeModel {
    CoalOre,
    IronOre,
    SulfurOre,
    /// Bare-rock vein — pickaxe-mineable, yields plain `stone`. Same
    /// silhouette as the ore variants but no embedded coal/iron/sulfur
    /// chunks on top, so it reads as "the rock under the ore".
    StoneVein,
    PineTreeSmall,
    PineTreeMedium,
    PineTreeLarge,
    BirchTreeSmall,
    BirchTreeMedium,
    BirchTreeLarge,
    /// Small rock lump sitting on the ground. E-pickup only.
    SurfaceStone,
    /// Bundle of fallen sticks. E-pickup only.
    BranchPile,
    /// Tuft of long grass. E-pickup only.
    HayGrass,
}

impl ResourceNodeModel {
    pub fn is_tree(self) -> bool {
        matches!(
            self,
            Self::PineTreeSmall
                | Self::PineTreeMedium
                | Self::PineTreeLarge
                | Self::BirchTreeSmall
                | Self::BirchTreeMedium
                | Self::BirchTreeLarge
        )
    }

    pub fn is_ore(self) -> bool {
        matches!(self, Self::CoalOre | Self::IronOre | Self::SulfurOre)
    }

    /// Crude, hand-harvestable starter resource (branch pile, surface
    /// stone, hay tuft). Used by the gather pipeline to skip tool checks
    /// and by the renderer to scale meshes smaller than full trees/ore.
    pub fn is_crude(self) -> bool {
        matches!(self, Self::SurfaceStone | Self::BranchPile | Self::HayGrass)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolRequirement {
    pub kind: ToolKind,
    pub min_tier: u8,
}

impl ToolRequirement {
    pub const fn new(kind: ToolKind, min_tier: u8) -> Self {
        Self { kind, min_tier }
    }

    pub fn allows(self, tool: ToolProfile) -> bool {
        // A `Hands` requirement means "this node can't be swung at — pick
        // it up with E". Swinging any tool (or empty hands) at a Hands
        // node is rejected so the player learns to use the quick-pickup
        // key for crude clutter. The matching `kind == Hands` check is
        // still what gates the E pickup path on both client and server.
        if self.kind == ToolKind::Hands {
            return false;
        }
        tool.kind == self.kind && tool.tier >= self.min_tier
    }

    pub fn label(self) -> String {
        if self.kind == ToolKind::Hands {
            return "Pick up with E".to_owned();
        }
        format!("{} tier {}", self.kind.label(), self.min_tier)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceMaterial {
    pub item_id: &'static str,
    pub quantity: u16,
}

impl ResourceMaterial {
    pub const fn new(item_id: &'static str, quantity: u16) -> Self {
        Self { item_id, quantity }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceNodeDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub model: ResourceNodeModel,
    pub required_tool: ToolRequirement,
    pub storage: &'static [ResourceMaterial],
    pub anchor_height: f32,
    pub ray_radius: f32,
}

pub const RESOURCE_NODE_DEFINITIONS: &[ResourceNodeDefinition] = &[
    ResourceNodeDefinition {
        id: COAL_NODE_ID,
        name: "Coal Node",
        model: ResourceNodeModel::CoalOre,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        storage: &[ResourceMaterial::new(COAL_ID, 72)],
        anchor_height: 0.62,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: IRON_NODE_ID,
        name: "Iron Node",
        model: ResourceNodeModel::IronOre,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        storage: &[ResourceMaterial::new(IRON_ORE_ID, 72)],
        anchor_height: 0.66,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: SULFUR_NODE_ID,
        name: "Sulfur Node",
        model: ResourceNodeModel::SulfurOre,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        storage: &[ResourceMaterial::new(SULFUR_ORE_ID, 72)],
        anchor_height: 0.58,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: STONE_NODE_ID,
        name: "Stone Vein",
        model: ResourceNodeModel::StoneVein,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        // Bigger than ore (96 stone = 16 swings at 6/swing) so the player
        // can stock up on stone for crafting without juggling pickup
        // rocks, but still finite enough to require moving around.
        storage: &[ResourceMaterial::new(STONE_ID, 96)],
        anchor_height: 0.60,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: PINE_TREE_SMALL_NODE_ID,
        name: "Pine Sapling",
        model: ResourceNodeModel::PineTreeSmall,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 24)],
        anchor_height: 1.35,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: PINE_TREE_NODE_ID,
        name: "Pine Tree",
        model: ResourceNodeModel::PineTreeMedium,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 48)],
        anchor_height: 1.45,
        ray_radius: 0.86,
    },
    ResourceNodeDefinition {
        id: PINE_TREE_LARGE_NODE_ID,
        name: "Old Pine",
        model: ResourceNodeModel::PineTreeLarge,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 84)],
        anchor_height: 1.55,
        ray_radius: 1.05,
    },
    ResourceNodeDefinition {
        id: BIRCH_TREE_SMALL_NODE_ID,
        name: "Birch Sapling",
        model: ResourceNodeModel::BirchTreeSmall,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 18)],
        anchor_height: 1.25,
        ray_radius: 0.68,
    },
    ResourceNodeDefinition {
        id: BIRCH_TREE_NODE_ID,
        name: "Birch Tree",
        model: ResourceNodeModel::BirchTreeMedium,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 42)],
        anchor_height: 1.40,
        ray_radius: 0.82,
    },
    ResourceNodeDefinition {
        id: BIRCH_TREE_LARGE_NODE_ID,
        name: "Old Birch",
        model: ResourceNodeModel::BirchTreeLarge,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 72)],
        anchor_height: 1.50,
        ray_radius: 0.98,
    },
    ResourceNodeDefinition {
        id: SURFACE_STONE_NODE_ID,
        name: "Loose Stone",
        model: ResourceNodeModel::SurfaceStone,
        required_tool: ToolRequirement::new(ToolKind::Hands, 0),
        storage: &[ResourceMaterial::new(STONE_ID, 1)],
        anchor_height: 0.18,
        ray_radius: 0.55,
    },
    ResourceNodeDefinition {
        id: BRANCH_PILE_NODE_ID,
        name: "Branch Pile",
        model: ResourceNodeModel::BranchPile,
        required_tool: ToolRequirement::new(ToolKind::Hands, 0),
        storage: &[ResourceMaterial::new(WOOD_ID, 1)],
        anchor_height: 0.16,
        ray_radius: 0.55,
    },
    ResourceNodeDefinition {
        id: HAY_GRASS_NODE_ID,
        name: "Tall Grass",
        model: ResourceNodeModel::HayGrass,
        required_tool: ToolRequirement::new(ToolKind::Hands, 0),
        storage: &[ResourceMaterial::new(FIBER_ID, 1)],
        anchor_height: 0.22,
        ray_radius: 0.45,
    },
];

/// Build-once `id → definition` lookup over [`RESOURCE_NODE_DEFINITIONS`].
/// `resource_node_definition` is on the swing/gather hot path; the linear
/// scan was fine for nine definitions but O(1) is free here.
fn resource_node_definitions_by_id()
-> &'static std::collections::HashMap<&'static str, &'static ResourceNodeDefinition> {
    static INDEX: std::sync::OnceLock<
        std::collections::HashMap<&'static str, &'static ResourceNodeDefinition>,
    > = std::sync::OnceLock::new();
    INDEX.get_or_init(|| {
        RESOURCE_NODE_DEFINITIONS
            .iter()
            .map(|definition| (definition.id, definition))
            .collect()
    })
}

pub fn resource_node_definition(definition_id: &str) -> Option<&'static ResourceNodeDefinition> {
    resource_node_definitions_by_id()
        .get(definition_id)
        .copied()
}

pub fn spawn_resource_node(spawn: &WorldResourceNodeSpawn) -> Option<ResourceNodeState> {
    let definition = resource_node_definition(&spawn.definition_id)?;
    Some(ResourceNodeState {
        id: spawn.id,
        definition_id: definition.id.to_owned(),
        position: spawn.position,
        yaw: spawn.yaw,
        storage: definition_storage_stacks(definition),
        respawn_progress: None,
    })
}

/// Build the freshly-spawned storage payload for a resource definition.
/// Called both at world generation time and when a node finishes
/// regenerating after being mined out.
pub fn definition_storage_stacks(definition: &ResourceNodeDefinition) -> Vec<ItemStack> {
    definition
        .storage
        .iter()
        .map(|material| ItemStack::new(material.item_id, material.quantity))
        .collect()
}

pub fn resource_node_anchor(node: &ResourceNodeState) -> Vec3Net {
    let height = resource_node_definition(&node.definition_id)
        .map(|definition| definition.anchor_height)
        .unwrap_or(0.6);
    node.position.plus(Vec3Net::new(0.0, height, 0.0))
}

pub fn resource_node_score(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    node: &ResourceNodeState,
) -> Option<f32> {
    // Regenerating nodes have no payout and shouldn't appear as gather
    // targets — the look-tooltip and swing dispatch both pivot on this.
    if node.respawn_progress.is_some() {
        return None;
    }
    let anchor = resource_node_anchor(node);
    let to_node = anchor.minus(eye);
    // Cheap distance cull before the trig in `look_forward` and the definition
    // lookup. Uses a conservative upper bound on ray_radius so it never rejects
    // a candidate the actual ray test would accept.
    let max_reach_sq = (RESOURCE_GATHER_RANGE + MAX_RESOURCE_RAY_RADIUS).powi(2);
    if to_node.length_squared() > max_reach_sq {
        return None;
    }

    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }
    let projection = to_node.dot(forward);
    if !(0.0..=RESOURCE_GATHER_RANGE).contains(&projection) {
        return None;
    }

    let ray_radius = resource_node_definition(&node.definition_id)
        .map(|definition| definition.ray_radius)
        .unwrap_or(DEFAULT_RESOURCE_RAY_RADIUS);
    let closest = eye.plus(forward.scale(projection));
    let lateral = anchor.minus(closest);
    if lateral.length_squared() > ray_radius * ray_radius {
        return None;
    }

    Some(projection)
}

pub fn can_gather_resource_node(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    node: &ResourceNodeState,
) -> bool {
    resource_node_score(eye, yaw, pitch, node).is_some()
}

pub fn best_resource_node_target<'a>(
    eye: Vec3Net,
    yaw: f32,
    pitch: f32,
    nodes: impl Iterator<Item = &'a ResourceNodeState>,
) -> Option<(&'a ResourceNodeState, f32)> {
    nodes
        .filter_map(|node| resource_node_score(eye, yaw, pitch, node).map(|score| (node, score)))
        .min_by(|(_, a), (_, b)| a.total_cmp(b))
}

pub fn next_resource_payout(node: &ResourceNodeState, tool: ToolProfile) -> Option<ItemStack> {
    let quantity = tool.gather_amount.max(1);
    node.storage
        .iter()
        .find(|stack| stack.quantity > 0)
        .map(|stack| ItemStack::new(stack.item_id.clone(), stack.quantity.min(quantity)))
}

pub fn remove_resource_from_storage(
    node: &mut ResourceNodeState,
    item_id: &str,
    mut quantity: u16,
) {
    for stack in &mut node.storage {
        if stack.item_id.as_ref() != item_id || quantity == 0 {
            continue;
        }
        let removed = stack.quantity.min(quantity);
        stack.quantity -= removed;
        quantity -= removed;
    }
    node.storage.retain(|stack| stack.quantity > 0);
}

pub fn resource_storage_is_empty(node: &ResourceNodeState) -> bool {
    node.storage.iter().all(|stack| stack.quantity == 0)
}

/// Returns an AABB collider for a live resource node, or `None` if the
/// node has no definition.
///
/// Trees get a vertical pillar at the trunk base, slightly wider than the
/// visible trunk so the player and camera don't clip the bark when brushing
/// past. Height is fixed at 3m — taller than the player AABB so the player
/// can't walk over or under it, but well below the canopy so the player's
/// bounding box never touches foliage.
///
/// Ores get a short square box sized to the rock-lump footprint. The
/// `add_rock_lump` mesh sits with its base at local y=0 and tops out
/// around y≈0.6–0.7, so the collider is centered at y=0.32 with half-
/// height 0.32 to span the visible rock without poking the floor or
/// floating above the peak.
pub fn resource_node_collider(node: &ResourceNodeState) -> Option<WorldBlock> {
    // Regenerating nodes are visual ghosts only — let players walk through
    // them so the area doesn't stay blocked while a tree regrows.
    if node.respawn_progress.is_some() {
        return None;
    }
    let definition = resource_node_definition(&node.definition_id)?;
    match definition.model {
        ResourceNodeModel::PineTreeSmall
        | ResourceNodeModel::PineTreeMedium
        | ResourceNodeModel::PineTreeLarge
        | ResourceNodeModel::BirchTreeSmall
        | ResourceNodeModel::BirchTreeMedium
        | ResourceNodeModel::BirchTreeLarge => Some(tree_collider_block(node, definition.model)),
        ResourceNodeModel::CoalOre
        | ResourceNodeModel::IronOre
        | ResourceNodeModel::SulfurOre
        | ResourceNodeModel::StoneVein => Some(ore_collider_block(node)),
        // Crude clutter (surface stones, branch piles, hay tufts) is
        // walk-through — small enough that a collider feels buggy and the
        // player needs to be able to stand on top to interact.
        ResourceNodeModel::SurfaceStone
        | ResourceNodeModel::BranchPile
        | ResourceNodeModel::HayGrass => None,
    }
}

fn tree_collider_block(node: &ResourceNodeState, model: ResourceNodeModel) -> WorldBlock {
    let half_width = match model {
        ResourceNodeModel::PineTreeSmall => 0.30,
        ResourceNodeModel::PineTreeMedium => 0.36,
        ResourceNodeModel::PineTreeLarge => 0.46,
        ResourceNodeModel::BirchTreeSmall => 0.24,
        ResourceNodeModel::BirchTreeMedium => 0.28,
        ResourceNodeModel::BirchTreeLarge => 0.34,
        _ => unreachable!("tree_collider_block called with non-tree model"),
    };
    let half_height = 1.5;
    let center = Vec3Net::new(node.position.x, half_height, node.position.z);
    let half_extents = Vec3Net::new(half_width, half_height, half_width);
    WorldBlock::new(center, half_extents)
}

fn ore_collider_block(node: &ResourceNodeState) -> WorldBlock {
    let half_width = 0.55;
    let half_height = 0.32;
    let center = Vec3Net::new(node.position.x, half_height, node.position.z);
    let half_extents = Vec3Net::new(half_width, half_height, half_width);
    WorldBlock::new(center, half_extents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_payout_uses_tool_amount_and_storage_remaining() {
        let mut node = ResourceNodeState {
            id: 1,
            definition_id: COAL_NODE_ID.to_owned(),
            position: Vec3Net::ZERO,
            yaw: 0.0,
            storage: vec![ItemStack::new(COAL_ID, 5)],
            respawn_progress: None,
        };
        let tool = ToolProfile {
            kind: ToolKind::Pickaxe,
            tier: 1,
            gather_amount: 3,
            cooldown_ticks: 1,
        };

        assert_eq!(
            next_resource_payout(&node, tool),
            Some(ItemStack::new(COAL_ID, 3))
        );
        remove_resource_from_storage(&mut node, COAL_ID, 3);
        assert_eq!(
            next_resource_payout(&node, tool),
            Some(ItemStack::new(COAL_ID, 2))
        );
    }

    #[test]
    fn ore_collider_sits_on_ground_and_covers_visible_rock() {
        for ore_id in [COAL_NODE_ID, IRON_NODE_ID, SULFUR_NODE_ID] {
            let node = ResourceNodeState {
                id: 1,
                definition_id: ore_id.to_owned(),
                position: Vec3Net::new(5.0, 0.0, -3.0),
                yaw: 0.0,
                storage: Vec::new(),
                respawn_progress: None,
            };
            let collider = resource_node_collider(&node)
                .unwrap_or_else(|| panic!("expected collider for {ore_id}"));
            let min = collider.min();
            let max = collider.max();
            assert_eq!(
                min.y, 0.0,
                "{ore_id} collider must sit on the ground, got min.y = {}",
                min.y
            );
            // Bounds should be square in x/z and tall enough to cover the
            // rock-lump peak at ~y=0.58 without poking through the floor.
            assert!(max.y >= 0.58, "{ore_id} collider too short: {}", max.y);
            assert!(max.y <= 0.8, "{ore_id} collider too tall: {}", max.y);
            assert_eq!(collider.half_extents.x, collider.half_extents.z);
            assert!(min.x < node.position.x && max.x > node.position.x);
            assert!(min.z < node.position.z && max.z > node.position.z);
        }
    }

    #[test]
    fn tree_models_still_return_tall_pillar_colliders() {
        let node = ResourceNodeState {
            id: 1,
            definition_id: PINE_TREE_NODE_ID.to_owned(),
            position: Vec3Net::new(0.0, 0.0, 0.0),
            yaw: 0.0,
            storage: Vec::new(),
            respawn_progress: None,
        };
        let collider = resource_node_collider(&node).expect("tree should have a collider");
        let size = collider.size();
        assert!(size.y >= 2.5, "tree collider should be a tall pillar");
        assert!(size.x < 1.0 && size.z < 1.0, "tree pillar should be narrow");
        assert_eq!(collider.min().y, 0.0);
    }

    #[test]
    fn resource_target_uses_view_ray_and_range() {
        let node = ResourceNodeState {
            id: 1,
            definition_id: COAL_NODE_ID.to_owned(),
            position: Vec3Net::new(0.0, 0.0, -2.2),
            yaw: 0.0,
            storage: vec![ItemStack::new(COAL_ID, 1)],
            respawn_progress: None,
        };
        let eye = Vec3Net::new(0.0, 1.62, 0.0);

        assert!(can_gather_resource_node(eye, 0.0, -0.42, &node));
        assert!(!can_gather_resource_node(
            eye,
            std::f32::consts::PI,
            -0.42,
            &node
        ));
    }
}
