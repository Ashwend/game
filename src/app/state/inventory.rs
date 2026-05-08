use bevy::prelude::*;
use bevy_egui::egui;

use crate::protocol::{DroppedItemId, ItemContainerSlot, ItemStack, Vec3Net};

#[derive(Resource, Default)]
pub(crate) struct InventoryUiState {
    pub(crate) drag: Option<InventoryDrag>,
    pub(crate) hovered_slot: Option<ItemContainerSlot>,
    pub(crate) inventory_rect: Option<egui::Rect>,
    pub(crate) actionbar_rect: Option<egui::Rect>,
    pub(crate) was_open: bool,
}

impl InventoryUiState {
    pub(crate) fn begin_frame(&mut self) {
        self.hovered_slot = None;
        self.inventory_rect = None;
        self.actionbar_rect = None;
    }

    pub(crate) fn cancel_drag(&mut self) {
        self.drag = None;
    }
}

#[derive(Debug, Clone)]
pub(crate) struct InventoryDrag {
    pub(crate) source: ItemContainerSlot,
    pub(crate) stack: ItemStack,
    pub(crate) quantity: u16,
    pub(crate) button: InventoryDragButton,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InventoryDragButton {
    Primary,
    Secondary,
}

#[derive(Resource, Debug, Clone, Default)]
pub(crate) struct PickupTargetState {
    pub(crate) dropped_item_id: Option<DroppedItemId>,
    pub(crate) stack: Option<ItemStack>,
    pub(crate) world_position: Option<Vec3Net>,
    pub(crate) screen_position: Option<Vec2>,
}

impl PickupTargetState {
    pub(crate) fn clear(&mut self) {
        self.dropped_item_id = None;
        self.stack = None;
        self.world_position = None;
        self.screen_position = None;
    }
}
