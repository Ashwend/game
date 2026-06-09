//! Crash + exception reporting to PostHog error tracking.
//!
//! PostHog's error tracking is fed by ordinary capture events named
//! `$exception` carrying a `$exception_list` property, so we don't need a
//! dedicated SDK (the Rust SDK has no exception capture anyway): we build the
//! event by hand and ship it through the same `/batch/` endpoint the analytics
//! worker uses.
//!
//! The panic hook ([`install_panic_hook`]) is always installed. It writes a
//! crash record straight to the log file via [`crate::logging::write_raw`] (so
//! a crash is on disk even when analytics is off) and, when analytics is
//! configured, ships a `$exception` event *synchronously* before the process
//! unwinds. The async worker can't be relied on to flush during a crash, so
//! this is a blocking POST with a short timeout. A handled-error path (a
//! non-fatal `$exception` with `handled = true`) can reuse
//! [`client::build_event_envelope`] the same way when we want it.

use std::{
    panic,
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{client, context::SharedProps};

/// How long the synchronous crash send is allowed to take. The user already
/// hit a crash; we will not hang their process for telemetry beyond this.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const SEND_TIMEOUT: Duration = Duration::from_secs(3);

/// Cap the raw backtrace we attach so a deep stack can't blow past PostHog's
/// per-property size limits.
const MAX_BACKTRACE_BYTES: usize = 8 * 1024;

/// Everything the crash sender needs, captured once analytics comes up enabled.
struct CrashConfig {
    api_key: String,
    host: String,
    distinct_id: Uuid,
    disable_geoip: bool,
    super_props: SharedProps,
}

static CONFIG: OnceLock<CrashConfig> = OnceLock::new();
static HOOK_INSTALLED: AtomicBool = AtomicBool::new(false);

/// Provide the PostHog credentials the crash sender uses. Called from the
/// analytics plugin only when analytics resolved to enabled. Idempotent; the
/// first call wins.
pub(crate) fn configure(
    api_key: String,
    host: String,
    distinct_id: Uuid,
    disable_geoip: bool,
    super_props: SharedProps,
) {
    let _ = CONFIG.set(CrashConfig {
        api_key,
        host,
        distinct_id,
        disable_geoip,
        super_props,
    });
}

/// Install our panic hook (chaining the existing one). Safe to call before
/// [`configure`]: a panic before analytics comes up is still written to the
/// log file, it just isn't sent over the network. Idempotent.
pub(crate) fn install_panic_hook() {
    if HOOK_INSTALLED.swap(true, Ordering::SeqCst) {
        return;
    }
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // The reporter must never turn one panic into two. Swallow anything
        // that goes wrong while recording, then fall through to the default
        // hook so the usual stderr message and abort/unwind still happen.
        let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| handle_panic(info)));
        previous(info);
    }));
}

fn handle_panic(info: &panic::PanicHookInfo<'_>) {
    let message = panic_message(info);
    let location = info.location().map(|loc| Location {
        file: loc.file().to_owned(),
        line: loc.line(),
    });
    let location_str = location
        .as_ref()
        .map(|loc| format!("{}:{}", loc.file, loc.line))
        .unwrap_or_else(|| "unknown".to_owned());
    let thread = std::thread::current()
        .name()
        .unwrap_or("unnamed")
        .to_owned();
    let backtrace = std::backtrace::Backtrace::force_capture().to_string();

    // Synchronous, unconditional: this is the bit that has to survive a crash
    // even with analytics off.
    crate::logging::write_raw(&format!(
        "==== PANIC ====\nversion: {}\nthread: {thread}\nlocation: {location_str}\nmessage: {message}\nbacktrace:\n{backtrace}\n===============",
        crate::protocol::GAME_VERSION
    ));
    bevy::log::error!("panic on thread '{thread}' at {location_str}: {message}");

    if let Some(cfg) = CONFIG.get() {
        let exception = exception_object("RustPanic", &message, location.as_ref());
        send_exception(cfg, exception, "RustPanic", &message, Some(&backtrace));
    }
}

struct Location {
    file: String,
    line: u32,
}

/// One entry of PostHog's `$exception_list`, the shape its error-tracking
/// ingestion expects. We attach a single frame for the panic location; full
/// Rust frame symbolication is left for a later pass (the raw backtrace rides
/// along as a property for now).
fn exception_object(exception_type: &str, value: &str, location: Option<&Location>) -> Value {
    let frames: Vec<Value> = location
        .map(|loc| {
            vec![json!({
                "platform": "rust:native",
                "lang": "rust",
                "filename": loc.file,
                "lineno": loc.line,
                "in_app": true,
                "resolved": true,
            })]
        })
        .unwrap_or_default();
    json!({
        "type": exception_type,
        "value": value,
        // Panics are by definition unhandled; this drives PostHog's
        // crash-vs-handled split in error tracking.
        "mechanism": { "handled": false, "synthetic": false },
        "stacktrace": { "type": "raw", "frames": frames },
    })
}

/// POST a single `$exception` event synchronously with a short timeout. Wrapped
/// in `catch_unwind` so a failure here (already on the panic path) can't recurse.
fn send_exception(
    cfg: &CrashConfig,
    exception: Value,
    exception_type: &str,
    message: &str,
    backtrace: Option<&str>,
) {
    let _ = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        let mut properties = Map::new();
        properties.insert("$exception_list".to_owned(), json!([exception]));
        // Group identical crashes into one issue.
        properties.insert(
            "$exception_fingerprint".to_owned(),
            json!(format!("{exception_type}:{message}")),
        );
        if let Some(backtrace) = backtrace {
            properties.insert("backtrace".to_owned(), json!(truncate(backtrace)));
        }

        let envelope = client::build_event_envelope(
            cfg.distinct_id,
            cfg.disable_geoip,
            &cfg.super_props,
            "$exception",
            properties,
            now_ms(),
        );
        let payload = json!({ "api_key": cfg.api_key, "batch": [envelope] });
        let url = format!("{}/batch/", cfg.host.trim_end_matches('/'));
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout(SEND_TIMEOUT)
            .build();
        if let Err(error) = agent.post(&url).send_json(payload) {
            bevy::log::warn!("analytics: posthog exception send failed: {error}");
        }
    }));
}

fn panic_message(info: &panic::PanicHookInfo<'_>) -> String {
    let payload = info.payload();
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "unknown panic payload".to_owned()
    }
}

fn truncate(text: &str) -> String {
    if text.len() <= MAX_BACKTRACE_BYTES {
        return text.to_owned();
    }
    let mut end = MAX_BACKTRACE_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n... [truncated]", &text[..end])
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}
