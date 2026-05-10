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
use lightyear::prelude::{MessageSender, server::ClientOf};

use super::{
    AuthoritativeServer, HostShutdown,
    routing::{ServerConnections, route_envelopes},
};
use crate::{
    net::dedicated::{DedicatedAdminRequest, DedicatedAdminResponse},
    protocol::ServerMessage,
};

const ADMIN_SOCKET_MODE: u32 = 0o660;

#[derive(Resource)]
pub(super) struct HostAdminSocket {
    listener: UnixListener,
    path: PathBuf,
}

impl HostAdminSocket {
    pub(super) fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!(
                    "could not create admin socket directory {}",
                    parent.display()
                )
            })?;
        }
        remove_stale_socket(&path)?;

        let listener = UnixListener::bind(&path)
            .with_context(|| format!("could not bind admin socket {}", path.display()))?;
        listener.set_nonblocking(true).with_context(|| {
            format!(
                "could not set admin socket {} to non-blocking",
                path.display()
            )
        })?;
        fs::set_permissions(&path, fs::Permissions::from_mode(ADMIN_SOCKET_MODE)).with_context(
            || {
                format!(
                    "could not set admin socket permissions on {}",
                    path.display()
                )
            },
        )?;

        Ok(Self { listener, path })
    }
}

impl Drop for HostAdminSocket {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub(super) fn drain_admin_socket(
    socket: Option<Res<HostAdminSocket>>,
    mut shutdown: ResMut<HostShutdown>,
    mut server: ResMut<AuthoritativeServer>,
    connections: Res<ServerConnections>,
    mut senders: Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    let Some(socket) = socket else {
        return;
    };

    loop {
        let (stream, _) = match socket.listener.accept() {
            Ok(accepted) => accepted,
            Err(error) if error.kind() == ErrorKind::WouldBlock => return,
            Err(error) => {
                eprintln!("could not accept admin socket request: {error}");
                return;
            }
        };

        handle_admin_stream(
            stream,
            &mut shutdown,
            &mut server,
            &connections,
            &mut senders,
        );
    }
}

fn handle_admin_stream(
    mut stream: UnixStream,
    shutdown: &mut HostShutdown,
    server: &mut AuthoritativeServer,
    connections: &ServerConnections,
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) {
    let result = (|| -> Result<String> {
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        stream.set_write_timeout(Some(Duration::from_secs(2)))?;
        let request = serde_json::from_reader(&mut stream)?;
        handle_admin_request(request, shutdown, server, connections, senders)
    })();

    match result {
        Ok(message) => write_admin_response(&mut stream, true, message),
        Err(error) => write_admin_response(&mut stream, false, error.to_string()),
    }
}

fn handle_admin_request(
    request: DedicatedAdminRequest,
    shutdown: &mut HostShutdown,
    server: &mut AuthoritativeServer,
    connections: &ServerConnections,
    senders: &mut Query<&mut MessageSender<ServerMessage>, With<ClientOf>>,
) -> Result<String> {
    match request {
        DedicatedAdminRequest::Announce { text } => {
            let envelopes = server.0.announce(text);
            if envelopes.is_empty() {
                bail!("announcement text is empty");
            }
            route_envelopes(connections, senders, envelopes);
            Ok("announcement sent".to_owned())
        }
        DedicatedAdminRequest::Shutdown { reason } => {
            route_envelopes(connections, senders, server.0.kick_all(reason));
            shutdown.requested = true;
            Ok("shutdown requested".to_owned())
        }
    }
}

fn write_admin_response(stream: &mut UnixStream, ok: bool, message: String) {
    let response = DedicatedAdminResponse { ok, message };
    if let Err(error) = serde_json::to_writer(&mut *stream, &response) {
        eprintln!("could not write admin socket response: {error}");
        return;
    }
    if let Err(error) = stream.write_all(b"\n") {
        eprintln!("could not write admin socket response: {error}");
    }
}

fn remove_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }

    match UnixStream::connect(path) {
        Ok(_) => bail!("admin socket {} is already in use", path.display()),
        Err(_) => fs::remove_file(path)
            .with_context(|| format!("could not remove stale admin socket {}", path.display())),
    }
}
