use std::path::Path;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum DedicatedAdminRequest {
    Announce {
        text: String,
    },
    Shutdown {
        reason: String,
    },
    /// Jump the day/night clock to the given seconds-of-day. Caller is
    /// responsible for parsing `HH:MM` etc. before serialising.
    SetTime {
        seconds_of_day: f32,
    },
    /// Change the day/night cycle speed. Clamped server-side to the safe
    /// multiplier range.
    SetTimeMultiplier {
        multiplier: f32,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DedicatedAdminResponse {
    pub ok: bool,
    pub message: String,
}

pub fn send_admin_request(
    socket_path: &Path,
    request: DedicatedAdminRequest,
) -> Result<DedicatedAdminResponse> {
    send_admin_request_platform(socket_path, request)
}

#[cfg(unix)]
fn send_admin_request_platform(
    socket_path: &Path,
    request: DedicatedAdminRequest,
) -> Result<DedicatedAdminResponse> {
    use std::{io::Write, net::Shutdown, os::unix::net::UnixStream, time::Duration};

    let mut stream = UnixStream::connect(socket_path)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    serde_json::to_writer(&mut stream, &request)?;
    stream.write_all(b"\n")?;
    stream.shutdown(Shutdown::Write)?;

    let response: DedicatedAdminResponse = serde_json::from_reader(stream)?;
    if response.ok {
        Ok(response)
    } else {
        bail!("{}", response.message);
    }
}

#[cfg(not(unix))]
fn send_admin_request_platform(
    _socket_path: &Path,
    _request: DedicatedAdminRequest,
) -> Result<DedicatedAdminResponse> {
    bail!("dedicated server admin sockets require a Unix-like OS")
}
