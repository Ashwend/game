//! Client-side thrown-explosive input: hold left click to wind up a held
//! powder bomb, release to throw with charge-scaled power.
//!
//! A thrown explosive (the powder bomb) does not land a melee hit (no melee
//! profile) and is not a ranged weapon (no `RangedProfile`), so neither the
//! melee dispatch nor the draw/fire loop ([`super::ranged`]) fires for it.
//! Instead this loop drives [`ThrowChargeState`](crate::app::state::ThrowChargeState) (the bow-draw idiom): a held
//! left click builds the charge (the wind-up pose pulls the bomb back, the HUD
//! charge bar fills), and releasing at or above the minimum fraction primes
//! the toss on the shared swing state machine
//! ([`GatherInputState::begin_primed_swing`](crate::app::state::GatherInputState::begin_primed_swing) with the [`ItemModel::ThrownBomb`]
//! archetype), which skips straight to the release beat. When the toss crosses
//! its release frame it plays the release cue and sends
//! [`ExplosiveCommand::Throw`] with the stashed power along the camera-forward
//! aim. A release under the minimum cancels: no throw, the pose settles back.
//! The server still owns the whole throw (velocity clamp, ballistics, bounce,
//! fuse, blast); this layer only charges, animates, and signals the release.

use bevy::prelude::*;

use crate::{
    app::{
        audio::{PlaySound, SoundId},
        state::{SwingFeelScales, ThrowAction},
    },
    items::{ExplosiveDelivery, ItemModel, item_definition, look_forward},
    protocol::ExplosiveCommand,
};

use super::GameplayInventoryShortcutsParams;
use super::send::send_gameplay_message;

/// True when the active actionbar item is a thrown explosive (the powder bomb).
fn holding_thrown_explosive(local_player: &crate::app::state::LocalPlayerState) -> bool {
    local_player
        .private
        .as_ref()
        .and_then(|p| p.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|def| def.explosive)
        .is_some_and(|e| e.delivery == ExplosiveDelivery::Thrown)
}

/// Drive the thrown-explosive input this frame. Returns `true` when a thrown
/// explosive is held (so the caller skips the melee swing entirely), or `false`
/// when the active item is not a thrown explosive and the melee/ranged paths
/// should run.
///
/// `local_dead` and `swapping` lock out the charge exactly as they lock out a
/// swing: a corpse or a mid-swap item can't wind up a throw.
pub(super) fn drive_explosive_input(
    params: &mut GameplayInventoryShortcutsParams,
    local_dead: bool,
    swapping: bool,
) -> bool {
    if !holding_thrown_explosive(&params.local_player) {
        // Not holding a bomb any more: abandon any charge in flight so a
        // swapped-away bomb can't fire a queued throw later.
        params.throw_charge.cancel();
        return false;
    }
    if local_dead || swapping {
        // Held but can't throw right now; cancel the charge and any in-flight
        // toss so no stale release fires, and still consume the branch so no
        // melee swing runs.
        params.throw_charge.cancel();
        params.gather_input.cancel();
        return true;
    }

    let feel = SwingFeelScales {
        duration_scale: params.settings.dev.combat.swing_duration_scale,
        impact_fraction_offset: params.settings.dev.combat.impact_fraction_offset,
    };
    let just_pressed = params.mouse_buttons.just_pressed(MouseButton::Left);
    let pressed = params.mouse_buttons.pressed(MouseButton::Left);

    // Charge while held; on a committed release, stash the power and prime the
    // toss at its release beat (the wind-up already played as the charge pose).
    match params
        .throw_charge
        .update(params.time.delta_secs(), just_pressed, pressed)
    {
        Some(ThrowAction::Release { power }) => {
            params.throw_charge.stash_power(power);
            params
                .gather_input
                .begin_primed_swing(ItemModel::ThrownBomb, feel);
        }
        Some(ThrowAction::Cancel) | None => {}
    }

    // Advance a primed toss (never started by the raw press: the charge above
    // owns the press) and fire the throw at its release frame. Doing this at
    // the release beat (not on the button release) makes the bomb leave the
    // hand exactly when the pose flicks forward.
    let impact = params.gather_input.update(
        params.time.delta_secs(),
        false,
        false,
        Some(ItemModel::ThrownBomb),
        None,
        feel,
    );
    if impact.is_some()
        && let Some(power) = params.throw_charge.take_pending_power()
        && let Some(view) = params.runtime.local_view()
    {
        params
            .play_sound
            .write(PlaySound::non_spatial(SoundId::BombThrowRelease));
        let dir = look_forward(view.yaw, view.pitch);
        send_gameplay_message(
            &mut params.runtime,
            &mut params.error_toasts,
            crate::protocol::ClientMessage::Explosive(ExplosiveCommand::Throw {
                aim_dir: crate::protocol::Vec3Net::new(dir.x, dir.y, dir.z),
                power,
            }),
            "throw",
        );
    }
    true
}
