pub mod analytics;
pub mod app;
pub mod auth;
pub mod building;
pub mod cli;
pub mod combat;
pub mod controller;
pub mod crafting;
pub mod game_balance;
pub mod inventory;
pub mod items;
pub mod local_crypto;
pub mod logging;
pub mod net;
pub mod protocol;
pub mod resources;
pub mod save;
pub mod server;
pub mod update;
pub mod util;
pub mod world;
pub mod world_time;

pub fn run() -> anyhow::Result<()> {
    cli::run()
}
