//! The RPC contract itself, generated from a single Rust trait
//! (ADR-IPC-001: tarpc instead of a separate schema language). `nexus-host`
//! implements this; `nexus-client` calls the generated client stub.

use crate::chat::{
    ConnectionView, ConversationSummary, ConversationView, ModelView, PersonaView,
    SaveGeminiConnectionRequest, SavePersonaRequest, SendMessageRequest, SendMessageResponse,
};
use crate::error::NexusError;
use crate::handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse};
use crate::ids::{ArtifactId, ConnectionId, ConversationId, MessageId, RunId};
use crate::runs::{
    BackupSummary, DecideApprovalRequest, EventsPage, RunView, Snapshot, SubmitNoteRequest,
    SubmitNoteResponse,
};

#[tarpc::service]
pub trait NexusService {
    /// First call on a new connection. Negotiates protocol version and
    /// identity; carries no secrets and no actor claim.
    async fn handshake(request: HandshakeRequest) -> Result<HandshakeResponse, NexusError>;

    /// Liveness/identity probe used after handshake (`CONTRACTE_NEXUS_RO.md` §8).
    async fn get_health() -> Result<GetHealthResponse, NexusError>;

    /// Persists the request and queues a run. Nothing is written yet: the
    /// host first prepares an exact preview that must be approved.
    async fn submit_note(request: SubmitNoteRequest) -> Result<SubmitNoteResponse, NexusError>;

    async fn get_snapshot(limit: u32) -> Result<Snapshot, NexusError>;

    async fn get_run(run_id: RunId) -> Result<RunView, NexusError>;

    /// Bound to the manifest hash the client previewed (`CONTRACTE_NEXUS_RO.md` §5).
    async fn decide_approval(request: DecideApprovalRequest) -> Result<RunView, NexusError>;

    async fn cancel_run(run_id: RunId) -> Result<RunView, NexusError>;

    async fn get_events(after: i64, limit: u32) -> Result<EventsPage, NexusError>;

    /// Content of a verified artifact, re-checked against its recorded hash.
    async fn get_artifact_content(artifact_id: ArtifactId) -> Result<String, NexusError>;

    /// Consistent snapshot of the database and the notes it references,
    /// with a manifest of hashes (BACKUP-01). Writes wait while it runs.
    async fn create_backup() -> Result<BackupSummary, NexusError>;

    /// Connections with their key redacted to `has_key`.
    async fn list_connections() -> Result<Vec<ConnectionView>, NexusError>;

    /// Stores the key in the Windows vault (never in the database), then
    /// checks it by listing models — a real call to Google, made only here.
    async fn save_gemini_connection(
        request: SaveGeminiConnectionRequest,
    ) -> Result<ConnectionView, NexusError>;

    /// Deletes the connection and its vault entry.
    async fn remove_connection(connection_id: ConnectionId) -> Result<(), NexusError>;

    /// Models the provider reports for this connection (a real call).
    async fn list_models(connection_id: ConnectionId) -> Result<Vec<ModelView>, NexusError>;

    /// Makes this connection and model the ones used for chat. There is no
    /// automatic fallback to another connection or provider.
    async fn select_model(
        connection_id: ConnectionId,
        model: String,
    ) -> Result<ConnectionView, NexusError>;

    async fn get_persona() -> Result<PersonaView, NexusError>;

    /// Saves a new persona version. It changes tone and style, never rights.
    async fn save_persona(request: SavePersonaRequest) -> Result<PersonaView, NexusError>;

    async fn list_conversations(limit: u32) -> Result<Vec<ConversationSummary>, NexusError>;

    async fn get_conversation(
        conversation_id: ConversationId,
    ) -> Result<ConversationView, NexusError>;

    /// Records the user's message and starts the reply; returns at once.
    async fn send_message(request: SendMessageRequest) -> Result<SendMessageResponse, NexusError>;

    /// Stops a reply in progress; what arrived so far is kept, marked cancelled.
    async fn cancel_reply(message_id: MessageId) -> Result<(), NexusError>;

    /// Explicit stop: no new work is taken, the current step finishes,
    /// uncertain effects stay recorded for reconciliation on next start.
    async fn stop_host() -> Result<(), NexusError>;
}
