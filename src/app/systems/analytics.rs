//! Bridge between Bevy state and the [`crate::analytics`] worker.
//!
//! Holds the small observer systems that turn ambient state changes into
//! discrete analytics events:
//!
//! - [`screen_viewed_system`], fires `screen_viewed` when the visible screen
//!   changes: the login splash (`sign_in`) while logged out, otherwise the
//!   [`MenuState::screen`] the user is on.
//! - [`session_started_system`], fires `session_started` when
//!   [`ClientRuntime::session`] flips from `None` to `Some`. Mode is derived
//!   from `active_world_id` (present → singleplayer, absent → multiplayer).
//! - [`session_ended_system`], fires `session_ended` when the session goes
//!   away, with a reason taken from [`PendingSessionEndReason`].
//! - [`error_relay_system`], relays queued [`ClientErrorToast`] messages
//!   into typed `error` analytics events.
//!
//! These systems live here (not in `analytics::`) so the analytics crate
//! stays a leaf with no Bevy/app dependencies. Wire them into the schedule
//! in `app::run_app`.

use std::time::Instant;

use bevy::prelude::*;

use crate::{
    analytics::{Analytics, ErrorCategory, Event, ScreenKind, SessionEndReason, SessionMode},
    app::state::{AuthFlow, ClientErrorToast, ClientRuntime, LocalPlayerState, MenuState, Screen},
    protocol::{EQUIPMENT_SLOT_COUNT, EquipmentSlot},
};

/// Last [`ScreenKind`] we emitted `screen_viewed` for. Initialised to `None` so
/// the very first frame fires for the launch screen (the login splash for a
/// logged-out user, the main menu for a returning one).
#[derive(Resource, Default)]
pub(crate) struct LastTrackedScreen(pub(crate) Option<ScreenKind>);

/// Per-session bookkeeping: when it started + the reason set by the caller
/// (kick path, user quit). The reason is consumed once and reset to
/// [`SessionEndReason::Disconnect`] (the catch-all) on the next session.
#[derive(Resource, Default)]
pub(crate) struct SessionTracker {
    started_at: Option<Instant>,
    /// Whether `runtime.session` was `Some` on the previous frame.
    was_active: bool,
}

/// One-shot "this session is ending because of X" hint, consumed by
/// [`session_ended_system`]. Set by code that knows *why* the session is
/// going away (kick handler, pause-menu Quit button). If left untouched, we
/// assume a disconnect.
#[derive(Resource, Default, Debug, Clone, Copy)]
pub(crate) struct PendingSessionEndReason(pub(crate) Option<SessionEndReason>);

pub(crate) fn screen_viewed_system(
    analytics: Res<Analytics>,
    auth: Res<AuthFlow>,
    menu: Res<MenuState>,
    mut last: ResMut<LastTrackedScreen>,
) {
    // The login splash is gated by `AuthFlow`, not `MenuState::screen` (which
    // sits at `MainMenu` underneath it), so derive the screen from auth first:
    // a logged-out user is looking at the sign-in screen, not the menu. The
    // `Verifying`/`Authenticating` spinners are transient sub-states of the
    // sign-in flow, emit nothing for them so a returning user's silent refresh
    // doesn't log a spurious `sign_in` view before the menu loads.
    let screen = match &*auth {
        AuthFlow::Authenticated => map_screen(menu.screen),
        // The provider-outage dialog is part of the sign-in surface.
        AuthFlow::LoggedOut { .. } | AuthFlow::Unreachable { .. } => ScreenKind::SignIn,
        AuthFlow::Verifying(_) | AuthFlow::Authenticating(_) => return,
    };
    if last.0 == Some(screen) {
        return;
    }
    last.0 = Some(screen);
    analytics.track(Event::ScreenViewed { screen });
}

pub(crate) fn session_started_system(
    analytics: Res<Analytics>,
    runtime: Res<ClientRuntime>,
    mut tracker: ResMut<SessionTracker>,
) {
    let now_active = runtime.session.is_some();
    if now_active && !tracker.was_active {
        tracker.started_at = Some(Instant::now());
        let mode = if runtime.active_world_id.is_some() {
            SessionMode::Singleplayer
        } else {
            SessionMode::Multiplayer
        };
        analytics.track(Event::SessionStarted { mode });
    }
    tracker.was_active = now_active;
}

pub(crate) fn session_ended_system(
    analytics: Res<Analytics>,
    runtime: Res<ClientRuntime>,
    mut tracker: ResMut<SessionTracker>,
    mut reason: ResMut<PendingSessionEndReason>,
) {
    let now_active = runtime.session.is_some();
    // `session_started_system` runs first and flips `was_active` to true on
    // the rising edge; we only fire on the falling edge here.
    if !now_active && tracker.started_at.is_some() {
        let duration_s = tracker
            .started_at
            .map(|started| started.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        let reason_value = reason.0.take().unwrap_or(SessionEndReason::Disconnect);
        analytics.track(Event::SessionEnded {
            duration_s,
            reason: reason_value,
        });
        tracker.started_at = None;
    }
    let _ = now_active;
}

pub(crate) fn error_relay_system(
    analytics: Res<Analytics>,
    mut events: MessageReader<ClientErrorToast>,
) {
    for event in events.read() {
        analytics.track(Event::Error {
            category: classify_error(&event.text),
        });
    }
}

/// Last frame's worn item id per equipment slot (`None` when empty), so
/// [`equipment_change_system`] can fire `item_equipped` on the rising edge of a
/// slot filling or changing. The server derives the worn slots authoritatively;
/// this only watches the replicated result, so it also catches an equip that
/// happened via a path other than the paperdoll drag (a quick-equip shortcut).
/// `seeded` gates the first populated frame of a session: the armor a save
/// already had worn is recorded as the baseline without firing, so a session
/// start does not read as a burst of fresh equips.
#[derive(Resource, Default)]
pub(crate) struct EquipmentWatch {
    worn: [Option<String>; EQUIPMENT_SLOT_COUNT],
    seeded: bool,
}

/// Fires `item_equipped` when a worn equipment slot gains or swaps an item
/// during a session. Unequips (a slot going empty) are deliberately not tracked;
/// the event is about what players choose to wear. Reads the replicated
/// [`LocalPlayerState`], so it is authoritative-server-consistent and does not
/// double-fire on an optimistic move that the server later rejects.
pub(crate) fn equipment_change_system(
    analytics: Res<Analytics>,
    local_player: Res<LocalPlayerState>,
    mut watch: ResMut<EquipmentWatch>,
) {
    let Some(private) = local_player.private.as_ref() else {
        // Disconnected: forget the worn set (and the seed) so the next session's
        // already-worn armor is re-baselined, not read as a fresh equip.
        if watch.seeded || watch.worn.iter().any(Option::is_some) {
            *watch = EquipmentWatch::default();
        }
        return;
    };

    let mut next: [Option<String>; EQUIPMENT_SLOT_COUNT] = Default::default();
    for slot in EquipmentSlot::ALL {
        let index = slot.index();
        let current = private
            .inventory
            .equipment_slots
            .get(index)
            .and_then(|maybe| maybe.as_ref())
            .map(|stack| stack.item_id.to_string());

        // Fire only on a real in-session change, and only once the baseline is
        // seeded (the first populated frame just records what was already worn).
        if watch.seeded
            && current != watch.worn[index]
            && let Some(item_id) = current.clone()
        {
            analytics.track(Event::ItemEquipped {
                item_id,
                slot: slot.label().to_ascii_lowercase(),
            });
        }
        next[index] = current;
    }
    watch.worn = next;
    watch.seeded = true;
}

/// Last frame's tier of the workbench the local player has open (`None` when no
/// bench is open), so [`workbench_upgrade_system`] can fire `workbench_upgraded`
/// on the tier increasing while the same bench stays open. The upgrade respawns
/// the bench entity under the same id, so the open pointer survives and the view
/// tier updates in place, which is the signal watched here.
#[derive(Resource, Default)]
pub(crate) struct WorkbenchWatch {
    open_tier: Option<u8>,
}

/// Fires `workbench_upgraded` when the open workbench's tier rises. Only an
/// increase counts (opening a fresh tier-2 bench, or the pointer clearing, is
/// not an upgrade), so the event tracks the deliberate in-UI upgrade action.
pub(crate) fn workbench_upgrade_system(
    analytics: Res<Analytics>,
    local_player: Res<LocalPlayerState>,
    mut watch: ResMut<WorkbenchWatch>,
) {
    let current = local_player
        .private
        .as_ref()
        .and_then(|private| private.open_workbench)
        .map(|view| view.tier);

    if let (Some(previous), Some(now)) = (watch.open_tier, current)
        && now > previous
    {
        analytics.track(Event::WorkbenchUpgraded { tier: now });
    }
    watch.open_tier = current;
}

/// The impact ticks of meteors whose strikes have already been checked for a
/// local witness, so [`meteor_shower_impact_system`] fires
/// `meteor_shower_impact_witnessed` at most once per meteor rather than every
/// frame of its crater phase.
#[derive(Resource, Default)]
pub(crate) struct MeteorShowerImpactWatch {
    checked_ticks: std::collections::HashSet<u64>,
}

/// Fires `meteor_shower_impact_witnessed` the first frame the authoritative
/// clock crosses one of a shower's impact ticks while the local player is
/// within that meteor's size-scaled danger radius (they saw or were caught in
/// the strike). Everything is derived client-side from the announce payload
/// plus the player position, so no wire traffic and no server-side gate is
/// needed.
pub(crate) fn meteor_shower_impact_system(
    analytics: Res<Analytics>,
    runtime: Res<ClientRuntime>,
    mut watch: ResMut<MeteorShowerImpactWatch>,
) {
    if runtime.meteor_showers.is_empty() {
        // No live event: clear so the next event's ticks can re-fire.
        if !watch.checked_ticks.is_empty() {
            watch.checked_ticks.clear();
        }
        return;
    }
    let estimated_tick = runtime.server_tick();
    for event in &runtime.meteor_showers {
        if !event.has_impacted(estimated_tick) || watch.checked_ticks.contains(&event.impact_tick) {
            continue;
        }
        // Only count it as witnessed when the player was near the impact (the
        // same size-scaled radius the escalating HUD evacuation warning uses).
        // A player across the map neither sees nor feels the strike.
        if let Some(view) = runtime.local_view() {
            let dx = view.position.x - event.impact_position.x;
            let dz = view.position.z - event.impact_position.z;
            let danger = crate::game_balance::METEOR_SHOWER_DANGER_RADIUS_M * event.size;
            if (dx * dx + dz * dz).sqrt() <= danger {
                analytics.track(Event::MeteorShowerImpactWitnessed);
            }
        }
        // Record it either way so a distant player does not re-check every
        // crater frame; each meteor has a distinct impact tick.
        watch.checked_ticks.insert(event.impact_tick);
    }
}

fn map_screen(screen: Screen) -> ScreenKind {
    match screen {
        Screen::MainMenu => ScreenKind::MainMenu,
        Screen::Options => ScreenKind::Options,
        Screen::Worlds => ScreenKind::Worlds,
        Screen::Multiplayer => ScreenKind::Multiplayer,
        Screen::InGame => ScreenKind::InGame,
    }
}

fn classify_error(text: &str) -> ErrorCategory {
    let lower = text.to_ascii_lowercase();
    if lower.contains("auth") {
        ErrorCategory::Auth
    } else if lower.contains("save") || lower.contains("world") {
        ErrorCategory::Save
    } else if lower.contains("protocol") || lower.contains("version") {
        ErrorCategory::Protocol
    } else {
        ErrorCategory::Network
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::workos::LoginHandle;

    /// A minimal app running just `screen_viewed_system`, so a test can assert
    /// which screen it would record (mirrored in `LastTrackedScreen`) for a
    /// given auth state + menu screen. Analytics is disabled (fire-and-forget
    /// to a worker isn't observable in-process), so `LastTrackedScreen` is the
    /// assertable surface.
    fn screen_app(auth: AuthFlow, screen: Screen) -> App {
        let mut app = App::new();
        app.insert_resource(Analytics::disabled());
        app.insert_resource(auth);
        app.insert_resource(MenuState {
            screen,
            ..Default::default()
        });
        app.insert_resource(LastTrackedScreen::default());
        app.add_systems(Update, screen_viewed_system);
        app
    }

    fn tracked(app: &App) -> Option<ScreenKind> {
        app.world().resource::<LastTrackedScreen>().0
    }

    #[test]
    fn logged_out_records_the_sign_in_screen_not_the_menu_underneath() {
        let mut app = screen_app(AuthFlow::LoggedOut { error: None }, Screen::MainMenu);
        app.update();
        assert_eq!(tracked(&app), Some(ScreenKind::SignIn));
    }

    #[test]
    fn authenticated_records_the_menu_screen() {
        let mut app = screen_app(AuthFlow::Authenticated, Screen::Worlds);
        app.update();
        assert_eq!(tracked(&app), Some(ScreenKind::Worlds));
    }

    #[test]
    fn auth_spinner_records_nothing_so_silent_restore_skips_a_sign_in_view() {
        let (handle, tx) = LoginHandle::pending();
        let mut app = screen_app(AuthFlow::Verifying(handle), Screen::MainMenu);
        app.update();
        assert_eq!(tracked(&app), None);
        drop(tx);
    }

    #[test]
    fn classify_error_recognises_obvious_keywords() {
        assert_eq!(
            classify_error("auth rejected: bad ticket"),
            ErrorCategory::Auth
        );
        assert_eq!(
            classify_error("could not save world: disk full"),
            ErrorCategory::Save
        );
        assert_eq!(
            classify_error("protocol version mismatch"),
            ErrorCategory::Protocol
        );
        assert_eq!(
            classify_error("network error: connection reset"),
            ErrorCategory::Network
        );
    }
}
