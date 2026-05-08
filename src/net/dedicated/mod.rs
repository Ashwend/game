mod connections;
mod movement;
mod protocol;
mod transport;

use std::{net::SocketAddr, time::Duration};

use anyhow::Result;
use bevy::{diagnostic::DiagnosticsPlugin, prelude::*, state::app::StatesPlugin};
use lightyear::prelude::server::ServerPlugins;
#[cfg(feature = "steam")]
use lightyear::steam::SteamAppExt;

use crate::{protocol::SERVER_TICK_RATE_HZ, save::WorldSave, steam::AuthMode};

use self::{
    connections::handle_connected_client,
    movement::authoritative_movement_system,
    protocol::{LightyearProtocolPlugin, NetworkWorld},
    transport::{handle_new_client, spawn_server_transport, start_server},
};

pub fn run_dedicated_server(
    bind_addr: SocketAddr,
    save: WorldSave,
    auth_mode: AuthMode,
) -> Result<()> {
    let fixed_delta = Duration::from_secs_f64(1.0 / f64::from(SERVER_TICK_RATE_HZ));
    let mut app = App::new();

    #[cfg(feature = "steam")]
    if auth_mode == AuthMode::Steam {
        app.add_steam_resources(transport::steam_app_id());
    }

    app.add_plugins((
        MinimalPlugins,
        StatesPlugin,
        DiagnosticsPlugin,
        ServerPlugins {
            tick_duration: fixed_delta,
        },
    ));
    app.add_plugins(LightyearProtocolPlugin);
    app.insert_resource(NetworkWorld(save.map.world_data()));
    app.add_observer(handle_new_client);
    app.add_observer(handle_connected_client);
    app.add_systems(FixedUpdate, authoritative_movement_system);

    spawn_server_transport(&mut app, bind_addr, auth_mode)?;
    app.add_systems(Startup, start_server);

    println!("lightyear server listening on {bind_addr} ({auth_mode:?})");
    app.run();
    Ok(())
}
