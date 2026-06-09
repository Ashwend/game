//! On-disk application log (client and dedicated server).
//!
//! Bevy's default `LogPlugin` only writes to stderr, which is invisible in a
//! packaged release (a double-clicked `.app` / `.exe` has no attached terminal)
//! and easy to lose on a server. This module mirrors every log line into a file
//! under the platform data directory so there is always something to inspect:
//! `<data_dir>/logs/ashwend.log` for the client, `ashwend-server.log` for the
//! dedicated server.
//!
//! Two entry points, because the two processes build their logging differently:
//! - **Client** ([`install_file_layer`]): the client runs Bevy's `LogPlugin`,
//!   so we just hand it an extra `fmt` layer via `LogPlugin { custom_layer, .. }`
//!   in `app.rs`. The global `EnvFilter` applies to it, so the file captures the
//!   same level the console does.
//! - **Dedicated server** ([`init_dedicated_server_logging`]): the server runs
//!   `MinimalPlugins`, which has *no* `LogPlugin` and therefore no tracing
//!   subscriber at all, so we build and install one ourselves (an `EnvFilter`
//!   plus a stderr layer for journald/systemd and a file layer).
//!
//! In both cases the previous run is preserved (`*.prev.log`, rotated on
//! startup) so a crash log survives the relaunch, and [`write_raw`] appends
//! straight to the same handle so the panic hook (see `crate::analytics::crash`)
//! can land a crash record even if the process aborts before normal logging
//! flushes.
//!
//! All failures here are non-fatal: if the file can't be opened we warn to
//! stderr and leave console/stderr logging untouched.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use bevy::{
    app::App,
    log::{
        BoxedLayer,
        tracing_subscriber::{
            self, EnvFilter, Layer, Registry, fmt::MakeWriter, layer::SubscriberExt,
            util::SubscriberInitExt,
        },
    },
};
use directories::ProjectDirs;

// Same identity triple as the rest of our platform-dir users (settings,
// saves, analytics id) so every Ashwend file lands under one data directory.
const QUALIFIER: &str = "com";
const ORGANIZATION: &str = "Ashwend";
const APPLICATION: &str = "Ashwend";

const CLIENT_LOG_FILE_NAME: &str = "ashwend.log";
const CLIENT_PREV_LOG_FILE_NAME: &str = "ashwend.prev.log";
const SERVER_LOG_FILE_NAME: &str = "ashwend-server.log";
const SERVER_PREV_LOG_FILE_NAME: &str = "ashwend-server.prev.log";

/// Default server log filter when `RUST_LOG` is unset. `info` everywhere, with
/// the usual render-crate spam muted (harmless on a headless server, but keeps
/// the filter identical should a future tool pull those crates in).
const DEFAULT_SERVER_FILTER: &str = "info,wgpu=error,naga=warn";

/// The single shared handle the `tracing` layer and [`write_raw`] both write
/// through. Behind a `Mutex` so the normal log path and a synchronous crash
/// append never interleave a half-written line.
static LOG_SINK: OnceLock<Arc<Mutex<File>>> = OnceLock::new();
/// Resolved path of the active log file, for surfacing in a startup log line.
static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

/// `LogPlugin::custom_layer` hook for the client. Opens (and rotates) the
/// client log file and returns a `fmt` layer that writes into it. Returns
/// `None` (console logging only) if the file can't be opened.
pub fn install_file_layer(_app: &mut App) -> Option<BoxedLayer> {
    let sink = open_sink(CLIENT_LOG_FILE_NAME, CLIENT_PREV_LOG_FILE_NAME, "client")?;
    let layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(SharedFileWriter(sink))
        .boxed();
    Some(layer)
}

/// Install the dedicated server's tracing subscriber: an `EnvFilter`
/// (`RUST_LOG`, defaulting to [`DEFAULT_SERVER_FILTER`]), a stderr layer so
/// journald/systemd captures everything, and a file layer to
/// `ashwend-server.log` when it can be opened. Must be called once, before the
/// server's Bevy app starts. Safe to call even if a subscriber somehow already
/// exists, it logs and moves on rather than panicking.
pub fn init_dedicated_server_logging() {
    let sink = open_sink(
        SERVER_LOG_FILE_NAME,
        SERVER_PREV_LOG_FILE_NAME,
        "dedicated server",
    );
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_SERVER_FILTER));
    let stderr_layer = tracing_subscriber::fmt::layer().with_writer(io::stderr);

    let result = match sink {
        Some(sink) => Registry::default()
            .with(filter)
            .with(stderr_layer)
            .with(
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_writer(SharedFileWriter(sink)),
            )
            .try_init(),
        None => Registry::default()
            .with(filter)
            .with(stderr_layer)
            .try_init(),
    };

    if let Err(error) = result {
        eprintln!("logging: could not install the dedicated server log subscriber: {error}");
        return;
    }
    match log_file_path() {
        Some(path) => bevy::log::info!("dedicated server logging to {}", path.display()),
        None => {
            bevy::log::warn!("dedicated server file logging unavailable; logging to stderr only");
        }
    }
}

/// Path of the active log file, once a logging entry point has opened it.
pub fn log_file_path() -> Option<PathBuf> {
    LOG_PATH.get().cloned()
}

/// Append a record straight to the log file, flushing immediately. Used by the
/// panic hook: it bypasses the normal tracing path so a crash report reaches
/// disk even if the process is about to abort. No-op before the file is open.
pub fn write_raw(text: &str) {
    if let Some(sink) = LOG_SINK.get() {
        write_line(sink, text);
    }
}

/// Resolve, rotate, and open a fresh log file, then register it as the process
/// log sink and stamp a run header. Returns the shared handle, or `None` if the
/// file can't be opened (callers fall back to console/stderr only).
fn open_sink(file_name: &str, prev_name: &str, role: &str) -> Option<Arc<Mutex<File>>> {
    let path = match log_path(file_name) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("logging: could not resolve log directory, file logging disabled: {error:#}");
            return None;
        }
    };
    let file = match open_log_file(&path, prev_name) {
        Ok(file) => file,
        Err(error) => {
            eprintln!(
                "logging: could not open {}, file logging disabled: {error}",
                path.display()
            );
            return None;
        }
    };

    let sink = Arc::new(Mutex::new(file));
    // Stamp a header so each run is easy to find when scanning the file.
    write_line(
        &sink,
        &format!(
            "==== Ashwend {} ({role}) started (unix_ms={}) ====",
            crate::protocol::GAME_VERSION,
            now_ms()
        ),
    );

    // First writer wins; if for some reason this runs twice we keep the
    // original handle rather than leaking a second open file.
    let sink = match LOG_SINK.set(Arc::clone(&sink)) {
        Ok(()) => sink,
        Err(_) => Arc::clone(LOG_SINK.get().expect("set fails only when already set")),
    };
    let _ = LOG_PATH.set(path);
    Some(sink)
}

fn write_line(sink: &Arc<Mutex<File>>, text: &str) {
    if let Ok(mut file) = sink.lock() {
        let _ = writeln!(file, "{text}");
        let _ = file.flush();
    }
}

fn log_path(file_name: &str) -> anyhow::Result<PathBuf> {
    let dirs = ProjectDirs::from(QUALIFIER, ORGANIZATION, APPLICATION)
        .ok_or_else(|| anyhow::anyhow!("could not resolve the platform data directory"))?;
    Ok(dirs.data_dir().join("logs").join(file_name))
}

/// Create the log directory, move the previous run's file aside, and open a
/// fresh (truncated) file. Keeping exactly one prior run bounds disk use while
/// still preserving the log across the relaunch that usually follows a crash.
fn open_log_file(path: &Path, prev_name: &str) -> io::Result<File> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if path.exists() {
        let prev = path.with_file_name(prev_name);
        // A failed rotate must not stop us opening the current file.
        let _ = fs::rename(path, prev);
    }
    OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default()
}

/// `MakeWriter` over the shared log file. Each log event locks the handle for
/// the duration of its write, so concurrent logging threads serialize cleanly.
#[derive(Clone)]
struct SharedFileWriter(Arc<Mutex<File>>);

impl<'a> MakeWriter<'a> for SharedFileWriter {
    type Writer = LockedFile<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        LockedFile(
            self.0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        )
    }
}

struct LockedFile<'a>(std::sync::MutexGuard<'a, File>);

impl Write for LockedFile<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}
