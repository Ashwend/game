pub mod app;
pub mod cli;
pub mod controller;
pub mod items;
pub mod net;
pub mod protocol;
pub mod resources;
pub mod save;
pub mod server;
pub mod steam;
pub mod world;

pub fn run() -> anyhow::Result<()> {
    cli::run()
}
