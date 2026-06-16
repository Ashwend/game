//! Wire protocol: the `ClientMessage`/`ServerMessage` surface plus the shared
//! shapes both sides serialise. Split by concern, re-exported flat so every
//! `crate::protocol::X` path is unchanged:
//!
//! - [`math`], `Vec3Net`/`QuatNet` wire-friendly vector types.
//! - [`items`], item stacks, containers, and inventory/crafting state.
//! - [`commands`], client action payloads (inventory, gather, deployable,
//!   furnace, loot-bag) and the per-client open-container views.
//! - [`world`], server-internal world/entity state shapes (also persisted).
//! - [`messages`], the two top-level message enums and their small payloads.
//!
//! Channel delivery preferences live on the message enums (`*::delivery`);
//! shared constants, id aliases, and the chat sanitiser stay here.

mod commands;
mod items;
mod math;
mod messages;
mod world;
mod world_map;

pub use commands::*;
pub use items::*;
pub use math::*;
pub use messages::*;
pub use world::*;
pub use world_map::*;

use serde::{Deserialize, Serialize};

pub type ClientId = u64;
pub type AccountId = u64;

/// Application-level wire/version handshake number. Sent in
/// [`ClientMessage::Auth`] and validated by `GameServer::connect`; a mismatch
/// is answered with [`ServerMessage::VersionMismatch`]. This is the primary
/// protocol gate now that the netcode `protocol_id`
/// ([`crate::net::channels::LIGHTYEAR_PROTOCOL_ID`]) is fixed and no longer
/// tracks it, bump it on any breaking wire change so mismatched builds are
/// cleanly rejected at the `Auth` handshake.
pub const PROTOCOL_VERSION: u32 = 37;
pub const GAME_VERSION: &str = env!("CARGO_PKG_VERSION");
pub const SERVER_TICK_RATE_HZ: f32 = 20.0;
pub const MAX_CHAT_LEN: usize = 240;
pub const MAX_HEALTH: f32 = 100.0;
/// How long a chat bubble floats above a player after they send a chat
/// message. Long enough to read a sentence at a glance, short enough that
/// idle chatter doesn't permanently clutter the world.
pub const CHAT_BUBBLE_DURATION_SECONDS: f32 = 6.0;
/// Player backpack capacity. Rendered as a 12x5 grid in the inventory panel
/// (see [`crate::app::ui::inventory_panel`]); the panel width is sized to fit
/// that column count exactly, so changing this also means re-checking the grid
/// dimensions there. Returning players whose save predates a change are
/// padded up to this length on load via
/// [`PlayerInventoryState::normalize_capacity`].
pub const INVENTORY_SLOT_COUNT: usize = 60;
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
/// which is the standard VoIP frame length, long enough to keep the codec
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
/// player's feet, see `docs/pvp.md`). Stable for the bag's
/// lifetime; the server picks it from a monotonic counter and uses
/// it to route `LootBagCommand` traffic.
pub type LootBagId = u64;
/// Slot count inside a loot bag, sized to hold the full inventory
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

/// Hard cap on the per-job batch size accepted by `Enqueue`. Anything
/// larger is clamped server-side. Chosen so the longest tier-1 recipe
/// (~14 s) at the cap finishes inside a few minutes, but a malicious
/// client can't queue years of work in one message.
pub const MAX_CRAFT_BATCH_SIZE: u16 = 100;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum PacketDelivery {
    /// Sequenced-unreliable: drop older-than-newest. Right for state where
    /// only the latest value matters (movement, snapshots, world time).
    Unreliable,
    /// Reliable-ordered.
    Reliable,
    /// Unordered-unreliable: deliver every packet that survives the link in
    /// whatever order they arrive. Right for streams where each packet is
    /// independent, most notably voice frames, where dropping a frame
    /// because it arrived a few milliseconds late produces audible holes.
    UnreliableUnordered,
}

/// Interns an `ItemId` from its on-wire `String` form. Shared by every wire
/// shape that carries an item id (kept here so each submodule references it via
/// `super::`).
pub(crate) fn deserialize_interned_item_id<'de, D>(
    deserializer: D,
) -> Result<crate::items::ItemId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    // Legacy item migration: `sticks` was folded into `wood` (2026-06).
    // Saves are postcard like the wire, so this single hook remaps old
    // inventories/drops/bags on load; remove once pre-fold saves are gone.
    let id = if raw == "sticks" {
        crate::items::WOOD_ID
    } else {
        raw.as_str()
    };
    Ok(crate::items::intern_item_id(id))
}

pub(crate) fn deserialize_interned_recipe_id<'de, D>(
    deserializer: D,
) -> Result<crate::crafting::RecipeId, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <String as serde::Deserialize>::deserialize(deserializer)?;
    Ok(crate::crafting::intern_recipe_id(&raw))
}

pub fn sanitize_chat(text: &str) -> Option<String> {
    // Strip control characters (newlines, tabs, NUL, ANSI/C1 escapes, ...)
    // before the length cap. Chat is a single-line plain string rendered into
    // every nearby peer's overlay, so a control char is never wanted and would
    // otherwise be a peer-to-peer UI-corruption vector. Re-trim afterwards in
    // case removing a control char exposed surrounding whitespace, then reject
    // anything that is now empty (e.g. a control-only message).
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_CHAT_LEN)
        .collect();
    let cleaned = cleaned.trim();
    if cleaned.is_empty() {
        return None;
    }

    Some(cleaned.to_owned())
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
    fn chat_strips_control_characters() {
        // Newlines, tabs, NUL, and escape sequences are removed, not preserved.
        assert_eq!(
            sanitize_chat("hi\nthere\tyou\u{0007}").as_deref(),
            Some("hithereyou")
        );
        // A message that is nothing but control characters sanitizes to empty.
        assert!(sanitize_chat("\n\t\u{0000}\u{001b}").is_none());
        // Stripping an interior control char must not leave dangling whitespace
        // at the ends.
        assert_eq!(sanitize_chat("a \u{0007}").as_deref(), Some("a"));
    }

    #[test]
    fn message_delivery_maps_network_channels() {
        // The client heartbeat rides the reliable channel: it's the server's
        // liveness signal, so a single dropped packet must not look like a
        // vanished client.
        assert_eq!(
            ClientMessage::Heartbeat.delivery(),
            PacketDelivery::Reliable
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
