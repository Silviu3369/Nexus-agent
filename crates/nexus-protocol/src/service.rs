//! The RPC contract itself, generated from a single Rust trait
//! (ADR-IPC-001: tarpc instead of a separate schema language). `nexus-host`
//! implements this; `nexus-client` calls the generated client stub.

use crate::automation::{AutomationRunView, AutomationView, SaveAutomationRequest};
use crate::chat::{
    AgentStepView, BackupModelView, ConnectionView, ConversationSummary, ConversationView,
    DeletedView, ModelView, PersonaView, SaveConnectionRequest, SavePersonaRequest,
    SendMessageRequest, SendMessageResponse, ThinkingLevel,
};
use crate::error::NexusError;
use crate::files::{FolderEntryView, FolderView, SaveFolderRequest};
use crate::handshake::{GetHealthResponse, HandshakeRequest, HandshakeResponse};
use crate::ids::{
    ArtifactId, AutomationId, ConnectionId, ConversationId, FolderId, McpServerId, MemoryId,
    MessageId, ProjectId, RecordId, RegisterId, RunId,
};
use crate::mcp::{McpServerView, SaveMcpServerRequest};
use crate::memory::{
    AddMemoryRequest, MemoryContextView, MemoryItemView, MemoryQuery, ProjectView,
    SaveProjectRequest,
};
use crate::records::{
    ExportView, ImportPreviewView, ImportRequest, ImportResultView, RecordDetailView, RecordQuery,
    RecordView, RegisterView, SaveRecordRequest,
};
use crate::runs::{
    BackupSummary, DecideApprovalRequest, EventsPage, RunView, Snapshot, SubmitNoteRequest,
    SubmitNoteResponse, SystemStatusView,
};
use crate::usage::{ModelPrice, UsageView};

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

    /// Reads the provider from the key's format (or the user's choice),
    /// checks the key with that provider only by listing its models — a
    /// real call — then stores it in the Windows vault (never in the
    /// database). A key the provider refuses is not kept.
    async fn save_connection(request: SaveConnectionRequest) -> Result<ConnectionView, NexusError>;

    async fn get_backup_model() -> Result<Option<BackupModelView>, NexusError>;

    /// Chooses (or with `None`, clears) the model used only when the active
    /// one stays unavailable after its retries.
    async fn set_backup_model(
        connection_id: Option<ConnectionId>,
        model: Option<String>,
    ) -> Result<Option<BackupModelView>, NexusError>;

    /// Deletes the connection and its vault entry.
    async fn remove_connection(connection_id: ConnectionId) -> Result<(), NexusError>;

    /// Models the provider reports for this connection (a real call).
    async fn list_models(connection_id: ConnectionId) -> Result<Vec<ModelView>, NexusError>;

    /// Makes this connection and model the ones used for chat. The only
    /// fallback is the backup model the user chose, if any.
    async fn select_model(
        connection_id: ConnectionId,
        model: String,
    ) -> Result<ConnectionView, NexusError>;

    /// Sets the speed/quality choice for this connection's model; each
    /// provider maps it to its own reasoning control, and a model that
    /// refuses it is asked without it.
    async fn set_thinking_level(
        connection_id: ConnectionId,
        level: ThinkingLevel,
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

    /// Deletes a task with its messages and steps, for good. Refused while
    /// a reply is being written. Facts it proposed and notes it wrote stay.
    async fn delete_conversation(
        conversation_id: ConversationId,
    ) -> Result<DeletedView, NexusError>;

    /// Deletes one request with its reply (`message_id` names either), so
    /// the model no longer sees them. The only exchange takes the task.
    async fn delete_exchange(message_id: MessageId) -> Result<DeletedView, NexusError>;

    /// What the agent did most recently, across conversations, newest first
    /// (at most `limit`, capped by the host). Running steps are included.
    async fn get_activity(limit: u32) -> Result<Vec<AgentStepView>, NexusError>;

    async fn list_projects() -> Result<Vec<ProjectView>, NexusError>;

    /// Creates or renames/redescribes a project.
    async fn save_project(request: SaveProjectRequest) -> Result<ProjectView, NexusError>;

    /// New tasks go to this project.
    async fn set_active_project(project_id: ProjectId) -> Result<ProjectView, NexusError>;

    async fn list_memory(query: MemoryQuery) -> Result<Vec<MemoryItemView>, NexusError>;

    /// A fact the user writes; active at once.
    async fn add_memory(request: AddMemoryRequest) -> Result<MemoryItemView, NexusError>;

    /// Corrects a fact; refused if it changed since `revision`.
    async fn edit_memory(
        memory_id: MemoryId,
        revision: i64,
        content: String,
    ) -> Result<MemoryItemView, NexusError>;

    /// Approves or rejects the agent's proposal as the user saw it.
    async fn decide_memory(
        memory_id: MemoryId,
        revision: i64,
        approve: bool,
    ) -> Result<MemoryItemView, NexusError>;

    /// Removes the fact for good, from search too.
    async fn delete_memory(memory_id: MemoryId) -> Result<(), NexusError>;

    /// Exactly the facts a request in this project sends to the model.
    async fn memory_context(project_id: ProjectId) -> Result<MemoryContextView, NexusError>;

    /// Writes the register to a new Excel file in a folder shared for
    /// writing; nothing is replaced.
    async fn export_register(
        register_id: RegisterId,
        folder_id: FolderId,
    ) -> Result<ExportView, NexusError>;

    /// What an Excel or CSV file in a shared folder would import.
    async fn import_preview(
        register_id: RegisterId,
        folder_id: FolderId,
        path: String,
    ) -> Result<ImportPreviewView, NexusError>;

    /// Adds the file's rows as the user's records, with the mapping the
    /// user confirmed; rows that cannot be used are listed, not guessed.
    async fn import_records(request: ImportRequest) -> Result<ImportResultView, NexusError>;

    /// Folders the user shared with Nexus.
    async fn list_folders() -> Result<Vec<FolderView>, NexusError>;

    async fn save_folder(request: SaveFolderRequest) -> Result<FolderView, NexusError>;

    /// Stops sharing it; nothing in the folder is touched.
    async fn remove_folder(folder_id: FolderId) -> Result<(), NexusError>;

    /// What a shared folder holds (a few levels deep, bounded).
    async fn folder_entries(folder_id: FolderId) -> Result<Vec<FolderEntryView>, NexusError>;

    /// The user's registers (job applications first) with their fields.
    async fn list_registers() -> Result<Vec<RegisterView>, NexusError>;

    async fn list_records(query: RecordQuery) -> Result<Vec<RecordView>, NexusError>;

    /// One record with its history, newest first.
    async fn record_detail(record_id: RecordId) -> Result<RecordDetailView, NexusError>;

    /// Adds or edits a record as the user.
    async fn save_record(request: SaveRecordRequest) -> Result<RecordView, NexusError>;

    /// For good, with its history.
    async fn delete_record(record_id: RecordId) -> Result<(), NexusError>;

    /// Reverses one change (the latest of its record); undoing an addition
    /// removes the record.
    async fn undo_record_change(change_id: i64) -> Result<(), NexusError>;

    /// Automations with their next run and last outcome.
    async fn list_automations() -> Result<Vec<AutomationView>, NexusError>;

    async fn save_automation(request: SaveAutomationRequest) -> Result<AutomationView, NexusError>;

    /// For good; the tasks it started stay.
    async fn delete_automation(automation_id: AutomationId) -> Result<(), NexusError>;

    /// Starts it now, as if due (not counted against its schedule).
    async fn run_automation_now(
        automation_id: AutomationId,
    ) -> Result<AutomationRunView, NexusError>;

    /// Its last `limit` runs, newest first.
    async fn automation_runs(
        automation_id: AutomationId,
        limit: u32,
    ) -> Result<Vec<AutomationRunView>, NexusError>;

    /// Remote MCP servers with their tools as last listed.
    async fn list_mcp_servers() -> Result<Vec<McpServerView>, NexusError>;

    /// Adds or edits a server, then asks it for its tools (a failure is
    /// recorded on the server, not raised). A key goes to the vault.
    async fn save_mcp_server(request: SaveMcpServerRequest) -> Result<McpServerView, NexusError>;

    /// Asks the server for its tools again.
    async fn refresh_mcp_tools(server_id: McpServerId) -> Result<McpServerView, NexusError>;

    /// Removes the server, its tools and its key.
    async fn remove_mcp_server(server_id: McpServerId) -> Result<(), NexusError>;

    /// Enables exactly the definition shown (`Some(sha256)`) or turns the
    /// tool off (`None`).
    async fn set_mcp_tool_enabled(
        server_id: McpServerId,
        tool: String,
        definition_sha256: Option<String>,
    ) -> Result<McpServerView, NexusError>;

    /// Host version plus CPU and memory of the machine, read when asked.
    async fn get_system_status() -> Result<SystemStatusView, NexusError>;

    /// Today's tokens per model, the daily budget and cost estimates.
    async fn get_usage() -> Result<UsageView, NexusError>;

    /// `None` removes the limit.
    async fn set_daily_token_budget(tokens: Option<u64>) -> Result<UsageView, NexusError>;

    /// What a model costs, as the user says; `None` forgets the price.
    async fn set_model_price(
        provider: String,
        model: String,
        price: Option<ModelPrice>,
    ) -> Result<UsageView, NexusError>;

    /// Explicit stop: no new work is taken, the current step finishes,
    /// uncertain effects stay recorded for reconciliation on next start.
    async fn stop_host() -> Result<(), NexusError>;
}
