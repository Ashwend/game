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
//! carry zero runtime cost. Unix-only (it uses `UnixListener`); the module is
//! `#[cfg(unix)]`-gated at the `mod` declaration.

use std::{
    fs,
    io::{ErrorKind, Write},
    os::unix::{
        fs::PermissionsExt,
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use bevy::{
    prelude::*,
    render::view::screenshot::{Screenshot, save_to_disk},
};
use serde::{Deserialize, Serialize};

use super::HeadlessCapture;
use crate::{
    app::state::{ClientRuntime, LocalPlayerState, LookState, MenuState, Screen},
    controller::MAX_LOOK_PITCH,
    items::intern_item_id,
    protocol::{ClientMessage, InventoryCommand, PlaceDeployableCommand, Vec3Net},
};

/// Owner+group only, matching the server admin socket. The socket grants full
/// control of the client, so it must stay in a user-private directory.
const CONTROL_SOCKET_MODE: u32 = 0o660;

/// One request from the controlling agent. Tagged JSON, e.g.
/// `{"command":"set_inventory_open","open":true}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub(crate) enum ControlRequest {
    /// Capture the primary window (3D scene + egui UI) to a PNG. Asynchronous:
    /// the file lands a frame or two later, so poll for it before reading.
    Screenshot { path: PathBuf },
    /// Forward a slash command to the server (no leading `/`), e.g. `test-kit`.
    SendCommand { text: String },
    /// Select an actionbar slot (0-based), making that slot's item the held /
    /// active one, exactly as pressing its number key would. Lets an agent put
    /// a specific tool in hand to verify its held viewmodel (e.g. after
    /// `test-kit`, the iron pickaxe lands in slot 3).
    SelectActionbarSlot { slot: usize },
    /// Place a deployable the player is carrying (e.g. `workbench_t1`,
    /// `crude_furnace`) onto level ground a short distance in front of them,
    /// turned to face the player. Position is derived from the local view yaw
    /// rather than the look ray, so it works headless without aiming at the
    /// floor. Lets an agent drop a structure to verify its authored in-world
    /// model. `distance` (metres, default ~2.2) must stay within placement
    /// reach; the server still validates inventory, ground, and overlap.
    PlaceDeployable {
        item_id: String,
        #[serde(default)]
        distance: Option<f32>,
    },
    /// Point the camera: absolute yaw/pitch in radians, exactly as if the
    /// mouse had moved there. Pitch is clamped to the same limit as mouse
    /// look. Lets an agent aim at ground-level targets (resource nodes,
    /// placed structures) for screenshots and for commands that target
    /// along the view ray (e.g. `/drain`).
    SetLook { yaw: f32, pitch: f32 },
    /// Navigate between menu screens (main_menu / worlds / multiplayer /
    /// options / in_game). Does not start a session; connect via `--connect`.
    SetScreen { screen: String },
    /// Open or close the inventory panel.
    SetInventoryOpen { open: bool },
    /// Return a JSON snapshot of key client state for assertions.
    DumpState,
}

#[derive(Debug, Serialize)]
struct ControlResponse {
    ok: bool,
    message: String,
}

/// JSON snapshot returned by [`ControlRequest::DumpState`]. Assembled by hand
/// because `ClientRuntime` / `MenuState` aren't `Serialize`; this is the stable
/// shape an agent asserts against.
#[derive(Debug, Serialize)]
struct ClientStateDump {
    client_id: Option<u64>,
    is_admin: bool,
    world_loaded: bool,
    world_version: u64,
    /// Strong "the world finished loading" signal: connected, world installed,
    /// and the local player's replicated entity has arrived.
    in_world: bool,
    /// Whether the owner-only `PlayerPrivate` (inventory/crafting) replicated.
    /// Distinguishes a fresh-but-empty inventory (Some) from one that never
    /// arrived (None), e.g. after a sleeping-body wake with a stale owner override.
    private_present: bool,
    screen: String,
    inventory_open: bool,
    crafting_open: bool,
    furnace_open: bool,
    loot_bag_open: bool,
    pause_open: bool,
    chat_open: bool,
    death_splash: bool,
    position: Option<[f32; 3]>,
    yaw: Option<f32>,
    pitch: Option<f32>,
    health: Option<f32>,
    local_ping_ms: u16,
    players: Vec<PlayerDump>,
}

#[derive(Debug, Serialize)]
struct PlayerDump {
    client_id: u64,
    name: String,
    ping_ms: u16,
}

#[derive(Resource)]
pub(crate) struct ClientControlSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl ClientControlSocket {
    /// Env var that both enables the socket and names its path.
    pub(crate) const ENV: &'static str = "GAME_CONTROL_SOCKET";

    pub(crate) fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create control socket directory {}",
                    parent.display()
                )
            })?;
        }
        remove_stale_socket(&path)?;

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("could not bind control socket {}", path.display()))?;
        listener
            .set_nonblocking(true)
            .context("could not set control socket to non-blocking")?;
        fs::set_permissions(&path, fs::Permissions::from_mode(CONTROL_SOCKET_MODE))
            .context("could not set control socket permissions")?;

        Ok(Self { listener, path })
    }
}

impl Drop for ClientControlSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Non-blocking drain of pending control requests, one per accepted connection.
/// Registered bare in the `Update` schedule (no ordering dependency) and a
/// no-op whenever the socket resource is absent.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drain_control_socket(
    mut commands: Commands,
    socket: Option<Res<ClientControlSocket>>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut look: ResMut<LookState>,
    local_player: Res<LocalPlayerState>,
    capture: Option<Res<HeadlessCapture>>,
) {
    let Some(socket) = socket else {
        return;
    };
    let capture = capture.as_deref();

    loop {
        let (stream, _) = match socket.listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            Err(error) => {
                eprintln!("could not accept control socket request: {error}");
                return;
            }
        };
        handle_stream(
            stream,
            &mut commands,
            &mut runtime,
            &mut menu,
            &mut look,
            &local_player,
            capture,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_stream(
    mut stream: UnixStream,
    commands: &mut Commands,
    runtime: &mut ClientRuntime,
    menu: &mut MenuState,
    look: &mut LookState,
    local_player: &LocalPlayerState,
    capture: Option<&HeadlessCapture>,
) {
    let result = (|| -> Result<String> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let request = serde_json::from_reader(&mut stream)?;
        handle_request(
            request,
            commands,
            runtime,
            menu,
            look,
            local_player,
            capture,
        )
    })();

    let (ok, message) = match result {
        Ok(message) => (true, message),
        Err(error) => (false, error.to_string()),
    };
    write_response(&mut stream, ok, message);
}

#[allow(clippy::too_many_arguments)]
fn handle_request(
    request: ControlRequest,
    commands: &mut Commands,
    runtime: &mut ClientRuntime,
    menu: &mut MenuState,
    look: &mut LookState,
    local_player: &LocalPlayerState,
    capture: Option<&HeadlessCapture>,
) -> Result<String> {
    match request {
        ControlRequest::Screenshot { path } => {
            // In headless-capture mode the primary camera renders to an
            // off-screen image (the window is hidden), so screenshot that image;
            // otherwise read the live window framebuffer as before.
            let screenshot = match capture {
                Some(capture) => Screenshot::image(capture.image.clone()),
                None => Screenshot::primary_window(),
            };
            commands
                .spawn(screenshot)
                .observe(save_to_disk(path.clone()));
            Ok(format!(
                "screenshot queued to {} (lands within a frame or two)",
                path.display()
            ))
        }
        ControlRequest::SendCommand { text } => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Command { text })?;
            Ok("command queued".to_owned())
        }
        ControlRequest::SelectActionbarSlot { slot } => {
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::Inventory(
                InventoryCommand::SelectActionbarSlot { slot },
            ))?;
            Ok(format!("selected actionbar slot {slot}"))
        }
        ControlRequest::PlaceDeployable { item_id, distance } => {
            let view = runtime
                .local_view()
                .context("no local player view (not in a world)")?;
            let dist = distance.unwrap_or(2.2);
            // Player forward is `(-sin yaw, 0, -cos yaw)` (see
            // `controller::movement`), so drop the structure that far ahead on
            // the floor (y = 0). A deployable's front is +Z, so leaving its yaw
            // equal to the view yaw turns that front back toward the camera.
            let (sin_yaw, cos_yaw) = view.yaw.sin_cos();
            let position = Vec3Net::new(
                view.position.x - sin_yaw * dist,
                0.0,
                view.position.z - cos_yaw * dist,
            );
            let session = runtime
                .session
                .as_mut()
                .context("no active session (not in a world)")?;
            session.send(ClientMessage::PlaceDeployable(PlaceDeployableCommand {
                item_id: intern_item_id(&item_id),
                position,
                yaw: view.yaw,
            }))?;
            Ok(format!(
                "place {item_id} queued at [{:.2}, 0.00, {:.2}]",
                position.x, position.z
            ))
        }
        ControlRequest::SetLook { yaw, pitch } => {
            if !yaw.is_finite() || !pitch.is_finite() {
                bail!("yaw/pitch must be finite radians");
            }
            look.yaw = yaw;
            look.pitch = pitch.clamp(-MAX_LOOK_PITCH, MAX_LOOK_PITCH);
            Ok(format!(
                "look set to yaw {:.3}, pitch {:.3}",
                look.yaw, look.pitch
            ))
        }
        ControlRequest::SetScreen { screen } => {
            menu.screen = parse_screen(&screen)?;
            Ok(format!("screen set to {:?}", menu.screen))
        }
        ControlRequest::SetInventoryOpen { open } => {
            menu.inventory_open = open;
            Ok(format!("inventory_open = {open}"))
        }
        ControlRequest::DumpState => {
            let dump = build_dump(runtime, menu, local_player);
            Ok(serde_json::to_string(&dump)?)
        }
    }
}

fn build_dump(
    runtime: &ClientRuntime,
    menu: &MenuState,
    local_player: &LocalPlayerState,
) -> ClientStateDump {
    let view = runtime.local_view();
    ClientStateDump {
        client_id: runtime.client_id,
        is_admin: runtime.is_admin,
        world_loaded: runtime.world.is_some(),
        world_version: runtime.world_version,
        in_world: runtime.client_id.is_some()
            && runtime.world.is_some()
            && local_player.entity.is_some(),
        private_present: local_player.private.is_some(),
        screen: format!("{:?}", menu.screen),
        inventory_open: menu.inventory_open,
        crafting_open: menu.crafting_open,
        furnace_open: menu.furnace_open,
        loot_bag_open: menu.loot_bag_open,
        pause_open: menu.pause_open,
        chat_open: menu.chat_open,
        death_splash: menu.death_splash.is_some(),
        position: view.map(|v| [v.position.x, v.position.y, v.position.z]),
        yaw: view.map(|v| v.yaw),
        pitch: view.map(|v| v.pitch),
        health: view.map(|v| v.health),
        local_ping_ms: runtime.local_ping_ms,
        players: runtime
            .players
            .iter()
            .map(|p| PlayerDump {
                client_id: p.client_id,
                name: p.name.clone(),
                ping_ms: p.ping_ms,
            })
            .collect(),
    }
}

/// Map an agent-supplied screen name to a [`Screen`]. Tolerant of case and of
/// `_`/`-`/space separators so `"main_menu"`, `"MainMenu"`, and `"in game"` all
/// work.
fn parse_screen(raw: &str) -> Result<Screen> {
    let normalized = raw.trim().to_ascii_lowercase().replace(['_', '-', ' '], "");
    Ok(match normalized.as_str() {
        "mainmenu" | "menu" | "main" => Screen::MainMenu,
        "worlds" => Screen::Worlds,
        "multiplayer" => Screen::Multiplayer,
        "options" => Screen::Options,
        "ingame" | "game" => Screen::InGame,
        other => bail!("unknown screen '{other}'"),
    })
}

fn write_response(stream: &mut UnixStream, ok: bool, message: String) {
    let response = ControlResponse { ok, message };
    if let Err(error) = serde_json::to_writer(&mut *stream, &response) {
        eprintln!("could not write control socket response: {error}");
        return;
    }
    let _ = stream.write_all(b"\n");
}

fn remove_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match UnixStream::connect(path) {
        Ok(_) => bail!("control socket {} is already in use", path.display()),
        Err(_) => fs::remove_file(path)
            .with_context(|| format!("could not remove stale control socket {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A short, unique temp socket path. macOS caps Unix-domain socket paths at
    /// ~104 bytes (`sockaddr_un::sun_path`), and the default temp dir is already
    /// ~50 chars, so a full UUID-suffixed name overflows and `bind` fails with
    /// `ENAMETOOLONG`. A truncated suffix stays comfortably under the limit while
    /// keeping per-test uniqueness.
    fn temp_socket_path(tag: &str) -> PathBuf {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        std::env::temp_dir().join(format!("ash-{tag}-{}.sock", &unique[..12]))
    }

    #[test]
    fn parse_screen_accepts_aliases_and_separators() {
        assert!(matches!(parse_screen("main_menu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("MainMenu"), Ok(Screen::MainMenu)));
        assert!(matches!(parse_screen("  worlds "), Ok(Screen::Worlds)));
        assert!(matches!(parse_screen("in game"), Ok(Screen::InGame)));
        assert!(matches!(
            parse_screen("multiplayer"),
            Ok(Screen::Multiplayer)
        ));
        assert!(parse_screen("nonsense").is_err());
    }

    #[test]
    fn requests_deserialize_from_the_agent_wire_format() {
        // Pin the exact JSON an agent will send so the wire shape can't drift.
        let inv: ControlRequest =
            serde_json::from_str(r#"{"command":"set_inventory_open","open":true}"#).unwrap();
        assert!(matches!(
            inv,
            ControlRequest::SetInventoryOpen { open: true }
        ));

        let cmd: ControlRequest =
            serde_json::from_str(r#"{"command":"send_command","text":"test-kit"}"#).unwrap();
        assert!(matches!(cmd, ControlRequest::SendCommand { text } if text == "test-kit"));

        let slot: ControlRequest =
            serde_json::from_str(r#"{"command":"select_actionbar_slot","slot":3}"#).unwrap();
        assert!(matches!(
            slot,
            ControlRequest::SelectActionbarSlot { slot: 3 }
        ));

        let shot: ControlRequest =
            serde_json::from_str(r#"{"command":"screenshot","path":"/tmp/a.png"}"#).unwrap();
        assert!(matches!(shot, ControlRequest::Screenshot { .. }));

        let dump: ControlRequest = serde_json::from_str(r#"{"command":"dump_state"}"#).unwrap();
        assert!(matches!(dump, ControlRequest::DumpState));

        let look: ControlRequest =
            serde_json::from_str(r#"{"command":"set_look","yaw":1.5,"pitch":-0.42}"#).unwrap();
        assert!(matches!(
            look,
            ControlRequest::SetLook { yaw, pitch }
                if (yaw - 1.5).abs() < f32::EPSILON && (pitch + 0.42).abs() < f32::EPSILON
        ));

        // `distance` is optional and defaults to None when omitted.
        let place: ControlRequest =
            serde_json::from_str(r#"{"command":"place_deployable","item_id":"crude_furnace"}"#)
                .unwrap();
        assert!(matches!(
            place,
            ControlRequest::PlaceDeployable { item_id, distance: None } if item_id == "crude_furnace"
        ));
        let place_dist: ControlRequest = serde_json::from_str(
            r#"{"command":"place_deployable","item_id":"workbench_t1","distance":3.0}"#,
        )
        .unwrap();
        assert!(matches!(
            place_dist,
            ControlRequest::PlaceDeployable { distance: Some(d), .. } if (d - 3.0).abs() < f32::EPSILON
        ));
    }

    #[test]
    fn bind_creates_then_drop_removes_the_socket() {
        let path = temp_socket_path("ctl");
        let socket = ClientControlSocket::bind(&path).expect("bind should succeed");
        assert!(path.exists(), "socket file should exist while bound");
        drop(socket);
        assert!(!path.exists(), "Drop should remove the socket file");
    }

    #[test]
    fn bind_reclaims_a_stale_socket_file() {
        let path = temp_socket_path("stale");
        // A leftover file with no listener behind it should be reclaimed.
        std::fs::write(&path, b"stale").unwrap();
        let socket = ClientControlSocket::bind(&path).expect("stale socket should be reclaimed");
        assert!(path.exists());
        drop(socket);
        let _ = std::fs::remove_file(&path);
    }
}
