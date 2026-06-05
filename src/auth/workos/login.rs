//! Native WorkOS login for the desktop client (RFC 8252, "OAuth 2.0 for Native
//! Apps"). We never embed WorkOS: the user signs in in their real browser, and
//! the game just orchestrates the round-trip.
//!
//! Flow: generate a PKCE verifier/challenge + state -> bind a one-shot loopback
//! HTTP listener on `127.0.0.1:<port>` -> open the system browser to the WorkOS
//! authorize URL -> the browser comes back to the loopback with `?code` -> swap
//! the code for tokens (public client, no secret, PKCE proves it's the same
//! app). The short-lived access token rides the game's `Auth` handshake and is
//! verified server-side against the JWKS (see [`crate::auth::WorkosVerifier`]);
//! the refresh token is kept in a sealed local file (see [`super::token_store`])
//! so we can silently re-auth on the next launch.
//!
//! A few `Session` fields (`email`, `expires_at`) are kept for upcoming work
//! (profile display, proactive token refresh) and aren't read yet.
#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant, SystemTime},
};

use crate::protocol::AccountId;

use super::{
    config::{AUTHORIZE_URL, WorkosConfig},
    pkce::{code_challenge, percent_decode, percent_encode, random_token},
    token_store::{clear_refresh_token, load_refresh_token, store_refresh_token},
    tokens::{post_authenticate, session_from},
};

/// How long the loopback listener waits for the browser to come back before
/// giving up, so a cancelled/abandoned login doesn't leak a thread forever.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Which AuthKit screen to land the browser on.
#[derive(Debug, Clone, Copy)]
pub enum ScreenHint {
    SignIn,
    SignUp,
}

impl ScreenHint {
    fn as_str(self) -> &'static str {
        match self {
            ScreenHint::SignIn => "sign-in",
            ScreenHint::SignUp => "sign-up",
        }
    }

    /// OIDC `prompt` value, if any, to send with the authorize request. An
    /// explicit sign-in always forces a fresh credential entry (`prompt=login`)
    /// so that signing out and back in, or switching accounts, can't silently
    /// reuse a lingering WorkOS browser session. Sign-up sends nothing so the
    /// browser lands on the registration screen untouched.
    fn prompt(self) -> Option<&'static str> {
        match self {
            ScreenHint::SignIn => Some("login"),
            ScreenHint::SignUp => None,
        }
    }
}

/// The WorkOS authorize URL carrying the PKCE challenge, CSRF state, and screen
/// hint. The browser is opened to this; WorkOS redirects back to the loopback.
fn authorize_url(config: &WorkosConfig, challenge: &str, state: &str, hint: ScreenHint) -> String {
    let mut url = format!(
        "{AUTHORIZE_URL}?response_type=code&provider=authkit&client_id={client_id}\
         &redirect_uri={redirect_uri}&code_challenge={challenge}&code_challenge_method=S256\
         &state={state}&screen_hint={hint}",
        client_id = percent_encode(&config.client_id),
        redirect_uri = percent_encode(&config.redirect_uri()),
        challenge = percent_encode(challenge),
        state = percent_encode(state),
        hint = hint.as_str(),
    );
    // Force reauthentication on an explicit sign-in (see `ScreenHint::prompt`).
    // The value is a fixed slug, so no percent-encoding is needed.
    if let Some(prompt) = hint.prompt() {
        url.push_str("&prompt=");
        url.push_str(prompt);
    }
    url
}

/// A signed-in WorkOS session. `account_id` is the same stable id the server
/// derives from the token's `sub`, so the client and server agree on identity.
#[derive(Debug, Clone)]
pub struct Session {
    pub account_id: AccountId,
    pub display_name: String,
    pub email: String,
    pub access_token: String,
    pub refresh_token: String,
    /// When the access token expires; refresh before this.
    pub expires_at: Option<SystemTime>,
}

/// Polled by the UI while a login is in flight.
#[derive(Debug)]
pub enum LoginOutcome {
    Pending,
    Success(Box<Session>),
    Failed(String),
}

/// Handle to an in-flight browser login. The work happens on a background
/// thread; the UI polls [`LoginHandle::poll`] each frame.
pub struct LoginHandle {
    // `Mutex` so the handle is `Sync` (an `mpsc::Receiver` is `Send` but not
    // `Sync`) and can live inside the Bevy `AuthFlow` resource.
    rx: Mutex<mpsc::Receiver<Result<Session, String>>>,
    // Flipped by [`LoginHandle::cancel`] when the player bails out of the
    // browser wait; the interactive login worker watches it and stops polling
    // the loopback listener so the bound port is released for a later attempt.
    cancel: Arc<AtomicBool>,
}

impl LoginHandle {
    pub fn poll(&self) -> LoginOutcome {
        let Ok(rx) = self.rx.lock() else {
            return LoginOutcome::Failed("sign-in state was lost".to_owned());
        };
        match rx.try_recv() {
            Ok(Ok(session)) => LoginOutcome::Success(Box::new(session)),
            Ok(Err(error)) => LoginOutcome::Failed(error),
            Err(mpsc::TryRecvError::Empty) => LoginOutcome::Pending,
            Err(mpsc::TryRecvError::Disconnected) => {
                LoginOutcome::Failed("sign-in was interrupted".to_owned())
            }
        }
    }

    /// Tell the background worker to stop waiting on the browser callback. The
    /// interactive-login worker checks this each poll and returns promptly,
    /// dropping its loopback listener so the next sign-in can re-bind the port.
    /// A no-op for workers that don't watch the flag (the silent startup
    /// restore is a single blocking refresh, not a listener poll).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl LoginHandle {
    /// Test-only handle that immediately resolves to `outcome`, so the auth
    /// state machine and UI can be driven without a real browser round-trip.
    pub(crate) fn ready(outcome: Result<Session, String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(outcome);
        Self {
            rx: Mutex::new(rx),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Test-only handle that stays `Pending`. The returned sender keeps the
    /// channel open; hold it for the duration of the test (drop it to make the
    /// handle report `Disconnected`).
    pub(crate) fn pending() -> (Self, mpsc::Sender<Result<Session, String>>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                rx: Mutex::new(rx),
                cancel: Arc::new(AtomicBool::new(false)),
            },
            tx,
        )
    }
}

/// Start an interactive browser login. Non-blocking: opens the browser and
/// listens on the loopback from a worker thread, then reports via the handle.
pub fn begin_login(config: &WorkosConfig, hint: ScreenHint) -> LoginHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let config = config.clone();
    thread::Builder::new()
        .name("workos-login".to_owned())
        .spawn(move || {
            let _ = tx.send(run_login_flow(&config, hint, &worker_cancel));
        })
        .ok();
    LoginHandle {
        rx: Mutex::new(rx),
        cancel,
    }
}

/// Whether a refresh token is stored (a fast, local file read). Lets the
/// caller decide between the "verifying" spinner and the login splash without
/// blocking on a network refresh.
pub fn has_stored_session() -> bool {
    load_refresh_token().is_some()
}

/// Background variant of [`restore_session`] for the startup "verifying" state:
/// runs the refresh on a worker thread and reports via a [`LoginHandle`].
pub fn begin_restore(config: &WorkosConfig) -> LoginHandle {
    let (tx, rx) = mpsc::channel();
    let config = config.clone();
    thread::Builder::new()
        .name("workos-restore".to_owned())
        .spawn(move || {
            let result =
                restore_session(&config).ok_or_else(|| "your session has expired".to_owned());
            let _ = tx.send(result);
        })
        .ok();
    LoginHandle {
        rx: Mutex::new(rx),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}

/// Silently restore a session at startup from the stored refresh token. Returns
/// `None` (and clears the stored token) if there's no token or the refresh
/// fails, the caller then shows the login splash.
pub fn restore_session(config: &WorkosConfig) -> Option<Session> {
    let refresh_token = load_refresh_token()?;
    match refresh(config, &refresh_token) {
        Ok(session) => Some(session),
        Err(_) => {
            clear_refresh_token();
            None
        }
    }
}

/// Refresh an access token that's expired or about to. Rotates and re-stores
/// the refresh token.
pub fn refresh(config: &WorkosConfig, refresh_token: &str) -> Result<Session, String> {
    let response = post_authenticate(serde_json::json!({
        "client_id": config.client_id,
        "grant_type": "refresh_token",
        "refresh_token": refresh_token,
    }))?;
    let session = session_from(response);
    store_refresh_token(&session.refresh_token);
    Ok(session)
}

/// Drop the persisted session so the next launch starts logged out. The caller
/// clears the in-memory session and returns to the login splash.
pub fn logout() {
    clear_refresh_token();
}

fn run_login_flow(
    config: &WorkosConfig,
    hint: ScreenHint,
    cancel: &AtomicBool,
) -> Result<Session, String> {
    let verifier = random_token(64);
    let challenge = code_challenge(&verifier);
    let state = random_token(24);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.redirect_port))
        .map_err(|err| format!("could not start local sign-in listener: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("could not configure sign-in listener: {err}"))?;

    super::open_url(&authorize_url(config, &challenge, &state, hint))
        .map_err(|err| format!("could not open the browser: {err}"))?;

    let (code, returned_state) = accept_callback(&listener, cancel)?;
    if returned_state != state {
        return Err("sign-in could not be verified (state mismatch)".to_owned());
    }

    let response = post_authenticate(serde_json::json!({
        "client_id": config.client_id,
        "grant_type": "authorization_code",
        "code": code,
        "code_verifier": verifier,
    }))?;
    let session = session_from(response);
    store_refresh_token(&session.refresh_token);
    Ok(session)
}

/// Poll-accept the single loopback callback within [`LOGIN_TIMEOUT`], bailing
/// early if `cancel` is raised (the player escaped the browser wait).
fn accept_callback(
    listener: &TcpListener,
    cancel: &AtomicBool,
) -> Result<(String, String), String> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("sign-in cancelled".to_owned());
        }
        match listener.accept() {
            Ok((stream, _)) => return handle_callback(stream),
            Err(ref err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for sign-in".to_owned());
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err(err) => return Err(format!("sign-in listener failed: {err}")),
        }
    }
}

fn handle_callback(mut stream: TcpStream) -> Result<(String, String), String> {
    let request_line = {
        let mut reader = BufReader::new(&stream);
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|err| format!("could not read sign-in callback: {err}"))?;
        line
    };

    let query = request_line
        .split_whitespace()
        .nth(1)
        .and_then(|path| path.split_once('?'))
        .map(|(_, query)| query.to_owned())
        .unwrap_or_default();

    let mut code = None;
    let mut state = None;
    let mut error = None;
    for pair in query.split('&') {
        let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
        match key {
            "code" => code = Some(percent_decode(value)),
            "state" => state = Some(percent_decode(value)),
            "error_description" | "error" => error = Some(percent_decode(value)),
            _ => {}
        }
    }

    let outcome = match (&code, &error) {
        (Some(_), _) => "You're signed in. Return to Ashwend and close this tab.",
        _ => "Sign-in failed. Return to Ashwend and try again.",
    };
    write_browser_response(&mut stream, outcome);

    if let Some(error) = error {
        return Err(error);
    }
    match (code, state) {
        (Some(code), Some(state)) => Ok((code, state)),
        _ => Err("sign-in callback was missing its code".to_owned()),
    }
}

fn write_browser_response(stream: &mut TcpStream, message: &str) {
    let body = format!(
        "<!doctype html><html><head><meta charset=utf-8><title>Ashwend</title></head>\
         <body style=\"font-family:system-ui,sans-serif;background:#0a0e13;color:#e6ebf2;\
         text-align:center;padding-top:64px\"><h2 style=\"font-weight:600\">Ashwend</h2>\
         <p style=\"color:#97a4b2\">{message}</p></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_hint_maps_to_authkit_slugs() {
        assert_eq!(ScreenHint::SignIn.as_str(), "sign-in");
        assert_eq!(ScreenHint::SignUp.as_str(), "sign-up");
    }

    fn test_config() -> WorkosConfig {
        WorkosConfig {
            client_id: "client_test".to_owned(),
            redirect_port: 8765,
        }
    }

    #[test]
    fn authorize_url_carries_pkce_and_redirect() {
        let url = authorize_url(&test_config(), "chal", "st8", ScreenHint::SignUp);
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("screen_hint=sign-up"));
        assert!(url.contains("client_id=client_test"));
        assert!(url.contains(&percent_encode("http://127.0.0.1:8765/callback")));
        // Sign-up should land on the registration screen, not be forced to log in.
        assert!(!url.contains("prompt="));
    }

    #[test]
    fn sign_in_forces_reauthentication() {
        let url = authorize_url(&test_config(), "chal", "st8", ScreenHint::SignIn);
        assert!(url.contains("screen_hint=sign-in"));
        // `prompt=login` makes WorkOS re-ask for credentials even if the browser
        // still holds a session, so sign-out → sign-in can't silently SSO back in.
        assert!(url.contains("prompt=login"));
    }

    fn sample_session() -> Session {
        Session {
            account_id: 5,
            display_name: "n".to_owned(),
            email: "e".to_owned(),
            access_token: "a".to_owned(),
            refresh_token: "r".to_owned(),
            expires_at: None,
        }
    }

    #[test]
    fn login_handle_reports_outcomes() {
        let ok = LoginHandle::ready(Ok(sample_session()));
        match ok.poll() {
            LoginOutcome::Success(session) => assert_eq!(session.account_id, 5),
            other => panic!("expected success, got {other:?}"),
        }

        let failed = LoginHandle::ready(Err("nope".to_owned()));
        assert!(matches!(failed.poll(), LoginOutcome::Failed(msg) if msg == "nope"));

        let (pending, tx) = LoginHandle::pending();
        assert!(matches!(pending.poll(), LoginOutcome::Pending));
        drop(tx);
        assert!(matches!(pending.poll(), LoginOutcome::Failed(_)));
    }

    #[test]
    fn handle_callback_extracts_code_and_state() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");

        let writer = thread::spawn(move || {
            let mut client = TcpStream::connect(addr).expect("connect loopback");
            client
                .write_all(b"GET /callback?code=abc123&state=xyz789 HTTP/1.1\r\nHost: x\r\n\r\n")
                .expect("write request");
            let mut sink = Vec::new();
            use std::io::Read;
            let _ = client.read_to_end(&mut sink);
        });

        let (server, _) = listener.accept().expect("accept callback");
        let (code, state) = handle_callback(server).expect("callback parsed");
        assert_eq!(code, "abc123");
        assert_eq!(state, "xyz789");
        writer.join().expect("writer thread");
    }

    #[test]
    fn handle_callback_surfaces_provider_error() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind loopback");
        let addr = listener.local_addr().expect("local addr");

        let writer = thread::spawn(move || {
            let mut client = TcpStream::connect(addr).expect("connect loopback");
            client
                .write_all(
                    b"GET /callback?error=access_denied&error_description=user+said+no HTTP/1.1\r\n\r\n",
                )
                .expect("write request");
            let mut sink = Vec::new();
            use std::io::Read;
            let _ = client.read_to_end(&mut sink);
        });

        let (server, _) = listener.accept().expect("accept callback");
        let err = handle_callback(server).expect_err("an error callback should fail");
        assert_eq!(err, "user said no");
        writer.join().expect("writer thread");
    }
}
