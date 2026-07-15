//! Request handlers, one function per [`ControlRequest`] variant, grouped by
//! domain, plus the [`HandlerContext`] borrow bundle and the thin
//! [`handle_request`] dispatch that destructures each variant onto its handler.

mod capture;
mod combat;
mod doors;
mod inventory;
mod movement;
mod placement;
mod screens;

use anyhow::{Context, Result};
use bevy::prelude::Commands;

use super::wire::{ControlRequest, DeployableDump};
use crate::{
    app::{
        state::{
            ClientRuntime, ConsumeChargeState, DeployablePlacementState, GatherInputState,
            InventoryUiState, LocalPlayerState, LookState, MenuState, RangedDrawState,
            WorldMapUiState,
        },
        systems::HeadlessCapture,
    },
    protocol::ClientMessage,
};

/// Every borrow a request handler may touch, bundled once per accepted
/// connection so handlers take one context instead of a dozen parameters.
pub(super) struct HandlerContext<'w, 's, 'a> {
    pub(super) commands: &'a mut Commands<'w, 's>,
    pub(super) runtime: &'a mut ClientRuntime,
    pub(super) menu: &'a mut MenuState,
    pub(super) look: &'a mut LookState,
    pub(super) world_map_ui: &'a mut WorldMapUiState,
    pub(super) ranged_input: &'a mut RangedDrawState,
    pub(super) consume_charge: &'a mut ConsumeChargeState,
    pub(super) gather_input: &'a mut GatherInputState,
    pub(super) inventory_ui: &'a mut InventoryUiState,
    pub(super) placement: &'a DeployablePlacementState,
    pub(super) local_player: &'a LocalPlayerState,
    pub(super) capture: Option<&'a HeadlessCapture>,
    pub(super) deployables: &'a [DeployableDump],
}

/// Thin dispatch: destructure the variant, call the domain handler.
pub(super) fn handle_request(request: ControlRequest, ctx: &mut HandlerContext) -> Result<String> {
    match request {
        ControlRequest::Screenshot { path } => capture::screenshot(ctx, path),
        ControlRequest::SendCommand { text } => send_command(ctx, text),
        ControlRequest::SelectActionbarSlot { slot } => inventory::select_actionbar_slot(ctx, slot),
        ControlRequest::SelectActionbarItem { item_id } => {
            inventory::select_actionbar_item(ctx, item_id)
        }
        ControlRequest::EquipItem { item_id } => inventory::equip_item(ctx, item_id),
        ControlRequest::PlaceDeployable {
            item_id,
            distance,
            height,
        } => placement::place_deployable(ctx, item_id, distance, height),
        ControlRequest::PlaceBuilding {
            piece,
            distance,
            height,
        } => placement::place_building(ctx, piece, distance, height),
        ControlRequest::PlaceDoor { code, flip, iron } => doors::place_door(ctx, code, flip, iron),
        ControlRequest::DoorInteract => doors::door_interact(ctx),
        ControlRequest::DoorPickUp => doors::door_pick_up(ctx),
        ControlRequest::OpenStorageBox => doors::open_storage_box(ctx),
        ControlRequest::CloseContainer => doors::close_container(ctx),
        ControlRequest::UpgradeBuilding { piece } => placement::upgrade_building(ctx, piece),
        ControlRequest::DemolishBuilding { piece } => placement::demolish_building(ctx, piece),
        ControlRequest::DoorEnterCode { code } => doors::door_enter_code(ctx, code),
        ControlRequest::SetLook { yaw, pitch } => movement::set_look(ctx, yaw, pitch),
        ControlRequest::SetScreen { screen } => screens::set_screen(ctx, screen),
        ControlRequest::SetInventoryOpen { open, admin_tab } => {
            screens::set_inventory_open(ctx, open, admin_tab)
        }
        ControlRequest::SetCraftingOpen { open } => screens::set_crafting_open(ctx, open),
        ControlRequest::SetWorldMapOpen { open } => screens::set_world_map_open(ctx, open),
        ControlRequest::AddWorldMapMarker { x, z } => screens::add_world_map_marker(ctx, x, z),
        ControlRequest::SetWorldMapView {
            zoom,
            center_x,
            center_z,
        } => screens::set_world_map_view(ctx, zoom, center_x, center_z),
        ControlRequest::Warp { x, z } => movement::warp(ctx, x, z),
        ControlRequest::Swing => combat::swing(ctx),
        ControlRequest::ThrowBomb { power } => combat::throw_bomb(ctx, power),
        ControlRequest::Respawn => combat::respawn(ctx),
        ControlRequest::RangedPoseDebug {
            draw,
            reload,
            recoil,
            aim,
            swing,
            use_charge,
        } => combat::ranged_pose_debug(ctx, draw, reload, recoil, aim, swing, use_charge),
        ControlRequest::Walk { seconds, run } => movement::walk(ctx, seconds, run),
        ControlRequest::DumpState => capture::dump_state(ctx),
    }
}

/// Forward a slash command to the server: the generic escape hatch, so it
/// lives beside the dispatch instead of in a domain module.
fn send_command(ctx: &mut HandlerContext, text: String) -> Result<String> {
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Command { text })?;
    Ok("command queued".to_owned())
}
