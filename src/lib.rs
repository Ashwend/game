pub mod analytics;
pub mod app;
pub mod cli;
pub mod combat;
pub mod controller;
pub mod crafting;
pub mod game_balance;
pub mod inventory;
pub mod items;
pub mod net;
pub mod protocol;
pub mod resources;
pub mod save;
pub mod server;
pub mod steam;
pub mod util;
pub mod workos_login;
pub mod world;
pub mod world_time;

pub fn run() -> anyhow::Result<()> {
    cli::run()
}
