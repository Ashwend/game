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
    tokens::{
        AUTH_RENEW_ATTEMPTS, AUTH_RETRY_ATTEMPTS, access_token_expiry,
        post_authenticate_with_retry, session_from,
    },
};

/// How long the loopback listener waits for the browser to come back before
/// giving up, so a cancelled/abandoned login doesn't leak a thread forever.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// Refresh the access token when it has less than this long left before it
/// expires. The server allows 30s of clock-skew leeway when it verifies the
/// `exp` claim (see [`crate::auth::WorkosVerifier`]); this sits comfortably
/// above that so a token that passes the client check still validates after the
/// connection handshake and any modest clock drift between client and server.
const REFRESH_LEEWAY: Duration = Duration::from_secs(90);

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

/// A failed login/restore outcome, split by whether trying again might help.
#[derive(Debug, Clone)]
pub struct LoginError {
    /// Player-presentable message; also what the auth poller logs.
    pub message: String,
    /// `true` when the failure was transport-shaped (network trouble or a
    /// provider outage) and the in-worker retry budget is already exhausted:
    /// the credentials were never REJECTED, so the player choosing to try
    /// again may well succeed. `false` for definitive provider rejections and
    /// local/browser-side failures (cancelled, timed out waiting, listener
    /// bind), which drop back to the ordinary login splash instead of the
    /// outage dialog.
    pub transient: bool,
}

impl LoginError {
    /// A local (non-provider) failure: never worth the outage dialog.
    fn local(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            transient: false,
        }
    }

    /// Classify a token-endpoint failure: transport-shaped errors are
    /// transient (the grant may be fine, the provider was unreachable),
    /// rejections are not.
    fn from_call(error: super::tokens::AuthCallError) -> Self {
        Self {
            transient: !error.is_rejected(),
            message: error.into_message(),
        }
    }
}

/// Polled by the UI while a login is in flight.
#[derive(Debug)]
pub enum LoginOutcome {
    Pending,
    Success(Box<Session>),
    Failed(LoginError),
}

/// Handle to an in-flight browser login. The work happens on a background
/// thread; the UI polls [`LoginHandle::poll`] each frame.
pub struct LoginHandle {
    // `Mutex` so the handle is `Sync` (an `mpsc::Receiver` is `Send` but not
    // `Sync`) and can live inside the Bevy `AuthFlow` resource.
    rx: Mutex<mpsc::Receiver<Result<Session, LoginError>>>,
    // Flipped by [`LoginHandle::cancel`] when the player bails out of the
    // browser wait; the workers watch it to stop polling the loopback listener
    // (so the bound port is released for a later attempt) and to cut a token
    // retry backoff short.
    cancel: Arc<AtomicBool>,
    // When the work began, for the poller's lifecycle logging (see
    // [`Self::started`]).
    started: Instant,
}

impl LoginHandle {
    /// When this handle's work began. The POLLER owns lifecycle logging (in
    /// flight / resolved, with durations off this instant): the silent restore
    /// starts before the Bevy app builds, so anything the worker thread logged
    /// would race the tracing subscriber's installation and be dropped.
    pub fn started(&self) -> Instant {
        self.started
    }

    pub fn poll(&self) -> LoginOutcome {
        let Ok(rx) = self.rx.lock() else {
            return LoginOutcome::Failed(LoginError::local("sign-in state was lost"));
        };
        match rx.try_recv() {
            Ok(Ok(session)) => LoginOutcome::Success(Box::new(session)),
            Ok(Err(error)) => LoginOutcome::Failed(error),
            Err(mpsc::TryRecvError::Empty) => LoginOutcome::Pending,
            Err(mpsc::TryRecvError::Disconnected) => {
                LoginOutcome::Failed(LoginError::local("sign-in was interrupted"))
            }
        }
    }

    /// Tell the background worker to stop. The interactive-login worker checks
    /// this each loopback poll and returns promptly, dropping its listener so
    /// the next sign-in can re-bind the port; both workers also check it
    /// between token-endpoint retries so a Cancel on the spinner cuts the
    /// outage backoff short.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
impl LoginHandle {
    /// Test-only handle that immediately resolves to `outcome`, so the auth
    /// state machine and UI can be driven without a real browser round-trip.
    pub(crate) fn ready(outcome: Result<Session, LoginError>) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(outcome);
        Self {
            rx: Mutex::new(rx),
            cancel: Arc::new(AtomicBool::new(false)),
            started: Instant::now(),
        }
    }

    /// Test-only handle that stays `Pending`. The returned sender keeps the
    /// channel open; hold it for the duration of the test (drop it to make the
    /// handle report `Disconnected`).
    pub(crate) fn pending() -> (Self, mpsc::Sender<Result<Session, LoginError>>) {
        let (tx, rx) = mpsc::channel();
        (
            Self {
                rx: Mutex::new(rx),
                cancel: Arc::new(AtomicBool::new(false)),
                started: Instant::now(),
            },
            tx,
        )
    }
}

#[cfg(test)]
impl LoginError {
    /// Test-only shorthands so state-machine tests read as intent.
    pub(crate) fn test_local(message: &str) -> Self {
        Self::local(message)
    }

    pub(crate) fn test_transient(message: &str) -> Self {
        Self {
            message: message.to_owned(),
            transient: true,
        }
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
        started: Instant::now(),
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
///
/// No logging here on purpose: this starts before the Bevy app (and thus the
/// tracing subscriber) exists, so worker-side log calls would be silently
/// dropped. The poller (`drive_auth_flow_system`) logs the lifecycle instead,
/// timed off [`LoginHandle::started`]. The error string travels to the poller
/// for that log; the UI never shows it for a silent restore.
pub fn begin_restore(config: &WorkosConfig) -> LoginHandle {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    let config = config.clone();
    thread::Builder::new()
        .name("workos-restore".to_owned())
        .spawn(move || {
            let _ = tx.send(restore_session(&config, Some(&worker_cancel)));
        })
        .ok();
    LoginHandle {
        rx: Mutex::new(rx),
        cancel,
        started: Instant::now(),
    }
}

/// Silently restore a session at startup from the stored refresh token, riding
/// the provider-outage retry policy (transport failures retry with backoff up
/// to [`AUTH_RETRY_ATTEMPTS`]; `cancel` cuts the backoff short).
///
/// A definitive provider rejection clears the stored token: it is dead, and the
/// next launch should go straight to the login splash. A transport failure
/// (offline boot, sleepy Wi-Fi, provider outage) keeps it AND comes back marked
/// `transient`, so the player is not signed out over a network blip and the
/// auth flow can offer a retry instead of silently appearing logged out.
pub fn restore_session(
    config: &WorkosConfig,
    cancel: Option<&AtomicBool>,
) -> Result<Session, LoginError> {
    let Some(refresh_token) = load_refresh_token() else {
        return Err(LoginError::local("no stored session"));
    };
    match refresh_grant(config, &refresh_token, AUTH_RETRY_ATTEMPTS, cancel) {
        Ok(session) => Ok(session),
        Err(error) => {
            if error.is_rejected() {
                clear_refresh_token();
            }
            Err(LoginError::from_call(error))
        }
    }
}

/// Refresh an access token that's expired or about to. Rotates and re-stores
/// the refresh token. Runs the LIGHT retry budget ([`AUTH_RENEW_ATTEMPTS`]):
/// the pre-connect caller has its own inline retry UX on the join prompt.
pub fn refresh(config: &WorkosConfig, refresh_token: &str) -> Result<Session, String> {
    refresh_grant(config, refresh_token, AUTH_RENEW_ATTEMPTS, None)
        .map_err(super::tokens::AuthCallError::into_message)
}

/// [`refresh`] with the rejected-vs-transport split preserved, for callers
/// (the silent restore) that must react differently to the two.
fn refresh_grant(
    config: &WorkosConfig,
    refresh_token: &str,
    attempts: u32,
    cancel: Option<&AtomicBool>,
) -> Result<Session, super::tokens::AuthCallError> {
    let response = post_authenticate_with_retry(
        serde_json::json!({
            "client_id": config.client_id,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        }),
        attempts,
        cancel,
    )?;
    let session = session_from(response);
    store_refresh_token(&session.refresh_token);
    Ok(session)
}

/// Outcome of [`ensure_fresh_token`]: a pre-flight check the client runs before
/// presenting its access token to a WorkOS-auth server.
pub enum TokenFreshness {
    /// The current token is valid for long enough; use it as-is.
    Fresh,
    /// The token was expired/near-expiry and has been renewed. The caller should
    /// connect with [`Session::access_token`] and may reinstall the session.
    Refreshed(Box<Session>),
    /// The token needs renewing but no refresh token is stored, so there's
    /// nothing to renew from. The user must sign in again.
    SignInRequired,
    /// A refresh token existed but renewing it failed (network/provider error).
    /// The caller can let the user retry. Carries the underlying error.
    RenewFailed(String),
}

/// Make sure the in-memory access token is good before connecting to a
/// WorkOS-auth server. Decodes the token's own `exp` (no verification, that's
/// the server's job) and, if it's expired or inside [`REFRESH_LEEWAY`], renews
/// it from the stored refresh token. This is what stops a token that quietly
/// expired during a long or backgrounded session (e.g. a detour into
/// singleplayer) from being rejected at the handshake with a confusing error.
pub fn ensure_fresh_token(config: &WorkosConfig, access_token: &str) -> TokenFreshness {
    if let Some(expires_at) = access_token_expiry(access_token)
        && expires_at > SystemTime::now() + REFRESH_LEEWAY
    {
        return TokenFreshness::Fresh;
    }
    // Expired, inside the refresh window, or unparseable: renew it. A token we
    // can't decode is treated as needing renewal rather than trusted blindly.
    let Some(refresh_token) = load_refresh_token() else {
        return TokenFreshness::SignInRequired;
    };
    match refresh(config, &refresh_token) {
        Ok(session) => TokenFreshness::Refreshed(Box::new(session)),
        Err(error) => TokenFreshness::RenewFailed(error),
    }
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
) -> Result<Session, LoginError> {
    let verifier = random_token(64);
    let challenge = code_challenge(&verifier);
    let state = random_token(24);

    // Everything up to the token exchange is local/browser-side: those
    // failures are not provider outages, so they surface as ordinary login
    // errors, never the outage dialog.
    let listener =
        TcpListener::bind((Ipv4Addr::LOCALHOST, config.redirect_port)).map_err(|err| {
            LoginError::local(format!("could not start local sign-in listener: {err}"))
        })?;
    listener
        .set_nonblocking(true)
        .map_err(|err| LoginError::local(format!("could not configure sign-in listener: {err}")))?;

    super::open_url(&authorize_url(config, &challenge, &state, hint))
        .map_err(|err| LoginError::local(format!("could not open the browser: {err}")))?;

    let (code, returned_state) = accept_callback(&listener, cancel).map_err(LoginError::local)?;
    if returned_state != state {
        return Err(LoginError::local(
            "sign-in could not be verified (state mismatch)",
        ));
    }

    // The code exchange is a provider call: ride the outage retry policy. A
    // definitive rejection (used/expired code) stops immediately; transport
    // failures retry with backoff and come back marked transient so the player
    // gets the try-again dialog rather than a bare login splash.
    let response = post_authenticate_with_retry(
        serde_json::json!({
            "client_id": config.client_id,
            "grant_type": "authorization_code",
            "code": code,
            "code_verifier": verifier,
        }),
        AUTH_RETRY_ATTEMPTS,
        Some(cancel),
    )
    .map_err(LoginError::from_call)?;
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
            account_id: crate::protocol::AccountId(5),
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
            LoginOutcome::Success(session) => {
                assert_eq!(session.account_id, crate::protocol::AccountId(5));
            }
            other => panic!("expected success, got {other:?}"),
        }

        let failed = LoginHandle::ready(Err(LoginError::test_local("nope")));
        assert!(matches!(failed.poll(), LoginOutcome::Failed(error) if error.message == "nope"));

        let (pending, tx) = LoginHandle::pending();
        assert!(matches!(pending.poll(), LoginOutcome::Pending));
        drop(tx);
        // A dropped worker is a local failure, never the outage dialog.
        assert!(matches!(
            pending.poll(),
            LoginOutcome::Failed(error) if !error.transient
        ));
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
