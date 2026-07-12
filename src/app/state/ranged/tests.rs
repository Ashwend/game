use super::*;
use crate::protocol::SERVER_TICK_RATE_HZ;

fn bow() -> RangedProfile {
    RangedProfile {
        damage_min: 15,
        damage_max: 40,
        projectile_speed_mps: 35.0,
        // 1.5 s draw at 20 Hz.
        draw_ticks_to_full: (1.5 * SERVER_TICK_RATE_HZ) as u64,
        cooldown_ticks: 5,
        ammo_item: "arrow",
        knockback_speed: 1.0,
        max_durability: Some(200),
    }
}

fn crossbow() -> RangedProfile {
    RangedProfile {
        damage_min: 55,
        damage_max: 55,
        projectile_speed_mps: 55.0,
        draw_ticks_to_full: 0,
        // 3.5 s reload at 20 Hz.
        cooldown_ticks: (3.5 * SERVER_TICK_RATE_HZ) as u64,
        ammo_item: "arrow",
        knockback_speed: 1.5,
        max_durability: Some(600),
    }
}

#[test]
fn bow_press_starts_a_draw_and_release_fires() {
    let mut state = RangedDrawState::default();

    // Press starts a draw (DrawStart), and the draw is now held.
    let action = state.update(0.0, true, true, Some(bow()), true);
    assert_eq!(action, Some(RangedAction::DrawStart));
    assert!(state.is_drawing());
    assert!(!state.active_is_instant_fire(), "a bow holds its draw");

    // Holding advances the draw but emits no action.
    let held = state.update(0.5, false, true, Some(bow()), true);
    assert_eq!(held, None);
    assert!(state.draw_fraction() > 0.0);

    // Release fires, carrying the draw fraction at release (0.5 s of the 1.5 s
    // window => a third).
    let fired = state.update(0.0, false, false, Some(bow()), true);
    match fired {
        Some(RangedAction::Fire { draw_fraction }) => {
            assert!(
                (draw_fraction - 1.0 / 3.0).abs() < 0.02,
                "release carries the held fraction, got {draw_fraction}"
            );
        }
        other => panic!("expected a fire on release, got {other:?}"),
    }
    assert!(!state.is_drawing(), "the draw ends on release");
}

#[test]
fn a_release_below_the_minimum_draw_cancels_instead_of_firing() {
    let mut state = RangedDrawState::default();
    let _ = state.update(0.0, true, true, Some(bow()), true);

    // Barely any hold (well under the minimum firing fraction of the 1.5 s
    // window): the release lowers the bow, it never tap-fires.
    let _ = state.update(0.05, false, true, Some(bow()), true);
    let released = state.update(0.0, false, false, Some(bow()), true);
    assert_eq!(released, Some(RangedAction::Cancel));
    assert!(!state.is_drawing(), "the abandoned draw is cleared");

    // A committed hold past the threshold fires as usual.
    let _ = state.update(0.0, true, true, Some(bow()), true);
    let _ = state.update(1.0, false, true, Some(bow()), true);
    assert!(matches!(
        state.update(0.0, false, false, Some(bow()), true),
        Some(RangedAction::Fire { .. })
    ));
}

#[test]
fn bow_draw_fraction_ramps_to_full_over_the_draw_window() {
    let mut state = RangedDrawState::default();
    let _ = state.update(0.0, true, true, Some(bow()), true);
    assert_eq!(state.draw_fraction(), 0.0, "fresh draw is at zero");

    // Half the 1.5 s window => ~0.5 fraction.
    let _ = state.update(0.75, false, true, Some(bow()), true);
    let mid = state.draw_fraction();
    assert!((mid - 0.5).abs() < 0.02, "half draw ~0.5, got {mid}");

    // Past the full window clamps at 1.0 (no overdraw).
    let _ = state.update(5.0, false, true, Some(bow()), true);
    assert_eq!(state.draw_fraction(), 1.0, "draw clamps at full");
}

#[test]
fn crossbow_press_fires_immediately_no_hold() {
    let mut state = RangedDrawState::default();
    // A crossbow press emits DrawStart (so the server sees the same ordering),
    // and the input layer will follow with an immediate Fire because
    // `active_is_instant_fire` is true.
    let action = state.update(0.0, true, true, Some(crossbow()), true);
    assert_eq!(action, Some(RangedAction::DrawStart));
    assert!(
        state.active_is_instant_fire(),
        "a crossbow fires on press rather than holding"
    );
    assert!(!state.is_drawing(), "a crossbow never enters a held draw");
    assert_eq!(
        state.draw_fraction(),
        0.0,
        "no draw fraction for a crossbow"
    );
}

#[test]
fn crossbow_dry_clicks_while_on_reload_cooldown() {
    let mut state = RangedDrawState::default();
    // Fire once, then arm the reload the way the input layer does.
    let _ = state.update(0.0, true, true, Some(crossbow()), true);
    state.begin_reload(crossbow());
    assert!(state.is_reloading());

    // Pressing again during the reload dry-clicks (no shot, quiet cue).
    let action = state.update(0.1, true, true, Some(crossbow()), true);
    assert_eq!(action, Some(RangedAction::DryClick));

    // Once the reload elapses, a press fires again.
    let reload_secs = crossbow().cooldown_ticks as f32 / SERVER_TICK_RATE_HZ;
    let _ = state.update(reload_secs, false, false, Some(crossbow()), true);
    assert!(!state.is_reloading(), "reload finished");
    let action = state.update(0.0, true, true, Some(crossbow()), true);
    assert_eq!(action, Some(RangedAction::DrawStart));
}

#[test]
fn no_ammo_dry_clicks_and_never_draws() {
    let mut state = RangedDrawState::default();
    let action = state.update(0.0, true, true, Some(bow()), false);
    assert_eq!(action, Some(RangedAction::DryClick));
    assert!(!state.is_drawing(), "an empty quiver can't start a draw");
}

#[test]
fn item_swap_cancels_an_active_draw() {
    let mut state = RangedDrawState::default();
    let _ = state.update(0.0, true, true, Some(bow()), true);
    assert!(state.is_drawing());

    // The active item is no longer a ranged weapon (swap to a tool / bare hands):
    // profile None cancels the draw with exactly one Cancel.
    let action = state.update(0.1, false, true, None, true);
    assert_eq!(action, Some(RangedAction::Cancel));
    assert!(!state.is_drawing());

    // A second None frame is idempotent: nothing left to cancel.
    let again = state.update(0.1, false, true, None, true);
    assert_eq!(again, None);
}

#[test]
fn cancel_if_active_is_idempotent() {
    let mut state = RangedDrawState::default();
    assert_eq!(
        state.cancel_if_active(),
        None,
        "nothing to cancel when idle"
    );
    let _ = state.update(0.0, true, true, Some(bow()), true);
    assert_eq!(state.cancel_if_active(), Some(RangedAction::Cancel));
    assert_eq!(state.cancel_if_active(), None, "already cancelled");
}

#[test]
fn reload_fraction_ramps_only_for_a_crossbow() {
    let mut state = RangedDrawState::default();

    // Bow: its tiny post-fire floor is not a reload, so reload_fraction stays 0.
    state.begin_reload(bow());
    assert!(!state.is_reloading(), "a bow has no reload cycle");
    assert_eq!(state.reload_fraction(), 0.0);

    // Crossbow: reload fraction ramps 0 -> 1 across the cooldown window.
    let mut cross = RangedDrawState::default();
    cross.begin_reload(crossbow());
    assert!(cross.is_reloading());
    assert_eq!(cross.reload_fraction(), 0.0, "just fired");
    let window = crossbow().cooldown_ticks as f32 / SERVER_TICK_RATE_HZ;
    let _ = cross.update(window * 0.5, false, false, Some(crossbow()), true);
    let mid = cross.reload_fraction();
    assert!((mid - 0.5).abs() < 0.05, "half reload ~0.5, got {mid}");
    let _ = cross.update(window, false, false, Some(crossbow()), true);
    assert_eq!(
        cross.reload_fraction(),
        0.0,
        "fully reloaded resets to idle"
    );
    assert!(!cross.is_reloading());
}

#[test]
fn crossbow_ads_eases_in_while_aiming_and_back_out() {
    // Holding the aim eases the fraction toward 1; releasing eases it back to
    // 0. Both directions take the same short window, so a quick tap never
    // snaps the viewmodel.
    let mut state = RangedDrawState::default();
    assert_eq!(state.aim_fraction(), 0.0);

    state.tick_aim(0.05, true);
    let partial = state.aim_fraction();
    assert!(
        partial > 0.0 && partial < 1.0,
        "a short hold is partway in, got {partial}"
    );
    state.tick_aim(1.0, true);
    assert_eq!(state.aim_fraction(), 1.0, "a long hold reaches full ADS");

    state.tick_aim(0.05, false);
    assert!(
        state.aim_fraction() < 1.0,
        "releasing starts easing back out"
    );
    state.tick_aim(1.0, false);
    assert_eq!(state.aim_fraction(), 0.0, "the ADS fully releases");
}

#[test]
fn reload_cooldown_advances_even_when_holstered() {
    // The reload must burn down even with no ranged weapon in hand, so switching
    // away and back can never leave the crossbow stuck mid-reload.
    let mut state = RangedDrawState::default();
    state.begin_reload(crossbow());
    let window = crossbow().cooldown_ticks as f32 / SERVER_TICK_RATE_HZ;
    // Holster (profile None) and let the whole window pass.
    let _ = state.update(window, false, false, None, false);
    assert!(
        !state.is_reloading(),
        "reload finishes even while holstered"
    );
}
