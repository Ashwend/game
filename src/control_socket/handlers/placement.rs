//! Placement handlers: drop a carried deployable or building block ahead of
//! the player (view-yaw aimed, so it works headless) and hammer-upgrade or
//! demolish the nearest building block.

use anyhow::{Context, Result};

use super::HandlerContext;
use crate::control_socket::targeting::{
    building_piece_needle, nearest_deployable_id, parse_building_piece, resolve_building_pose,
};
use crate::{
    items::intern_item_id,
    protocol::{ClientMessage, PlaceDeployableCommand, Vec3Net},
};

pub(super) fn place_deployable(
    ctx: &mut HandlerContext,
    item_id: String,
    distance: Option<f32>,
    height: Option<f32>,
) -> Result<String> {
    let view = ctx
        .runtime
        .local_view()
        .context("no local player view (not in a world)")?;
    let dist = distance.unwrap_or(2.2);
    // Player forward is `(-sin yaw, 0, -cos yaw)` (see
    // `controller::movement`), so drop the structure that far ahead on
    // the floor (or the surface at `height`). A deployable's front is
    // +Z, so leaving its yaw equal to the view yaw turns that front
    // back toward the camera.
    let (sin_yaw, cos_yaw) = view.yaw.sin_cos();
    let position = Vec3Net::new(
        view.position.x - sin_yaw * dist,
        height.unwrap_or(0.0),
        view.position.z - cos_yaw * dist,
    );
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::PlaceDeployable(PlaceDeployableCommand {
        item_id: intern_item_id(&item_id),
        position,
        yaw: view.yaw,
        wall_mounted: false,
    }))?;
    Ok(format!(
        "place {item_id} queued at [{:.2}, 0.00, {:.2}]",
        position.x, position.z
    ))
}

pub(super) fn place_building(
    ctx: &mut HandlerContext,
    piece: String,
    distance: Option<f32>,
    height: Option<f32>,
) -> Result<String> {
    let piece = parse_building_piece(&piece)?;
    let view = ctx
        .runtime
        .local_view()
        .context("no local player view (not in a world)")?;
    let dist = distance.unwrap_or(3.0);
    let (sin_yaw, cos_yaw) = view.yaw.sin_cos();
    let aim = Vec3Net::new(
        view.position.x - sin_yaw * dist,
        height.unwrap_or(0.0),
        view.position.z - cos_yaw * dist,
    );
    let (position, yaw) = resolve_building_pose(piece, aim, view.yaw, ctx.deployables);
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::PlaceBuilding(
        crate::protocol::PlaceBuildingCommand {
            piece,
            position,
            yaw,
        },
    ))?;
    Ok(format!(
        "place building {piece:?} queued at [{:.2}, {:.2}, {:.2}] (server snaps)",
        position.x, position.y, position.z
    ))
}

pub(super) fn upgrade_building(ctx: &mut HandlerContext, piece: Option<String>) -> Result<String> {
    let needle = building_piece_needle(piece.as_deref())?;
    let target = nearest_deployable_id(ctx.runtime, ctx.deployables, &needle)
        .context("no matching building block in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Building(
        crate::protocol::BuildingCommand::Upgrade { id: target },
    ))?;
    Ok(format!("upgrade queued for building {target}"))
}

pub(super) fn demolish_building(ctx: &mut HandlerContext, piece: Option<String>) -> Result<String> {
    let needle = building_piece_needle(piece.as_deref())?;
    let target = nearest_deployable_id(ctx.runtime, ctx.deployables, &needle)
        .context("no matching building block in AoI")?;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Building(
        crate::protocol::BuildingCommand::Demolish { id: target },
    ))?;
    Ok(format!("demolish queued for building {target}"))
}
