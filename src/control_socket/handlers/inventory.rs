//! Inventory handlers: actionbar slot/item selection and the paperdoll
//! quick-equip a headless (unfocused) window can't drive with the mouse.

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result};

use super::HandlerContext;
use crate::protocol::{ClientMessage, InventoryCommand};

pub(super) fn select_actionbar_slot(ctx: &mut HandlerContext, slot: usize) -> Result<String> {
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Inventory(
        InventoryCommand::SelectActionbarSlot { slot },
    ))?;
    Ok(format!("selected actionbar slot {slot}"))
}

pub(super) fn select_actionbar_item(ctx: &mut HandlerContext, item_id: String) -> Result<String> {
    let private = ctx
        .local_player
        .private
        .as_ref()
        .context("not in a world (no inventory)")?;
    let holds = |stack: &Option<crate::protocol::ItemStack>| {
        stack.as_ref().map(|s| s.item_id.as_ref()) == Some(item_id.as_str())
    };
    // Prefer an actionbar slot that already holds the item.
    if let Some(slot) = private.inventory.actionbar_slots.iter().position(holds) {
        let session = ctx
            .runtime
            .session
            .as_mut()
            .context("no active session (not in a world)")?;
        session.send(ClientMessage::Inventory(
            InventoryCommand::SelectActionbarSlot { slot },
        ))?;
        return Ok(format!("selected actionbar slot {slot} ({item_id})"));
    }
    // Fall back to the inventory grid: the test-kit overflows equipables
    // past the ninth into the bag (e.g. the crossbow), so an agent still
    // needs to hold them. Move the stack into a free actionbar slot, then
    // select it. This is a dev-harness convenience only; the server still
    // validates the move.
    let inv_slot = private
        .inventory
        .inventory_slots
        .iter()
        .position(holds)
        .with_context(|| format!("item '{item_id}' is not in the actionbar or inventory"))?;
    let free_actionbar = private
        .inventory
        .actionbar_slots
        .iter()
        .position(Option::is_none)
        .context("no free actionbar slot to move the item into")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    // A distinct seq per move so the server never treats it as stale.
    static MOVE_SEQ: AtomicU32 = AtomicU32::new(1);
    let seq = MOVE_SEQ.fetch_add(1, Ordering::Relaxed);
    session.send(ClientMessage::Inventory(InventoryCommand::Move {
        from: crate::protocol::ItemContainerSlot::inventory(inv_slot),
        to: crate::protocol::ItemContainerSlot::actionbar(free_actionbar),
        quantity: None,
        seq,
    }))?;
    session.send(ClientMessage::Inventory(
        InventoryCommand::SelectActionbarSlot {
            slot: free_actionbar,
        },
    ))?;
    Ok(format!(
        "moved {item_id} from inventory {inv_slot} to actionbar {free_actionbar} and selected it"
    ))
}

pub(super) fn equip_item(ctx: &mut HandlerContext, item_id: String) -> Result<String> {
    let private = ctx
        .local_player
        .private
        .as_ref()
        .context("not in a world (no inventory)")?;
    let profile = crate::items::armor_profile(&item_id)
        .with_context(|| format!("'{item_id}' has no armor profile (not wearable)"))?;
    let holds = |stack: &Option<crate::protocol::ItemStack>| {
        stack.as_ref().map(|s| s.item_id.as_ref()) == Some(item_id.as_str())
    };
    let from = if let Some(slot) = private.inventory.inventory_slots.iter().position(holds) {
        crate::protocol::ItemContainerSlot::inventory(slot)
    } else if let Some(slot) = private.inventory.actionbar_slots.iter().position(holds) {
        crate::protocol::ItemContainerSlot::actionbar(slot)
    } else {
        anyhow::bail!("item '{item_id}' is not in the bag or actionbar");
    };
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    // A distinct seq per move so the server never treats it as stale.
    static EQUIP_SEQ: AtomicU32 = AtomicU32::new(1);
    let seq = EQUIP_SEQ.fetch_add(1, Ordering::Relaxed);
    session.send(ClientMessage::Inventory(InventoryCommand::Move {
        from,
        to: crate::protocol::ItemContainerSlot::equipment(profile.slot),
        quantity: None,
        seq,
    }))?;
    Ok(format!(
        "equip queued: {item_id} -> {:?} slot",
        profile.slot
    ))
}
