//! Movement and look handlers: absolute camera aim, the position warp, and the
//! timed forward-walk order that exercises the real controller headless.

use std::time::Duration;

use anyhow::{Context, Result, bail};

use super::HandlerContext;
use crate::{controller::MAX_LOOK_PITCH, protocol::Vec3Net};

pub(super) fn set_look(ctx: &mut HandlerContext, yaw: f32, pitch: f32) -> Result<String> {
    if !yaw.is_finite() || !pitch.is_finite() {
        bail!("yaw/pitch must be finite radians");
    }
    ctx.look.yaw = yaw;
    ctx.look.pitch = pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
    Ok(format!(
        "look set to yaw {:.3}, pitch {:.3}",
        ctx.look.yaw, ctx.look.pitch
    ))
}

pub(super) fn warp(ctx: &mut HandlerContext, x: f32, z: f32) -> Result<String> {
    if !x.is_finite() || !z.is_finite() {
        bail!("x/z must be finite");
    }
    let predicted = ctx
        .runtime
        .predicted_local
        .as_mut()
        .context("no local player (not in a world)")?;
    // Keep the current height; the controller + server gravity settle
    // it. Zero momentum so the avatar doesn't keep sliding from the warp.
    predicted.position = Vec3Net::new(x, predicted.position.y, z);
    predicted.velocity = Vec3Net::ZERO;
    Ok(format!("warped to [{x:.2}, {z:.2}]"))
}

pub(super) fn walk(ctx: &mut HandlerContext, seconds: f32, run: Option<bool>) -> Result<String> {
    let run = run.unwrap_or(false);
    let seconds = seconds.clamp(0.0, 30.0);
    ctx.look.agent_walk = (seconds > 0.0).then(|| crate::app::state::AgentWalk {
        deadline: std::time::Instant::now() + Duration::from_secs_f32(seconds),
        run,
    });
    Ok(format!("walking forward for {seconds:.1}s (run={run})"))
}
