//! Door and container handlers: hang, toggle, code, and pick up the nearest
//! door, and open/close the shared storage-container panel.

use anyhow::{Context, Result};

use super::HandlerContext;
use crate::control_socket::targeting::nearest_deployable_id;
use crate::protocol::ClientMessage;

pub(super) fn place_door(
    ctx: &mut HandlerContext,
    code: String,
    flip: bool,
    iron: bool,
) -> Result<String> {
    let doorway = nearest_deployable_id(ctx.runtime, ctx.deployables, "Doorway")
        .context("no doorway building block in AoI")?;
    let variant = if iron {
        crate::items::DoorVariant::Iron
    } else {
        crate::items::DoorVariant::HewnLog
    };
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Door(crate::protocol::DoorCommand::Place {
        doorway_id: doorway,
        variant,
        flip,
        code,
    }))?;
    Ok(format!("door placement queued in doorway {doorway}"))
}

pub(super) fn door_interact(ctx: &mut HandlerContext) -> Result<String> {
    let door =
        nearest_deployable_id(ctx.runtime, ctx.deployables, "Door").context("no door in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Door(
        crate::protocol::DoorCommand::Interact { id: door },
    ))?;
    Ok(format!("door interact queued for {door}"))
}

pub(super) fn door_pick_up(ctx: &mut HandlerContext) -> Result<String> {
    let door =
        nearest_deployable_id(ctx.runtime, ctx.deployables, "Door").context("no door in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Door(crate::protocol::DoorCommand::PickUp {
        id: door,
    }))?;
    Ok(format!("door pickup queued for {door}"))
}

pub(super) fn door_enter_code(ctx: &mut HandlerContext, code: String) -> Result<String> {
    let door =
        nearest_deployable_id(ctx.runtime, ctx.deployables, "Door").context("no door in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Door(
        crate::protocol::DoorCommand::EnterCode { id: door, code },
    ))?;
    Ok(format!("door code entry queued for {door}"))
}

pub(super) fn open_storage_box(ctx: &mut HandlerContext) -> Result<String> {
    // Ruin caches share the storage container wire path, so the same
    // request opens whichever is nearer when no box is around; this
    // lets an agent verify cache lootability headlessly.
    let target = nearest_deployable_id(ctx.runtime, ctx.deployables, "StorageBox")
        .or_else(|| nearest_deployable_id(ctx.runtime, ctx.deployables, "RuinCache"))
        .context("no storage box or ruin cache in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::OpenStorageBox { id: target })?;
    Ok(format!("container open queued for {target}"))
}

pub(super) fn close_container(ctx: &mut HandlerContext) -> Result<String> {
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::LootBag(
        crate::protocol::LootBagCommand::Close,
    ))?;
    Ok("container close queued".to_owned())
}
