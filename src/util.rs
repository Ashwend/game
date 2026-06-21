//! Tiny cross-module helpers. Keep this module dependency-light: anything
//! in here should be reachable from `protocol`, `controller`, `server`, and
//! the client tree without pulling in heavy crates.

pub mod fs;
pub mod hash;
pub mod platform;
pub mod variation;

/// Open `url` in the system browser. Best-effort; returns the launch error so a
/// caller can surface it if it cares. Shared by the menu's Discord link, the
/// WorkOS login flow, and the updater's download-page fallback so the per-OS
/// launch lives in one place.
///
/// Delegates to the `open` crate rather than hand-rolling a per-OS launcher. The
/// previous Windows path shelled out to `cmd /C start "" <url>`, but `cmd`
/// treats `&` as a statement separator and Rust's `Command` does not quote a
/// bare URL, so the WorkOS authorize URL was sliced at its first `&` (after
/// `response_type=code`): the browser received a request with no `client_id`,
/// `provider`, or `redirect_uri`, and WorkOS answered with a generic
/// "Something went wrong" page. Sign-in therefore failed on Windows while
/// working on macOS (`open <url>` passes the URL as a single argv element with
/// no shell parsing). `open` uses `ShellExecuteW` on Windows, which takes the
/// URL as one wide string with no shell parsing, handles arbitrary length and
/// special characters, opens no console window, and removes the
/// pass-a-URL-through-`cmd` command-injection surface. `that_detached` launches
/// the handler without waiting on it.
pub fn open_url(url: &str) -> std::io::Result<()> {
    open::that_detached(url)
}
