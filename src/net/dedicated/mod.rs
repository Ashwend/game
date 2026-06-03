mod admin;

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use anyhow::Result;

use crate::{
    auth::{AuthMode, WorkosVerifier},
    save::{WorldSave, WorldStore, save_world_file},
};

use super::host::{AutoSaveSink, run_game_server};

pub use admin::{DedicatedAdminRequest, DedicatedAdminResponse, send_admin_request};

#[derive(Debug, Clone)]
pub enum DedicatedWorldPersistence {
    Store(WorldStore),
    File(PathBuf),
}

impl DedicatedWorldPersistence {
    fn save(&self, world: &WorldSave) -> Result<()> {
        match self {
            Self::Store(store) => store.save_world(world),
            Self::File(path) => save_world_file(path, world),
        }
    }
}

pub fn run_dedicated_server(
    bind_addr: SocketAddr,
    save: WorldSave,
    auth_mode: AuthMode,
    workos: Option<Arc<WorkosVerifier>>,
    persistence: DedicatedWorldPersistence,
    admin_socket: Option<PathBuf>,
) -> Result<()> {
    // Periodic auto-save writes through the same persistence target as the
    // final shutdown save, so a crash mid-session loses at most one interval.
    let auto_save_persistence = persistence.clone();
    let auto_save = AutoSaveSink(Box::new(move |world: &WorldSave| {
        auto_save_persistence.save(world)
    }));
    let final_save = run_game_server(
        bind_addr,
        save,
        auth_mode,
        workos,
        admin_socket,
        Some(auto_save),
    )?;
    persistence.save(&final_save)
}
