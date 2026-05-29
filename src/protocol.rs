use bevy::prelude::{Reflect, Vec3};
use serde::{Deserialize, Serialize};

use crate::{
    world::{MapType, WorldData},
    world_time::WorldTimeSnapshot,
};

pub type ClientId = u64;
pub type SteamId = u64;

pub const PROTOCOL_VERSION: u32 = 29;
pub const GAME_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SERVER_TICK_RATE_HZ: f32 = 20.0;
pub const MAX_CHAT_LEN: usize = 240;
pub const MAX_HEALTH: f32 = 100.0;
/// How long a chat bubble floats above a player after they send a chat
/// message. Long enough to read a sentence at a glance, short enough that
/// idle chatter doesn't permanently clutter the world.
pub const CHAT_BUBBLE_DURATION_SECONDS: f32 = 6.0;
pub const INVENTORY_SLOT_COUNT: usize = 40;
pub const ACTIONBAR_SLOT_COUNT: usize = 9;
/// Number of input/output slots in a furnace. Small enough to fit on
/// one row of the furnace UI and to keep the auto-smelt loop fast (the
/// server walks every slot each tick the head item completes), but
/// roomy enough that the player can preload a stack of ore and walk
/// away. The fuel slot is separate and not counted here.
pub const FURNACE_ITEM_SLOT_COUNT: usize = 6;

/// Sample rate the voice pipeline encodes/decodes at end-to-end. 48 kHz is
/// the only rate libopus supports natively without resampling at its highest
/// quality tier, so we standardise both sides on it.
pub const VOICE_SAMPLE_RATE_HZ: u32 = 48_000;
/// Number of audio samples in one Opus frame. 960 samples @ 48 kHz = 20 ms,
/// which is the standard VoIP frame length — long enough to keep the codec
/// overhead reasonable, short enough to keep mouth-to-ear latency under the
/// audible-glass-cliff threshold.
pub const VOICE_FRAME_SAMPLES: usize = 960;
/// Hard cap on the encoded Opus payload, well above the ~120 byte high-water
/// mark for the bit-rates we target. Defends the snapshot/voice mux against
/// a misbehaving (or malicious) client trying to flood the wire.
pub const MAX_VOICE_FRAME_BYTES: usize = 512;

pub type DroppedItemId = u64;
pub type ResourceNodeId = u64;
/// Identifier for a loot bag (the container spawned at a dead
/// player's feet — see `docs/pvp.md`). Stable for the bag's
/// lifetime; the server picks it from a monotonic counter and uses
/// it to route `LootBagCommand` traffic.
pub type LootBagId = u64;
/// Slot count inside a loot bag — sized to hold the full inventory
/// plus actionbar of one player, the worst case any death can produce.
/// Bags spawned by death start with their slots filled from index 0;
/// trailing slots stay empty.
pub const LOOT_BAG_SLOT_COUNT: usize = INVENTORY_SLOT_COUNT + ACTIONBAR_SLOT_COUNT;
/// Identifier assigned by the server when a crafting job enters the queue.
/// Stable for the job's lifetime so the client can target it with
/// [`CraftingCommand::Cancel`] without worrying about queue reordering.
pub type CraftingJobId = u64;
/// Identifier for a structure the player has placed in the world
/// (workbench, furnace, …). Stable for the entity's lifetime; the server
/// assigns it at place time and uses it to target health updates and
/// future destroy commands.
pub type DeployedEntityId = u64;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Reflect)]
pub struct Vec3Net {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3Net {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    pub fn length_squared(self) -> f32 {
        self.x
            .mul_add(self.x, self.y.mul_add(self.y, self.z * self.z))
    }

    pub fn normalize_or_zero(self) -> Self {
        let len_sq = self.length_squared();
        if len_sq <= f32::EPSILON {
            return Self::ZERO;
        }

        let inv_len = len_sq.sqrt().recip();
        Self::new(self.x * inv_len, self.y * inv_len, self.z * inv_len)
    }

    pub fn scale(self, value: f32) -> Self {
        Self::new(self.x * value, self.y * value, self.z * value)
    }

    pub fn plus(self, other: Self) -> Self {
        Self::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    pub fn minus(self, other: Self) -> Self {
        Self::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    pub fn dot(self, other: Self) -> f32 {
        self.x
            .mul_add(other.x, self.y.mul_add(other.y, self.z * other.z))
    }
}

impl From<Vec3Net> for Vec3 {
    fn from(value: Vec3Net) -> Self {
        Vec3::new(value.x, value.y, value.z)
    }
}

impl From<Vec3> for Vec3Net {
    fn from(value: Vec3) -> Self {
        Self::new(value.x, value.y, value.z)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Reflect)]
pub struct QuatNet {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl QuatNet {
    pub const IDENTITY: Self = Self::new(0.0, 0.0, 0.0, 1.0);

    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }
}

impl Default for QuatNet {
    fn default() -> Self {
        Self::IDENTITY
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMessage {
    Auth {
        protocol_version: u32,
        #[serde(default)]
        client_version: Option<String>,
        steam_id: SteamId,
        display_name: String,
        token: String,
    },
    Movement(PlayerMovement),
    Chat {
        text: String,
    },
    /// Server-evaluated slash command. The full text (without the leading
    /// `/`) is shipped verbatim — the server is the source of truth for
    /// parsing, validation, and the admin check.
    Command {
        text: String,
    },
    Inventory(InventoryCommand),
    Crafting(CraftingCommand),
    Gather(ResourceGatherCommand),
    /// Place the active actionbar deployable at `position` with the given
    /// yaw. Server validates that the player actually holds a stack of
    /// that item, that the placement is in reach, and that nothing
    /// already occupies the footprint. One item is consumed on success.
    PlaceDeployable(PlaceDeployableCommand),
    /// Open/close/operate a furnace the player is standing next to. The
    /// server tracks at most one open furnace per client; opening a new
    /// one auto-closes the previous.
    Furnace(FurnaceCommand),
    /// Damage a placed structure (workbench, furnace, …). Server
    /// validates the active tool, the target's range/cone, and applies
    /// per-tool damage; the structure despawns when health reaches 0.
    DamageDeployable(DamageDeployableCommand),
    /// Swing the equipped tool at another player. Server re-validates
    /// tool, range, view cone, and line-of-sight against the world
    /// blocks; on success it applies armor-reduced damage and sends
    /// `PlayerImpact` + `Knockback`. See `docs/pvp.md`.
    AttackPlayer(AttackPlayerCommand),
    /// Respawn the calling client after death. Rejected unless the
    /// client is currently dead — the server is the authority on the
    /// lifecycle state.
    Respawn,
    /// Open / close / move-items in a loot bag (the container spawned
    /// at a dead player's feet). Server keeps the authoritative slots
    /// and gates the move on the player having the bag open.
    LootBag(LootBagCommand),
    /// Client's view-radius preference (Low/Medium/High). The server uses
    /// this to decide how many concentric chunk rings to include in this
    /// client's per-tick snapshot. Sent on connect and whenever the
    /// player changes the setting in-game.
    SetViewRadius {
        tier: ViewRadiusTier,
    },
    /// One Opus-encoded voice frame. Unreliable — losing a 20 ms frame is
    /// better than waiting for a retransmit. The server routes these to
    /// peers within audible range only.
    Voice(VoiceFrame),
    Heartbeat,
    Disconnect,
}

/// Player-controlled view radius for chunk AoI streaming. Resolved to a
/// concentric chunk-ring size server-side. Living in the protocol layer
/// keeps the wire type free of server-internal details.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ViewRadiusTier {
    Low,
    #[default]
    Medium,
    High,
}

impl ViewRadiusTier {
    pub const ALL: [Self; 3] = [Self::Low, Self::Medium, Self::High];

    pub fn label(self) -> &'static str {
        match self {
            Self::Low => "Low",
            Self::Medium => "Medium",
            Self::High => "High",
        }
    }
}

impl ClientMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Auth { .. }
            | Self::Chat { .. }
            | Self::Command { .. }
            | Self::Inventory(_)
            | Self::Crafting(_)
            | Self::Gather(_)
            | Self::PlaceDeployable(_)
            | Self::Furnace(_)
            | Self::DamageDeployable(_)
            | Self::AttackPlayer(_)
            | Self::Respawn
            | Self::LootBag(_)
            | Self::SetViewRadius { .. }
            | Self::Disconnect => PacketDelivery::Reliable,
            // Voice frames are each independent (Opus packets carry their own
            // decoder state) so we want every delivered frame played — *not*
            // dropped for being slightly out-of-order, which is what
            // `Sequenced` would do. Movement is the opposite: a newer pose
            // makes an older one obsolete.
            Self::Voice(_) => PacketDelivery::UnreliableUnordered,
            Self::Movement(_) | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

/// One Opus-encoded voice packet. `sequence` lets the receiver drop reordered
/// frames; `frame` holds the raw codec bytes (capped at
/// [`MAX_VOICE_FRAME_BYTES`]). Sample-rate/frame-length are global constants
/// so the wire format only needs to carry the codec payload.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VoiceFrame {
    pub sequence: u16,
    pub frame: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemStack {
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub quantity: u16,
}

impl ItemStack {
    pub fn new(item_id: impl AsRef<str>, quantity: u16) -> Self {
        Self {
            item_id: crate::items::intern_item_id(item_id.as_ref()),
            quantity,
        }
    }
}

fn deserialize_interned_item_id<'de, D>(deserializer: D) -> Result<crate::items::ItemId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::items::intern_item_id(&raw))
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ItemContainer {
    Inventory,
    Actionbar,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct ItemContainerSlot {
    pub container: ItemContainer,
    pub slot: usize,
}

impl ItemContainerSlot {
    pub const fn inventory(slot: usize) -> Self {
        Self {
            container: ItemContainer::Inventory,
            slot,
        }
    }

    pub const fn actionbar(slot: usize) -> Self {
        Self {
            container: ItemContainer::Actionbar,
            slot,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum InventoryCommand {
    Move {
        from: ItemContainerSlot,
        to: ItemContainerSlot,
        quantity: Option<u16>,
        /// Optimistic-prediction sequence number (see [`InventoryCommand::action_seq`]).
        seq: u32,
    },
    Drop {
        from: ItemContainerSlot,
        quantity: Option<u16>,
        /// Optimistic-prediction sequence number (see [`InventoryCommand::action_seq`]).
        seq: u32,
    },
    PickUp {
        dropped_item_id: DroppedItemId,
        /// Optimistic-prediction sequence number (see [`InventoryCommand::action_seq`]).
        seq: u32,
    },
    /// Quick-pick a crude (hand-harvestable) resource node — surface
    /// stones, branch piles, grass tufts. Server treats this as an
    /// instant full drain: as much of the node's storage as fits flows
    /// straight into the player's inventory, and the node despawns if
    /// fully emptied. Rejected server-side for non-crude nodes (trees,
    /// ore veins) — those still require a tool swing.
    PickUpResourceNode {
        resource_node_id: ResourceNodeId,
    },
    SelectActionbarSlot {
        slot: usize,
    },
    SelectActionbarOffset {
        offset: i8,
    },
}

impl InventoryCommand {
    /// The optimistic-prediction sequence number for the client-predicted
    /// variants (`Move`/`Drop`/`PickUp`); `None` for variants the client does
    /// not predict. The server advances the per-client `applied_action_seq` to
    /// this value — whether it accepts or rejects the command — so the client
    /// can prune the matching pending overlay op and either confirm or revert.
    pub fn action_seq(&self) -> Option<u32> {
        match self {
            Self::Move { seq, .. } | Self::Drop { seq, .. } | Self::PickUp { seq, .. } => {
                Some(*seq)
            }
            Self::PickUpResourceNode { .. }
            | Self::SelectActionbarSlot { .. }
            | Self::SelectActionbarOffset { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResourceGatherCommand {
    pub resource_node_id: ResourceNodeId,
    /// Optimistic-prediction sequence number. The client tags each predicted
    /// gather so the server can echo it back via `PlayerPrivate.applied_action_seq`,
    /// letting the client prune the matching pending inventory overlay op.
    pub seq: u32,
}

/// Client → server placement intent for a deployable structure. The
/// server re-validates that `position` is a legal placement; the client
/// is only responsible for sending a reasonable best-guess pose so the
/// player sees instant feedback (placement preview moves where they aim).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlaceDeployableCommand {
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Client → server damage intent for a placed structure. Server picks
/// the damage amount from the player's currently-equipped tool — no
/// damage payload on the wire so clients can't lie about how hard they
/// hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DamageDeployableCommand {
    pub id: DeployedEntityId,
}

/// Client → server PvP melee attack intent. Same shape as
/// `DamageDeployableCommand` — only an id is shipped, the server reads
/// the attacker's active tool itself so the client can't lie about
/// what it's swinging.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttackPlayerCommand {
    pub target_player_id: ClientId,
}

/// Loot bag commands. Same Open/Close/Move shape as
/// `FurnaceCommand` — the bag is essentially "a furnace with no
/// smelt loop" from the wire layer's perspective. The server gates
/// every move on the player having the bag open.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LootBagCommand {
    /// Open the bag's UI server-side. Replied to by replicating the
    /// `OpenLootBagView` on `PlayerPrivate`.
    Open { id: LootBagId },
    /// Close the active bag, if any. Idempotent — no-op when there's
    /// nothing open. If the bag is empty when this lands the server
    /// also despawns the entity.
    Close,
    /// Move an `ItemStack` between any pair of {player inventory,
    /// player actionbar, bag slot}. The server validates the bag is
    /// the one currently open before applying.
    Move {
        from: LootBagSlotRef,
        to: LootBagSlotRef,
        quantity: Option<u16>,
    },
    /// Shift-click "send this somewhere useful" — same idea as
    /// `FurnaceCommand::QuickTransfer`. From a bag slot, the stack
    /// flows back into the player's inventory; from a player slot,
    /// it lands in the first empty bag slot. Lets the player loot
    /// a full bag without dragging every stack manually.
    QuickTransfer { from: LootBagSlotRef },
}

/// Addressable slot used by [`LootBagCommand::Move`]. Refers either
/// to a slot in the player's own inventory/actionbar or to one of
/// the bag's [`LOOT_BAG_SLOT_COUNT`] slots.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum LootBagSlotRef {
    PlayerInventory(usize),
    PlayerActionbar(usize),
    Bag(usize),
}

/// Per-client view of the bag currently open on the server.
/// Replicated as a field of `PlayerPrivate.open_loot_bag` so the
/// owning client renders the transfer UI off its replicated data.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenLootBagView {
    pub id: LootBagId,
    pub slots: Vec<Option<ItemStack>>,
}

/// Client → server messages for furnace interaction. The server gates
/// `Move`/`SetActive` on the player currently having `id` open; this
/// keeps the per-message validation cheap and means a player can't
/// stuff items into a furnace they aren't standing next to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum FurnaceCommand {
    /// Open the furnace UI on the server side. The server replies by
    /// including `open_furnace` in the next snapshot for this client.
    Open { id: DeployedEntityId },
    /// Close the active furnace, if any. Idempotent — no-op when the
    /// player has no furnace open.
    Close,
    /// Toggle the furnace's burn state. Auto-shutoff still applies on
    /// server-side ticks, so a `SetActive { active: true }` with no
    /// fuel just flips back to `false` on the next idle tick.
    SetActive { active: bool },
    /// Move items between player containers and the furnace. The server
    /// validates that the player has the targeted furnace open before
    /// applying the move.
    Move {
        from: FurnaceSlotRef,
        to: FurnaceSlotRef,
        quantity: Option<u16>,
    },
    /// Shift+click "send this somewhere useful" intent.
    ///
    /// Server resolves the destination based on the source location and
    /// the item kind:
    /// - From a player slot, fuel items go to the fuel slot (swapping if
    ///   it's a different fuel), everything else fills the furnace items
    ///   grid (merge into a matching stack first, else first empty).
    /// - From a furnace slot, the stack flows back into the player's
    ///   inventory (matching stacks first, then first empty inventory
    ///   slot).
    ///
    /// Authoritative item-kind detection lives server-side so the client
    /// doesn't have to duplicate `fuel_burn_ticks_for` or smelt-recipe
    /// tables — saves one wire format coupling per added fuel/recipe.
    QuickTransfer { from: FurnaceSlotRef },
}

/// Addressable slot used by [`FurnaceCommand::Move`]. Refers either to
/// a slot in the player's own inventory/actionbar or to one of the
/// furnace's slots — both endpoints flow through one move command.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum FurnaceSlotRef {
    PlayerInventory(usize),
    PlayerActionbar(usize),
    /// The furnace's single fuel slot.
    Fuel,
    /// One of `FURNACE_ITEM_SLOT_COUNT` smelt input/output slots.
    Item(usize),
}

/// Per-client view of the furnace currently open on the server.
/// `progress_fraction` is the smelt timer of the head input slot for
/// quick UI rendering — the per-slot inputs themselves are not split
/// into separate "input vs output" lists since items in a furnace slot
/// can be either, depending on whether they're smeltable.
///
/// Replicated as a field of `PlayerPrivate.open_furnace`, not as a
/// top-level wire message. Lives in `protocol.rs` because it's
/// serialised across the wire (inside the parent component) and also
/// shared between server build-up and client UI read-out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenFurnaceView {
    pub id: DeployedEntityId,
    pub fuel: Option<ItemStack>,
    pub items: Vec<Option<ItemStack>>,
    pub active: bool,
    /// 0.0..1.0 — fraction of the current smelt operation. 0 when idle.
    pub smelt_fraction: f32,
    /// 0.0..1.0 — fraction of the currently-burning fuel unit. 0 when
    /// no fuel is burning. Drives the small "fuel" indicator in the UI.
    pub fuel_fraction: f32,
}

/// Client → server crafting intent. Enqueue costs `inputs × quantity` of
/// the recipe's inputs immediately; cancel refunds whatever's left of them.
/// The recipe id is shipped as a plain `String` on the wire and resolved
/// against [`crate::crafting`] server-side. `quantity` is the batch size
/// for the job — a quantity of 5 takes 5× the inputs, 5× the total tick
/// time, and produces 5× the output in a single completion event. Server
/// clamps to `[1, MAX_CRAFT_BATCH_SIZE]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CraftingCommand {
    Enqueue { recipe_id: String, quantity: u16 },
    Cancel { job_id: CraftingJobId },
}

/// Hard cap on the per-job batch size accepted by `Enqueue`. Anything
/// larger is clamped server-side. Chosen so the longest tier-1 recipe
/// (~14 s) at the cap finishes inside a few minutes, but a malicious
/// client can't queue years of work in one message.
pub const MAX_CRAFT_BATCH_SIZE: u16 = 100;

/// One in-progress crafting job. `progress_ticks` advances toward
/// `total_ticks`; when they meet the server grants the recipe's output
/// (multiplied by `quantity`) and pops the job. Inputs are not echoed back
/// — they were taken at enqueue time and the recipe id lets the client
/// reconstruct everything else from the static registry.
///
/// `quantity` is the batch size. A job with `quantity = 3` ran with
/// 3× the inputs at enqueue time, has `total_ticks = ticks_per_unit × 3`,
/// and on completion grants `output_quantity × 3` of the output item in a
/// single grant. The UI uses `quantity > 1` to render `×N` next to the
/// job's name in the queue HUD.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CraftingJob {
    pub job_id: CraftingJobId,
    #[serde(deserialize_with = "deserialize_interned_recipe_id")]
    pub recipe_id: crate::crafting::RecipeId,
    pub progress_ticks: u32,
    pub total_ticks: u32,
    pub quantity: u16,
}

impl CraftingJob {
    pub fn new(
        job_id: CraftingJobId,
        recipe_id: impl AsRef<str>,
        total_ticks: u32,
        quantity: u16,
    ) -> Self {
        Self {
            job_id,
            recipe_id: crate::crafting::intern_recipe_id(recipe_id.as_ref()),
            progress_ticks: 0,
            total_ticks,
            quantity,
        }
    }

    /// Fraction of the head job's craft time that has elapsed, in `[0.0, 1.0]`.
    /// Returns `1.0` for zero-duration recipes so the UI doesn't divide by
    /// zero or stall on a permanent empty bar.
    pub fn progress_fraction(&self) -> f32 {
        if self.total_ticks == 0 {
            return 1.0;
        }
        (self.progress_ticks as f32 / self.total_ticks as f32).clamp(0.0, 1.0)
    }
}

fn deserialize_interned_recipe_id<'de, D>(
    deserializer: D,
) -> Result<crate::crafting::RecipeId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::crafting::intern_recipe_id(&raw))
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerCraftingState {
    pub jobs: Vec<CraftingJob>,
}

impl PlayerCraftingState {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn len(&self) -> usize {
        self.jobs.len()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlayerInventoryState {
    pub inventory_slots: Vec<Option<ItemStack>>,
    pub actionbar_slots: Vec<Option<ItemStack>>,
    pub active_actionbar_slot: usize,
}

impl Default for PlayerInventoryState {
    fn default() -> Self {
        Self::empty()
    }
}

impl PlayerInventoryState {
    pub fn empty() -> Self {
        Self {
            inventory_slots: vec![None; INVENTORY_SLOT_COUNT],
            actionbar_slots: vec![None; ACTIONBAR_SLOT_COUNT],
            active_actionbar_slot: 0,
        }
    }

    pub fn active_actionbar_stack(&self) -> Option<&ItemStack> {
        self.actionbar_slots
            .get(self.active_actionbar_slot)
            .and_then(Option::as_ref)
    }

    /// Read-only access to the stack in a specific slot. Returns `None` for
    /// an empty *or* out-of-range slot. Used by the client-side move
    /// prediction to gate on an empty destination.
    pub fn slot(&self, slot: ItemContainerSlot) -> Option<&ItemStack> {
        match slot.container {
            ItemContainer::Inventory => self.inventory_slots.get(slot.slot),
            ItemContainer::Actionbar => self.actionbar_slots.get(slot.slot),
        }
        .and_then(Option::as_ref)
    }
}

/// Server-internal authoritative shape of a dropped item. Post-Phase-6
/// this is no longer a wire type — the client receives `DroppedItem` +
/// `DroppedItemTransform` via Lightyear replication. Still used as the
/// `GameServer::dropped_items` map value and persisted on save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DroppedWorldItem {
    pub id: DroppedItemId,
    pub stack: ItemStack,
    pub position: Vec3Net,
    pub yaw: f32,
    #[serde(default)]
    pub rotation: QuatNet,
}

/// Server-internal authoritative shape of a placed structure (workbench,
/// furnace, …). Post-Phase-6 this is no longer a wire type — the client
/// receives `Deployable` + `DeployableTransform` + `DeployableHealth` +
/// `DeployableActive` via Lightyear replication. Still used as the
/// `GameServer::deployed_entities` map value and persisted on save.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeployedEntityState {
    pub id: DeployedEntityId,
    #[serde(deserialize_with = "deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub kind: crate::items::DeployableKind,
    pub position: Vec3Net,
    pub yaw: f32,
    pub health: u32,
    pub max_health: u32,
    /// Public "is it doing work?" flag — for furnaces this drives the
    /// glow/smoke and tells nearby players the structure is on. Always
    /// `false` for kinds that have no active state (workbench).
    #[serde(default)]
    pub active: bool,
}

/// Server-internal authoritative shape of a live resource node. Post-Phase-6
/// this is no longer a wire type — the client receives `ResourceNode` +
/// `ResourceNodeStorage` via Lightyear replication instead. The struct
/// stays here because the server still keys its in-memory map and the
/// persisted save layer by this shape; Phase 1b would eventually fold it
/// into the ECS entities.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResourceNodeState {
    pub id: ResourceNodeId,
    pub definition_id: String,
    pub position: Vec3Net,
    pub yaw: f32,
    pub storage: Vec<ItemStack>,
}

/// Per-frame intent emitted by the client controller. Never serialized — the
/// wire format is `PlayerMovement` (the *result* of integrating the input),
/// not the input itself. The simulator reads `time.delta_secs()` for the
/// integration step, so the input carries no time field.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerInput {
    pub sequence: u64,
    pub direction: Vec3Net,
    pub run: bool,
    pub jump: bool,
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PlayerMovement {
    pub sequence: u64,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub grounded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ServerMessage {
    Welcome {
        client_id: ClientId,
        map: MapType,
        world: WorldData,
        is_admin: bool,
        /// Prediction-seed for the connecting client's local
        /// controller — position, velocity, yaw, pitch, health,
        /// grounded, last_processed_input. Phase 6.6 retired the
        /// full-snapshot Welcome payload; everything else now flows
        /// through Lightyear replication.
        local_seed: PlayerState,
        world_time: WorldTimeSnapshot,
    },
    AuthRejected {
        reason: String,
    },
    Kicked {
        reason: String,
    },
    PlayerEvent(PlayerEvent),
    Correction(PlayerState),
    Chat(ChatMessage),
    ItemMerged {
        #[serde(deserialize_with = "deserialize_interned_item_id")]
        item_id: crate::items::ItemId,
        quantity: u16,
    },
    Toast(ToastMessage),
    /// A remote player landed a successful gather hit. The server sends this
    /// to every client except the swinger (whose client already triggered
    /// the impact locally via prediction) so all nearby players hear and
    /// see the same hit feedback. Position is the gathered node's center
    /// so spatial audio attenuates naturally with distance.
    ResourceImpact {
        position: Vec3Net,
        kind: ResourceImpactKind,
    },
    /// A PvP attack landed on a player. Broadcast to every client
    /// except the attacker (the attacker already produced their own
    /// feedback via prediction). Drives the chip burst, hit audio,
    /// floating damage number, and — on the target client only — the
    /// camera-kick hit reaction.
    PlayerImpact {
        attacker: ClientId,
        target: ClientId,
        /// Chest-height world position of the target at impact time —
        /// the visual + audio anchor.
        position: Vec3Net,
        tool: crate::items::ToolKind,
        /// Post-armor damage in HP. Used for the floating damage text.
        damage_dealt: u32,
    },
    /// Knockback impulse sent only to the target of a PvP hit. The
    /// target applies it to its local velocity predictor; a cheater
    /// ignoring this message only forfeits their own pushback.
    Knockback {
        impulse: Vec3Net,
    },
    /// Sent to the dying player when their HP reaches zero. Triggers the
    /// death splash and the respawn UI. `killer_name` is resolved
    /// server-side so the client doesn't have to look it up.
    PlayerKilled {
        killer: Option<ClientId>,
        killer_name: Option<String>,
    },
    /// A resource node was actually depleted (storage drained, node
    /// removed) — distinct from "the node just left this player's
    /// AoI". The client uses this to decide whether a node disappearing
    /// from the snapshot deserves a death animation (tree felling, ore
    /// shatter, crude pickup burst, …). Without this signal, every
    /// chunk-boundary crossing animated the death of every node leaving
    /// the player's view ring.
    ResourceNodeDepleted {
        id: ResourceNodeId,
    },
    /// Authoritative day/night clock. Sent every ~60 s as a routine drift
    /// realignment, and immediately after an admin command changes the
    /// clock or speed. Clients integrate locally between broadcasts using
    /// the same multiplier, so the visible cycle stays smooth.
    WorldTime(WorldTimeSnapshot),
    /// A voice frame forwarded from `speaker` after the server confirmed
    /// the listener is within audible range. The position is the speaker's
    /// authoritative position at send time so the client can apply spatial
    /// gain even when its last `Snapshot` is a few frames stale.
    Voice {
        speaker: ClientId,
        sequence: u16,
        position: Vec3Net,
        frame: Vec<u8>,
    },
    /// Server-side perf stats payload, broadcast on a slow tick (~1 Hz)
    /// when the perf HUD is being shown. The client uses this for the
    /// overlay panel; it doesn't affect gameplay.
    PerfStats(PerfStatsSnapshot),
    Heartbeat,
}

/// Snapshot of server-side perf counters relevant to the chunk system —
/// loaded chunks, live nodes, scheduled regrows, plus the requesting
/// player's classification and AoI count.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct PerfStatsSnapshot {
    pub loaded_chunks: u32,
    pub live_nodes: u32,
    pub pending_regrows: u32,
    pub aoi_visible_nodes: u32,
    pub player_chunk_x: i32,
    pub player_chunk_z: i32,
    pub player_classification: PerfClassificationId,
}

/// Wire-friendly enum mirror of [`crate::world::ChunkClassification`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PerfClassificationId {
    Forest,
    RockyOutcrop,
    OreVein,
    Plains,
    Mixed,
    /// Player isn't inside a loaded chunk (off-world / between chunks).
    None,
}

impl PerfClassificationId {
    pub fn label(self) -> &'static str {
        match self {
            Self::Forest => "Forest",
            Self::RockyOutcrop => "Rocky outcrop",
            Self::OreVein => "Ore vein",
            Self::Plains => "Plains",
            Self::Mixed => "Mixed",
            Self::None => "-",
        }
    }
}

/// Which class of resource a `ResourceImpact` was produced on. The
/// client derives both the visual particle effect and the impact-audio
/// `(tool, surface)` pair from this. New ore types should each get their
/// own variant so they can have distinct audio without a follow-up
/// protocol change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ResourceImpactKind {
    Tree,
    CoalOre,
    IronOre,
    SulfurOre,
    /// Bare-rock vein — same stone-shard burst as the ore variants but
    /// the audio cue routes through the plain-stone surface instead of
    /// an ore-specific one.
    StoneVein,
    /// Crude wood material (branch pile). Lighter wood chip burst than a
    /// felled tree.
    Branches,
    /// Crude stone material (surface rock). Lighter stone shard burst
    /// than an ore vein.
    SurfaceStone,
    /// Plant fibres (hay tuft). Soft thud, no particle burst yet.
    HayGrass,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ToastKind {
    Info,
    Success,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToastMessage {
    pub kind: ToastKind,
    pub text: String,
}

impl ToastMessage {
    pub fn new(kind: ToastKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

impl ServerMessage {
    pub fn delivery(&self) -> PacketDelivery {
        match self {
            Self::Welcome { .. }
            | Self::AuthRejected { .. }
            | Self::Kicked { .. }
            | Self::PlayerEvent(_)
            | Self::Chat(_)
            | Self::ItemMerged { .. }
            | Self::ResourceNodeDepleted { .. }
            | Self::PlayerImpact { .. }
            | Self::Knockback { .. }
            | Self::PlayerKilled { .. }
            | Self::Toast(_) => PacketDelivery::Reliable,
            // Impact effects are pure cosmetic feedback. Dropping one is
            // far less bad than the extra latency of a reliable resend,
            // and the next swing will queue another regardless.
            // See the matching comment on `ClientMessage::delivery` —
            // voice rides an unordered unreliable channel so every
            // delivered frame is played even if it arrives out of order.
            Self::Voice { .. } => PacketDelivery::UnreliableUnordered,
            Self::Correction(_)
            | Self::ResourceImpact { .. }
            | Self::WorldTime(_)
            | Self::PerfStats(_)
            | Self::Heartbeat => PacketDelivery::Unreliable,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketDelivery {
    /// Sequenced-unreliable: drop older-than-newest. Right for state where
    /// only the latest value matters (movement, snapshots, world time).
    Unreliable,
    /// Reliable-ordered.
    Reliable,
    /// Unordered-unreliable: deliver every packet that survives the link in
    /// whatever order they arrive. Right for streams where each packet is
    /// independent — most notably voice frames, where dropping a frame
    /// because it arrived a few milliseconds late produces audible holes.
    UnreliableUnordered,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum PlayerEvent {
    Joined { client_id: ClientId, name: String },
    Left { client_id: ClientId, name: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChatMessage {
    pub from: String,
    pub text: String,
}

/// Per-client wire payload used by:
///
/// - `ServerMessage::Welcome.local_seed` — the initial prediction
///   bootstrap (server tells the connecting client where its
///   controller starts).
/// - `ServerMessage::Correction` — server-authoritative correction of
///   a divergent prediction (health rollback today, more fields if
///   prediction grows).
///
/// All other per-player state moved off the wire to Lightyear
/// replication (`PlayerPublic` / `PlayerPrivate`) during the Phase 6
/// migration; this struct is now strictly a prediction-seed shape.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlayerState {
    pub client_id: ClientId,
    pub position: Vec3Net,
    pub velocity: Vec3Net,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub grounded: bool,
    pub last_processed_input: u64,
}

pub fn sanitize_chat(text: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(trimmed.chars().take(MAX_CHAT_LEN).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_zero_stays_zero() {
        assert_eq!(Vec3Net::ZERO.normalize_or_zero(), Vec3Net::ZERO);
    }

    #[test]
    fn normalize_regular_vector() {
        let normalized = Vec3Net::new(3.0, 0.0, 4.0).normalize_or_zero();
        assert!((normalized.x - 0.6).abs() < 0.0001);
        assert!((normalized.z - 0.8).abs() < 0.0001);
    }

    #[test]
    fn chat_is_trimmed_and_limited() {
        let long = format!("  {}  ", "a".repeat(MAX_CHAT_LEN + 50));
        let sanitized = sanitize_chat(&long).expect("chat should be valid");
        assert_eq!(sanitized.len(), MAX_CHAT_LEN);
        assert!(sanitize_chat("   ").is_none());
    }

    #[test]
    fn message_delivery_maps_network_channels() {
        assert_eq!(
            ClientMessage::Heartbeat.delivery(),
            PacketDelivery::Unreliable
        );
        assert_eq!(
            ClientMessage::Chat {
                text: "hello".to_owned(),
            }
            .delivery(),
            PacketDelivery::Reliable
        );
        assert_eq!(
            ServerMessage::Heartbeat.delivery(),
            PacketDelivery::Unreliable
        );
        assert_eq!(
            ServerMessage::Kicked {
                reason: "restart".to_owned(),
            }
            .delivery(),
            PacketDelivery::Reliable
        );
    }
}
