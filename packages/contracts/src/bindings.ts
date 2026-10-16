// Generated from Rust by `NEXUS_UPDATE_BINDINGS=1 cargo test -p desktop`. Do not edit.

export type RequestId = string

export type OperationId = string

export type TaskId = string

export type RunId = string

export type ApprovalId = string

export type ArtifactId = string

/**
 * Stable error codes from `CONTRACTE_NEXUS_RO.md` §9. The code is what
 * callers branch on; `message` is user-facing text, never a stack trace.
 */
export type ErrorCode = "InvalidRequest" | "UnsupportedVersion" | "Unauthenticated" | "Forbidden" | "ApprovalRequired" | "ApprovalExpired" | "ApprovalInvalidated" | "RevisionConflict" | "ResourceChanged" | "NotFound" | "Busy" | "QuotaExceeded" | "DeadlineExceeded" | "ProviderUnavailable" | "Cancelled" | "OutcomeUnknown" | "InternalFailure"

/**
 * Wire-level error envelope. `retryable` is a declared characteristic, not
 * an instruction to retry blindly — callers still consult operation state,
 * tool semantics and budget before retrying (`CONTRACTE_NEXUS_RO.md` §9).
 */
export type NexusError = { code: ErrorCode; message: string; request_id: RequestId | null; operation_id: OperationId | null; retryable: boolean }

/**
 * What the dashboard may show about the host. Every variant comes from a
 * real connection attempt; there is no optimistic or cached "connected".
 */
export type HostStatus = { state: "Connected"; host_identity: string; protocol_version: number; uptime_seconds: number; paused: boolean } | { state: "NotRunning" } | { state: "Unreachable"; reason: string } | { state: "Rejected"; error: NexusError } | { state: "Untrusted"; reason: string }

/**
 * What a bridge command reports to the webview: the host's stable code when
 * the host refused, none when the call never got an answer.
 */
export type BridgeError = { code: ErrorCode | null; message: string }

/**
 * Mirrors `nexus_core::RunState`; the host converts with an exhaustive match.
 */
export type RunStatus = "Queued" | "Running" | "AwaitingApproval" | "Verifying" | "CancellationRequested" | "NeedsReconciliation" | "NeedsReview" | "Succeeded" | "Failed" | "Cancelled" | "Denied"

/**
 * Exactly what will be written and where. Built by the host from the same
 * manifest the approval is bound to.
 */
export type NotePreview = { destination: string; content: string; size_bytes: number; sha256: string }

export type PendingApproval = { approval_id: ApprovalId; manifest_sha256: string; expires_at_ms: number; preview: NotePreview | null }

export type ArtifactSummary = { artifact_id: ArtifactId; path: string; sha256: string; size_bytes: number; created_at_ms: number }

export type RunFailure = { code: string; message: string }

export type RunOrigin = { message_id: MessageId; conversation_id: ConversationId; conversation_title: string }

export type RunView = { run_id: RunId; title: string; status: RunStatus; revision: number; failure: RunFailure | null; pending_approval: PendingApproval | null; artifact: ArtifactSummary | null; origin: RunOrigin | null; created_at_ms: number; updated_at_ms: number }

export type Snapshot = { runs: RunView[]; watermark: number }

export type SubmitNoteResponse = { task_id: TaskId; run_id: RunId; created: boolean }

export type BackupSummary = { path: string; created_at_ms: number; artifacts: number; unmatched_artifacts: number; database_sha256: string }

export type EventView = { seq: number; kind: string; entity_id: string; created_at_ms: number }

export type EventsPage = { events: EventView[]; next_after: number }

/**
 * Facts about the machine and the host process, for the dashboard. Each
 * figure is measured when asked; one that cannot be read is `None`.
 */
export type SystemStatusView = { host_version: string; cpu_percent: number | null; memory_used_mib: number | null; memory_total_mib: number | null; logical_processors: number | null }

export type ConnectionId = string

export type ConversationId = string

export type MessageId = string

export type ConnectionView = { connection_id: ConnectionId; provider: string; display_name: string; model: string | null; free_tier: boolean; has_key: boolean; active: boolean; thinking_level: ThinkingLevel }

/**
 * The user's speed/quality choice, where the model supports it (Gemini
 * 3.x). Ignored for a model that does not.
 */
export type ThinkingLevel = "Low" | "Medium" | "High"

/**
 * The AI providers this build can talk to. `ConnectionView::provider`
 * carries the same names as text ("Gemini", "OpenAI", "Anthropic",
 * "DeepSeek").
 */
export type ProviderChoice = "Gemini" | "OpenAi" | "Anthropic" | "DeepSeek"

/**
 * The model used only when the active one is overloaded, chosen by the
 * user. There is no other fallback.
 */
export type BackupModelView = { connection_id: ConnectionId; display_name: string; provider: string; model: string }

export type ModelView = { id: string; display_name: string; input_token_limit: number | null; output_token_limit: number | null }

export type PersonaView = { version: number; name: string; tone: string; style: string }

export type ChatRole = "User" | "Model"

export type ChatStatus = "Streaming" | "Complete" | "Failed" | "Cancelled"

export type AgentStepId = string

export type AgentStepStatus = "Running" | "Done" | "Failed"

/**
 * One step's kind-specific facts. Metadata only: a web read shows its
 * address, status and size, never the page's text.
 */
export type AgentStepDetail = { kind: "ModelCall"; model: string; first_byte_ms: number | null; duration_ms: number | null; error: string | null; attempt: number | null; backup: boolean | null } | { kind: "Fetch"; url: string; final_url: string | null; http_status: number | null; text_bytes: number | null; truncated: boolean | null; error: string | null } | { kind: "NoteProposed"; title: string; run_id: RunId | null; error: string | null }

export type AgentStepView = { step_id: AgentStepId; message_id: MessageId; conversation_id: ConversationId; conversation_title: string; seq: number; status: AgentStepStatus; detail: AgentStepDetail; started_at_ms: number; finished_at_ms: number | null }

export type ChatMessageView = { message_id: MessageId; role: ChatRole; text: string; status: ChatStatus; model: string | null; persona_version: number | null; failure: RunFailure | null; proposed_run_id: RunId | null; prompt_tokens: number | null; output_tokens: number | null; created_at_ms: number; steps: AgentStepView[] }

export type ConversationSummary = { conversation_id: ConversationId; title: string; updated_at_ms: number }

export type ConversationView = { conversation_id: ConversationId; title: string; messages: ChatMessageView[] }

export type SendMessageResponse = { conversation_id: ConversationId; user_message_id: MessageId; reply_message_id: MessageId }

