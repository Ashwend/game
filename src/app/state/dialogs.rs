use std::{
    net::SocketAddr,
    sync::{Mutex, mpsc::Receiver},
};

use uuid::Uuid;

use crate::{
    net::ClientSession,
    save::WorldSummary,
    world::{MapType, ProceduralMapSize},
};

#[derive(Debug, Clone)]
pub(crate) struct ConfirmationDialog {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) cancel_label: String,
    pub(crate) action: ConfirmationAction,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
}

impl ConfirmationDialog {
    pub(crate) fn delete_world(world_id: Uuid, world_name: &str) -> Self {
        Self {
            title: "Delete World".to_owned(),
            body: format!("Permanently delete \"{world_name}\"? This cannot be undone."),
            confirm_label: "Delete".to_owned(),
            cancel_label: "Cancel".to_owned(),
            action: ConfirmationAction::DeleteWorld { world_id },
            closing: false,
            confirmed: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum ConfirmationAction {
    DeleteWorld { world_id: Uuid },
}

#[derive(Debug, Clone)]
pub(crate) struct NoticeDialog {
    pub(crate) title: String,
    pub(crate) body: String,
    pub(crate) confirm_label: String,
    pub(crate) closing: bool,
}

impl NoticeDialog {
    pub(crate) fn disconnected(reason: impl Into<String>) -> Self {
        Self {
            title: "Disconnected".to_owned(),
            body: reason.into(),
            confirm_label: "OK".to_owned(),
            closing: false,
        }
    }
}

pub(crate) type DirectConnectResult = std::result::Result<(SocketAddr, ClientSession), String>;

pub(crate) struct DirectConnectAttempt {
    pub(crate) receiver: Mutex<Receiver<DirectConnectResult>>,
}

pub(crate) type WorldStartResult = std::result::Result<ClientSession, String>;

pub(crate) struct WorldStartAttempt {
    pub(crate) world_id: Uuid,
    pub(crate) receiver: Mutex<Receiver<WorldStartResult>>,
}

pub(crate) struct DirectConnectDialog {
    pub(crate) host: String,
    pub(crate) port: String,
    pub(crate) error: Option<String>,
    pub(crate) closing: bool,
    pub(crate) attempt: Option<DirectConnectAttempt>,
}

impl DirectConnectDialog {
    pub(crate) fn new(address: &str) -> Self {
        let (host, port) = split_host_port(address);
        Self {
            host,
            port,
            error: None,
            closing: false,
            attempt: None,
        }
    }

    pub(crate) fn is_connecting(&self) -> bool {
        self.attempt.is_some()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CreateWorldDialog {
    pub(crate) name: String,
    pub(crate) map_kind: CreateWorldMapKind,
    pub(crate) procedural_size: ProceduralMapSize,
    pub(crate) seed: String,
    pub(crate) error: Option<String>,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
}

impl Default for CreateWorldDialog {
    fn default() -> Self {
        Self::new()
    }
}

impl CreateWorldDialog {
    pub(crate) fn new() -> Self {
        Self {
            name: "New World".to_owned(),
            map_kind: CreateWorldMapKind::Test,
            procedural_size: ProceduralMapSize::Medium,
            seed: random_seed().to_string(),
            error: None,
            closing: false,
            confirmed: false,
        }
    }

    pub(crate) fn refresh_seed(&mut self) {
        self.seed = random_seed().to_string();
        self.error = None;
    }

    pub(crate) fn selected_map(&self) -> Result<MapType, &'static str> {
        match self.map_kind {
            CreateWorldMapKind::Test => Ok(MapType::Test),
            CreateWorldMapKind::Procedural => {
                let seed = self
                    .seed
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| "Seed must be a whole number.")?;
                Ok(MapType::Procedural {
                    seed,
                    size: self.procedural_size,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CreateWorldMapKind {
    Test,
    Procedural,
}

#[derive(Debug, Clone)]
pub(crate) struct EditWorldDialog {
    pub(crate) world_id: Uuid,
    pub(crate) name: String,
    pub(crate) map: MapType,
    pub(crate) error: Option<String>,
    pub(crate) closing: bool,
    pub(crate) confirmed: bool,
}

impl EditWorldDialog {
    pub(crate) fn new(world: &WorldSummary) -> Self {
        Self {
            world_id: world.id,
            name: world.name.clone(),
            map: world.map.clone(),
            error: None,
            closing: false,
            confirmed: false,
        }
    }
}

fn random_seed() -> u64 {
    let bytes = Uuid::new_v4().into_bytes();
    u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn split_host_port(address: &str) -> (String, String) {
    match address.parse::<std::net::SocketAddr>() {
        Ok(addr) => (addr.ip().to_string(), addr.port().to_string()),
        Err(_) => address
            .rsplit_once(':')
            .map(|(host, port)| {
                (
                    host.trim_matches(['[', ']']).trim().to_owned(),
                    port.trim().to_owned(),
                )
            })
            .unwrap_or_else(|| (address.trim().to_owned(), "7777".to_owned())),
    }
}
