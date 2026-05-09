use crate::protocol::{DroppedWorldItem, ItemStack, Vec3Net};

pub const TEST_ORE_ID: &str = "test_ore";
pub const TEST_BANDAGE_ID: &str = "test_bandage";
pub const TEST_RELIC_ID: &str = "test_relic";

pub const PICKUP_RANGE: f32 = 3.4;
const PICKUP_RAY_RADIUS: f32 = 0.58;
const PICKUP_ANCHOR_HEIGHT: f32 = 0.28;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemModel {
    Bag,
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemDefinition {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub stack_size: u16,
    pub equipable: bool,
    pub model: ItemModel,
    pub tint: ItemTint,
}

impl ItemDefinition {
    pub fn effective_stack_size(self) -> u16 {
        if self.equipable {
            1
        } else {
            self.stack_size.max(1)
        }
    }
}

pub const REGISTERED_ITEMS: &[ItemDefinition] = &[
    ItemDefinition {
        id: TEST_ORE_ID,
        name: "Test Ore",
        description: "A stackable mineral used to exercise inventory merging.",
        stack_size: 20,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(111, 174, 226),
    },
    ItemDefinition {
        id: TEST_BANDAGE_ID,
        name: "Test Bandage",
        description: "A compact stackable utility item for split-stack controls.",
        stack_size: 8,
        equipable: false,
        model: ItemModel::Bag,
        tint: ItemTint::new(226, 202, 143),
    },
    ItemDefinition {
        id: TEST_RELIC_ID,
        name: "Test Relic",
        description: "An equipable placeholder item that renders in first person.",
        stack_size: 99,
        equipable: true,
        model: ItemModel::Bag,
        tint: ItemTint::new(183, 136, 229),
    },
];

pub fn item_definition(item_id: &str) -> Option<&'static ItemDefinition> {
    REGISTERED_ITEMS
        .iter()
        .find(|definition| definition.id == item_id)
}

pub fn stack_limit(item_id: &str) -> Option<u16> {
    item_definition(item_id).map(|definition| definition.effective_stack_size())
}

pub fn normalize_stack(stack: &ItemStack) -> Option<ItemStack> {
    let limit = stack_limit(&stack.item_id)?;
    let quantity = stack.quantity.clamp(1, limit);
    Some(ItemStack::new(stack.item_id.clone(), quantity))
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
    let forward = look_forward(yaw, pitch);
    if forward.length_squared() <= f32::EPSILON {
        return None;
    }

    let to_item = pickup_anchor(item).minus(eye);
    let projection = to_item.dot(forward);
    if !(0.0..=PICKUP_RANGE).contains(&projection) {
        return None;
    }

    let closest = eye.plus(forward.scale(projection));
    let lateral = pickup_anchor(item).minus(closest);
    if lateral.length_squared() > PICKUP_RAY_RADIUS * PICKUP_RAY_RADIUS {
        return None;
    }

    Some(projection)
}

pub fn can_pick_up(eye: Vec3Net, yaw: f32, pitch: f32, item: &DroppedWorldItem) -> bool {
    pickup_score(eye, yaw, pitch, item).is_some()
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
    fn equipable_items_force_stack_size_one() {
        assert_eq!(stack_limit(TEST_RELIC_ID), Some(1));
        assert_eq!(stack_limit(TEST_ORE_ID), Some(20));
        assert_eq!(
            normalize_stack(&ItemStack::new(TEST_RELIC_ID, 40)),
            Some(ItemStack::new(TEST_RELIC_ID, 1))
        );
    }

    #[test]
    fn pickup_target_uses_view_ray_and_range() {
        let item = DroppedWorldItem {
            id: 1,
            stack: ItemStack::new(TEST_ORE_ID, 1),
            position: Vec3Net::new(0.0, 0.0, -2.0),
            yaw: 0.0,
            rotation: QuatNet::IDENTITY,
        };
        let eye = Vec3Net::new(0.0, 0.6, 0.0);

        assert!(can_pick_up(eye, 0.0, -0.16, &item));
        assert!(!can_pick_up(eye, std::f32::consts::PI, -0.16, &item));
    }
}
