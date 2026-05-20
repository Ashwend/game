use bevy::prelude::*;

use crate::{
    items::{ItemModel, ToolKind},
    protocol::{DroppedItemId, ItemStack, ResourceNodeId, Vec3Net},
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

/// Impact a remote player produced on a tree or ore node. Written by the
/// network tick when a `ServerMessage::ResourceImpact` arrives so the
/// audio and visual effect systems can render the same hit feedback we
/// produce locally — minus the camera kick, which belongs to the swinger.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct RemoteImpactEvent {
    pub(crate) anchor: Vec3,
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

// Tool-swap entry animation tuning. Lighter items reach rest faster; the
// pickaxe is heavy enough that lifting it off the player's back should feel
// like effort, but not so long that it becomes annoying.
const SWAP_DURATION_BAG: f32 = 0.20;
const SWAP_DURATION_HATCHET: f32 = 0.24;
const SWAP_DURATION_PICKAXE: f32 = 0.42;

pub(crate) fn swap_duration_for_model(model: ItemModel) -> f32 {
    match model {
        ItemModel::Bag => SWAP_DURATION_BAG,
        ItemModel::Hatchet => SWAP_DURATION_HATCHET,
        ItemModel::Pickaxe => SWAP_DURATION_PICKAXE,
    }
}

/// Tracks the animation that plays when a new item enters the player's hand
/// — used to lock out tool swings while the new tool is still being lifted
/// into view, and to drive the held-item visual offset.
#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct ToolSwapState {
    current: Option<String>,
    elapsed: f32,
    duration: f32,
}

impl ToolSwapState {
    pub(crate) fn reset(&mut self) {
        self.current = None;
        self.elapsed = 0.0;
        self.duration = 0.0;
    }

    /// Returns `0.0` when the tool has just started entering view and `1.0`
    /// once it has fully settled into the rest pose.
    pub(crate) fn fraction(&self) -> f32 {
        if self.duration <= 0.0 {
            return 1.0;
        }
        (self.elapsed / self.duration).clamp(0.0, 1.0)
    }

    pub(crate) fn is_swapping(&self) -> bool {
        self.duration > 0.0 && self.elapsed < self.duration
    }

    /// Step the animation forward, or reset to a new tool if the active
    /// item has changed since the last tick.
    pub(crate) fn observe(&mut self, delta_seconds: f32, active: Option<(&str, ItemModel)>) {
        match (self.current.as_deref(), active) {
            (None, None) => {
                self.elapsed = 0.0;
                self.duration = 0.0;
            }
            (Some(_), None) => {
                self.reset();
            }
            (None, Some((id, model))) => {
                self.current = Some(id.to_owned());
                self.duration = swap_duration_for_model(model);
                self.elapsed = 0.0;
            }
            (Some(old), Some((new_id, model))) if old != new_id => {
                self.current = Some(new_id.to_owned());
                self.duration = swap_duration_for_model(model);
                self.elapsed = 0.0;
            }
            (Some(_), Some(_)) => {
                self.elapsed = (self.elapsed + delta_seconds.max(0.0)).min(self.duration);
            }
        }
    }
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
    /// Seconds since the last full pickup-target scan. The scan is throttled
    /// to ~33 ms (≈ 30 Hz) — that's well above the cadence a player can
    /// react to a tooltip highlight and saves an O(N×M) sweep over every
    /// dropped item and resource node every render frame.
    pub(crate) elapsed_since_scan: f32,
}

/// Minimum interval between pickup-target scans. 33ms keeps the scan at
/// roughly 30Hz regardless of frame rate.
pub(crate) const PICKUP_TARGET_SCAN_INTERVAL_SECS: f32 = 0.033;

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

    /// Inspect the pending impact without clearing it. Used by the sound
    /// system so it can spawn an audio cue alongside the visual chips that
    /// the effects system spawns when it takes the impact later in the same
    /// frame.
    pub(crate) fn peek_pending_impact(&self) -> Option<&PendingImpactEffect> {
        self.pending_impact.as_ref()
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
    use crate::protocol::{ItemStack, Vec3Net};

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
            elapsed_since_scan: 0.0,
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
