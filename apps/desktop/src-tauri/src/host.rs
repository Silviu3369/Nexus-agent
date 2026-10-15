//! Narrow bridge between the webview and `nexus-host`. The renderer can only
//! ask for the host's status and ask the shell to launch the host binary
//! that ships next to it — no paths, arguments or commands come from JS.

use std::io;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;

use nexus_client::{CallError, ConnectError, NexusClient};
use nexus_protocol::{ErrorCode, GetHealthResponse, HandshakeResponse, HealthStatus, NexusError};
use serde::Serialize;
use tauri::async_runtime::Mutex;

/// What a bridge command reports to the webview: the host's stable code when
/// the host refused, none when the call never got an answer.
#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct BridgeError {
    pub code: Option<ErrorCode>,
    pub message: String,
}

impl From<CallError> for BridgeError {
    fn from(err: CallError) -> Self {
        match err {
            CallError::Host(e) => Self {
                code: Some(e.code),
                message: e.message,
            },
            CallError::Transport(e) => Self {
                code: None,
                message: e.to_string(),
            },
        }
    }
}

impl From<ConnectError> for BridgeError {
    fn from(err: ConnectError) -> Self {
        match err {
            ConnectError::HandshakeRejected(e) => Self {
                code: Some(e.code),
                message: e.message,
            },
            other => Self {
                code: None,
                message: other.to_string(),
            },
        }
    }
}

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
        /// A restored profile: requests are recorded but nothing executes.
        paused: bool,
    },
    /// No host listens for this profile (or it has never run).
    NotRunning,
    /// A host exists but the connection or call failed.
    Unreachable { reason: String },
    /// The host answered and refused us (pairing token or protocol version).
    Rejected { error: NexusError },
    /// We refused the process owning the pipe: it does not run as this
    /// Windows user (or its identity could not be checked). No token was sent.
    Untrusted { reason: String },
}

struct Session {
    client: NexusClient,
    handshake: HandshakeResponse,
}

/// The lock guards only which session is current; calls run on a shared
/// handle, so a slow one (a provider key check) does not block the others.
#[derive(Default)]
pub struct HostConnection(Mutex<Option<Arc<Session>>>);

impl HostConnection {
    async fn session(&self) -> Result<Arc<Session>, BridgeError> {
        let mut slot = self.0.lock().await;
        if let Some(live) = slot.as_ref() {
            return Ok(Arc::clone(live));
        }
        let (client, handshake) = NexusClient::connect(CLIENT_NAME).await?;
        let live = Arc::new(Session { client, handshake });
        *slot = Some(Arc::clone(&live));
        Ok(live)
    }

    /// Drops `dead` unless another call has already replaced it.
    async fn forget(&self, dead: &Arc<Session>) {
        let mut slot = self.0.lock().await;
        if slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, dead)) {
            *slot = None;
        }
    }

    /// Runs one host call on the live session, connecting first if needed.
    /// A transport failure drops the session so the next call reconnects;
    /// a host refusal keeps it (the connection itself is fine).
    pub async fn call<T>(
        &self,
        f: impl AsyncFnOnce(&NexusClient) -> Result<T, CallError>,
    ) -> Result<T, BridgeError> {
        let live = self.session().await?;
        match f(&live.client).await {
            Err(CallError::Transport(err)) => {
                self.forget(&live).await;
                Err(CallError::Transport(err).into())
            }
            other => other.map_err(BridgeError::from),
        }
    }
}

#[tauri::command]
pub async fn host_status(
    connection: tauri::State<'_, HostConnection>,
) -> Result<HostStatus, String> {
    let existing = connection.0.lock().await.clone();
    if let Some(live) = existing {
        match live.client.get_health().await {
            Ok(Ok(health)) => return Ok(connected(&live.handshake, &health)),
            Ok(Err(error)) => {
                connection.forget(&live).await;
                return Ok(HostStatus::Rejected { error });
            }
            // The host may have restarted: forget the dead session and
            // reconnect below instead of reporting a stale state.
            Err(_) => connection.forget(&live).await,
        }
    }

    let mut session = connection.0.lock().await;
    let (client, handshake) = match NexusClient::connect(CLIENT_NAME).await {
        Ok(pair) => pair,
        Err(err) => return Ok(status_from_connect_error(err)),
    };

    let status = match client.get_health().await {
        Ok(Ok(health)) => connected(&handshake, &health),
        Ok(Err(error)) => return Ok(HostStatus::Rejected { error }),
        Err(err) => {
            return Ok(HostStatus::Unreachable {
                reason: err.to_string(),
            });
        }
    };
    *session = Some(Arc::new(Session { client, handshake }));
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

fn connected(handshake: &HandshakeResponse, health: &GetHealthResponse) -> HostStatus {
    HostStatus::Connected {
        host_identity: handshake.host_identity.clone(),
        protocol_version: handshake.server_protocol_version,
        uptime_seconds: health.uptime_seconds,
        paused: health.status == HealthStatus::Paused,
    }
}

fn status_from_connect_error(err: ConnectError) -> HostStatus {
    if err.host_not_running() {
        return HostStatus::NotRunning;
    }
    if err.untrusted_server() {
        return HostStatus::Untrusted {
            reason: err.to_string(),
        };
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
