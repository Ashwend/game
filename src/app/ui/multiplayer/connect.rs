//! Connection engine for the single official multiplayer server.
//!
//! Joining runs on a background thread so the resolve + handshake never
//! block the UI. The screen module ([`super`]) owns the prompt; this module
//! only knows how to start an attempt, poll it, and apply the result.

use std::{
    sync::mpsc::{self, TryRecvError},
    thread,
};

use bevy_egui::egui;

use crate::{
    analytics::{Analytics, ConnectFailReason, Event},
    app::state::{
        ClientRuntime, CurrentUser, DirectConnectAttempt, DirectConnectDialog, DirectConnectResult,
        JoinError, MenuState, NoticeDialog, Screen, WorkosAuth,
    },
    auth::{
        AuthenticatedUser,
        workos::{TokenFreshness, WorkosConfig, ensure_fresh_token},
    },
    net::{ClientNetwork, ClientSession},
};

use self::target::{DirectConnectTarget, direct_connect_target, resolve_direct_connect_target};

mod target;

pub(super) fn start_attempt(
    ctx: &egui::Context,
    dialog: &mut DirectConnectDialog,
    user: &CurrentUser,
    network: &ClientNetwork,
    workos: Option<&WorkosAuth>,
) -> std::result::Result<(), String> {
    let target = direct_connect_target(dialog).map_err(|error| error.to_string())?;

    let (tx, receiver) = mpsc::channel::<DirectConnectResult>();
    let user = user.0.clone();
    let network = network.clone();
    // The official server verifies a WorkOS token, so the worker renews it (if
    // needed) before the handshake. The bypass/test path has no WorkOS config
    // and talks to a NoAuth server, so it connects with the identity as-is.
    let workos = workos.map(|workos| workos.0.clone());
    thread::Builder::new()
        .name("server-join-attempt".to_owned())
        .spawn(move || {
            let result = connect_to_target(target, user, network, workos);
            let _ = tx.send(result);
        })
        .map_err(|error| format!("Could not start connection attempt: {error}"))?;

    dialog.error = None;
    dialog.attempt = Some(DirectConnectAttempt {
        receiver: std::sync::Mutex::new(receiver),
    });
    ctx.request_repaint();
    Ok(())
}

fn connect_to_target(
    target: DirectConnectTarget,
    mut user: AuthenticatedUser,
    network: ClientNetwork,
    workos: Option<WorkosConfig>,
) -> DirectConnectResult {
    // Pre-flight the access token: renew it if it has expired or is about to,
    // so a token that quietly lapsed during a long session (or a detour into
    // singleplayer) doesn't get rejected at the handshake with a confusing
    // "wrong version" error. NoAuth/bypass connections (no WorkOS config) skip
    // this and connect with whatever identity they were given.
    if let Some(config) = workos.as_ref() {
        match ensure_fresh_token(config, &user.token) {
            TokenFreshness::Fresh => {}
            // Identity is keyed off the token's `sub`, which a refresh
            // preserves, so only the token itself needs swapping in.
            TokenFreshness::Refreshed(session) => user.token = session.access_token,
            TokenFreshness::SignInRequired => return Err(JoinError::SignInRequired),
            TokenFreshness::RenewFailed(error) => return Err(JoinError::RenewFailed(error)),
        }
    }

    let addr = resolve_direct_connect_target(&target)
        .map_err(|error| JoinError::Connection(format!("{error:#}")))?;
    let session = ClientSession::connect(addr, &user, network)
        .map_err(|error| JoinError::Connection(format!("{error:#}")))?;
    Ok((addr, session))
}

pub(super) fn take_finished(dialog: &mut DirectConnectDialog) -> Option<DirectConnectResult> {
    enum AttemptPoll {
        Result(std::result::Result<DirectConnectResult, TryRecvError>),
        Poisoned,
    }

    let attempt = dialog.attempt.as_ref()?;
    let poll = match attempt.receiver.lock() {
        Ok(receiver) => AttemptPoll::Result(receiver.try_recv()),
        Err(_) => AttemptPoll::Poisoned,
    };

    match poll {
        AttemptPoll::Poisoned => {
            dialog.attempt = None;
            Some(Err(JoinError::Connection(
                "Connection attempt state is unavailable.".to_owned(),
            )))
        }
        AttemptPoll::Result(Ok(result)) => {
            dialog.attempt = None;
            Some(result)
        }
        AttemptPoll::Result(Err(TryRecvError::Empty)) => None,
        AttemptPoll::Result(Err(TryRecvError::Disconnected)) => {
            dialog.attempt = None;
            Some(Err(JoinError::Connection(
                "Connection attempt ended before returning a result.".to_owned(),
            )))
        }
    }
}

pub(super) fn finish(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    result: DirectConnectResult,
    analytics: &Analytics,
) {
    let error = match result {
        Ok((addr, session)) => {
            analytics.track(Event::ConnectSucceeded);
            runtime.start_session(session, None);
            menu.multiplayer_addr = addr.to_string();
            menu.direct_connect = None;
            menu.enter_in_game();
            return;
        }
        Err(error) => error,
    };

    // Every failure drops the "Joining server" splash; the specific kind decides
    // whether the player retries, gets a notice, or is signed out.
    menu.loading_splash = None;

    match error {
        JoinError::Connection(raw) => {
            let reason = classify_connect_error(&raw);
            analytics.track(Event::ConnectFailed { reason });
            show_join_error(menu, friendly_connect_error(reason, &raw));
        }
        JoinError::RenewFailed(raw) => {
            analytics.track(Event::ConnectFailed {
                reason: ConnectFailReason::AuthRejected,
            });
            // The refresh call itself failed (most likely offline). Keep the
            // join prompt up with the reason so the player can fix their
            // connection and click Join again.
            show_join_error(
                menu,
                format!(
                    "Couldn't renew your sign-in session. Check your connection and try again.\n\
                     If this keeps happening, sign out and back in.\nDetails: {raw}"
                ),
            );
        }
        JoinError::SignInRequired => {
            analytics.track(Event::ConnectFailed {
                reason: ConnectFailReason::AuthRejected,
            });
            // There's no stored refresh token to renew from, so the session is
            // unrecoverable. Drop the join overlay and sign the player out with
            // a reason (handled by `drive_auth_flow_system`) so the login splash
            // explains why instead of bouncing them silently.
            menu.direct_connect = None;
            menu.screen = Screen::MainMenu;
            menu.force_sign_out = Some(
                "Your sign-in session expired and couldn't be renewed. Please sign in again to \
                 play."
                    .to_owned(),
            );
        }
    }
}

/// Surface a failed-join message: inline on the join prompt when it's still
/// open (so the player can retry with one click), otherwise as an acknowledged
/// notice (a quick-join from the screen button leaves no dialog, and a footer
/// status line is too easy to miss for a failed join).
fn show_join_error(menu: &mut MenuState, message: String) {
    if let Some(dialog) = menu.direct_connect.as_mut() {
        dialog.error = Some(message);
    } else {
        menu.notice = Some(NoticeDialog::error("Couldn't join server", message));
    }
}

/// Player-facing copy for a failed connection attempt. The classification
/// buckets double as the analytics reason, so the message and the metric
/// can't drift apart. The raw error rides along on a second line: the
/// headline tells the player what to do, the detail gives a server admin
/// (or a bug report) something concrete.
fn friendly_connect_error(reason: ConnectFailReason, raw: &str) -> String {
    let headline = match reason {
        ConnectFailReason::Timeout => {
            "Server unreachable. Check the address and your internet connection, then try again."
        }
        ConnectFailReason::Refused => {
            "The server refused the connection. It may be offline or restarting."
        }
        ConnectFailReason::VersionMismatch => {
            "Your game version doesn't match the server. Update Ashwend and try again."
        }
        ConnectFailReason::AuthRejected => {
            "Your login couldn't be verified. Try signing out and back in."
        }
        ConnectFailReason::BadAddress => {
            "That address couldn't be found. Check the host name and port."
        }
        ConnectFailReason::Other => "Connection failed.",
    };
    if raw.trim().is_empty() {
        headline.to_owned()
    } else {
        format!("{headline}\nDetails: {raw}")
    }
}

fn classify_connect_error(error: &str) -> ConnectFailReason {
    let lower = error.to_ascii_lowercase();
    if lower.contains("timeout") || lower.contains("timed out") {
        ConnectFailReason::Timeout
    } else if lower.contains("refused") {
        ConnectFailReason::Refused
    } else if lower.contains("version") || lower.contains("protocol") {
        ConnectFailReason::VersionMismatch
    } else if lower.contains("auth") {
        ConnectFailReason::AuthRejected
    } else if lower.contains("address") || lower.contains("resolve") || lower.contains("dns") {
        ConnectFailReason::BadAddress
    } else {
        ConnectFailReason::Other
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::DirectConnectDialog;

    #[test]
    fn classify_connect_error_buckets_known_phrases_case_insensitively() {
        assert_eq!(
            classify_connect_error("Connection TIMED OUT"),
            ConnectFailReason::Timeout
        );
        assert_eq!(
            classify_connect_error("socket timeout"),
            ConnectFailReason::Timeout
        );
        assert_eq!(
            classify_connect_error("Connection refused"),
            ConnectFailReason::Refused
        );
        assert_eq!(
            classify_connect_error("protocol version mismatch"),
            ConnectFailReason::VersionMismatch
        );
        assert_eq!(
            classify_connect_error("auth token rejected"),
            ConnectFailReason::AuthRejected
        );
        assert_eq!(
            classify_connect_error("could not resolve address via dns"),
            ConnectFailReason::BadAddress
        );
    }

    #[test]
    fn classify_connect_error_falls_back_to_other() {
        assert_eq!(
            classify_connect_error("something unexpected happened"),
            ConnectFailReason::Other
        );
        assert_eq!(classify_connect_error(""), ConnectFailReason::Other);
    }

    #[test]
    fn classify_connect_error_prioritises_timeout_over_later_buckets() {
        // A message that matches multiple substrings resolves to the first
        // branch in declaration order (timeout wins over refused here).
        assert_eq!(
            classify_connect_error("timeout: connection refused"),
            ConnectFailReason::Timeout
        );
    }

    #[test]
    fn friendly_connect_error_pairs_actionable_copy_with_raw_detail() {
        let message = friendly_connect_error(ConnectFailReason::Timeout, "socket timed out");
        assert!(
            message.starts_with("Server unreachable."),
            "headline tells the player what to do: {message}"
        );
        assert!(
            message.contains("Details: socket timed out"),
            "raw error preserved for reports: {message}"
        );

        // Every classified bucket gets copy that is not the generic line.
        for reason in [
            ConnectFailReason::Refused,
            ConnectFailReason::VersionMismatch,
            ConnectFailReason::AuthRejected,
            ConnectFailReason::BadAddress,
        ] {
            let copy = friendly_connect_error(reason, "raw");
            assert!(
                !copy.starts_with("Connection failed."),
                "{reason:?} should have dedicated copy"
            );
        }

        // No raw detail still reads cleanly.
        let bare = friendly_connect_error(ConnectFailReason::Other, "  ");
        assert_eq!(bare, "Connection failed.");
    }

    #[test]
    fn take_finished_returns_none_without_an_attempt() {
        let mut dialog = DirectConnectDialog {
            host: "host".to_owned(),
            port: "7777".to_owned(),
            error: None,
            attempt: None,
        };
        assert!(take_finished(&mut dialog).is_none());
    }
}
