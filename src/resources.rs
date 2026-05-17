use crate::{
    items::{COAL_ID, IRON_ORE_ID, SULFUR_ORE_ID, ToolKind, ToolProfile, WOOD_ID, look_forward},
    protocol::{ItemStack, ResourceNodeState, Vec3Net},
    world::WorldResourceNodeSpawn,
};

pub const COAL_NODE_ID: &str = "coal_node";
pub const IRON_NODE_ID: &str = "iron_node";
pub const SULFUR_NODE_ID: &str = "sulfur_node";
pub const PINE_TREE_NODE_ID: &str = "pine_tree";
pub const BIRCH_TREE_NODE_ID: &str = "birch_tree";
pub const DEAD_TREE_NODE_ID: &str = "dead_tree";

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
    PineTree,
    BirchTree,
    DeadTree,
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
        tool.kind == self.kind && tool.tier >= self.min_tier
    }

    pub fn label(self) -> String {
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
        storage: &[ResourceMaterial::new(COAL_ID, 24)],
        anchor_height: 0.62,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: IRON_NODE_ID,
        name: "Iron Node",
        model: ResourceNodeModel::IronOre,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        storage: &[ResourceMaterial::new(IRON_ORE_ID, 24)],
        anchor_height: 0.66,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: SULFUR_NODE_ID,
        name: "Sulfur Node",
        model: ResourceNodeModel::SulfurOre,
        required_tool: ToolRequirement::new(ToolKind::Pickaxe, 1),
        storage: &[ResourceMaterial::new(SULFUR_ORE_ID, 24)],
        anchor_height: 0.58,
        ray_radius: 0.72,
    },
    ResourceNodeDefinition {
        id: PINE_TREE_NODE_ID,
        name: "Pine Tree",
        model: ResourceNodeModel::PineTree,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 32)],
        anchor_height: 1.35,
        ray_radius: 0.82,
    },
    ResourceNodeDefinition {
        id: BIRCH_TREE_NODE_ID,
        name: "Birch Tree",
        model: ResourceNodeModel::BirchTree,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 28)],
        anchor_height: 1.25,
        ray_radius: 0.78,
    },
    ResourceNodeDefinition {
        id: DEAD_TREE_NODE_ID,
        name: "Dead Tree",
        model: ResourceNodeModel::DeadTree,
        required_tool: ToolRequirement::new(ToolKind::Axe, 1),
        storage: &[ResourceMaterial::new(WOOD_ID, 18)],
        anchor_height: 1.05,
        ray_radius: 0.72,
    },
];

pub fn resource_node_definition(definition_id: &str) -> Option<&'static ResourceNodeDefinition> {
    RESOURCE_NODE_DEFINITIONS
        .iter()
        .find(|definition| definition.id == definition_id)
}

pub fn spawn_resource_node(spawn: &WorldResourceNodeSpawn) -> Option<ResourceNodeState> {
    let definition = resource_node_definition(&spawn.definition_id)?;
    Some(ResourceNodeState {
        id: spawn.id,
        definition_id: definition.id.to_owned(),
        position: spawn.position,
        yaw: spawn.yaw,
        storage: definition
            .storage
            .iter()
            .map(|material| ItemStack::new(material.item_id, material.quantity))
            .collect(),
    })
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
        if stack.item_id != item_id || quantity == 0 {
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
    fn resource_target_uses_view_ray_and_range() {
        let node = ResourceNodeState {
            id: 1,
            definition_id: COAL_NODE_ID.to_owned(),
            position: Vec3Net::new(0.0, 0.0, -2.2),
            yaw: 0.0,
            storage: vec![ItemStack::new(COAL_ID, 1)],
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
