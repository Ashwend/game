//! Dev-only client control socket.
//!
//! Lets an external agent drive the running client (screenshot, slash command,
//! menu navigation, state dump) over a Unix socket, so automated tests can
//! launch the game, act, and assert on JSON state instead of pixels.
//!
//! This is a thin transport adapter, exactly the role the admin socket plays on
//! the server side (`src/net/host/admin.rs`); it owns no gameplay rules, it only
//! pokes existing client resources or forwards a `ClientMessage::Command`.
//!
//! Inert by default: the socket is bound only when `GAME_CONTROL_SOCKET` names a
//! path, so a normal `./cli client` launch never opens it and shipped builds
//! carry zero runtime cost. Unix-only (it uses `UnixListener`) and dev-only: the
//! module is `#[cfg(all(unix, debug_assertions))]`-gated at its `lib.rs`
//! declaration, so release builds compile it out entirely.
//!
//! Split by concern into submodules:
//! - `wire`: the serde request/response/state-dump shapes.
//! - `listener`: the Unix-socket bind/accept/read/reply plumbing.
//! - `handlers`: per-domain request handlers behind a thin dispatch.
//! - `targeting`: building-snap geometry for scripted placement.

mod handlers;
mod listener;
mod targeting;
mod wire;

pub(crate) use listener::{ClientControlSocket, drain_control_socket};
