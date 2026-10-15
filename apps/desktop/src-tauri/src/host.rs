//! Narrow bridge between the webview and `nexus-host`. The renderer can only
//! ask for the host's status and ask the shell to launch the host binary
//! that ships next to it — no paths, arguments or commands come from JS.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use nexus_client::{ConnectError, NexusClient};
use nexus_protocol::{HandshakeResponse, NexusError};
use serde::Serialize;
use tauri::async_runtime::Mutex;

const CLIENT_NAME: &str = "nexus-desktop";
const HOST_EXE: &str = "nexus-host.exe";

const DETACHED_PROCESS: u32 = 0x0000_0008;
const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_BREAKAWAY_FROM_JOB: u32 = 0x0100_0000;

/// What the dashboard may show about the host. Every variant comes from a
/// real connection attempt; there is no optimistic or cached "connected".
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state")]
pub enum HostStatus {
    Connected {
        host_identity: String,
        protocol_version: u32,
        uptime_seconds: u32,
    },
    /// No host listens for this profile (or it has never run).
    NotRunning,
    /// A host exists but the connection or call failed.
    Unreachable { reason: String },
    /// The host answered and refused us (pairing token or protocol version).
    Rejected { error: NexusError },
}

struct Session {
    client: NexusClient,
    handshake: HandshakeResponse,
}

#[derive(Default)]
pub struct HostConnection(Mutex<Option<Session>>);

#[tauri::command]
pub async fn host_status(
    connection: tauri::State<'_, HostConnection>,
) -> Result<HostStatus, String> {
    let mut session = connection.0.lock().await;

    if let Some(existing) = session.as_ref() {
        match existing.client.get_health().await {
            Ok(Ok(health)) => return Ok(connected(&existing.handshake, health.uptime_seconds)),
            Ok(Err(error)) => {
                *session = None;
                return Ok(HostStatus::Rejected { error });
            }
            // The host may have restarted: forget the dead session and
            // reconnect below instead of reporting a stale state.
            Err(_) => *session = None,
        }
    }

    let (client, handshake) = match NexusClient::connect(CLIENT_NAME).await {
        Ok(pair) => pair,
        Err(err) => return Ok(status_from_connect_error(err)),
    };

    let status = match client.get_health().await {
        Ok(Ok(health)) => connected(&handshake, health.uptime_seconds),
        Ok(Err(error)) => return Ok(HostStatus::Rejected { error }),
        Err(err) => {
            return Ok(HostStatus::Unreachable {
                reason: err.to_string(),
            });
        }
    };
    *session = Some(Session { client, handshake });
    Ok(status)
}

/// Launches the host shipped next to this executable, detached so closing
/// the dashboard never terminates it (ADR-HOST-001). Starting it twice is
/// harmless: the second host fails its single-instance bind and exits.
#[tauri::command]
pub fn start_host() -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot locate dashboard: {e}"))?;
    let host = exe.with_file_name(HOST_EXE);
    if !host.exists() {
        return Err(format!("{HOST_EXE} not found at {}", host.display()));
    }
    spawn_detached(&host).map_err(|e| format!("cannot start {HOST_EXE}: {e}"))
}

fn connected(handshake: &HandshakeResponse, uptime_seconds: u32) -> HostStatus {
    HostStatus::Connected {
        host_identity: handshake.host_identity.clone(),
        protocol_version: handshake.server_protocol_version,
        uptime_seconds,
    }
}

fn status_from_connect_error(err: ConnectError) -> HostStatus {
    if err.host_not_running() {
        return HostStatus::NotRunning;
    }
    match err {
        ConnectError::HandshakeRejected(error) => HostStatus::Rejected { error },
        other => HostStatus::Unreachable {
            reason: other.to_string(),
        },
    }
}

fn spawn_detached(path: &Path) -> io::Result<()> {
    let spawn = |flags: u32| {
        Command::new(path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(flags)
            .spawn()
    };
    let base = DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP;
    // Leave the parent's Job Object when allowed (e.g. under `cargo run`,
    // whose job kills members on exit); fall back when the job forbids it.
    spawn(base | CREATE_BREAKAWAY_FROM_JOB)
        .or_else(|_| spawn(base))
        .map(|_child| ())
}
