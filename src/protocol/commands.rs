//! Client → server action payloads (inventory, gather, deployable, PvP,
//! furnace, loot-bag, crafting) and the per-client open-container views the
//! server replicates back.

use serde::{Deserialize, Serialize};

use super::*;

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
    /// Quick-pick a crude (hand-harvestable) resource node, surface
    /// stones, branch piles, grass tufts. Server treats this as an
    /// instant full drain: as much of the node's storage as fits flows
    /// straight into the player's inventory, and the node despawns if
    /// fully emptied. Rejected server-side for non-crude nodes (trees,
    /// ore veins), those still require a tool swing.
    PickUpResourceNode {
        resource_node_id: ResourceNodeId,
        /// Optimistic-prediction sequence number (see [`InventoryCommand::action_seq`]).
        seq: u32,
    },
    SelectActionbarSlot {
        slot: usize,
    },
    SelectActionbarOffset {
        offset: i8,
    },
    /// Auto-stack and tidy the main inventory bag: merge partial stacks of the
    /// same item and re-order the result by name, packing freed slots to the
    /// end. Server-authoritative and deliberately not client-predicted (it
    /// rewrites many slots at once), so the tidied layout arrives via the
    /// normal `PlayerPrivate` inventory replication.
    Sort,
}

impl InventoryCommand {
    /// The optimistic-prediction sequence number for the client-predicted
    /// variants (`Move`/`Drop`/`PickUp`/`PickUpResourceNode`); `None` for
    /// variants the client does not predict. The server advances the
    /// per-client `applied_action_seq` to this value, whether it accepts or
    /// rejects the command, so the client can prune the matching pending
    /// overlay op and either confirm or revert.
    pub fn action_seq(&self) -> Option<u32> {
        match self {
            Self::Move { seq, .. }
            | Self::Drop { seq, .. }
            | Self::PickUp { seq, .. }
            | Self::PickUpResourceNode { seq, .. } => Some(*seq),
            Self::SelectActionbarSlot { .. } | Self::SelectActionbarOffset { .. } | Self::Sort => {
                None
            }
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
    #[serde(deserialize_with = "super::deserialize_interned_item_id")]
    pub item_id: crate::items::ItemId,
    pub position: Vec3Net,
    pub yaw: f32,
    /// Torch-only: `true` when the client aimed at a wall and the torch
    /// should mount on its side (the server bakes this into
    /// `DeployableKind::Torch { wall }`). Ignored for every other
    /// deployable, which always stand upright on a surface. Defaults to
    /// `false` so a pre-field save/replay still parses as a floor mount.
    #[serde(default)]
    pub wall_mounted: bool,
}

/// Client → server damage intent for a placed structure. Server picks
/// the damage amount from the player's currently-equipped tool, no
/// damage payload on the wire so clients can't lie about how hard they
/// hit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DamageDeployableCommand {
    pub id: DeployedEntityId,
}

/// Client → server PvP melee attack intent. Same shape as
/// `DamageDeployableCommand`, only an id is shipped, the server reads
/// the attacker's active tool itself so the client can't lie about
/// what it's swinging.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct AttackPlayerCommand {
    pub target_player_id: ClientId,
}

/// Client → server intent to place a building block from the building
/// plan. Pieces always spawn at the sticks tier; the server re-derives
/// the snap (ground for foundations, a foundation wall socket for
/// wall-like pieces), validates the material cost, and snaps the pose,
/// the client preview is a best guess exactly like deployable placement.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PlaceBuildingCommand {
    pub piece: crate::building::BuildingPiece,
    pub position: Vec3Net,
    pub yaw: f32,
}

/// Hammer actions on an existing building block (and doors, for
/// demolish). No payloads beyond the id: costs, tier walks, and the
/// demolish window all resolve server-side so the client can't lie.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum BuildingCommand {
    /// One hammer repair hit: restores a fraction of max HP and consumes
    /// tier materials from the swinger.
    Repair { id: DeployedEntityId },
    /// Upgrade the piece to the next tier (sticks → wood → stone).
    /// Owner-only; consumes the target tier's materials and refills HP.
    Upgrade { id: DeployedEntityId },
    /// Demolish the piece outright. Owner-only and rejected once the
    /// piece has stood longer than the demolish window.
    Demolish { id: DeployedEntityId },
}

/// Door lifecycle + code-lock commands.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum DoorCommand {
    /// Hang a door (consumed from inventory) in the doorway opening of
    /// building block `doorway_id`. `flip` mirrors the hinge side chosen
    /// during ghost placement; `code` is the lock code the placer set.
    /// Nobody, including the placer, is authorized until they enter the
    /// code at the door once.
    Place {
        doorway_id: DeployedEntityId,
        flip: bool,
        code: String,
    },
    /// E-press on the door: toggles open/closed when the sender is
    /// authorized, otherwise the server replies with
    /// [`super::ServerMessage::DoorCodePrompt`].
    Interact { id: DeployedEntityId },
    /// Code entry from the prompt. A correct code authorizes the sender's
    /// account on this door and opens it.
    EnterCode { id: DeployedEntityId, code: String },
    /// Change the lock code. Only an already-authorized account may
    /// change it; doing so revokes every other authorization so a stolen
    /// code can be rotated away.
    ChangeCode { id: DeployedEntityId, code: String },
}

/// Sleeping-bag commands: rename (hold-E wheel) and pick-up (tap E).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SleepingBagCommand {
    Rename { id: DeployedEntityId, name: String },
    PickUp { id: DeployedEntityId },
}

/// One sleeping-bag respawn option, carried by
/// [`super::ServerMessage::PlayerKilled`] so the death screen can offer
/// "spawn at <bag>" buttons without an extra round trip.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RespawnBagOption {
    pub id: DeployedEntityId,
    pub name: String,
}

/// Loot bag commands. Same Open/Close/Move shape as
/// `FurnaceCommand`, the bag is essentially "a furnace with no
/// smelt loop" from the wire layer's perspective. The server gates
/// every move on the player having the bag open.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum LootBagCommand {
    /// Open the bag's UI server-side. Replied to by replicating the
    /// `OpenLootBagView` on `PlayerPrivate`.
    Open { id: LootBagId },
    /// Close the active bag, if any. Idempotent, no-op when there's
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
    /// Shift-click "send this somewhere useful", same idea as
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

/// What kind of container an [`OpenLootBagView`] describes. Purely a
/// UI hint (panel title and copy); the slots and commands are
/// identical for every kind.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ContainerViewKind {
    /// A death-drop loot bag on the ground.
    #[default]
    LootBag,
    /// A logged-out sleeping player's live inventory.
    Sleeper,
    /// A placed storage box deployable.
    StorageBox,
}

/// Per-client view of the container currently open on the server (loot
/// bag, sleeping body, or storage box; they share one transfer UI).
/// Replicated as a field of `PlayerPrivate.open_loot_bag` so the
/// owning client renders the transfer UI off its replicated data. `id`
/// is an opaque handle scoped to `kind`: a `LootBagId`, the sleeper's
/// `ClientId`, or a `DeployedEntityId`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenLootBagView {
    pub id: LootBagId,
    pub slots: Vec<Option<ItemStack>>,
    pub kind: ContainerViewKind,
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
    /// Close the active furnace, if any. Idempotent, no-op when the
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
    /// tables, saves one wire format coupling per added fuel/recipe.
    QuickTransfer { from: FurnaceSlotRef },
}

/// Addressable slot used by [`FurnaceCommand::Move`]. Refers either to
/// a slot in the player's own inventory/actionbar or to one of the
/// furnace's slots, both endpoints flow through one move command.
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
/// quick UI rendering, the per-slot inputs themselves are not split
/// into separate "input vs output" lists since items in a furnace slot
/// can be either, depending on whether they're smeltable.
///
/// Replicated as a field of `PlayerPrivate.open_furnace`, not as a
/// top-level wire message. Lives in `protocol` because it's
/// serialised across the wire (inside the parent component) and also
/// shared between server build-up and client UI read-out.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OpenFurnaceView {
    pub id: DeployedEntityId,
    pub fuel: Option<ItemStack>,
    pub items: Vec<Option<ItemStack>>,
    pub active: bool,
    /// 0.0..1.0, fraction of the current smelt operation. 0 when idle.
    pub smelt_fraction: f32,
    /// 0.0..1.0, fraction of the currently-burning fuel unit. 0 when
    /// no fuel is burning. Drives the small "fuel" indicator in the UI.
    pub fuel_fraction: f32,
}

/// Client → server crafting intent. Enqueue costs `inputs × quantity` of
/// the recipe's inputs immediately; cancel refunds whatever's left of them.
/// The recipe id is shipped as a plain `String` on the wire and resolved
/// against [`crate::crafting`] server-side. `quantity` is the batch size
/// for the job, a quantity of 5 takes 5× the inputs, 5× the total tick
/// time, and produces 5× the output in a single completion event. Server
/// clamps to `[1, MAX_CRAFT_BATCH_SIZE]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CraftingCommand {
    Enqueue { recipe_id: String, quantity: u16 },
    Cancel { job_id: CraftingJobId },
}
