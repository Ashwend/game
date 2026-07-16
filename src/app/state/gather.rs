use bevy::prelude::*;

use crate::{
    app::audio::surface::SurfaceMaterial,
    game_balance::COMBAT_MISS_RECOVERY_SECONDS,
    items::{DeployableKind, ItemModel},
    protocol::{ClientId, DeployedEntityId, DroppedItemId, ItemStack, ResourceNodeId, Vec3Net},
};

// Each swing archetype has its own length and impact moment, keyed on
// [`ItemModel`] (the same archetype the first-person pose dispatch and the
// tool-swap lift read) so a tool and a weapon that animate the same way share
// one timing. Pickaxe swings are slower and connect later in their arc than the
// lighter hatchet; the four weapons stake out distinct points across that
// spectrum. Audio (both hit and miss whoosh) and visual feedback all fire from
// the same impact crossing, so there's exactly one decision per swing and the
// two sounds can never play together.
//
// The impact fraction must match the phase at which the model's pose bottoms out
// in `swing_poses.rs`, otherwise the impact sound drifts off the visual contact.
// The hatchet is a deliberate heavy chop: a long wind-up that hangs over the
// shoulder, then a fast strike that lands at pose phase 0.58 (see
// `hatchet_swing_pose`). The swing is slow enough (0.78s) that it, not the 0.30s
// server gather cooldown, gates the attack cadence, so the heavy tempo is what
// the player feels. Keep each duration/fraction pair in step with its pose.
const AXE_SWING_SECONDS: f32 = 0.78;
const AXE_IMPACT_FRACTION: f32 = 0.58;
const PICKAXE_SWING_SECONDS: f32 = 1.60;
const PICKAXE_IMPACT_FRACTION: f32 = 0.68;
// Bag/deployable-in-hand and bare hands share this short, snappy "punch". Sits
// between the axe and the pickaxe so a sequence of hand-picks feels purposeful
// but doesn't lose rhythm next to a hatchet swing. (Bare hands never swing
// locally; the remote path animates a received hands swing at this cadence.)
const BAG_SWING_SECONDS: f32 = 0.42;
const BAG_IMPACT_FRACTION: f32 = 0.55;

// The four melee weapons. Each duration is coherent with the weapon's server
// cooldown (duration ~= cooldown_ticks / SERVER_TICK_RATE_HZ), so the swing the
// player feels tracks the anti-spam floor, and each pair matches its pose in
// `swing_poses.rs`. The ordering (club fastest, then sword, spear, mace) mirrors
// the cooldown ordering in game_balance. The mace is deliberately a touch slower
// than its cooldown alone would imply, its huge wind-up and follow-through are
// its identity.
const CLUB_SWING_SECONDS: f32 = 0.42;
const CLUB_IMPACT_FRACTION: f32 = 0.45;
const SWORD_SWING_SECONDS: f32 = 0.46;
// Contact is when the blade CROSSES the crosshair mid-sweep (the slash carries
// on across the whole frame after the hit), not the end of the travel.
const SWORD_IMPACT_FRACTION: f32 = 0.34;
const SPEAR_SWING_SECONDS: f32 = 0.62;
const SPEAR_IMPACT_FRACTION: f32 = 0.55;
const MACE_SWING_SECONDS: f32 = 0.95;
const MACE_IMPACT_FRACTION: f32 = 0.70;

// The sickle's reaping cut: brisker than the hatchet chop (a light crescent,
// not a heavy head) but more committed than the sword's whip, and contact is
// the crescent crossing under the crosshair mid-sweep (see
// `sickle_swing_pose`).
const SICKLE_SWING_SECONDS: f32 = 0.62;
const SICKLE_IMPACT_FRACTION: f32 = 0.42;

// The two ranged archetypes. A draw is NOT a swing: the hold is driven by
// `RangedDrawState` (src/app/state/ranged.rs) and never enters the swing state
// machine, so these table entries are the FIRE-RECOVERY beats: the short
// post-release settle window (mirroring the viewmodel's release flick / recoil
// kick) that keeps the archetype's row in the timing tables real rather than a
// bag placeholder. The bow's recovery matches its string-snap settle; the
// crossbow's is longer, the punchier recoil of the heavier bolt. Both contact
// fractions sit early: the shot leaves at the start of the beat, the rest is
// recovery.
const BOW_FIRE_RECOVERY_SECONDS: f32 = 0.35;
const BOW_FIRE_IMPACT_FRACTION: f32 = 0.20;
const CROSSBOW_FIRE_RECOVERY_SECONDS: f32 = 0.50;
const CROSSBOW_FIRE_IMPACT_FRACTION: f32 = 0.15;

// The thrown bomb's toss beat: a short wind-up-and-release lob. Unlike a melee
// swing it does not connect (no target); this is the recovery beat that plays
// the toss pose and gates re-throw so a bomb cannot be spammed faster than its
// window. The release (the moment the bomb leaves the hand) lands at the impact
// fraction, where the release cue fires and the toss pose bottoms out. Kept a
// touch heavier than the bag punch so a lob reads as committed.
const THROW_BOMB_SECONDS: f32 = 0.55;
const THROW_BOMB_IMPACT_FRACTION: f32 = 0.45;

/// Attacker-side hit-stop window, in seconds: on a locally-predicted CONFIRMED
/// player hit the swing's phase advance is held for this long, a few frames of
/// viewmodel freeze that sells the weight of contact (the same "brief hit-stop
/// on the attacker's viewmodel" the feel spec calls for). Deliberately short so
/// it reads as impact heft, not a stutter; the Dev `hit_stop_scale` slider scales
/// it live (0 disables) for tuning. Client-side viewmodel feel only, it never
/// touches the server, the wire, or the swing's server-side cooldown. A hit-stop
/// only lengthens the local animation, so the next `SwingStart`/impact simply
/// fires a hair later; peers are unaffected.
pub(crate) const HIT_STOP_SECONDS: f32 = 0.05;

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
    pub(crate) fn for_resource_model(model: crate::resource_nodes::ResourceNodeModel) -> Self {
        use crate::resource_nodes::ResourceNodeModel::*;
        match model {
            PineTreeSmall | PineTreeMedium | PineTreeLarge | BirchTreeSmall | BirchTreeMedium
            | BirchTreeLarge => Self::WoodChips,
            CoalOre | IronOre | SulfurOre | StoneVein | Meteorite => Self::StoneShards,
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

/// Impact a remote player produced on a tree or ore node, or a PvP hit. Written
/// by the network tick when a `ServerMessage::ResourceImpact` or `PlayerImpact`
/// arrives so the audio and visual effect systems can render the same hit
/// feedback we produce locally, minus the camera kick, which belongs to the
/// swinger.
///
/// For a resource impact, `surface` drives the audio system's `(tool_kind,
/// surface)` impact-pool lookup (via `tool.as_tool_kind()`); the visual
/// `effect_kind` is computed at the receive site from the wire
/// [`crate::protocol::ResourceImpactKind`] so crude materials (branch piles,
/// surface stones, hay tufts) can still get their dedicated small bursts even
/// though they share surfaces with the heavier tree/ore types. For a PvP hit
/// (`is_player_hit`), the audio routes off `model` through the per-weapon PvP
/// pool instead.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct RemoteImpactEvent {
    pub(crate) anchor: Vec3,
    /// The swing's impact identity (weapon or gather-tool archetype). Drives PvP
    /// audio directly, and the resource-impact audio via its underlying tool kind.
    pub(crate) model: ItemModel,
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

/// Swing length in seconds for an [`ItemModel`] archetype. Keyed on the model
/// (not the tool kind) so tools and weapons that animate the same way share one
/// timing and a new weapon is a table row here plus its pose in `swing_poses.rs`.
/// The existing Bag/Hatchet/Pickaxe values are preserved exactly.
pub(crate) fn swing_duration_seconds(model: ItemModel) -> f32 {
    match model {
        // Bag/deployable-in-hand swings at the short "punch" cadence (the same
        // one bare hands use on the remote path).
        ItemModel::Bag | ItemModel::Deployable => BAG_SWING_SECONDS,
        ItemModel::Hatchet => AXE_SWING_SECONDS,
        ItemModel::Pickaxe => PICKAXE_SWING_SECONDS,
        ItemModel::Club => CLUB_SWING_SECONDS,
        ItemModel::Spear => SPEAR_SWING_SECONDS,
        ItemModel::Sword => SWORD_SWING_SECONDS,
        ItemModel::Mace => MACE_SWING_SECONDS,
        ItemModel::Sickle => SICKLE_SWING_SECONDS,
        // Ranged weapons don't swing: the draw hold lives in `RangedDrawState`.
        // These entries are the fire-recovery beats (see the constants above).
        ItemModel::Bow => BOW_FIRE_RECOVERY_SECONDS,
        ItemModel::Crossbow => CROSSBOW_FIRE_RECOVERY_SECONDS,
        // The thrown bomb's lob beat (wind-up + release + recovery).
        ItemModel::ThrownBomb => THROW_BOMB_SECONDS,
        // The bandage does not swing at all: its whole animation is the use
        // charge, which lives in `ConsumeChargeState`, not the swing clock. This
        // entry is never consulted for a real swing (the input layer claims the
        // frame before the melee path runs); it exists only to keep the table
        // total, so it reports the inert bag beat.
        ItemModel::Bandage => BAG_SWING_SECONDS,
    }
}

/// Contact fraction (0..1 of the swing) for an [`ItemModel`] archetype, matched
/// to the phase its pose bottoms out at in `swing_poses.rs`.
pub(crate) fn swing_impact_fraction(model: ItemModel) -> f32 {
    match model {
        ItemModel::Bag | ItemModel::Deployable => BAG_IMPACT_FRACTION,
        ItemModel::Hatchet => AXE_IMPACT_FRACTION,
        ItemModel::Pickaxe => PICKAXE_IMPACT_FRACTION,
        ItemModel::Club => CLUB_IMPACT_FRACTION,
        ItemModel::Spear => SPEAR_IMPACT_FRACTION,
        ItemModel::Sword => SWORD_IMPACT_FRACTION,
        ItemModel::Mace => MACE_IMPACT_FRACTION,
        ItemModel::Sickle => SICKLE_IMPACT_FRACTION,
        // Ranged fire-recovery beats: the shot leaves early in the beat, the rest
        // of the window is the settle (see the constants above).
        ItemModel::Bow => BOW_FIRE_IMPACT_FRACTION,
        ItemModel::Crossbow => CROSSBOW_FIRE_IMPACT_FRACTION,
        // The bomb leaves the hand at the toss pose's release point.
        ItemModel::ThrownBomb => THROW_BOMB_IMPACT_FRACTION,
        // The bandage never swings, so it has no contact frame. See
        // `swing_duration_seconds`.
        ItemModel::Bandage => BAG_IMPACT_FRACTION,
    }
}

/// Dev combat-feel scales threaded from `settings.dev.combat` into a swing so the
/// live tuning panel can stretch the timing and shift the contact cue without a
/// recompile. Neutral (`swing_duration_scale = 1.0`, `impact_fraction_offset = 0.0`)
/// reproduces the shipped timing byte-for-byte, so a release build (Dev tab hidden)
/// and an untouched dev session are identical. Kept as a tiny plain struct so the
/// scaling math is unit-testable in isolation from the swing state machine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SwingFeelScales {
    pub(crate) duration_scale: f32,
    pub(crate) impact_fraction_offset: f32,
}

impl Default for SwingFeelScales {
    fn default() -> Self {
        Self {
            duration_scale: 1.0,
            impact_fraction_offset: 0.0,
        }
    }
}

impl SwingFeelScales {
    /// The tuned swing duration for `model`: the base duration scaled by the dev
    /// multiplier. The multiplier is guarded to a positive finite value so a
    /// pathological setting can never freeze a swing (a `0` or NaN scale falls
    /// back to neutral).
    pub(crate) fn duration_for(self, model: ItemModel) -> f32 {
        let scale = if self.duration_scale.is_finite() && self.duration_scale > 0.0 {
            self.duration_scale
        } else {
            1.0
        };
        swing_duration_seconds(model) * scale
    }

    /// The tuned impact fraction for `model`: the base fraction plus the dev
    /// offset, clamped to a sane window so the contact cue can never land on the
    /// very first or last frame of the swing.
    pub(crate) fn impact_fraction_for(self, model: ItemModel) -> f32 {
        use crate::app::state::{DEV_COMBAT_IMPACT_FRACTION_MAX, DEV_COMBAT_IMPACT_FRACTION_MIN};
        let offset = if self.impact_fraction_offset.is_finite() {
            self.impact_fraction_offset
        } else {
            0.0
        };
        (swing_impact_fraction(model) + offset).clamp(
            DEV_COMBAT_IMPACT_FRACTION_MIN,
            DEV_COMBAT_IMPACT_FRACTION_MAX,
        )
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
        // them feeling consistent without a bespoke pose. The thrown bomb is a
        // light held bundle, so it lifts at the bag cadence too.
        // The bandage is a light held bundle too, so it lifts at the bag cadence.
        ItemModel::Bag | ItemModel::Deployable | ItemModel::ThrownBomb | ItemModel::Bandage => {
            SWAP_DURATION_BAG
        }
        // The club, spear, sword, and sickle lift like the hatchet (a
        // one/two-handed haft); the mace is the heaviest thing you can carry,
        // so it lifts like the pickaxe.
        ItemModel::Hatchet
        | ItemModel::Club
        | ItemModel::Spear
        | ItemModel::Sword
        | ItemModel::Sickle => SWAP_DURATION_HATCHET,
        ItemModel::Pickaxe | ItemModel::Mace => SWAP_DURATION_PICKAXE,
        // The bow is a light wooden two-hander: it lifts at the hatchet cadence.
        ItemModel::Bow => SWAP_DURATION_HATCHET,
        // The crossbow is heavy iron machinery shouldered into place: it lifts
        // slowly like the pickaxe, part of the "55 damage feels earned" weight.
        ItemModel::Crossbow => SWAP_DURATION_PICKAXE,
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

/// Whether the local player may build at the Tool Cupboard they're
/// looking at. Precomputed at pickup-target time so the tooltip, the
/// tap-E toggle, and the hold-E wheel all read one value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CupboardAuthState {
    /// You're on the authorized list (the placer starts here too, and can
    /// toggle themselves off like anyone else).
    Authorized,
    /// You must authorize yourself before you can build here.
    Unauthorized,
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
    /// Replicated structural stability of the targeted building piece,
    /// for the tooltip readout. `None` for non-structural targets.
    pub(crate) deployable_stability: Option<u8>,
    /// For a targeted Tool Cupboard, whether the local player is the
    /// owner / authorized / not yet. `None` for every non-cupboard target.
    pub(crate) deployable_cupboard_auth: Option<CupboardAuthState>,
    /// Whether the local player may upgrade/demolish the targeted building
    /// piece (authorized at the covering claim, or the builder of an
    /// unclaimed piece). Drives whether the hammer wheel offers anything.
    pub(crate) deployable_can_modify: bool,
    /// Whether the targeted building piece is still within its demolish
    /// window, predicted from the replicated `placed_at_tick`. Hides the
    /// demolish option once the piece has set.
    pub(crate) deployable_demolishable: bool,
    /// Remote player the swing would land on. Set when the look ray
    /// intersects a remote player's body AABB within attack range. The
    /// swing dispatch routes through `dispatch_player_swing` when this
    /// is `Some`, sending `ClientMessage::AttackPlayer` to the server.
    pub(crate) player_id: Option<ClientId>,
    /// Loot bag the player is currently looking at. Drives the E-to-
    /// open path, pressing E sends `LootBagCommand::Open` and the
    /// transfer UI panel becomes visible via `PlayerPrivate.open_loot_bag`.
    pub(crate) loot_bag_id: Option<crate::protocol::LootBagId>,
    /// Set when the look ray lands on a logged-out sleeping body: the
    /// sleeper's display name and current health, for the look-at tooltip.
    /// `player_id` is set alongside it so a swing still lands on the body.
    pub(crate) sleeping_player: Option<(String, f32)>,
    /// Stuck (at-rest) arrow the player is looking at. E recovers it back
    /// into the bag (`InventoryCommand::RecoverProjectile`) before its
    /// despawn TTL runs out. `stack` is set alongside it so the tooltip
    /// shows the ammo name + "Press E to pick up" like a dropped item.
    pub(crate) projectile_id: Option<crate::protocol::ProjectileId>,
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
        self.deployable_stability = None;
        self.deployable_cupboard_auth = None;
        self.deployable_can_modify = false;
        self.deployable_demolishable = false;
        self.player_id = None;
        self.loot_bag_id = None;
        self.sleeping_player = None;
        self.projectile_id = None;
    }
}

#[derive(Resource, Debug, Default, Clone)]
pub(crate) struct GatherInputState {
    active: Option<ActiveSwing>,
    pending_impact: Option<PendingImpactEffect>,
    pending_audio_cue: Option<PendingAudioCue>,
    pending_miss_audio: bool,
    swing_seed: u32,
    /// `(seq, model)` of a swing that began since the last drain, queued for the
    /// swing driver to ship as [`crate::protocol::ClientMessage::SwingStart`]
    /// so peers can play the matching third-person swing. Set at the moment a
    /// swing starts (so it fires on whiffs too, before any impact resolves) and
    /// drained once per frame.
    pending_swing_start: Option<(u32, ItemModel)>,
    /// Seconds of swing lockout left after a whiff. A missed swing sets
    /// this to [`COMBAT_MISS_RECOVERY_SECONDS`] once it finishes; while
    /// it ticks down no new swing can start, so spraying at empty air
    /// costs tempo. A landed swing never sets it, so timing your hits
    /// keeps full cadence.
    recovery_remaining: f32,
    /// Seconds of attacker-side hit-stop left: a brief viewmodel freeze armed on
    /// a confirmed local player hit (see [`HIT_STOP_SECONDS`]). While it ticks
    /// down the active swing's phase advance is held, so the viewmodel hangs at
    /// the contact frame for a few frames before resuming. Purely local feel; it
    /// only stretches this client's swing animation, never the wire timing.
    hit_stop_remaining: f32,
    /// DEV-ONLY swing-fraction override (headless agent capture). When set,
    /// [`Self::swing_fraction`] returns this frozen value so an agent can
    /// screenshot a melee viewmodel mid-swing (the real swing advances by wall
    /// time and can't be paused for a shot). Never set outside the dev control
    /// socket.
    #[cfg(debug_assertions)]
    debug_swing_override: Option<f32>,
}

#[derive(Debug, Clone, Copy)]
struct ActiveSwing {
    /// The swing archetype ([`ItemModel`]): a weapon's own Club/Spear/Sword/Mace
    /// or a gather tool's Hatchet/Pickaxe (or Bag for the empty-hand punch). This
    /// is the impact identity carried into the [`SwingImpact`] and the wire
    /// `SwingStart`/`PlayerImpact`, and it keys the audio pool, camera kick, and
    /// peer VFX. Timing/poses were resolved from this model at `start_swing` time
    /// and baked into `duration`/`impact_time`, but the model persists so the
    /// impact frame can carry it onto the feedback + wire.
    model: ItemModel,
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
    /// The swing archetype, carried into the feedback dispatch (audio pool,
    /// camera kick) and onto the wire (`SwingStart`/`AttackPlayer`).
    pub(crate) model: ItemModel,
}

/// Anchor + (model, surface) for a hit sound queued from the impact
/// dispatcher. The audio system drains it the same frame to spawn a
/// spatial sound; the pair drives selection of the per-archetype, per-surface
/// impact pool independently of the visual particle kind.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingAudioCue {
    pub(crate) anchor: Vec3,
    /// The swing archetype. For a resource/deployable hit it selects the impact
    /// pool with `surface`; for a player hit (`is_player_hit`) it selects the
    /// per-weapon PvP pool.
    pub(crate) model: ItemModel,
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
        equipped: Option<ItemModel>,
        target: Option<SwingTarget>,
        feel: SwingFeelScales,
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
            && let Some(model) = equipped
        {
            self.start_swing(model, feel);
        }

        let mut active = self.active?;

        // Attacker-side hit-stop: on a confirmed local player hit the swing's
        // phase advance is held for a few frames (see `HIT_STOP_SECONDS`), so the
        // viewmodel hangs at contact before the follow-through resumes. Burn the
        // window down here and skip the advance while it is active. This only
        // stretches the local animation; the impact has already fired (the hit is
        // only armed after the impact frame), so no cue is delayed on the wire.
        if self.hit_stop_remaining > 0.0 {
            self.hit_stop_remaining = (self.hit_stop_remaining - delta).max(0.0);
            self.active = Some(active);
            return None;
        }

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
                model: active.model,
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
            } else if pressed && let Some(model) = equipped {
                // Landed (or hit something): continue swinging while LMB is
                // held, no recovery penalty.
                self.start_swing(model, feel);
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
        // A tool swap / death cancels any un-sent swing-start so the peer
        // animation isn't kicked off for a swing that never really happened.
        self.pending_swing_start = None;
        // A tool swap or death clears the swing entirely, including any
        // pending miss penalty, the next tool shouldn't inherit a stun.
        self.recovery_remaining = 0.0;
        // Drop any in-flight hit-stop too; a swapped-in tool must not inherit a
        // frozen viewmodel from the previous weapon's hit.
        self.hit_stop_remaining = 0.0;
    }

    /// Arm the attacker-side hit-stop: hold the active swing's phase advance for
    /// `HIT_STOP_SECONDS * scale` seconds (the Dev `hit_stop_scale` slider),
    /// starting next frame. Called from the local player-hit dispatch, the same
    /// place the confirmed-hit audio cue is queued, so the freeze lands exactly on
    /// a landed player hit and never on a node/deployable hit or a whiff. A
    /// non-positive or non-finite scale (0 disables) leaves the swing untouched.
    pub(crate) fn trigger_hit_stop(&mut self, scale: f32) {
        let scale = if scale.is_finite() {
            scale.max(0.0)
        } else {
            1.0
        };
        let window = HIT_STOP_SECONDS * scale;
        if window > 0.0 {
            // Take the max so a rapid second confirmed hit can only extend, never
            // shorten, an in-flight freeze.
            self.hit_stop_remaining = self.hit_stop_remaining.max(window);
        }
    }

    /// Start a swing PRIMED just before its impact frame, for the charged bomb
    /// toss: the wind-up already played as the held charge pose, so the swing
    /// clock skips straight to the release beat. The impact fires within a
    /// frame or two of the release (the throw sends there) and the
    /// follow-through plays out normally. Peers still get the full
    /// `SwingStart` (their replay runs the whole toss; the wind-up they missed
    /// is a fraction of a second).
    pub(crate) fn begin_primed_swing(&mut self, model: ItemModel, feel: SwingFeelScales) {
        self.start_swing(model, feel);
        if let Some(active) = &mut self.active {
            active.elapsed = (active.impact_time - 0.02).max(0.0);
        }
    }

    fn start_swing(&mut self, model: ItemModel, feel: SwingFeelScales) {
        self.swing_seed = self.swing_seed.wrapping_add(1);
        // Dev-tuned duration + contact fraction (neutral scales reproduce the
        // shipped timing exactly), both keyed on the swing archetype `model`. The
        // impact cue rides the same clock, so stretching the duration keeps the
        // contact frame at the same relative point in the swing.
        let duration = feel.duration_for(model);
        let impact_time = duration * feel.impact_fraction_for(model);
        self.active = Some(ActiveSwing {
            model,
            duration,
            impact_time,
            elapsed: 0.0,
            impact_handled: false,
            seed: self.swing_seed,
            missed: false,
        });
        // Queue a swing-start signal for the network driver. Stamped here (not
        // at impact) so peers see the wind-up, and so a whiff still animates.
        self.pending_swing_start = Some((self.swing_seed, model));
    }

    /// Drain the `(seq, model)` of a swing that began since the last call. The
    /// swing driver sends one [`crate::protocol::ClientMessage::SwingStart`]
    /// per drained value so every swing, hit or miss, animates on peers.
    pub(crate) fn take_swing_start(&mut self) -> Option<(u32, ItemModel)> {
        self.pending_swing_start.take()
    }

    pub(crate) fn swing_fraction(&self) -> f32 {
        #[cfg(debug_assertions)]
        if let Some(frozen) = self.debug_swing_override {
            return frozen.clamp(0.0, 1.0);
        }
        match self.active {
            Some(active) if active.duration > 0.0 => {
                (active.elapsed / active.duration).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

    /// DEV-ONLY: freeze the swing fraction for headless agent capture (a frozen
    /// mid-swing viewmodel frame). `None` clears back to the live swing clock.
    #[cfg(debug_assertions)]
    pub(crate) fn set_debug_swing_override(&mut self, frozen: Option<f32>) {
        self.debug_swing_override = frozen;
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
