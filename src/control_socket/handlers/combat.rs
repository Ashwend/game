//! Combat-flavored handlers: the cosmetic swing, the powder-bomb throw,
//! respawn, and the debug pose overrides used to screenshot viewmodel
//! animations headless.

use std::sync::atomic::{AtomicU32, Ordering};

use anyhow::{Context, Result};

use super::HandlerContext;
use crate::{
    items::{ItemModel, item_definition},
    protocol::{ClientMessage, SwingStartCommand, Vec3Net},
};

pub(super) fn swing(ctx: &mut HandlerContext) -> Result<String> {
    // Derive the swing archetype from the held item exactly as the client
    // and server do: a weapon's own model, a gather tool's archetype, else
    // the bag punch. The server re-derives it too, so this only picks the
    // animation for the headless-harness swing.
    let model = ctx
        .local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .map(|definition| definition.swing_model())
        .unwrap_or(ItemModel::Bag);
    // Monotonic per-process seq so the server never rejects it as stale
    // (it keeps the max). One source for all clients in this process is
    // fine: the server dedupes per client_id.
    static SWING_SEQ: AtomicU32 = AtomicU32::new(0);
    let seq = SWING_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::SwingStart(SwingStartCommand { seq, model }))?;
    Ok(format!("swing {model:?} (seq {seq}) sent"))
}

pub(super) fn throw_bomb(ctx: &mut HandlerContext, power: Option<f32>) -> Result<String> {
    let power = power.unwrap_or(1.0).clamp(0.0, 1.0);
    let view = ctx
        .runtime
        .local_view()
        .context("no local player view (not in a world)")?;
    let dir = crate::items::look_forward(view.yaw, view.pitch);
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Explosive(
        crate::protocol::ExplosiveCommand::Throw {
            aim_dir: Vec3Net::new(dir.x, dir.y, dir.z),
            power,
        },
    ))?;
    Ok(format!("bomb thrown at power {power:.2}"))
}

pub(super) fn respawn(ctx: &mut HandlerContext) -> Result<String> {
    let session = ctx
        .runtime
        .session
        .as_mut()
        .context("no active session (not in a world)")?;
    session.send(ClientMessage::Respawn)?;
    // Mirror the death-splash button: drop the splash so the HUD
    // returns the moment the respawned state replicates.
    ctx.menu.death_splash = None;
    Ok("respawn requested".to_owned())
}

pub(super) fn ranged_pose_debug(
    ctx: &mut HandlerContext,
    draw: Option<f32>,
    reload: Option<f32>,
    recoil: Option<f32>,
    aim: Option<f32>,
    swing: Option<f32>,
    use_charge: Option<f32>,
) -> Result<String> {
    // Force the ranged / swing / consumable pose so the animated bow /
    // crossbow / melee / bandage viewmodel can be screenshotted headless.
    // Clears to live input when every field is None.
    let any_ranged = draw.is_some() || reload.is_some() || recoil.is_some() || aim.is_some();
    let over = any_ranged.then_some(crate::app::state::RangedPoseOverride {
        draw,
        reload,
        recoil,
        aim,
    });
    ctx.ranged_input.set_debug_override(over);
    ctx.gather_input.set_debug_swing_override(swing);
    ctx.consume_charge.set_debug_use(use_charge);
    Ok(format!(
        "pose override set (draw={draw:?} reload={reload:?} recoil={recoil:?} aim={aim:?} swing={swing:?} use={use_charge:?})"
    ))
}
