//! Native WorkOS login for the desktop client (RFC 8252, "OAuth 2.0 for Native
//! Apps"). We never embed WorkOS: the user signs in in their real browser, and
//! the game just orchestrates the round-trip.
//!
//! Flow: generate a PKCE verifier/challenge + state → bind a one-shot loopback
//! HTTP listener on `127.0.0.1:<port>` → open the system browser to the WorkOS
//! authorize URL → the browser comes back to the loopback with `?code` → swap
//! the code for tokens at `/user_management/authenticate` (public client, no
//! secret — PKCE proves it's the same app). The short-lived access token rides
//! the game's `Auth` handshake and is verified server-side against the JWKS
//! (see [`crate::steam::WorkosVerifier`]); the refresh token is kept in the OS
//! keychain so we can silently re-auth on the next launch.
//!
//! A few fields (`Session::email`, `Session::expires_at`) are kept for upcoming
//! work (profile display, proactive token refresh) and aren't read yet.
#![allow(dead_code)]

use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, TcpListener, TcpStream},
    process::Command,
    sync::{Mutex, mpsc},
    thread,
    time::{Duration, Instant, SystemTime},
};

use base64::Engine;
use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::{protocol::SteamId, steam::account_id_from_sub};

/// WorkOS client id baked into the build (public — safe to ship). Override with
/// `GAME_WORKOS_CLIENT_ID` for a different environment. TODO: swap to the
/// production client id before release.
const DEFAULT_CLIENT_ID: &str = "client_01KSZSFDYP8ZVPE63P94ZWJ3WX";
/// Loopback port the browser is redirected back to. Must be registered as a
/// redirect URI in the WorkOS dashboard: `http://127.0.0.1:8765/callback`.
const DEFAULT_REDIRECT_PORT: u16 = 8765;
/// Where "Manage account" sends the player (override with `GAME_ACCOUNT_URL`).
const DEFAULT_ACCOUNT_URL: &str = "https://ashwend.com";

const AUTHORIZE_URL: &str = "https://api.workos.com/user_management/authorize";
const AUTHENTICATE_URL: &str = "https://api.workos.com/user_management/authenticate";

/// Keychain coordinates for the persisted refresh token.
const KEYRING_SERVICE: &str = "ashwend";
const KEYRING_ACCOUNT: &str = "workos-refresh-token";

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
}

/// Resolved WorkOS login configuration. Everything here is public.
#[derive(Debug, Clone)]
pub struct WorkosLoginConfig {
    pub client_id: String,
    pub redirect_port: u16,
}

impl WorkosLoginConfig {
    pub fn from_env() -> Self {
        let client_id =
            std::env::var("GAME_WORKOS_CLIENT_ID").unwrap_or_else(|_| DEFAULT_CLIENT_ID.to_owned());
        let redirect_port = std::env::var("GAME_WORKOS_REDIRECT_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(DEFAULT_REDIRECT_PORT);
        Self {
            client_id,
            redirect_port,
        }
    }

    fn redirect_uri(&self) -> String {
        format!("http://127.0.0.1:{}/callback", self.redirect_port)
    }

    fn authorize_url(&self, challenge: &str, state: &str, hint: ScreenHint) -> String {
        format!(
            "{AUTHORIZE_URL}?response_type=code&provider=authkit&client_id={client_id}\
             &redirect_uri={redirect_uri}&code_challenge={challenge}&code_challenge_method=S256\
             &state={state}&screen_hint={hint}",
            client_id = percent_encode(&self.client_id),
            redirect_uri = percent_encode(&self.redirect_uri()),
            challenge = percent_encode(challenge),
            state = percent_encode(state),
            hint = hint.as_str(),
        )
    }
}

/// A signed-in WorkOS session. `account_id` is the same stable id the server
/// derives from the token's `sub`, so the client and server agree on identity.
#[derive(Debug, Clone)]
pub struct Session {
    pub account_id: SteamId,
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
}

#[cfg(test)]
impl LoginHandle {
    /// Test-only handle that immediately resolves to `outcome`, so the auth
    /// state machine and UI can be driven without a real browser round-trip.
    pub(crate) fn ready(outcome: Result<Session, String>) -> Self {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(outcome);
        Self { rx: Mutex::new(rx) }
    }

    /// Test-only handle that stays `Pending`. The returned sender keeps the
    /// channel open; hold it for the duration of the test (drop it to make the
    /// handle report `Disconnected`).
    pub(crate) fn pending() -> (Self, mpsc::Sender<Result<Session, String>>) {
        let (tx, rx) = mpsc::channel();
        (Self { rx: Mutex::new(rx) }, tx)
    }
}

/// Start an interactive browser login. Non-blocking: opens the browser and
/// listens on the loopback from a worker thread, then reports via the handle.
pub fn begin_login(config: &WorkosLoginConfig, hint: ScreenHint) -> LoginHandle {
    let (tx, rx) = mpsc::channel();
    let config = config.clone();
    thread::Builder::new()
        .name("workos-login".to_owned())
        .spawn(move || {
            let _ = tx.send(run_login_flow(&config, hint));
        })
        .ok();
    LoginHandle { rx: Mutex::new(rx) }
}

/// Whether a refresh token is stored (a fast, local keychain read). Lets the
/// caller decide between the "verifying" spinner and the login splash without
/// blocking on a network refresh.
pub fn has_stored_session() -> bool {
    load_refresh_token().is_some()
}

/// Background variant of [`restore_session`] for the startup "verifying" state:
/// runs the refresh on a worker thread and reports via a [`LoginHandle`], same
/// as [`begin_login`].
pub fn begin_restore(config: &WorkosLoginConfig) -> LoginHandle {
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
    LoginHandle { rx: Mutex::new(rx) }
}

/// Silently restore a session at startup from the stored refresh token. Returns
/// `None` (and clears the stored token) if there's no token or the refresh
/// fails — the caller then shows the login splash.
pub fn restore_session(config: &WorkosLoginConfig) -> Option<Session> {
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
pub fn refresh(config: &WorkosLoginConfig, refresh_token: &str) -> Result<Session, String> {
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
/// is responsible for clearing the in-memory session and returning to the login
/// splash.
pub fn logout() {
    clear_refresh_token();
}

/// Open the account page in the system browser. WorkOS has no hosted end-user
/// profile page, so this points at our own site (override with
/// `GAME_ACCOUNT_URL`), where account management can grow over time.
pub fn open_account_page() {
    let url = std::env::var("GAME_ACCOUNT_URL").unwrap_or_else(|_| DEFAULT_ACCOUNT_URL.to_owned());
    let _ = open_url(&url);
}

// ─── Login worker ───────────────────────────────────────────────────────────

fn run_login_flow(config: &WorkosLoginConfig, hint: ScreenHint) -> Result<Session, String> {
    let verifier = random_token(64);
    let challenge = code_challenge(&verifier);
    let state = random_token(24);

    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, config.redirect_port))
        .map_err(|err| format!("could not start local sign-in listener: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("could not configure sign-in listener: {err}"))?;

    open_url(&config.authorize_url(&challenge, &state, hint))
        .map_err(|err| format!("could not open the browser: {err}"))?;

    let (code, returned_state) = accept_callback(&listener)?;
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

/// Poll-accept the single loopback callback within [`LOGIN_TIMEOUT`].
fn accept_callback(listener: &TcpListener) -> Result<(String, String), String> {
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    loop {
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
        (Some(_), _) => "You're signed in. Return to Ashwend — you can close this tab.",
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

// ─── WorkOS token endpoint ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct AuthResponse {
    access_token: String,
    refresh_token: String,
    user: WorkosUser,
}

#[derive(Debug, Deserialize)]
struct WorkosUser {
    id: String,
    #[serde(default)]
    email: String,
    #[serde(default)]
    first_name: Option<String>,
}

fn post_authenticate(body: serde_json::Value) -> Result<AuthResponse, String> {
    ureq::post(AUTHENTICATE_URL)
        .send_json(body)
        .map_err(describe_ureq_error)?
        .into_json::<AuthResponse>()
        .map_err(|err| format!("unexpected sign-in response: {err}"))
}

fn session_from(response: AuthResponse) -> Session {
    let display_name = response
        .user
        .first_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| response.user.email.clone());
    Session {
        account_id: account_id_from_sub(&response.user.id),
        display_name,
        email: response.user.email,
        expires_at: access_token_expiry(&response.access_token),
        access_token: response.access_token,
        refresh_token: response.refresh_token,
    }
}

/// Read `exp` out of the access-token JWT (no verification — the client only
/// needs to know when to refresh; the server does the real verification).
fn access_token_expiry(token: &str) -> Option<SystemTime> {
    #[derive(Deserialize)]
    struct Claims {
        exp: u64,
    }
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Claims = serde_json::from_slice(&bytes).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::from_secs(claims.exp))
}

fn describe_ureq_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let detail = response.into_string().unwrap_or_default();
            format!("sign-in rejected ({code}): {detail}")
        }
        ureq::Error::Transport(transport) => format!("sign-in network error: {transport}"),
    }
}

// ─── Keychain ───────────────────────────────────────────────────────────────

fn keyring_entry() -> Option<keyring::Entry> {
    keyring::Entry::new(KEYRING_SERVICE, KEYRING_ACCOUNT).ok()
}

fn store_refresh_token(token: &str) {
    if let Some(entry) = keyring_entry() {
        let _ = entry.set_password(token);
    }
}

fn load_refresh_token() -> Option<String> {
    keyring_entry()?.get_password().ok()
}

fn clear_refresh_token() {
    if let Some(entry) = keyring_entry() {
        let _ = entry.delete_credential();
    }
}

// ─── PKCE / encoding helpers ────────────────────────────────────────────────

/// Random base64url token of `bytes` bytes of OS entropy (via UUID v4), used
/// for the PKCE verifier and the CSRF `state`.
fn random_token(bytes: usize) -> String {
    let mut raw = Vec::with_capacity(bytes);
    while raw.len() < bytes {
        raw.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    raw.truncate(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw)
}

fn code_challenge(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            byte => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut command = Command::new("open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut command = Command::new("xdg-open");
        command.arg(url);
        command
    };
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut command = Command::new("cmd");
        command.args(["/C", "start", "", url]);
        command
    };
    command.spawn().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_challenge_is_url_safe_base64_of_sha256() {
        // Known RFC 7636 appendix-B vector.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            code_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn percent_round_trips() {
        let raw = "http://127.0.0.1:8765/callback?x=a b&y=z";
        assert_eq!(percent_decode(&percent_encode(raw)), raw);
    }

    #[test]
    fn random_token_has_expected_length_and_charset() {
        let token = random_token(64);
        // base64url-no-pad of 64 bytes is 86 chars, all URL-safe.
        assert_eq!(token.len(), 86);
        assert!(
            token
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        );
    }

    #[test]
    fn authorize_url_carries_pkce_and_redirect() {
        let config = WorkosLoginConfig {
            client_id: "client_test".to_owned(),
            redirect_port: 8765,
        };
        let url = config.authorize_url("chal", "st8", ScreenHint::SignUp);
        assert!(url.contains("code_challenge=chal"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("screen_hint=sign-up"));
        assert!(url.contains("client_id=client_test"));
        assert!(url.contains(&percent_encode("http://127.0.0.1:8765/callback")));
    }

    #[test]
    fn screen_hint_maps_to_authkit_slugs() {
        assert_eq!(ScreenHint::SignIn.as_str(), "sign-in");
        assert_eq!(ScreenHint::SignUp.as_str(), "sign-up");
    }

    #[test]
    fn config_redirect_uri_is_loopback_callback() {
        let config = WorkosLoginConfig {
            client_id: "client_x".to_owned(),
            redirect_port: 9000,
        };
        assert_eq!(config.redirect_uri(), "http://127.0.0.1:9000/callback");
    }

    #[test]
    fn percent_decode_handles_plus_and_malformed_escapes() {
        // `+` decodes to a space (form encoding).
        assert_eq!(percent_decode("a+b"), "a b");
        // A valid escape decodes; a malformed one is passed through verbatim.
        assert_eq!(percent_decode("%41"), "A");
        assert_eq!(percent_decode("%ZZ"), "%ZZ");
        // A trailing, truncated escape can't be decoded and is left as-is.
        assert_eq!(percent_decode("x%4"), "x%4");
    }

    #[test]
    fn session_from_prefers_first_name_then_falls_back_to_email() {
        let with_name = session_from(AuthResponse {
            access_token: "h.e.s".to_owned(),
            refresh_token: "refresh".to_owned(),
            user: WorkosUser {
                id: "user_01ABC".to_owned(),
                email: "player@example.com".to_owned(),
                first_name: Some("  Ada  ".to_owned()),
            },
        });
        // The display name is the trimmed first name when present.
        assert_eq!(with_name.display_name, "Ada");
        assert_eq!(with_name.email, "player@example.com");
        assert_eq!(with_name.refresh_token, "refresh");
        assert_eq!(with_name.account_id, account_id_from_sub("user_01ABC"));

        // A blank/whitespace first name falls back to the email address.
        let no_name = session_from(AuthResponse {
            access_token: "h.e.s".to_owned(),
            refresh_token: "r".to_owned(),
            user: WorkosUser {
                id: "user_01XYZ".to_owned(),
                email: "fallback@example.com".to_owned(),
                first_name: Some("   ".to_owned()),
            },
        });
        assert_eq!(no_name.display_name, "fallback@example.com");
    }

    #[test]
    fn access_token_expiry_reads_exp_claim_else_none() {
        // Craft a JWT-shaped token whose payload carries an `exp` claim.
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(br#"{"exp":1700000000,"sub":"user_1"}"#);
        let token = format!("header.{payload}.signature");
        assert_eq!(
            access_token_expiry(&token),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000))
        );

        // Not a JWT (no second segment) -> None.
        assert_eq!(access_token_expiry("not-a-jwt"), None);
        // Second segment isn't valid base64 -> None.
        assert_eq!(access_token_expiry("h.@@@.s"), None);
        // Valid base64 but no `exp` field -> None.
        let no_exp = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(br#"{"sub":"x"}"#);
        assert_eq!(access_token_expiry(&format!("h.{no_exp}.s")), None);
    }

    #[test]
    fn login_handle_reports_outcomes() {
        // Success.
        let ok = LoginHandle::ready(Ok(Session {
            account_id: 5,
            display_name: "n".to_owned(),
            email: "e".to_owned(),
            access_token: "a".to_owned(),
            refresh_token: "r".to_owned(),
            expires_at: None,
        }));
        match ok.poll() {
            LoginOutcome::Success(session) => assert_eq!(session.account_id, 5),
            other => panic!("expected success, got {other:?}"),
        }

        // Failure.
        let failed = LoginHandle::ready(Err("nope".to_owned()));
        assert!(matches!(failed.poll(), LoginOutcome::Failed(msg) if msg == "nope"));

        // Pending while the sender is alive.
        let (pending, tx) = LoginHandle::pending();
        assert!(matches!(pending.poll(), LoginOutcome::Pending));
        // Dropping the sender disconnects the channel -> reported as failure.
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
            // Read the browser response so the server-side write doesn't race
            // a premature close.
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
