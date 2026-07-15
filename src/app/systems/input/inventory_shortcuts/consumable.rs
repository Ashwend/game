//! Client-side consumable input: hold left click to bind a wound with a held
//! bandage, release early to abandon it.
//!
//! A consumable has no melee profile, no `RangedProfile`, and no explosive
//! profile, so none of the other input loops claim it. This one drives
//! [`ConsumeChargeState`](crate::app::state::ConsumeChargeState) on the same
//! hold-to-charge idiom as the bow draw and
//! the bomb wind-up: a held left click builds the charge (the viewmodel raises
//! the roll and unrolls its tail, the HUD charge bar fills), and letting go
//! before it completes abandons the use for free.
//!
//! ## What this layer deliberately does NOT do
//!
//! It never decides that the use *finished*, and it has no message with which to
//! say so. It sends exactly two things: `UseStart` on the press and `UseCancel`
//! when the hold ends. The server runs its own charge clock and applies the heal
//! itself (see `server::heal`), which is why a forged client cannot conjure an
//! instant heal. The client learns the bandage landed the same way it learns
//! about damage: the replicated health changes.
//!
//! That also means the local charge bar filling to 100% is a *prediction*. The
//! server crosses the line a frame or two later. The viewmodel simply holds at
//! full until the item leaves the inventory.

use bevy::prelude::*;

use crate::{
    app::state::ConsumeAction,
    items::{ConsumableProfile, item_definition},
    protocol::ConsumableCommand,
};

use super::GameplayInventoryShortcutsParams;
use super::send::send_gameplay_message;

/// The `ConsumableProfile` of the active actionbar item, if it has one.
fn held_consumable(
    local_player: &crate::app::state::LocalPlayerState,
) -> Option<ConsumableProfile> {
    local_player
        .private
        .as_ref()
        .and_then(|private| private.inventory.active_actionbar_stack())
        .and_then(|stack| item_definition(&stack.item_id))
        .and_then(|definition| definition.consumable)
}

/// Drive the consumable input this frame. Returns `true` when a consumable is
/// held (so the caller skips the melee swing entirely), `false` when the active
/// item is not a consumable and the other paths should run.
///
/// `local_dead` and `swapping` lock out the charge exactly as they lock out a
/// swing: a corpse can't bandage itself, and a mid-swap item isn't in hand yet.
pub(super) fn drive_consumable_input(
    params: &mut GameplayInventoryShortcutsParams,
    local_dead: bool,
    swapping: bool,
) -> bool {
    let Some(profile) = held_consumable(&params.local_player) else {
        // Not holding a consumable any more: abandon any use in flight so a
        // swapped-away bandage can't complete behind the player's back.
        cancel_if_active(params);
        return false;
    };

    // A corpse or a mid-swap item can't be used: abandon any in-flight use, but
    // still claim the frame so the melee swing path doesn't run for a bandage.
    if local_dead || swapping {
        cancel_if_active(params);
        return true;
    }

    let just_pressed = params.mouse_buttons.just_pressed(MouseButton::Left);
    let pressed = params.mouse_buttons.pressed(MouseButton::Left);

    let action = params.consume_charge.update(
        params.time.delta_secs(),
        just_pressed,
        pressed,
        Some(profile),
    );
    dispatch(params, action);

    true
}

/// Abandon any in-flight use and tell the server, exactly once. Idempotent: the
/// state machine only yields a `Cancel` when a use was actually live, so this is
/// safe to call every frame from the overlay / focus / swap guards.
pub(super) fn cancel_if_active(params: &mut GameplayInventoryShortcutsParams) {
    let action = params.consume_charge.cancel_if_active();
    dispatch(params, action);
}

fn dispatch(params: &mut GameplayInventoryShortcutsParams, action: Option<ConsumeAction>) {
    match action {
        Some(ConsumeAction::UseStart) => send(params, ConsumableCommand::UseStart),
        Some(ConsumeAction::Cancel) => send(params, ConsumableCommand::UseCancel),
        None => {}
    }
}

fn send(params: &mut GameplayInventoryShortcutsParams, command: ConsumableCommand) {
    send_gameplay_message(
        &mut params.runtime,
        &mut params.error_toasts,
        crate::protocol::ClientMessage::Consumable(command),
        "consumable",
    );
}
