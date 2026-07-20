pub(crate) mod analytics;
pub(crate) mod app;
pub(crate) mod auth;
pub mod building;
pub mod cinematic;
pub(crate) mod cli;
pub(crate) mod combat;
pub(crate) mod console;
// Dev/agent-only client control socket: Unix-only and gated on
// `debug_assertions`, so shipped release builds compile it out entirely.
#[cfg(all(unix, debug_assertions))]
pub(crate) mod control_socket;
pub mod controller;
pub mod crafting;
pub mod game_balance;
pub(crate) mod inventory;
pub mod items;
pub(crate) mod local_crypto;
pub(crate) mod logging;
pub(crate) mod net;
pub mod protocol;
pub mod resource_nodes;
pub mod save;
pub mod server;
pub(crate) mod update;
pub(crate) mod util;
pub mod world;
pub(crate) mod world_time;

pub fn run() -> anyhow::Result<()> {
    cli::run()
}
