//! IPC client reused by the Tauri shell (`ARHITECTURA_NEXUS_RO.md` §5).
//! Connects to `nexus-host` over the same named pipe + framed-JSON tarpc
//! transport the host serves, and presents the local pairing token.

mod server_identity;

use nexus_protocol::service::NexusServiceClient;
use nexus_protocol::{
    ArtifactId, DecideApprovalRequest, EventsPage, HandshakeRequest, HandshakeResponse, NexusError,
    PROTOCOL_VERSION, RunId, RunView, Snapshot, SubmitNoteRequest, SubmitNoteResponse, pairing,
};
use tarpc::context;
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};
use tokio_serde::formats::Json;
use tokio_util::codec::LengthDelimitedCodec;

const MAX_FRAME_BYTES: usize = 1024 * 1024;

const ERROR_FILE_NOT_FOUND: i32 = 2;
const ERROR_PIPE_BUSY: i32 = 231;
/// The host always keeps one free pipe instance open, so "busy" only lasts
/// the instant between two accepts; ~1 s of retries covers it.
const BUSY_RETRIES: u32 = 20;
const BUSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(50);

/// Calls that wait for the AI provider (key check, model list) get longer
/// than tarpc's default 10 s, which the host also enforces.
pub const PROVIDER_CALL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

fn provider_context() -> context::Context {
    let mut ctx = context::current();
    ctx.deadline = std::time::Instant::now() + PROVIDER_CALL_DEADLINE;
    ctx
}

#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    #[error("could not read local pairing token: {0}")]
    MissingPairingToken(#[source] std::io::Error),
    #[error("could not connect to nexus-host pipe: {0}")]
    Pipe(#[source] std::io::Error),
    #[error("host rejected handshake: {0}")]
    HandshakeRejected(#[source] NexusError),
    #[error("pipe server runs as {actual}, expected {expected}; the pairing token was not sent")]
    UntrustedServer { expected: String, actual: String },
    #[error("could not verify the pipe server's identity, so the pairing token was not sent: {0}")]
    ServerIdentityUnknown(#[source] std::io::Error),
}

impl ConnectError {
    /// True when no host is listening for this profile (pipe absent, or the
    /// host has never run so no pairing token exists yet) — as opposed to a
    /// host that exists but failed or refused.
    pub fn host_not_running(&self) -> bool {
        match self {
            ConnectError::MissingPairingToken(err) => err.kind() == std::io::ErrorKind::NotFound,
            ConnectError::Pipe(err) => err.raw_os_error() == Some(ERROR_FILE_NOT_FOUND),
            ConnectError::HandshakeRejected(_)
            | ConnectError::UntrustedServer { .. }
            | ConnectError::ServerIdentityUnknown(_) => false,
        }
    }

    /// True when this client refused to trust the process owning the pipe.
    pub fn untrusted_server(&self) -> bool {
        matches!(
            self,
            ConnectError::UntrustedServer { .. } | ConnectError::ServerIdentityUnknown(_)
        )
    }
}

/// Refuses unless the process serving `pipe` runs as `expected_sid`. Fails
/// closed: if the identity cannot be read, the server is not trusted.
pub fn verify_pipe_server_user(
    pipe: &NamedPipeClient,
    expected_sid: &str,
) -> Result<(), ConnectError> {
    let actual =
        server_identity::pipe_server_user_sid(pipe).map_err(ConnectError::ServerIdentityUnknown)?;
    if actual == expected_sid {
        Ok(())
    } else {
        Err(ConnectError::UntrustedServer {
            expected: expected_sid.to_string(),
            actual,
        })
    }
}

pub use server_identity::current_user_sid;

pub struct NexusClient {
    client: NexusServiceClient,
}

impl NexusClient {
    /// Connects to the host's named pipe for the current Windows account and
    /// performs the handshake. Returns the connected client plus the host's
    /// handshake response (so callers can check `compatible` before use).
    pub async fn connect(
        client_name: impl Into<String>,
    ) -> Result<(Self, HandshakeResponse), ConnectError> {
        let pairing_token = pairing::read_token().map_err(ConnectError::MissingPairingToken)?;
        Self::connect_with(&pairing::pipe_name(), pairing_token, client_name).await
    }

    /// Lower-level entry point used by tests and by anything that must talk
    /// to a host on a non-default pipe name (e.g. an isolated test host).
    pub async fn connect_with(
        pipe_name: &str,
        pairing_token: String,
        client_name: impl Into<String>,
    ) -> Result<(Self, HandshakeResponse), ConnectError> {
        let pipe = open_pipe(pipe_name).await.map_err(ConnectError::Pipe)?;
        let me = current_user_sid().map_err(ConnectError::ServerIdentityUnknown)?;
        verify_pipe_server_user(&pipe, &me)?;

        let codec = LengthDelimitedCodec::builder()
            .max_frame_length(MAX_FRAME_BYTES)
            .new_codec();
        let framed = tokio_util::codec::Framed::new(pipe, codec);
        let transport = tarpc::serde_transport::new(framed, Json::default());

        let client = NexusServiceClient::new(tarpc::client::Config::default(), transport).spawn();

        let request = HandshakeRequest {
            request_id: nexus_protocol::RequestId::new(),
            client_protocol_version: PROTOCOL_VERSION,
            client_name: client_name.into(),
            pairing_token,
        };

        let response = client
            .handshake(context::current(), request)
            .await
            .map_err(|e| ConnectError::Pipe(std::io::Error::other(e)))?
            .map_err(ConnectError::HandshakeRejected)?;

        Ok((Self { client }, response))
    }

    pub async fn get_health(
        &self,
    ) -> Result<Result<nexus_protocol::GetHealthResponse, NexusError>, tarpc::client::RpcError>
    {
        self.client.get_health(context::current()).await
    }

    pub async fn submit_note(
        &self,
        request: SubmitNoteRequest,
    ) -> Result<SubmitNoteResponse, CallError> {
        Ok(self
            .client
            .submit_note(context::current(), request)
            .await??)
    }

    pub async fn snapshot(&self, limit: u32) -> Result<Snapshot, CallError> {
        Ok(self
            .client
            .get_snapshot(context::current(), limit)
            .await??)
    }

    pub async fn run(&self, run_id: RunId) -> Result<RunView, CallError> {
        Ok(self.client.get_run(context::current(), run_id).await??)
    }

    pub async fn decide_approval(
        &self,
        request: DecideApprovalRequest,
    ) -> Result<RunView, CallError> {
        Ok(self
            .client
            .decide_approval(context::current(), request)
            .await??)
    }

    pub async fn cancel_run(&self, run_id: RunId) -> Result<RunView, CallError> {
        Ok(self.client.cancel_run(context::current(), run_id).await??)
    }

    pub async fn events(&self, after: i64, limit: u32) -> Result<EventsPage, CallError> {
        Ok(self
            .client
            .get_events(context::current(), after, limit)
            .await??)
    }

    pub async fn artifact_content(&self, artifact_id: ArtifactId) -> Result<String, CallError> {
        Ok(self
            .client
            .get_artifact_content(context::current(), artifact_id)
            .await??)
    }

    pub async fn list_connections(&self) -> Result<Vec<nexus_protocol::ConnectionView>, CallError> {
        Ok(self.client.list_connections(context::current()).await??)
    }

    pub async fn save_connection(
        &self,
        request: nexus_protocol::SaveConnectionRequest,
    ) -> Result<nexus_protocol::ConnectionView, CallError> {
        Ok(self
            .client
            .save_connection(provider_context(), request)
            .await??)
    }

    pub async fn backup_model(&self) -> Result<Option<nexus_protocol::BackupModelView>, CallError> {
        Ok(self.client.get_backup_model(context::current()).await??)
    }

    pub async fn set_backup_model(
        &self,
        connection_id: Option<nexus_protocol::ConnectionId>,
        model: Option<String>,
    ) -> Result<Option<nexus_protocol::BackupModelView>, CallError> {
        Ok(self
            .client
            .set_backup_model(context::current(), connection_id, model)
            .await??)
    }

    pub async fn remove_connection(
        &self,
        id: nexus_protocol::ConnectionId,
    ) -> Result<(), CallError> {
        Ok(self
            .client
            .remove_connection(context::current(), id)
            .await??)
    }

    pub async fn list_models(
        &self,
        id: nexus_protocol::ConnectionId,
    ) -> Result<Vec<nexus_protocol::ModelView>, CallError> {
        Ok(self.client.list_models(provider_context(), id).await??)
    }

    pub async fn select_model(
        &self,
        id: nexus_protocol::ConnectionId,
        model: String,
    ) -> Result<nexus_protocol::ConnectionView, CallError> {
        Ok(self
            .client
            .select_model(context::current(), id, model)
            .await??)
    }

    pub async fn set_thinking_level(
        &self,
        id: nexus_protocol::ConnectionId,
        level: nexus_protocol::ThinkingLevel,
    ) -> Result<nexus_protocol::ConnectionView, CallError> {
        Ok(self
            .client
            .set_thinking_level(context::current(), id, level)
            .await??)
    }

    pub async fn persona(&self) -> Result<nexus_protocol::PersonaView, CallError> {
        Ok(self.client.get_persona(context::current()).await??)
    }

    pub async fn save_persona(
        &self,
        request: nexus_protocol::SavePersonaRequest,
    ) -> Result<nexus_protocol::PersonaView, CallError> {
        Ok(self
            .client
            .save_persona(context::current(), request)
            .await??)
    }

    pub async fn conversations(
        &self,
        limit: u32,
    ) -> Result<Vec<nexus_protocol::ConversationSummary>, CallError> {
        Ok(self
            .client
            .list_conversations(context::current(), limit)
            .await??)
    }

    pub async fn conversation(
        &self,
        id: nexus_protocol::ConversationId,
    ) -> Result<nexus_protocol::ConversationView, CallError> {
        Ok(self
            .client
            .get_conversation(context::current(), id)
            .await??)
    }

    pub async fn send_message(
        &self,
        request: nexus_protocol::SendMessageRequest,
    ) -> Result<nexus_protocol::SendMessageResponse, CallError> {
        Ok(self
            .client
            .send_message(context::current(), request)
            .await??)
    }

    pub async fn activity(
        &self,
        limit: u32,
    ) -> Result<Vec<nexus_protocol::AgentStepView>, CallError> {
        Ok(self
            .client
            .get_activity(context::current(), limit)
            .await??)
    }

    pub async fn cancel_reply(&self, id: nexus_protocol::MessageId) -> Result<(), CallError> {
        Ok(self.client.cancel_reply(context::current(), id).await??)
    }

    pub async fn delete_conversation(
        &self,
        id: nexus_protocol::ConversationId,
    ) -> Result<nexus_protocol::DeletedView, CallError> {
        Ok(self
            .client
            .delete_conversation(context::current(), id)
            .await??)
    }

    pub async fn delete_exchange(
        &self,
        id: nexus_protocol::MessageId,
    ) -> Result<nexus_protocol::DeletedView, CallError> {
        Ok(self
            .client
            .delete_exchange(context::current(), id)
            .await??)
    }

    pub async fn create_backup(&self) -> Result<nexus_protocol::BackupSummary, CallError> {
        Ok(self.client.create_backup(context::current()).await??)
    }

    pub async fn projects(&self) -> Result<Vec<nexus_protocol::ProjectView>, CallError> {
        Ok(self.client.list_projects(context::current()).await??)
    }

    pub async fn save_project(
        &self,
        request: nexus_protocol::SaveProjectRequest,
    ) -> Result<nexus_protocol::ProjectView, CallError> {
        Ok(self
            .client
            .save_project(context::current(), request)
            .await??)
    }

    pub async fn set_active_project(
        &self,
        id: nexus_protocol::ProjectId,
    ) -> Result<nexus_protocol::ProjectView, CallError> {
        Ok(self
            .client
            .set_active_project(context::current(), id)
            .await??)
    }

    pub async fn memory(
        &self,
        query: nexus_protocol::MemoryQuery,
    ) -> Result<Vec<nexus_protocol::MemoryItemView>, CallError> {
        Ok(self.client.list_memory(context::current(), query).await??)
    }

    pub async fn add_memory(
        &self,
        request: nexus_protocol::AddMemoryRequest,
    ) -> Result<nexus_protocol::MemoryItemView, CallError> {
        Ok(self
            .client
            .add_memory(context::current(), request)
            .await??)
    }

    pub async fn edit_memory(
        &self,
        id: nexus_protocol::MemoryId,
        revision: i64,
        content: String,
    ) -> Result<nexus_protocol::MemoryItemView, CallError> {
        Ok(self
            .client
            .edit_memory(context::current(), id, revision, content)
            .await??)
    }

    pub async fn decide_memory(
        &self,
        id: nexus_protocol::MemoryId,
        revision: i64,
        approve: bool,
    ) -> Result<nexus_protocol::MemoryItemView, CallError> {
        Ok(self
            .client
            .decide_memory(context::current(), id, revision, approve)
            .await??)
    }

    pub async fn delete_memory(&self, id: nexus_protocol::MemoryId) -> Result<(), CallError> {
        Ok(self.client.delete_memory(context::current(), id).await??)
    }

    pub async fn memory_context(
        &self,
        project: nexus_protocol::ProjectId,
    ) -> Result<nexus_protocol::MemoryContextView, CallError> {
        Ok(self
            .client
            .memory_context(context::current(), project)
            .await??)
    }

    pub async fn mcp_servers(&self) -> Result<Vec<nexus_protocol::McpServerView>, CallError> {
        Ok(self.client.list_mcp_servers(context::current()).await??)
    }

    /// Saving lists the server's tools, so it waits like a provider call.
    pub async fn save_mcp_server(
        &self,
        request: nexus_protocol::SaveMcpServerRequest,
    ) -> Result<nexus_protocol::McpServerView, CallError> {
        Ok(self
            .client
            .save_mcp_server(provider_context(), request)
            .await??)
    }

    pub async fn refresh_mcp_tools(
        &self,
        id: nexus_protocol::McpServerId,
    ) -> Result<nexus_protocol::McpServerView, CallError> {
        Ok(self
            .client
            .refresh_mcp_tools(provider_context(), id)
            .await??)
    }

    pub async fn remove_mcp_server(
        &self,
        id: nexus_protocol::McpServerId,
    ) -> Result<(), CallError> {
        Ok(self
            .client
            .remove_mcp_server(context::current(), id)
            .await??)
    }

    /// `definition_sha256` is the definition the user looked at; `None`
    /// turns the tool off.
    pub async fn set_mcp_tool_enabled(
        &self,
        id: nexus_protocol::McpServerId,
        tool: String,
        definition_sha256: Option<String>,
    ) -> Result<nexus_protocol::McpServerView, CallError> {
        Ok(self
            .client
            .set_mcp_tool_enabled(context::current(), id, tool, definition_sha256)
            .await??)
    }

    pub async fn system_status(&self) -> Result<nexus_protocol::SystemStatusView, CallError> {
        Ok(self.client.get_system_status(context::current()).await??)
    }

    pub async fn stop_host(&self) -> Result<(), CallError> {
        Ok(self.client.stop_host(context::current()).await??)
    }
}

/// A call either never got a usable answer (transport) or the host answered
/// with a refusal that has a stable code.
#[derive(Debug, thiserror::Error)]
pub enum CallError {
    #[error("transport: {0}")]
    Transport(#[from] tarpc::client::RpcError),
    #[error("{0}")]
    Host(#[from] NexusError),
}

impl CallError {
    pub fn code(&self) -> Option<nexus_protocol::ErrorCode> {
        match self {
            CallError::Host(err) => Some(err.code),
            CallError::Transport(_) => None,
        }
    }
}

async fn open_pipe(
    pipe_name: &str,
) -> std::io::Result<tokio::net::windows::named_pipe::NamedPipeClient> {
    let mut attempt = 0;
    loop {
        match ClientOptions::new().open(pipe_name) {
            Err(err) if err.raw_os_error() == Some(ERROR_PIPE_BUSY) && attempt < BUSY_RETRIES => {
                attempt += 1;
                tokio::time::sleep(BUSY_RETRY_DELAY).await;
            }
            result => return result,
        }
    }
}
