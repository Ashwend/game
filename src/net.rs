mod channels;
mod client;
mod dedicated;
mod host;

pub(crate) use channels::LightyearProtocolPlugin;
pub use client::ClientSession;
pub(crate) use client::{ClientNetwork, ClientNetworkPlugin, client_plugins};
pub use dedicated::{
    DedicatedAdminRequest, DedicatedWorldPersistence, run_dedicated_server,
    send_admin_request as send_dedicated_admin_request,
};

#[cfg(test)]
mod tests;
