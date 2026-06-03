use bevy::prelude::*;

use crate::{
    app::audio::surface::SurfaceMaterial,
    game_balance::COMBAT_MISS_RECOVERY_SECONDS,
    items::{DeployableKind, ItemModel, ToolKind},
    protocol::{ClientId, DeployedEntityId, DroppedItemId, ItemStack, ResourceNodeId, Vec3Net},
};

// Each tool has its own swing length and impact moment. Pickaxe swings are
// slower and connect later in their arc than the lighter hatchet. Audio
// (both hit and miss whoosh) and visual feedback all fire from the same
// impact crossing, so there's exactly one decision per swing and the two
// sounds can never play together.
//
// The impact fraction must match the phase at which the tool's pose bottoms
// out in `swing_poses.rs`, otherwise the chop/impact sound drifts off the
// visual contact. The hatchet is a deliberate heavy chop: a long wind-up that
// hangs over the shoulder, then a fast strike that lands at pose phase 0.58
// (see `hatchet_swing_pose`). The swing is slow enough (0.78s) that it, not
// the 0.30s server gather cooldown, gates the attack cadence, so the heavy
// tempo is what the player feels. Keep these two constants in step.
const AXE_SWING_SECONDS: f32 = 0.78;
const AXE_IMPACT_FRACTION: f32 = 0.58;
const PICKAXE_SWING_SECONDS: f32 = 1.60;
const PICKAXE_IMPACT_FRACTION: f32 = 0.68;
// Bare-hand "punch", short, snappy. Sits between the axe and the
// pickaxe so a sequence of hand-picks feels purposeful but doesn't lose
// rhythm next to a hatchet swing.
const HANDS_SWING_SECONDS: f32 = 0.42;
const HANDS_IMPACT_FRACTION: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImpactEffectKind {
    /// Heavy wood-chip burst, full tree-felling palette, used when a
    /// hatchet bites into a trunk or for branch-pile *deaths* if those
    /// ever scaled up.
    WoodChips,
    /// Heavy stone-shard burst, full ore-pickaxe palette.
    StoneShards,
    /// Small wood-toned burst for branch piles. Same palette as
    /// `WoodChips` but lower count/size/lifetime so picking up a
    /// stick pile feels lighter than felling a tree.
    Sticks,
    /// Small stone-toned burst for surface rocks. Same palette as
    /// `StoneShards` but lower count/size/lifetime.
    Pebbles,
    /// Tiny green burst for hay/grass tufts, uses a dedicated
    /// grass-coloured material and a very small footprint.
    GrassBlades,
    /// Small reddish-grey dust burst for PvP melee hits. Lower particle
    /// count + shorter lifetime than `StoneShards`, with a warmer
    /// (reddish) tint so a player-hit reads differently from a stone
    /// chip. Phase 4 wires the dedicated colour ramp into the particle
    /// system; until then the palette mapping below falls back to the
    /// stone-shard material so the swing still produces visible chips.
    FleshHit,
}

impl ImpactEffectKind {
    /// Map a surface material to the visual particle palette. Used by
    /// remote-impact events that only carry the surface; the
    /// crude-material variants are unreachable through this entry point
    /// because their nodes route through
    /// [`ImpactEffectKind::for_resource_impact`] instead.
    pub(crate) fn for_surface(surface: SurfaceMaterial) -> Self {
        match surface {
            SurfaceMaterial::Wood => Self::WoodChips,
            SurfaceMaterial::Dirt
            | SurfaceMaterial::Concrete
            | SurfaceMaterial::Sand
            | SurfaceMaterial::Stone
            | SurfaceMaterial::Iron
            | SurfaceMaterial::Coal
            | SurfaceMaterial::Sulfur => Self::StoneShards,
        }
    }

    /// Map a resource node model to the visual particle palette. Used at
    /// the swing-impact site because the model carries enough information
    /// to distinguish a full tree (heavy WoodChips) from a branch pile
    /// (light Sticks), an ore vein from a surface rock, and grass tufts
    /// from anything else.
    pub(crate) fn for_resource_model(model: crate::resources::ResourceNodeModel) -> Self {
        use crate::resources::ResourceNodeModel::*;
        match model {
            PineTreeSmall | PineTreeMedium | PineTreeLarge | BirchTreeSmall | BirchTreeMedium
            | BirchTreeLarge => Self::WoodChips,
            CoalOre | IronOre | SulfurOre | StoneVein => Self::StoneShards,
            BranchPile => Self::Sticks,
            SurfaceStone => Self::Pebbles,
            HayGrass => Self::GrassBlades,
        }
    }

    /// Map a wire-side `ResourceImpactKind` to the visual palette. Used
    /// at the remote-impact receive site, which has the protocol kind
    /// directly without going through model/surface lookups.
    pub(crate) fn for_resource_impact(kind: crate::protocol::ResourceImpactKind) -> Self {
        use crate::protocol::ResourceImpactKind;
        match kind {
            ResourceImpactKind::Tree => Self::WoodChips,
            ResourceImpactKind::CoalOre
            | ResourceImpactKind::IronOre
            | ResourceImpactKind::SulfurOre
            | ResourceImpactKind::StoneVein => Self::StoneShards,
            ResourceImpactKind::Branches => Self::Sticks,
            ResourceImpactKind::SurfaceStone => Self::Pebbles,
            ResourceImpactKind::HayGrass => Self::GrassBlades,
        }
    }
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
/// produce locally, minus the camera kick, which belongs to the swinger.
///
/// `tool` + `surface` drive the audio system's per-pair impact pool
/// lookup; the visual `effect_kind` is computed at the receive site
/// from the wire [`crate::protocol::ResourceImpactKind`] so crude
/// materials (branch piles, surface stones, hay tufts) can still get
/// their dedicated small bursts even though they share surfaces with
/// the heavier tree/ore types.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct RemoteImpactEvent {
    pub(crate) anchor: Vec3,
    pub(crate) tool: ToolKind,
    pub(crate) surface: SurfaceMaterial,
    pub(crate) effect_kind: ImpactEffectKind,
    pub(crate) seed: u32,
    /// When `true` the audio dispatcher routes through
    /// `impact_sound_for_player` instead of the `(tool, surface)`
    /// lookup, picking the dedicated PvP-impact pool. The `surface`
    /// field is still set (used by visuals fallback) but ignored by
    /// the audio path in this case.
    pub(crate) is_player_hit: bool,
}

pub(crate) fn swing_duration_seconds(tool: ToolKind) -> f32 {
    match tool {
        ToolKind::Hands => HANDS_SWING_SECONDS,
        ToolKind::Axe => AXE_SWING_SECONDS,
        ToolKind::Pickaxe => PICKAXE_SWING_SECONDS,
    }
}

pub(crate) fn swing_impact_fraction(tool: ToolKind) -> f32 {
    match tool {
        ToolKind::Hands => HANDS_IMPACT_FRACTION,
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
        // Deployables are bulky like the bag, same lift cadence keeps
        // them feeling consistent without a bespoke pose.
        ItemModel::Bag | ItemModel::Deployable => SWAP_DURATION_BAG,
        ItemModel::Hatchet => SWAP_DURATION_HATCHET,
        ItemModel::Pickaxe => SWAP_DURATION_PICKAXE,
    }
}

/// Tracks the animation that plays when a new item enters the player's hand
///, used to lock out tool swings while the new tool is still being lifted
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
    /// Placed structure the player is currently looking at (workbench,
    /// furnace, …). Used by the interact handler to route the E key to
    /// the right entity-specific action (e.g. open furnace UI).
    pub(crate) deployable_id: Option<DeployedEntityId>,
    pub(crate) deployable_kind: Option<DeployableKind>,
    /// Remote player the swing would land on. Set when the look ray
    /// intersects a remote player's body AABB within attack range. The
    /// swing dispatch routes through `dispatch_player_swing` when this
    /// is `Some`, sending `ClientMessage::AttackPlayer` to the server.
    pub(crate) player_id: Option<ClientId>,
    /// Loot bag the player is currently looking at. Drives the E-to-
    /// open path, pressing E sends `LootBagCommand::Open` and the
    /// transfer UI panel becomes visible via `PlayerPrivate.open_loot_bag`.
    pub(crate) loot_bag_id: Option<crate::protocol::LootBagId>,
    /// Seconds since the last full pickup-target scan. The scan is throttled
    /// to ~33 ms (≈ 30 Hz), that's well above the cadence a player can
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
        self.deployable_id = None;
        self.deployable_kind = None;
        self.player_id = None;
        self.loot_bag_id = None;
    }
}

#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct GatherInputState {
    active: Option<ActiveSwing>,
    pending_impact: Option<PendingImpactEffect>,
    pending_audio_cue: Option<PendingAudioCue>,
    pending_miss_audio: bool,
    swing_seed: u32,
    /// Seconds of swing lockout left after a whiff. A missed swing sets
    /// this to [`COMBAT_MISS_RECOVERY_SECONDS`] once it finishes; while
    /// it ticks down no new swing can start, so spraying at empty air
    /// costs tempo. A landed swing never sets it, so timing your hits
    /// keeps full cadence.
    recovery_remaining: f32,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSwing {
    tool: ToolKind,
    duration: f32,
    impact_time: f32,
    elapsed: f32,
    impact_handled: bool,
    seed: u32,
    /// Set at the impact frame when the swing connected with nothing.
    /// Read when the swing completes to decide whether the post-swing
    /// recovery gap applies before the next swing may start.
    missed: bool,
}

/// What the swing connected with at its impact frame. Resource nodes
/// run through the existing gather command (item payout + node
/// shrink); placed structures run through a damage command (no
/// payout, just HP decrement + auto-destroy on 0); players run
/// through `ClientMessage::AttackPlayer` (HP decrement + knockback,
/// no payout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SwingTarget {
    ResourceNode(ResourceNodeId),
    Deployable(DeployedEntityId),
    Player(ClientId),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SwingImpact {
    pub(crate) target: Option<SwingTarget>,
    pub(crate) tool: ToolKind,
}

/// Anchor + (tool, surface) for a hit sound queued from the impact
/// dispatcher. The audio system drains it the same frame to spawn a
/// spatial sound, the pair drives selection of the per-tool, per-surface
/// impact pool independently of the visual particle kind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingAudioCue {
    pub(crate) anchor: Vec3,
    pub(crate) tool: ToolKind,
    pub(crate) surface: SurfaceMaterial,
    /// Same flag as [`RemoteImpactEvent::is_player_hit`], when `true`
    /// the audio system picks the PvP-impact pool.
    pub(crate) is_player_hit: bool,
}

impl GatherInputState {
    /// Drive the swing animation and resolve impacts.
    ///
    /// The tool always swings on left-click as long as a tool is equipped.
    /// At the impact frame the swing emits one [`SwingImpact`] whose `target`
    /// is `Some` for a hit and `None` for a miss. The dispatcher decides hit
    /// vs. miss feedback (audio + visual + gather command) from that single
    /// event, so there's no separate audio path that could double-fire.
    ///
    /// A whiff (impact frame with `target == None`) imposes a
    /// [`COMBAT_MISS_RECOVERY_SECONDS`] recovery gap before the next swing
    /// can start: holding LMB through a miss no longer rolls straight into
    /// the next swing, so spray-and-pray costs tempo while a player who
    /// times their hits keeps full cadence.
    pub(crate) fn update(
        &mut self,
        delta_seconds: f32,
        just_pressed: bool,
        pressed: bool,
        equipped_tool: Option<ToolKind>,
        target: Option<SwingTarget>,
    ) -> Option<SwingImpact> {
        let delta = delta_seconds.max(0.0);

        // Burn down the post-miss recovery only in the gap between swings;
        // it never overlaps an active swing (a miss ends the swing before
        // the gap starts), but the guard keeps the bookkeeping obvious.
        if self.active.is_none() && self.recovery_remaining > 0.0 {
            self.recovery_remaining = (self.recovery_remaining - delta).max(0.0);
        }

        if self.active.is_none()
            && self.recovery_remaining <= 0.0
            && (just_pressed || pressed)
            && let Some(tool) = equipped_tool
        {
            self.start_swing(tool);
        }

        let mut active = self.active?;
        let previous_elapsed = active.elapsed;
        active.elapsed = (active.elapsed + delta).min(active.duration);

        let crossed_impact = !active.impact_handled
            && previous_elapsed < active.impact_time
            && active.elapsed >= active.impact_time;
        let impact = if crossed_impact {
            active.impact_handled = true;
            // A whiff is remembered here (at the impact frame, the same
            // instant the dispatcher reads the target) so the recovery gap
            // below keys off the moment of contact, not whatever the player
            // happens to be aiming at when the animation finally ends.
            active.missed = target.is_none();
            Some(SwingImpact {
                target,
                tool: active.tool,
            })
        } else {
            None
        };

        if active.elapsed >= active.duration {
            if active.missed {
                // Whiffed: charge the recovery gap and stop here. Holding
                // LMB resumes only after `recovery_remaining` ticks out.
                self.recovery_remaining = COMBAT_MISS_RECOVERY_SECONDS;
                self.active = None;
            } else if pressed && let Some(tool) = equipped_tool {
                // Landed (or hit something): continue swinging while LMB is
                // held, no recovery penalty.
                self.start_swing(tool);
            } else {
                self.active = None;
            }
            return impact;
        }

        self.active = Some(active);
        impact
    }

    pub(crate) fn cancel(&mut self) {
        self.active = None;
        self.pending_impact = None;
        self.pending_audio_cue = None;
        self.pending_miss_audio = false;
        // A tool swap or death clears the swing entirely, including any
        // pending miss penalty, the next tool shouldn't inherit a stun.
        self.recovery_remaining = 0.0;
    }

    fn start_swing(&mut self, tool: ToolKind) {
        self.swing_seed = self.swing_seed.wrapping_add(1);
        let duration = swing_duration_seconds(tool);
        let impact_time = duration * swing_impact_fraction(tool);
        self.active = Some(ActiveSwing {
            tool,
            duration,
            impact_time,
            elapsed: 0.0,
            impact_handled: false,
            seed: self.swing_seed,
            missed: false,
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

    pub(crate) fn set_pending_audio_cue(&mut self, cue: PendingAudioCue) {
        self.pending_audio_cue = Some(cue);
    }

    pub(crate) fn take_pending_audio_cue(&mut self) -> Option<PendingAudioCue> {
        self.pending_audio_cue.take()
    }

    pub(crate) fn set_pending_miss_audio(&mut self) {
        self.pending_miss_audio = true;
    }

    pub(crate) fn take_pending_miss_audio(&mut self) -> bool {
        std::mem::take(&mut self.pending_miss_audio)
    }

    pub(crate) fn current_swing_seed(&self) -> u32 {
        self.active
            .map(|swing| swing.seed)
            .unwrap_or(self.swing_seed)
    }
}

#[cfg(test)]
mod tests;
