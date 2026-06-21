mod multiplayer_test;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use crate::{
    app,
    auth::{AuthMode, WorkosVerifier, workos::WorkosConfig},
    net,
    protocol::AccountId,
    save::{WorldSave, WorldStore, load_world_file, save_world_file},
    world_time::parse_time_token,
};

use self::multiplayer_test::run_multiplayer_test;

const DEFAULT_ADMIN_SOCKET: &str = "/run/game-server/admin.sock";
const DEFAULT_SHUTDOWN_REASON: &str =
    "Server is stopping for maintenance. Please reconnect after it restarts.";

#[derive(Debug, Parser)]
#[command(
    name = "Ashwend",
    version,
    about = "Ashwend client and authoritative server"
)]
struct Args {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    Client {
        /// When set, skip the main menu and connect directly to the given
        /// address as soon as the client window is ready. Used by the
        /// `multiplayer-test` helper so spawned windows enter the test
        /// world without any clicking.
        #[arg(long)]
        connect: Option<SocketAddr>,
    },
    Server {
        #[arg(long, default_value = "127.0.0.1:7777")]
        bind: SocketAddr,
        #[arg(long)]
        world: Option<PathBuf>,
        #[arg(long, value_enum, default_value_t = AuthModeArg::Workos)]
        auth: AuthModeArg,
        /// Overrides the WorkOS client id used to fetch the JWKS and verify
        /// access tokens. Defaults to the value from `workos.local.toml` /
        /// `GAME_WORKOS_CLIENT_ID` / the baked-in default (see
        /// [`crate::auth::workos::WorkosConfig`]). Only used with `--auth workos`.
        #[arg(long)]
        workos_client_id: Option<String>,
        #[arg(long)]
        admin_socket: Option<PathBuf>,
        /// Grant admin to these account ids on boot, merged into the world's
        /// admin list. Pass once per id (`--admin 1 --admin 2`). Intended for
        /// `--auth no-auth` dev/automation: it lets a control-socket agent
        /// whose `GAME_ACCOUNT_ID` matches run admin-gated slash commands
        /// (`/test-kit`, `/spawn`, `/time`, `/speed`) without a WorkOS
        /// token. Under `--auth workos`, prefer the `urn:ashwend:admin` token
        /// claim so admin isn't pinned to a raw account id.
        #[arg(long = "admin")]
        admins: Vec<AccountId>,
        /// Map size used only when generating a *fresh* world. Existing
        /// saves keep whatever size they were authored with.
        #[arg(long, value_enum, default_value_t = MapSizeArg::Medium)]
        map_size: MapSizeArg,
    },
    Admin {
        #[arg(long, default_value = DEFAULT_ADMIN_SOCKET)]
        socket: PathBuf,
        #[command(subcommand)]
        command: AdminCommand,
    },
    /// Developer helper: launch a fresh local server with a brand-new test
    /// world and two client windows that auto-connect with distinct names.
    /// Use to exercise multiplayer visuals (movement, nametags, chat
    /// bubbles, player models) without manual menu work.
    MultiplayerTest {
        /// Port the temporary server listens on. Defaults to a free port.
        #[arg(long, default_value_t = 0)]
        port: u16,
        /// Names assigned to the two test clients. Pass twice to override
        /// both, once to override the first. Defaults: `Alpha`, `Bravo`.
        #[arg(long, num_args = 1..=2)]
        names: Option<Vec<String>>,
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
    /// Set the day/night clock. Accepts `HH:MM` or an integer/decimal
    /// hour (`/admin time 18` for 6 pm).
    Time { time: String },
    /// Set the day/night cycle speed multiplier. `1.0` is the default
    /// (one cycle per 30 real minutes). `0` pauses the cycle. (Renamed from
    /// `speed`, which in-game now means the player run-speed cheat.)
    TimeSpeed { multiplier: f32 },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AuthModeArg {
    /// Verify WorkOS access tokens against the WorkOS JWKS. The default.
    Workos,
    /// Trust the client's claimed identity with no token check. Localhost only
    /// (singleplayer loopback, `multiplayer-test`), never expose to the net.
    NoAuth,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MapSizeArg {
    Small,
    Medium,
    Large,
}

impl From<MapSizeArg> for crate::world::ProceduralMapSize {
    fn from(value: MapSizeArg) -> Self {
        match value {
            MapSizeArg::Small => Self::Small,
            MapSizeArg::Medium => Self::Medium,
            MapSizeArg::Large => Self::Large,
        }
    }
}

struct ServerWorld {
    save: WorldSave,
    persistence: net::DedicatedWorldPersistence,
}

impl From<AuthModeArg> for AuthMode {
    fn from(value: AuthModeArg) -> Self {
        match value {
            AuthModeArg::Workos => Self::Workos,
            AuthModeArg::NoAuth => Self::NoAuth,
        }
    }
}

pub fn run() -> Result<()> {
    // Windows GUI-subsystem builds start with no console; reattach to the
    // launching terminal (if any) before clap parses so `--help`/`--version`,
    // parse errors, and `server`/`admin` output still reach it. No-op on a GUI
    // double-click and on non-Windows. See `crate::console`.
    crate::console::attach_parent_console();
    let args = Args::parse();
    match args.command.unwrap_or(Command::Client { connect: None }) {
        Command::Client { connect } => app::run_app(connect),
        Command::Server {
            bind,
            world,
            auth,
            workos_client_id,
            admin_socket,
            admins,
            map_size,
        } => {
            // The server runs MinimalPlugins (no LogPlugin), so install our own
            // tracing subscriber + file log and the crash/exception reporter
            // before anything else can log or panic. Mirrors what the client
            // gets through LogPlugin + AnalyticsPlugin.
            crate::logging::init_dedicated_server_logging();
            crate::analytics::install_dedicated_server_crash_reporter();
            let auth_mode: AuthMode = auth.into();
            let workos = match auth_mode {
                AuthMode::Workos => {
                    // The client id comes from the resolved WorkOS config
                    // (workos.local.toml / GAME_WORKOS_CLIENT_ID / baked-in
                    // default); `--workos-client-id` overrides it.
                    let client_id =
                        workos_client_id.unwrap_or_else(|| WorkosConfig::load().client_id);
                    Some(Arc::new(WorkosVerifier::new(&client_id)))
                }
                AuthMode::NoAuth => None,
            };
            let mut world = load_server_world(world, map_size.into())?;
            seed_admin_accounts(&mut world.save, &admins);
            net::run_dedicated_server(
                bind,
                world.save,
                auth_mode,
                workos,
                world.persistence,
                admin_socket,
            )
        }
        Command::Admin { socket, command } => run_admin_command(socket, command),
        Command::MultiplayerTest { port, names } => run_multiplayer_test(port, names),
    }
}

/// Merge operator-supplied `--admin` account ids into the world's admin list,
/// skipping ids already present (and the never-valid `0`). The in-memory save
/// is what the running `GameServer` checks in `is_admin`, so this grants admin
/// for the session and persists with the next auto-save/shutdown write. Mirrors
/// how `multiplayer-test` pre-seeds `save.admins` before launch.
fn seed_admin_accounts(save: &mut WorldSave, admins: &[AccountId]) {
    for &account_id in admins {
        if account_id != 0 && !save.admins.contains(&account_id) {
            save.admins.push(account_id);
        }
    }
}

fn load_server_world(
    path: Option<PathBuf>,
    map_size: crate::world::ProceduralMapSize,
) -> Result<ServerWorld> {
    let fresh_map = match crate::world::MapType::default() {
        crate::world::MapType::Procedural { seed, .. } => crate::world::MapType::Procedural {
            seed,
            size: map_size,
        },
    };
    if let Some(path) = path {
        let save = if path.exists() {
            match load_world_file(&path) {
                Ok(save) => {
                    ensure_map_size_matches(&save, map_size, &path)?;
                    save
                }
                Err(error) => {
                    // Dedicated servers run unattended, when a save format
                    // version bump (or any other unreadable state) makes the
                    // existing file unloadable, preserve the broken file
                    // under a `.bak.<unix-ts>` suffix and start fresh.
                    // Keeping the original means an operator can still pull
                    // names/positions out of it later, or pin down which
                    // version it was authored under.
                    let backup_path = unloadable_save_backup_path(&path);
                    eprintln!(
                        "could not load world save {}: {error:#}. Renaming to {} and starting fresh.",
                        path.display(),
                        backup_path.display(),
                    );
                    std::fs::rename(&path, &backup_path).with_context(|| {
                        format!(
                            "could not move unloadable world save {} to {}",
                            path.display(),
                            backup_path.display(),
                        )
                    })?;
                    let save = WorldSave::new_with_map("Dedicated File", None, fresh_map.clone());
                    save_world_file(&path, &save)?;
                    save
                }
            }
        } else {
            let save = WorldSave::new_with_map("Dedicated File", None, fresh_map.clone());
            save_world_file(&path, &save)?;
            save
        };
        return Ok(ServerWorld {
            save,
            persistence: net::DedicatedWorldPersistence::File(path),
        });
    }

    // No `--world` path: fall back to the platform default store with an
    // unowned dedicated world (admins are managed out-of-band for a real
    // dedicated server, not seeded from a local identity).
    let store = WorldStore::platform_default()?;
    let save = store.load_or_create_dedicated(None)?;
    Ok(ServerWorld {
        save,
        persistence: net::DedicatedWorldPersistence::Store(store),
    })
}

/// Refuse to boot a dedicated server against an existing save whose map size
/// doesn't match the requested `--map-size`. Map size is baked into the world
/// geometry at generation time and can't be changed in place, so silently
/// honoring the save's size (and ignoring the flag) would mask an operator
/// mistake. Pointing `--world` at a fresh file is the intended way to switch
/// sizes, that regenerates the world rather than corrupting one.
fn ensure_map_size_matches(
    save: &WorldSave,
    requested: crate::world::ProceduralMapSize,
    path: &std::path::Path,
) -> Result<()> {
    let crate::world::MapType::Procedural { size: existing, .. } = &save.map;
    let existing = *existing;
    if existing != requested {
        anyhow::bail!(
            "world save {} was generated as a {} map but --map-size {} was requested; \
             refusing to start. Map size is fixed at generation. Either pass --map-size {}, \
             or point --world at a fresh file to generate a new {} world.",
            path.display(),
            existing.label(),
            requested.label(),
            existing.label(),
            requested.label(),
        );
    }
    Ok(())
}

/// Build a sibling path for an unloadable save: `<original>.bak.<unix-ts>`.
/// The timestamp prevents the next failed boot from clobbering the previous
/// backup, so an operator can step through successive broken versions.
fn unloadable_save_backup_path(path: &std::path::Path) -> PathBuf {
    let suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default();
    let mut file_name = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| std::ffi::OsString::from("world.save"));
    file_name.push(format!(".bak.{suffix}"));
    path.with_file_name(file_name)
}

fn run_admin_command(socket: PathBuf, command: AdminCommand) -> Result<()> {
    let request = match command {
        AdminCommand::Announce { message } => net::DedicatedAdminRequest::Announce {
            text: message.join(" "),
        },
        AdminCommand::Shutdown { reason } => net::DedicatedAdminRequest::Shutdown { reason },
        AdminCommand::Time { time } => {
            let Some(seconds_of_day) = parse_time_token(&time) else {
                anyhow::bail!("could not parse '{time}'; expected HH:MM or an hour like 14");
            };
            net::DedicatedAdminRequest::SetTime { seconds_of_day }
        }
        AdminCommand::TimeSpeed { multiplier } => {
            net::DedicatedAdminRequest::SetTimeMultiplier { multiplier }
        }
    };
    let response = net::send_dedicated_admin_request(&socket, request)
        .with_context(|| format!("could not send admin command to {}", socket.display()))?;
    println!("{}", response.message);
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::*;
    use crate::net::DedicatedWorldPersistence;
    use crate::world::{MapType, ProceduralMapSize};

    fn temp_world_path() -> PathBuf {
        std::env::temp_dir().join(format!("game-cli-world-test-{}.save", Uuid::new_v4()))
    }

    #[test]
    fn seed_admin_accounts_appends_unique_nonzero_ids() {
        let mut save = WorldSave::new_with_map("seed test", None, MapType::default());
        save.admins = vec![10];

        // 0 is skipped, 10 is already present (no dup), 20 and 30 are added.
        seed_admin_accounts(&mut save, &[0, 10, 20, 20, 30]);

        assert_eq!(save.admins, vec![10, 20, 30]);
    }

    #[test]
    fn load_server_world_creates_fresh_save_when_path_missing() {
        let path = temp_world_path();
        let world = load_server_world(Some(path.clone()), ProceduralMapSize::Large)
            .expect("fresh world should load");

        assert!(matches!(
            world.persistence,
            DedicatedWorldPersistence::File(_)
        ));
        assert!(path.exists(), "save file should have been created");
        assert_eq!(
            world.save.map,
            MapType::Procedural {
                seed: crate::world::TEST_WORLD_SEED,
                size: ProceduralMapSize::Large,
            },
            "fresh dedicated world should honor the requested map size"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_server_world_rejects_size_mismatch_against_existing_save() {
        let path = temp_world_path();
        // Generate a Large world up front.
        load_server_world(Some(path.clone()), ProceduralMapSize::Large)
            .expect("fresh large world should load");

        // Re-loading with a different size must refuse rather than silently
        // honoring the on-disk size.
        let mismatch = load_server_world(Some(path.clone()), ProceduralMapSize::Medium);
        assert!(
            mismatch.is_err(),
            "loading a Large save with --map-size medium should be rejected"
        );

        // Loading with the matching size still works.
        load_server_world(Some(path.clone()), ProceduralMapSize::Large)
            .expect("matching size should reload");

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn load_server_world_replaces_unloadable_save_with_fresh_world() {
        let path = temp_world_path();
        fs::write(&path, b"not a real save file").expect("garbage save should be written");

        let world = load_server_world(Some(path.clone()), ProceduralMapSize::Medium)
            .expect("unloadable save should be replaced");

        // The fresh save should be loadable on a second call, proving the
        // unreadable file was renamed and a valid one written in its place.
        let reloaded = load_server_world(Some(path.clone()), ProceduralMapSize::Medium)
            .expect("fresh save should reload");
        assert_eq!(world.save.id, reloaded.save.id);

        // A `<path>.bak.<ts>` sibling should have been created from the
        // original garbage so an operator can salvage it later. We don't
        // pin the exact timestamp; check that at least one matching file
        // landed next to the active save.
        let parent = path.parent().expect("temp world has a parent");
        let stem = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .expect("temp world has a name");
        let backup_count = fs::read_dir(parent)
            .expect("temp dir readable")
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(&format!("{stem}.bak."))
            })
            .count();
        assert!(
            backup_count >= 1,
            "expected at least one .bak sibling next to {}, found {backup_count}",
            path.display()
        );
        for entry in fs::read_dir(parent).expect("temp dir readable").flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with(&format!("{stem}.bak."))
            {
                let _ = fs::remove_file(entry.path());
            }
        }

        let _ = fs::remove_file(&path);
    }
}
