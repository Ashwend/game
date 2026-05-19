use std::{net::SocketAddr, path::PathBuf};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    app, net,
    save::{WorldSave, WorldStore, load_world_file, save_world_file},
    steam::{AuthMode, OfflineSteamBackend, SteamBackend},
};

const DEFAULT_ADMIN_SOCKET: &str = "/run/game-server/admin.sock";
const DEFAULT_SHUTDOWN_REASON: &str =
    "Server is stopping for maintenance. Please reconnect after it restarts.";

#[derive(Debug, Parser)]
#[command(name = "Game", version, about = "Game client and authoritative server")]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Client,
    Server {
        #[arg(long, default_value = "127.0.0.1:7777")]
        bind: SocketAddr,
        #[arg(long)]
        world: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = AuthModeArg::Offline)]
        auth: AuthModeArg,
        #[arg(long)]
        admin_socket: Option<PathBuf>,
    },
    Admin {
        #[arg(long, default_value = DEFAULT_ADMIN_SOCKET)]
        socket: PathBuf,
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Debug, Subcommand)]
enum AdminCommand {
    Announce {
        #[arg(required = true, num_args = 1.., trailing_var_arg = true)]
        message: Vec<String>,
    },
    Shutdown {
        #[arg(long, default_value = DEFAULT_SHUTDOWN_REASON)]
        reason: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AuthModeArg {
    Offline,
    Steam,
}

struct ServerWorld {
    save: WorldSave,
    persistence: net::DedicatedWorldPersistence,
}

impl From<AuthModeArg> for AuthMode {
    fn from(value: AuthModeArg) -> Self {
        match value {
            AuthModeArg::Offline => Self::Offline,
            AuthModeArg::Steam => Self::Steam,
        }
    }
}

pub fn run() -> Result<()> {
    let args = Args::parse();
    match args.command.unwrap_or(Command::Client) {
        Command::Client => app::run_app(),
        Command::Server {
            bind,
            world,
            auth,
            admin_socket,
        } => {
            let world = load_server_world(world)?;
            net::run_dedicated_server(
                bind,
                world.save,
                auth.into(),
                world.persistence,
                admin_socket,
            )
        }
        Command::Admin { socket, command } => run_admin_command(socket, command),
    }
}

fn load_server_world(path: Option<PathBuf>) -> Result<ServerWorld> {
    if let Some(path) = path {
        let save = if path.exists() {
            load_world_file(&path)
                .with_context(|| format!("could not load world save {}", path.display()))?
        } else {
            let save = WorldSave::new("Dedicated File", None);
            save_world_file(&path, &save)?;
            save
        };
        return Ok(ServerWorld {
            save,
            persistence: net::DedicatedWorldPersistence::File(path),
        });
    }

    let steam = OfflineSteamBackend;
    let user = steam.current_user()?;
    let store = WorldStore::platform_default()?;
    let save = store.load_or_create_dedicated(Some(user.steam_id))?;
    Ok(ServerWorld {
        save,
        persistence: net::DedicatedWorldPersistence::Store(store),
    })
}

fn run_admin_command(socket: PathBuf, command: AdminCommand) -> Result<()> {
    let request = match command {
        AdminCommand::Announce { message } => net::DedicatedAdminRequest::Announce {
            text: message.join(" "),
        },
        AdminCommand::Shutdown { reason } => net::DedicatedAdminRequest::Shutdown { reason },
    };
    let response = net::send_dedicated_admin_request(&socket, request)
        .with_context(|| format!("could not send admin command to {}", socket.display()))?;
    println!("{}", response.message);
    Ok(())
}
