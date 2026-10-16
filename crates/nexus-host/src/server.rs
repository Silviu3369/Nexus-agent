use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

use futures::StreamExt;
use nexus_protocol::service::NexusService;
use nexus_protocol::{
    AddMemoryRequest, AgentStepView, ArtifactId, BackupModelView, BackupSummary, ConnectionId,
    ConnectionView, ConversationId, ConversationSummary, ConversationView, DecideApprovalRequest,
    DeletedView, ErrorCode, EventsPage, GetHealthResponse, HandshakeRequest, HandshakeResponse,
    HealthStatus, McpServerId, McpServerView, MemoryContextView, MemoryId, MemoryItemView,
    MemoryQuery, MessageId, ModelView, NexusError, PROTOCOL_VERSION, PersonaView, ProjectId,
    ProjectView, RunId, RunView, SaveConnectionRequest, SaveMcpServerRequest, SavePersonaRequest,
    SaveProjectRequest, SendMessageRequest, SendMessageResponse, Snapshot, SubmitNoteRequest,
    SubmitNoteResponse, SystemStatusView, ThinkingLevel,
};
use tarpc::context::Context;
use tarpc::server::{BaseChannel, Channel};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio_serde::formats::Json;
use tokio_util::codec::LengthDelimitedCodec;

use crate::runtime::Runtime;

/// IPC-02: reject a message above this size before it is ever deserialized.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// `CreateNamedPipe` with `first_pipe_instance(true)` fails with one of these
/// when the name is already owned — by another host (LIFE-02) or by any other
/// process squatting the name. Either way this host must not start.
const ERROR_ACCESS_DENIED: i32 = 5;
const ERROR_PIPE_BUSY: i32 = 231;

/// Request handler for one connection. tarpc clones it per call, so the
/// `authenticated` flag is shared by the calls of a single connection and
/// never across clients.
#[derive(Clone)]
pub struct HostServer {
    pairing_token: String,
    started_at: Instant,
    authenticated: Arc<AtomicBool>,
    runtime: Arc<Runtime>,
}

impl HostServer {
    pub fn for_connection(
        pairing_token: String,
        started_at: Instant,
        runtime: Arc<Runtime>,
    ) -> Self {
        Self {
            pairing_token,
            started_at,
            authenticated: Arc::new(AtomicBool::new(false)),
            runtime,
        }
    }

    /// Every call except `handshake` requires a successful handshake on this
    /// same connection first (IPC-02: refuse before any privileged command).
    fn require_session(&self) -> Result<(), NexusError> {
        if self.authenticated.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(NexusError::new(
                ErrorCode::Unauthenticated,
                "handshake required before this call",
            ))
        }
    }
}

/// Constant-time comparison so response timing does not reveal how much of a
/// guessed token was right. Length is not secret (tokens are UUIDs).
fn tokens_match(presented: &str, expected: &str) -> bool {
    let (a, b) = (presented.as_bytes(), expected.as_bytes());
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

impl NexusService for HostServer {
    async fn handshake(
        self,
        _ctx: Context,
        request: HandshakeRequest,
    ) -> Result<HandshakeResponse, NexusError> {
        // A failed handshake also revokes any earlier success on this
        // connection, so a client cannot keep a session after a bad attempt.
        self.authenticated.store(false, Ordering::Release);

        if !tokens_match(&request.pairing_token, &self.pairing_token) {
            tracing::warn!("rejected handshake with invalid pairing token");
            return Err(NexusError::new(
                ErrorCode::Unauthenticated,
                "pairing token invalid",
            ));
        }
        if request.client_protocol_version != PROTOCOL_VERSION {
            tracing::warn!(
                client_version = request.client_protocol_version,
                host_version = PROTOCOL_VERSION,
                "rejected handshake with incompatible protocol version"
            );
            return Err(NexusError::new(
                ErrorCode::UnsupportedVersion,
                format!(
                    "client protocol {} incompatible with host protocol {}",
                    request.client_protocol_version, PROTOCOL_VERSION
                ),
            ));
        }
        self.authenticated.store(true, Ordering::Release);
        tracing::info!(client = %request.client_name, "handshake accepted");
        Ok(HandshakeResponse {
            request_id: request.request_id,
            server_protocol_version: PROTOCOL_VERSION,
            host_identity: "nexus-host".to_string(),
            compatible: true,
        })
    }

    async fn get_health(self, _ctx: Context) -> Result<GetHealthResponse, NexusError> {
        self.require_session()?;
        Ok(GetHealthResponse {
            status: if self.runtime.is_paused() {
                HealthStatus::Paused
            } else {
                HealthStatus::Ok
            },
            uptime_seconds: u32::try_from(self.started_at.elapsed().as_secs()).unwrap_or(u32::MAX),
        })
    }

    async fn submit_note(
        self,
        _ctx: Context,
        request: SubmitNoteRequest,
    ) -> Result<SubmitNoteResponse, NexusError> {
        self.require_session()?;
        self.runtime.submit_note(request).await
    }

    async fn get_snapshot(self, _ctx: Context, limit: u32) -> Result<Snapshot, NexusError> {
        self.require_session()?;
        self.runtime.snapshot(limit).await
    }

    async fn get_run(self, _ctx: Context, run_id: RunId) -> Result<RunView, NexusError> {
        self.require_session()?;
        self.runtime.run(run_id).await
    }

    async fn decide_approval(
        self,
        _ctx: Context,
        request: DecideApprovalRequest,
    ) -> Result<RunView, NexusError> {
        self.require_session()?;
        self.runtime.decide(request).await
    }

    async fn cancel_run(self, _ctx: Context, run_id: RunId) -> Result<RunView, NexusError> {
        self.require_session()?;
        self.runtime.cancel(run_id).await
    }

    async fn get_events(
        self,
        _ctx: Context,
        after: i64,
        limit: u32,
    ) -> Result<EventsPage, NexusError> {
        self.require_session()?;
        self.runtime.events(after, limit).await
    }

    async fn get_artifact_content(
        self,
        _ctx: Context,
        artifact_id: ArtifactId,
    ) -> Result<String, NexusError> {
        self.require_session()?;
        self.runtime.artifact_content(artifact_id).await
    }

    async fn create_backup(self, _ctx: Context) -> Result<BackupSummary, NexusError> {
        self.require_session()?;
        self.runtime.create_backup().await
    }

    async fn list_connections(self, _ctx: Context) -> Result<Vec<ConnectionView>, NexusError> {
        self.require_session()?;
        self.runtime.list_connections().await
    }

    async fn save_connection(
        self,
        _ctx: Context,
        request: SaveConnectionRequest,
    ) -> Result<ConnectionView, NexusError> {
        self.require_session()?;
        self.runtime.save_connection(request).await
    }

    async fn get_backup_model(self, _ctx: Context) -> Result<Option<BackupModelView>, NexusError> {
        self.require_session()?;
        self.runtime.get_backup_model().await
    }

    async fn set_backup_model(
        self,
        _ctx: Context,
        connection_id: Option<ConnectionId>,
        model: Option<String>,
    ) -> Result<Option<BackupModelView>, NexusError> {
        self.require_session()?;
        self.runtime.set_backup_model(connection_id, model).await
    }

    async fn remove_connection(
        self,
        _ctx: Context,
        connection_id: ConnectionId,
    ) -> Result<(), NexusError> {
        self.require_session()?;
        self.runtime.remove_connection(connection_id).await
    }

    async fn list_models(
        self,
        _ctx: Context,
        connection_id: ConnectionId,
    ) -> Result<Vec<ModelView>, NexusError> {
        self.require_session()?;
        self.runtime.list_models(connection_id).await
    }

    async fn select_model(
        self,
        _ctx: Context,
        connection_id: ConnectionId,
        model: String,
    ) -> Result<ConnectionView, NexusError> {
        self.require_session()?;
        self.runtime.select_model(connection_id, model).await
    }

    async fn set_thinking_level(
        self,
        _ctx: Context,
        connection_id: ConnectionId,
        level: ThinkingLevel,
    ) -> Result<ConnectionView, NexusError> {
        self.require_session()?;
        self.runtime.set_thinking_level(connection_id, level).await
    }

    async fn get_persona(self, _ctx: Context) -> Result<PersonaView, NexusError> {
        self.require_session()?;
        self.runtime.get_persona().await
    }

    async fn save_persona(
        self,
        _ctx: Context,
        request: SavePersonaRequest,
    ) -> Result<PersonaView, NexusError> {
        self.require_session()?;
        self.runtime.save_persona(request).await
    }

    async fn list_conversations(
        self,
        _ctx: Context,
        limit: u32,
    ) -> Result<Vec<ConversationSummary>, NexusError> {
        self.require_session()?;
        self.runtime.list_conversations(limit).await
    }

    async fn get_conversation(
        self,
        _ctx: Context,
        conversation_id: ConversationId,
    ) -> Result<ConversationView, NexusError> {
        self.require_session()?;
        self.runtime.get_conversation(conversation_id).await
    }

    async fn send_message(
        self,
        _ctx: Context,
        request: SendMessageRequest,
    ) -> Result<SendMessageResponse, NexusError> {
        self.require_session()?;
        self.runtime.send_message(request).await
    }

    async fn cancel_reply(self, _ctx: Context, message_id: MessageId) -> Result<(), NexusError> {
        self.require_session()?;
        self.runtime.cancel_reply(message_id).await
    }

    async fn delete_conversation(
        self,
        _ctx: Context,
        conversation_id: ConversationId,
    ) -> Result<DeletedView, NexusError> {
        self.require_session()?;
        self.runtime.delete_conversation(conversation_id).await
    }

    async fn delete_exchange(
        self,
        _ctx: Context,
        message_id: MessageId,
    ) -> Result<DeletedView, NexusError> {
        self.require_session()?;
        self.runtime.delete_exchange(message_id).await
    }

    async fn get_activity(
        self,
        _ctx: Context,
        limit: u32,
    ) -> Result<Vec<AgentStepView>, NexusError> {
        self.require_session()?;
        self.runtime.get_activity(limit).await
    }

    async fn list_projects(self, _ctx: Context) -> Result<Vec<ProjectView>, NexusError> {
        self.require_session()?;
        self.runtime.list_projects().await
    }

    async fn save_project(
        self,
        _ctx: Context,
        request: SaveProjectRequest,
    ) -> Result<ProjectView, NexusError> {
        self.require_session()?;
        self.runtime.save_project(request).await
    }

    async fn set_active_project(
        self,
        _ctx: Context,
        project_id: ProjectId,
    ) -> Result<ProjectView, NexusError> {
        self.require_session()?;
        self.runtime.set_active_project(project_id).await
    }

    async fn list_memory(
        self,
        _ctx: Context,
        query: MemoryQuery,
    ) -> Result<Vec<MemoryItemView>, NexusError> {
        self.require_session()?;
        self.runtime.list_memory(query).await
    }

    async fn add_memory(
        self,
        _ctx: Context,
        request: AddMemoryRequest,
    ) -> Result<MemoryItemView, NexusError> {
        self.require_session()?;
        self.runtime.add_memory(request).await
    }

    async fn edit_memory(
        self,
        _ctx: Context,
        memory_id: MemoryId,
        revision: i64,
        content: String,
    ) -> Result<MemoryItemView, NexusError> {
        self.require_session()?;
        self.runtime.edit_memory(memory_id, revision, content).await
    }

    async fn decide_memory(
        self,
        _ctx: Context,
        memory_id: MemoryId,
        revision: i64,
        approve: bool,
    ) -> Result<MemoryItemView, NexusError> {
        self.require_session()?;
        self.runtime
            .decide_memory(memory_id, revision, approve)
            .await
    }

    async fn delete_memory(self, _ctx: Context, memory_id: MemoryId) -> Result<(), NexusError> {
        self.require_session()?;
        self.runtime.delete_memory(memory_id).await
    }

    async fn memory_context(
        self,
        _ctx: Context,
        project_id: ProjectId,
    ) -> Result<MemoryContextView, NexusError> {
        self.require_session()?;
        self.runtime.memory_context(project_id).await
    }

    async fn list_mcp_servers(self, _ctx: Context) -> Result<Vec<McpServerView>, NexusError> {
        self.require_session()?;
        self.runtime.list_mcp_servers().await
    }

    async fn save_mcp_server(
        self,
        _ctx: Context,
        request: SaveMcpServerRequest,
    ) -> Result<McpServerView, NexusError> {
        self.require_session()?;
        self.runtime.save_mcp_server(request).await
    }

    async fn refresh_mcp_tools(
        self,
        _ctx: Context,
        server_id: McpServerId,
    ) -> Result<McpServerView, NexusError> {
        self.require_session()?;
        self.runtime.refresh_mcp_tools(server_id).await
    }

    async fn remove_mcp_server(
        self,
        _ctx: Context,
        server_id: McpServerId,
    ) -> Result<(), NexusError> {
        self.require_session()?;
        self.runtime.remove_mcp_server(server_id).await
    }

    async fn set_mcp_tool_enabled(
        self,
        _ctx: Context,
        server_id: McpServerId,
        tool: String,
        definition_sha256: Option<String>,
    ) -> Result<McpServerView, NexusError> {
        self.require_session()?;
        self.runtime
            .set_mcp_tool_enabled(server_id, tool, definition_sha256)
            .await
    }

    async fn get_system_status(self, _ctx: Context) -> Result<SystemStatusView, NexusError> {
        self.require_session()?;
        Ok(self.runtime.system_status().await)
    }

    async fn stop_host(self, _ctx: Context) -> Result<(), NexusError> {
        self.require_session()?;
        tracing::info!("explicit stop requested");
        self.runtime.request_stop();
        Ok(())
    }
}

/// Binds the first instance of the named pipe. Fails if another host already
/// owns this pipe name (LIFE-02) instead of silently becoming a second queue
/// owner.
pub fn bind_first_instance(pipe_name: &str) -> anyhow::Result<NamedPipeServer> {
    match ServerOptions::new()
        .first_pipe_instance(true)
        .create(pipe_name)
    {
        Ok(pipe) => Ok(pipe),
        Err(err)
            if matches!(
                err.raw_os_error(),
                Some(ERROR_ACCESS_DENIED) | Some(ERROR_PIPE_BUSY)
            ) =>
        {
            anyhow::bail!(
                "pipe {pipe_name} is already owned by another process (another nexus-host for this profile, or a process squatting the name); refusing to start a second queue owner"
            );
        }
        Err(err) => Err(err.into()),
    }
}

/// Runs the accept loop until the runtime is asked to stop. `listener` must
/// come from `bind_first_instance` so the single-instance guarantee holds for
/// the life of the process.
pub async fn serve_forever(
    mut listener: NamedPipeServer,
    pipe_name: String,
    pairing_token: String,
    runtime: Arc<Runtime>,
) -> anyhow::Result<()> {
    let started_at = Instant::now();
    let mut stop = runtime.stop_signal();

    loop {
        tokio::select! {
            connected = listener.connect() => connected?,
            _ = stop.changed() => break,
        }
        let connected = listener;
        // Open the next instance immediately so a second client can queue
        // while this one is served.
        listener = ServerOptions::new().create(&pipe_name)?;

        let server = HostServer::for_connection(pairing_token.clone(), started_at, runtime.clone());

        tokio::spawn(async move {
            if let Err(err) = serve_connection(connected, server).await {
                tracing::warn!(error = %err, "connection ended with error");
            }
        });
    }
    tracing::info!("stopped accepting connections");
    Ok(())
}

async fn serve_connection(pipe: NamedPipeServer, server: HostServer) -> io::Result<()> {
    let codec = LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec();
    let framed = tokio_util::codec::Framed::new(pipe, codec);
    let transport = tarpc::serde_transport::new(framed, Json::default());
    let channel = BaseChannel::with_defaults(transport);

    channel
        .execute(server.serve())
        .for_each(|response_future| async move {
            tokio::spawn(response_future);
        })
        .await;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::tokens_match;

    #[test]
    fn token_comparison_requires_exact_match() {
        assert!(tokens_match("3f2a-token", "3f2a-token"));
        assert!(!tokens_match("3f2a-tokem", "3f2a-token"));
        assert!(!tokens_match("3f2a", "3f2a-token"));
        assert!(!tokens_match("", "3f2a-token"));
    }
}
