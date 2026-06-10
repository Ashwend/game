//! Connection engine for the single official multiplayer server.
//!
//! Joining runs on a background thread so the resolve + handshake never
//! block the UI. The screen module ([`super`]) owns the prompt; this module
//! only knows how to start an attempt, poll it, and apply the result.

use std::{
    net::SocketAddr,
    sync::mpsc::{self, TryRecvError},
    thread,
};

use anyhow::Result;
use bevy_egui::egui;

use crate::{
    analytics::{Analytics, ConnectFailReason, Event},
    app::state::{
        ClientRuntime, CurrentUser, DirectConnectAttempt, DirectConnectDialog, DirectConnectResult,
        MenuState,
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
) -> std::result::Result<(), String> {
    let target = direct_connect_target(dialog).map_err(|error| error.to_string())?;

    let (tx, receiver) = mpsc::channel::<DirectConnectResult>();
    let user = user.0.clone();
    let network = network.clone();
    thread::Builder::new()
        .name("server-join-attempt".to_owned())
        .spawn(move || {
            let result =
                connect_to_target(target, user, network).map_err(|error| format!("{error:#}"));
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
    user: crate::auth::AuthenticatedUser,
    network: ClientNetwork,
) -> Result<(SocketAddr, ClientSession)> {
    let addr = resolve_direct_connect_target(&target)?;
    let session = ClientSession::connect(addr, &user, network)?;
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
            Some(Err("Connection attempt state is unavailable.".to_owned()))
        }
        AttemptPoll::Result(Ok(result)) => {
            dialog.attempt = None;
            Some(result)
        }
        AttemptPoll::Result(Err(TryRecvError::Empty)) => None,
        AttemptPoll::Result(Err(TryRecvError::Disconnected)) => {
            dialog.attempt = None;
            Some(Err(
                "Connection attempt ended before returning a result.".to_owned()
            ))
        }
    }
}

pub(super) fn finish(
    menu: &mut MenuState,
    runtime: &mut ClientRuntime,
    result: DirectConnectResult,
    analytics: &Analytics,
) {
    match result {
        Ok((addr, session)) => {
            analytics.track(Event::ConnectSucceeded);
            runtime.start_session(session, None);
            menu.multiplayer_addr = addr.to_string();
            menu.direct_connect = None;
            menu.enter_in_game();
        }
        Err(error) => {
            let reason = classify_connect_error(&error);
            analytics.track(Event::ConnectFailed { reason });
            let message = friendly_connect_error(reason, &error);
            if let Some(dialog) = menu.direct_connect.as_mut() {
                dialog.error = Some(message);
            } else {
                // Dialog already closed (e.g. quick-join from the screen
                // button): a footer status line is too easy to miss for a
                // failed join, raise the acknowledged notice instead.
                menu.notice = Some(crate::app::state::NoticeDialog::error(
                    "Couldn't join server",
                    message,
                ));
            }
            menu.loading_splash = None;
        }
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
