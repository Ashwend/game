//! Client-side ranged-weapon input: the draw/fire/reload loop for a held bow or
//! crossbow.
//!
//! A ranged weapon does not run the melee swing state machine ([`super::swing`]).
//! Instead the primary-attack button drives [`crate::app::state::RangedDrawState`]:
//! a press begins a draw (`DrawStart`) and a release fires (`Fire { aim_dir }`),
//! with the camera-forward look direction as the aim. A crossbow fires the instant
//! it is pressed (still `DrawStart` then `Fire` back to back, matching the server's
//! validation ordering) when it is off its reload cooldown, and dry-clicks
//! otherwise. Item swaps and overlays cancel an in-flight draw with `DrawCancel`.
//!
//! Audio: the ranged loop is deliberately near-silent for now. Every draw,
//! release, whoosh, fire, and reload cue tried so far read wrong in play
//! (owner reports), so only the dry-click plays locally; the camera kick
//! carries the shot's punch and the spatial arrow-lodge impact is the shot's
//! audible payoff. The server owns the shot outcome; this layer only signals
//! intent and paces feel.
//!
//! Own-arrow prediction lives in `crate::app::systems::items::projectiles`; this
//! module signals a fired shot to it via a [`PredictedArrowEvent`] the instant a
//! `Fire` is sent, so the local arrow visual appears without waiting for the
//! replicated projectile to arrive.

use bevy::prelude::*;

use crate::{
    analytics::Event,
    app::{
        audio::{PlaySound, SoundId},
        state::RangedAction,
    },
    inventory::count_items_in_inventory,
    items::{ItemId, RangedProfile, item_definition, look_forward},
    protocol::RangedCommand,
};

use super::GameplayInventoryShortcutsParams;
use super::send::send_ranged_command;

/// One-in-`RANGED_FIRE_SAMPLE_EVERY` sampling of `ranged_fired`. A held
/// crossbow trigger fires at most one shot per reload cooldown, so shots are not
/// per-frame spammy, but a long session still adds up; sampling keeps the pipe
/// modest while preserving the weapon mix. Cheap and privacy-sane: it only ever
/// carries the weapon id.
const RANGED_FIRE_SAMPLE_EVERY: u32 = 5;

/// Rolling shot counter behind the `ranged_fired` sampler. Persists across the
/// system's frames as a Bevy resource.
#[derive(Resource, Default)]
pub(crate) struct RangedFireSampler {
    shots: u32,
}

impl RangedFireSampler {
    /// Count one shot; return `true` on every `RANGED_FIRE_SAMPLE_EVERY`th so the
    /// first shot of a fresh counter is always sampled.
    fn should_sample(&mut self) -> bool {
        let sample = self.shots.is_multiple_of(RANGED_FIRE_SAMPLE_EVERY);
        self.shots = self.shots.wrapping_add(1);
        sample
    }
}

/// Fired the instant the local player looses a shot, so the projectile renderer can
/// spawn a predicted arrow visual immediately (before the replicated projectile
/// arrives). Carries the launch parameters the server used so the predicted arc
/// matches the authoritative one. Both weapon archetypes loose the same arrow
/// visual, so no model selector travels here.
#[derive(Message, Debug, Clone, Copy)]
pub(crate) struct PredictedArrowEvent {
    /// Eye position the shot launches from.
    pub(crate) origin: Vec3,
    /// Launch velocity: `aim_dir * projectile_speed`.
    pub(crate) velocity: Vec3,
}

/// Resolve the held ranged weapon: its id, profile, and whether the player has at
/// least one arrow (client-side count; the server re-checks). `None` when the
/// active item is not a ranged weapon, in which case the melee swing path runs
/// instead. The id is carried so the fire path can tag the `ranged_fired`
/// analytics event with the weapon.
pub(super) fn held_ranged(
    local_player: &crate::app::state::LocalPlayerState,
) -> Option<(ItemId, RangedProfile, bool)> {
    let private = local_player.private.as_ref()?;
    let stack = private.inventory.active_actionbar_stack()?;
    let definition = item_definition(&stack.item_id)?;
    let profile = definition.ranged?;
    let has_ammo = count_items_in_inventory(&private.inventory, profile.ammo_item) >= 1;
    Some((stack.item_id.clone(), profile, has_ammo))
}

/// Drive the ranged draw/fire loop for the held weapon this frame. Returns `true`
/// when a ranged weapon is held (so the caller skips the melee swing entirely), or
/// `false` when the active item is not ranged and the melee path should run.
///
/// `local_dead` and `swapping` short-circuit to a cancel: a corpse or a
/// mid-swap weapon can't draw, exactly as the melee path locks out swings.
pub(super) fn drive_ranged_input(
    params: &mut GameplayInventoryShortcutsParams,
    local_dead: bool,
    swapping: bool,
) -> bool {
    let Some((weapon_id, profile, has_ammo)) = held_ranged(&params.local_player) else {
        // Not a ranged weapon: keep the reload clock honest and make sure no stale
        // draw lingers, then let the melee path run.
        idle_tick_and_cancel(params);
        return false;
    };

    // A corpse or a mid-swap weapon can't draw: idle-tick (reload keeps burning)
    // and abandon any in-flight draw.
    if local_dead || swapping {
        idle_tick_and_cancel(params);
        return true;
    }

    let just_pressed = params.mouse_buttons.just_pressed(MouseButton::Left);
    let pressed = params.mouse_buttons.pressed(MouseButton::Left);

    let action = params.ranged_input.update(
        params.time.delta_secs(),
        just_pressed,
        pressed,
        Some(profile),
        has_ammo,
    );

    // Crossbow aim-down-sights: holding right mouse with a READY crossbow
    // (instant-fire profile, not mid-reload) eases the ADS fraction in; any
    // other state eases it back out. Pure client feel: it centres the
    // viewmodel to the eye line and pinches the FOV so an experienced shooter
    // can read where the bolt will land. The bow ignores it (its RMB is free).
    let is_crossbow = profile.draw_ticks_to_full == 0;
    let aiming = is_crossbow
        && params.mouse_buttons.pressed(MouseButton::Right)
        && !params.ranged_input.is_reloading();
    params
        .ranged_input
        .tick_aim(params.time.delta_secs(), aiming);

    match action {
        Some(RangedAction::DrawStart) => {
            send_ranged_command(
                &mut params.runtime,
                &mut params.error_toasts,
                RangedCommand::DrawStart,
            );
            // A crossbow has no draw hold: fire immediately on the same press, so
            // the server sees DrawStart then Fire back to back. A bow's draw is
            // deliberately SILENT: every draw cue tried so far (synthetic creak
            // ramp, stretched foley) read wrong in play (owner reports), so the
            // release thunk is the bow's first sound.
            if params.ranged_input.active_is_instant_fire() {
                fire_shot(params, &weapon_id, profile, 1.0);
            }
        }
        Some(RangedAction::Fire { draw_fraction }) => {
            fire_shot(params, &weapon_id, profile, draw_fraction);
        }
        Some(RangedAction::Cancel) => {
            send_ranged_command(
                &mut params.runtime,
                &mut params.error_toasts,
                RangedCommand::DrawCancel,
            );
        }
        Some(RangedAction::DryClick) => {
            // Quiet no-shot cue: still on reload, or no arrow. No message.
            params
                .play_sound
                .write(PlaySound::non_spatial(SoundId::RangedDryClick));
        }
        None => {}
    }

    true
}

/// Send the `Fire`, arm the local reload cooldown, and spawn the predicted
/// arrow. Called for both a bow release (with its draw fraction at release) and
/// a crossbow's immediate fire (always `1.0`).
fn fire_shot(
    params: &mut GameplayInventoryShortcutsParams,
    weapon_id: &ItemId,
    profile: RangedProfile,
    draw_fraction: f32,
) {
    // Aim is the camera forward (the same look ray the melee aim + the server's fire
    // path use), built from the local predicted yaw/pitch.
    let Some(view) = params.runtime.local_view() else {
        return;
    };
    let dir = look_forward(view.yaw, view.pitch);
    let aim = Vec3::new(dir.x, dir.y, dir.z);

    send_ranged_command(
        &mut params.runtime,
        &mut params.error_toasts,
        RangedCommand::Fire {
            aim_dir: crate::protocol::Vec3Net::new(aim.x, aim.y, aim.z),
        },
    );

    // Sampled analytics: one in every `RANGED_FIRE_SAMPLE_EVERY` shots, tagged
    // with the weapon id only. Fired here (a real shot was just sent) rather than
    // on the dry-click or cancel paths.
    if params.ranged_fire_sampler.should_sample() {
        params.analytics.track(Event::RangedFired {
            weapon: weapon_id.to_string(),
        });
    }

    // Camera recoil ON FIRE: give the shot real punch. The kick profile is keyed
    // on the weapon's ItemModel (bow = medium string-snap, crossbow = heavy
    // lurch); `camera_kick.trigger` applies the live DevCombat kick scales exactly
    // like the melee swing path, which `camera_follow_system` syncs every frame.
    if let Some(model) = item_definition(weapon_id).map(|def| def.model) {
        params.camera_kick.trigger(model);
    }

    // Arm the local reload/recoil clock so the client can dry-click + drive the
    // reload pose without a server round trip.
    params.ranged_input.begin_reload(profile);

    // Predict the own-arrow visual from the same ballistic params the server uses:
    // launch from the eye at `aim * speed`, the speed scaled by the released draw
    // exactly like the server's fire path.
    let eye = Vec3::from(view.position) + Vec3::Y * crate::app::EYE_HEIGHT;
    params.predicted_arrows.write(PredictedArrowEvent {
        origin: eye,
        velocity: aim * profile.speed_for_draw_fraction(draw_fraction),
    });

    // No launch audio for either weapon (for now): every release/whoosh cue
    // tried so far read wrong or as a reused existing sample (owner reports),
    // so the camera kick carries the shot's punch and the arrow-lodge impact
    // is the shot's audible payoff.
}

/// Idle-tick the ranged state for a frame with no ranged input: an overlay is up,
/// the wheel is open, the player is dead / mid-swap, or the held item is not a
/// ranged weapon. The reload cooldown keeps burning (the server's cooldown ticks
/// regardless of any local overlay, per the gameplay-never-pauses invariant, so
/// the local mirror must not fall behind and dry-click after the menu closes),
/// and any in-flight draw is abandoned with exactly one `DrawCancel` so the
/// server lowers the bow and restores movement. Idempotent when already idle.
pub(super) fn idle_tick_and_cancel(params: &mut GameplayInventoryShortcutsParams) {
    // No ranged input this frame also means no ADS: ease the aim back out so an
    // overlay opening (or a swap / death) lowers the crossbow from the eye.
    params
        .ranged_input
        .tick_aim(params.time.delta_secs(), false);
    if params
        .ranged_input
        .update(params.time.delta_secs(), false, false, None, false)
        == Some(RangedAction::Cancel)
    {
        send_ranged_command(
            &mut params.runtime,
            &mut params.error_toasts,
            RangedCommand::DrawCancel,
        );
    }
}
