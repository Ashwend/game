use bevy::prelude::*;
use bevy_egui::egui;

use crate::{
    items::ToolKind,
    protocol::{DroppedItemId, ItemContainerSlot, ItemStack, ResourceNodeId, Vec3Net},
};

const AXE_SWING_SECONDS: f32 = 0.50;
const AXE_IMPACT_FRACTION: f32 = 0.50;
const PICKAXE_SWING_SECONDS: f32 = 1.60;
const PICKAXE_IMPACT_FRACTION: f32 = 0.68;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImpactEffectKind {
    WoodChips,
    StoneShards,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingImpactEffect {
    pub(crate) anchor: Vec3,
    pub(crate) spray_direction: Vec3,
    pub(crate) kind: ImpactEffectKind,
    pub(crate) seed: u32,
}

pub(crate) fn swing_duration_seconds(tool: ToolKind) -> f32 {
    match tool {
        ToolKind::Axe => AXE_SWING_SECONDS,
        ToolKind::Pickaxe => PICKAXE_SWING_SECONDS,
    }
}

pub(crate) fn swing_impact_fraction(tool: ToolKind) -> f32 {
    match tool {
        ToolKind::Axe => AXE_IMPACT_FRACTION,
        ToolKind::Pickaxe => PICKAXE_IMPACT_FRACTION,
    }
}

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
    pub(crate) resource_node_id: Option<ResourceNodeId>,
    pub(crate) resource_definition_id: Option<String>,
    pub(crate) resource_storage: Vec<ItemStack>,
    pub(crate) world_position: Option<Vec3Net>,
    pub(crate) screen_position: Option<Vec2>,
}

impl PickupTargetState {
    pub(crate) fn clear(&mut self) {
        self.dropped_item_id = None;
        self.stack = None;
        self.resource_node_id = None;
        self.resource_definition_id = None;
        self.resource_storage.clear();
        self.world_position = None;
        self.screen_position = None;
    }
}

#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct GatherInputState {
    active: Option<ActiveSwing>,
    pending_impact: Option<PendingImpactEffect>,
    swing_seed: u32,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSwing {
    tool: ToolKind,
    duration: f32,
    impact_fraction: f32,
    elapsed: f32,
    impact_handled: bool,
    seed: u32,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SwingImpact {
    pub(crate) target: Option<ResourceNodeId>,
    pub(crate) tool: ToolKind,
}

impl GatherInputState {
    /// Drive the swing animation and resolve impacts.
    ///
    /// The tool always swings on left-click as long as a tool is equipped.
    /// On the impact frame, the swing emits a [`SwingImpact`] whose `target`
    /// is `Some` only if a valid resource target is in view — that is the
    /// signal to dispatch a gather command. Visual impact effects are
    /// queued separately via [`Self::set_pending_impact`].
    pub(crate) fn update(
        &mut self,
        delta_seconds: f32,
        just_pressed: bool,
        pressed: bool,
        equipped_tool: Option<ToolKind>,
        target: Option<ResourceNodeId>,
    ) -> Option<SwingImpact> {
        if self.active.is_none()
            && (just_pressed || pressed)
            && let Some(tool) = equipped_tool
        {
            self.start_swing(tool);
        }

        let mut active = self.active?;
        let previous_elapsed = active.elapsed;
        active.elapsed = (active.elapsed + delta_seconds.max(0.0)).min(active.duration);

        let impact_time = active.duration * active.impact_fraction;
        let crossed_impact = !active.impact_handled
            && previous_elapsed < impact_time
            && active.elapsed >= impact_time;

        let impact = if crossed_impact {
            active.impact_handled = true;
            Some(SwingImpact {
                target,
                tool: active.tool,
            })
        } else {
            None
        };

        if active.elapsed >= active.duration {
            if pressed && let Some(tool) = equipped_tool {
                // Continue swinging while LMB is held.
                self.start_swing(tool);
            } else {
                self.active = None;
                return impact;
            }
        } else {
            self.active = Some(active);
        }

        impact
    }

    pub(crate) fn cancel(&mut self) {
        self.active = None;
        self.pending_impact = None;
    }

    fn start_swing(&mut self, tool: ToolKind) {
        self.swing_seed = self.swing_seed.wrapping_add(1);
        self.active = Some(ActiveSwing {
            tool,
            duration: swing_duration_seconds(tool),
            impact_fraction: swing_impact_fraction(tool),
            elapsed: 0.0,
            impact_handled: false,
            seed: self.swing_seed,
        });
    }

    pub(crate) fn swing_fraction(&self) -> f32 {
        match self.active {
            Some(active) if active.duration > 0.0 => {
                (active.elapsed / active.duration).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    pub(crate) fn set_pending_impact(&mut self, impact: PendingImpactEffect) {
        self.pending_impact = Some(impact);
    }

    pub(crate) fn take_pending_impact(&mut self) -> Option<PendingImpactEffect> {
        self.pending_impact.take()
    }

    pub(crate) fn current_swing_seed(&self) -> u32 {
        self.active
            .map(|swing| swing.seed)
            .unwrap_or(self.swing_seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{ItemContainerSlot, ItemStack, Vec3Net};

    #[test]
    fn inventory_ui_state_resets_frame_and_drag_state() {
        let mut state = InventoryUiState {
            drag: Some(InventoryDrag {
                source: ItemContainerSlot::inventory(2),
                stack: ItemStack::new("ore", 4),
                quantity: 2,
                button: InventoryDragButton::Secondary,
            }),
            hovered_slot: Some(ItemContainerSlot::actionbar(1)),
            inventory_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(10.0, 10.0),
            )),
            actionbar_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(5.0, 5.0),
            )),
            was_open: true,
        };

        state.begin_frame();

        assert!(state.hovered_slot.is_none());
        assert!(state.inventory_rect.is_none());
        assert!(state.actionbar_rect.is_none());
        assert!(state.drag.is_some());
        assert!(state.was_open);

        state.cancel_drag();
        assert!(state.drag.is_none());
    }

    #[test]
    fn pickup_target_clear_removes_cached_target() {
        let mut state = PickupTargetState {
            dropped_item_id: Some(7),
            stack: Some(ItemStack::new("ore", 1)),
            resource_node_id: Some(8),
            resource_definition_id: Some("node".to_owned()),
            resource_storage: vec![ItemStack::new("wood", 2)],
            world_position: Some(Vec3Net::new(1.0, 2.0, 3.0)),
            screen_position: Some(Vec2::new(10.0, 20.0)),
        };

        state.clear();

        assert!(state.dropped_item_id.is_none());
        assert!(state.stack.is_none());
        assert!(state.resource_node_id.is_none());
        assert!(state.resource_definition_id.is_none());
        assert!(state.resource_storage.is_empty());
        assert!(state.world_position.is_none());
        assert!(state.screen_position.is_none());
    }

    #[test]
    fn gather_input_sends_at_swing_impact_and_repeats_while_held() {
        let mut state = GatherInputState::default();
        let tool = ToolKind::Axe;
        let duration = swing_duration_seconds(tool);
        let impact_time = duration * swing_impact_fraction(tool);

        assert!(
            state
                .update(0.01, true, true, Some(tool), Some(4))
                .is_none()
        );
        assert!(state.swing_fraction() > 0.0);

        let impact = state
            .update(impact_time, false, true, Some(tool), Some(4))
            .expect("impact should emit at the impact fraction of the swing");
        assert_eq!(impact.target, Some(4));
        assert_eq!(impact.tool, tool);
        assert!(
            state
                .update(0.01, false, true, Some(tool), Some(4))
                .is_none()
        );

        let _ = state.update(duration, false, true, Some(tool), Some(5));
        // Swing rolled over into a new swing while LMB is held.
        assert!(state.swing_fraction() < 0.2);
    }

    #[test]
    fn gather_input_swings_without_target_and_yields_no_impact() {
        let mut state = GatherInputState::default();
        let tool = ToolKind::Pickaxe;
        let duration = swing_duration_seconds(tool);
        let impact_time = duration * swing_impact_fraction(tool);

        // Click with no target — swing still starts.
        let _ = state.update(0.01, true, true, Some(tool), None);
        assert!(state.swing_fraction() > 0.0);

        // Crossing the impact fraction emits a SwingImpact with no target.
        let impact = state
            .update(impact_time, false, true, Some(tool), None)
            .expect("impact frame should still fire");
        assert!(impact.target.is_none());
        assert_eq!(impact.tool, tool);
    }

    #[test]
    fn gather_input_does_nothing_without_a_tool_equipped() {
        let mut state = GatherInputState::default();
        assert!(state.update(0.01, true, true, None, Some(4)).is_none());
        assert_eq!(state.swing_fraction(), 0.0);
    }
}
