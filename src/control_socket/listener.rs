//! Unix-socket plumbing: bind (with owner-only permissions and stale-socket
//! recovery), the non-blocking per-frame accept drain registered as a Bevy
//! system, and the per-connection read/dispatch/reply cycle.

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
use bevy::prelude::*;

use super::handlers::{HandlerContext, handle_request};
use super::wire::{ControlResponse, DeployableDump};
use crate::app::{
    state::{ClientRuntime, LocalPlayerState, LookState, MenuState, WorldMapUiState},
    systems::HeadlessCapture,
};

/// Owner+group only, matching the server admin socket. The socket grants full
/// control of the client, so it must stay in a user-private directory.
const CONTROL_SOCKET_MODE: u32 = 0o660;

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
#[expect(clippy::too_many_arguments, reason = "Bevy system params")]
pub(crate) fn drain_control_socket(
    mut commands: Commands,
    socket: Option<Res<ClientControlSocket>>,
    mut runtime: ResMut<ClientRuntime>,
    mut menu: ResMut<MenuState>,
    mut look: ResMut<LookState>,
    mut world_map_ui: ResMut<WorldMapUiState>,
    mut ranged_input: ResMut<crate::app::state::RangedDrawState>,
    mut consume_charge: ResMut<crate::app::state::ConsumeChargeState>,
    mut gather_input: ResMut<crate::app::state::GatherInputState>,
    mut inventory_ui: ResMut<crate::app::state::InventoryUiState>,
    placement: Res<crate::app::state::DeployablePlacementState>,
    local_player: Res<LocalPlayerState>,
    capture: Option<Res<HeadlessCapture>>,
    replicated_deployables: Query<(
        &crate::server::Deployable,
        &crate::server::DeployableTransform,
        &crate::server::DeployableHealth,
        &crate::server::DeployableActive,
    )>,
) {
    let Some(socket) = socket else {
        return;
    };
    let capture = capture.as_deref();

    // Snapshot once per drain; requests are rare (agent-paced) and the
    // AoI deployable set is small.
    let deployables: Vec<DeployableDump> = replicated_deployables
        .iter()
        .map(|(meta, transform, health, active)| DeployableDump {
            id: meta.id,
            kind: format!("{:?}", meta.kind),
            position: [
                transform.position.x,
                transform.position.y,
                transform.position.z,
            ],
            yaw: transform.yaw,
            health: health.0,
            max_health: meta.max_health,
            active: active.0,
        })
        .collect();

    loop {
        let (stream, _) = match socket.listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            Err(error) => {
                eprintln!("could not accept control socket request: {error}");
                return;
            }
        };
        // Bundle the borrows only after a request actually arrived: the
        // `ResMut` derefs below mark their resources changed, and an idle
        // drain must not trip change detection every frame.
        let mut ctx = HandlerContext {
            commands: &mut commands,
            runtime: &mut runtime,
            menu: &mut menu,
            look: &mut look,
            world_map_ui: &mut world_map_ui,
            ranged_input: &mut ranged_input,
            consume_charge: &mut consume_charge,
            gather_input: &mut gather_input,
            inventory_ui: &mut inventory_ui,
            placement: &placement,
            local_player: &local_player,
            capture,
            deployables: &deployables,
        };
        handle_stream(stream, &mut ctx);
    }
}

fn handle_stream(mut stream: UnixStream, ctx: &mut HandlerContext) {
    let result = (|| -> Result<String> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let request = serde_json::from_reader(&mut stream)?;
        handle_request(request, ctx)
    })();

    let (ok, message) = match result {
        Ok(message) => (true, message),
        Err(error) => (false, error.to_string()),
    };
    write_response(&mut stream, ok, message);
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
