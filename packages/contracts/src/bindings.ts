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
export type ErrorCode = "InvalidRequest" | "UnsupportedVersion" | "Unauthenticated" | "Forbidden" | "ApprovalRequired" | "ApprovalExpired" | "ApprovalInvalidated" | "RevisionConflict" | "ResourceChanged" | "NotFound" | "Busy" | "QuotaExceeded" | "BudgetExceeded" | "DeadlineExceeded" | "ProviderUnavailable" | "Cancelled" | "OutcomeUnknown" | "InternalFailure"

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
 * What Settings shows: whether the Host starts at sign-in, and with which
 * program (another copy of Nexus may have set it).
 */
export type AutostartView = { enabled: boolean; command: string | null; points_here: boolean }

/**
 * Mirrors `nexus_core::RunState`; the host converts with an exhaustive match.
 */
export type RunStatus = "Queued" | "Running" | "AwaitingApproval" | "Verifying" | "CancellationRequested" | "NeedsReconciliation" | "NeedsReview" | "Succeeded" | "Failed" | "Cancelled" | "Denied"

/**
 * Exactly what will be written and where. Built by the host from the same
 * manifest the approval is bound to.
 */
export type NotePreview = { destination: string; content: string; size_bytes: number; sha256: string }

/**
 * The tool as the user enabled it; the server's own words, untrusted.
 */
export type McpCallPreview = { description: string; destructive_hint: boolean | null; definition_sha256: string }

export type PendingApproval = { approval_id: ApprovalId; manifest_sha256: string; expires_at_ms: number; preview: NotePreview | null; call: McpCallPreview | null; records: RecordChangePreview[] | null }

export type ArtifactSummary = { artifact_id: ArtifactId; path: string; sha256: string; size_bytes: number; created_at_ms: number }

export type RunFailure = { code: string; message: string }

export type RunOrigin = { message_id: MessageId; conversation_id: ConversationId; conversation_title: string }

/**
 * What a run does.
 */
export type RunKind = "Note" | "McpCall" | "Document" | "RecordBatch"

/**
 * A call to an MCP tool, exactly as the user approves it.
 */
export type McpCallView = { server: string; url: string; tool: string; arguments_json: string; result_chars: number | null; truncated: boolean | null; duration_ms: number | null }

/**
 * Who did or asked for something (ADR-IDENTITY-001).
 */
export type ActorView = { kind: string; name: string }

/**
 * One change of a batch, as the user decides it.
 */
export type RecordChangePreview = { register_key: string; register_name: string; record_id: RecordId | null; title: string; fields: FieldChange[] }

/**
 * One tool and what the approval policy lets it do.
 */
export type PermissionView = { tool_id: string; label: string; effect_class: string; sends: string; enabled: boolean; decision: string; max_auto: number | null; default_max_auto: number | null; changed_at_ms: number | null }

/**
 * What one connection may receive, and why.
 */
export type ConnectionDataView = { connection_id: ConnectionId; display_name: string; provider: string; free_tier: boolean; can_receive: string; reason: string; terms_url: string; dpa: string; dpa_signed: boolean; allows_training: boolean }

/**
 * A source the agent may read and send: a shared folder, a register, an
 * MCP server.
 */
export type SourceDataView = { kind: string; id: string; name: string; data_class: string }

/**
 * What leaves Nexus, and for which connection.
 */
export type DataPolicyView = { in_eea: boolean; terms_checked: string; connections: ConnectionDataView[]; sources: SourceDataView[] }

/**
 * One change to what data may leave.
 */
export type DataChange = { InEea: { in_eea: boolean } } | { Connection: { connection_id: ConnectionId; dpa_signed: boolean; allows_training: boolean } } | { Source: { kind: string; id: string; data_class: string } }

export type RunView = { run_id: RunId; title: string; status: RunStatus; effect_class: string | null; evidence: string | null; undone_at_ms: number | null; can_undo: boolean; revision: number; failure: RunFailure | null; pending_approval: PendingApproval | null; artifact: ArtifactSummary | null; origin: RunOrigin | null; kind: RunKind; call: McpCallView | null; requested_by: ActorView | null; decided_by: ActorView | null; created_at_ms: number; updated_at_ms: number }

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
export type AgentStepDetail = { kind: "ModelCall"; model: string; first_byte_ms: number | null; duration_ms: number | null; error: string | null; attempt: number | null; backup: boolean | null; memory_items: number | null; mcp_servers: string[] | null; mcp_tools: number | null; prompt_tokens: number | null; output_tokens: number | null } | { kind: "Fetch"; url: string; final_url: string | null; http_status: number | null; text_bytes: number | null; truncated: boolean | null; error: string | null } | { kind: "MemoryProposed"; content: string; scope: MemoryScope | null; memory_id: MemoryId | null; error: string | null } | { kind: "McpCall"; server: string; tool: string; arguments: string; result_chars: number | null; truncated: boolean | null; is_error: boolean | null; duration_ms: number | null; error: string | null; run_id: RunId | null } | { kind: "NoteProposed"; title: string; run_id: RunId | null; error: string | null } | { kind: "RecordsFound"; register: string; query: string; count: number | null; error: string | null } | { kind: "FilesListed"; folder: string; path: string; count: number | null; error: string | null } | { kind: "DocumentRead"; folder: string; path: string; format: string | null; chars: number | null; offset: number | null; truncated: boolean | null; error: string | null } | { kind: "SkillUsed"; skill: string; file: string | null; chars: number | null; error: string | null } | { kind: "DocumentProposed"; folder: string; file_name: string; run_id: RunId | null; error: string | null } | { kind: "RecordChanged"; register: string; action: string; title: string; fields: string[]; record_id: RecordId | null; change_id: number | null; undone: boolean; error: string | null }

export type AgentStepView = { step_id: AgentStepId; message_id: MessageId; conversation_id: ConversationId; conversation_title: string; seq: number; status: AgentStepStatus; detail: AgentStepDetail; started_at_ms: number; finished_at_ms: number | null }

export type ChatMessageView = { message_id: MessageId; role: ChatRole; text: string; status: ChatStatus; model: string | null; persona_version: number | null; failure: RunFailure | null; proposed_run_id: RunId | null; prompt_tokens: number | null; output_tokens: number | null; created_at_ms: number; steps: AgentStepView[]; turn: TurnView | null }

export type ConversationSummary = { conversation_id: ConversationId; project_id: ProjectId | null; title: string; updated_at_ms: number }

export type ConversationView = { conversation_id: ConversationId; project_id: ProjectId | null; title: string; requested_by: ActorView | null; messages: ChatMessageView[] }

export type SendMessageResponse = { conversation_id: ConversationId; user_message_id: MessageId; reply_message_id: MessageId }

/**
 * What a deletion removed.
 */
export type DeletedView = { conversation_id: ConversationId; messages: number; conversation_deleted: boolean }

export type ProjectId = string

export type MemoryId = string

export type MemoryScope = "Personal" | "Project"

export type MemoryStatus = "Proposed" | "Active" | "Rejected"

export type MemorySource = "User" | "Agent"

export type ProjectView = { project_id: ProjectId; name: string; description: string; active: boolean; conversations: number; memory_items: number; created_at_ms: number }

export type MemoryItemView = { memory_id: MemoryId; scope: MemoryScope; project_id: ProjectId | null; project_name: string | null; content: string; status: MemoryStatus; source: MemorySource; edited_by_user: boolean; conversation_id: ConversationId | null; conversation_title: string | null; influences: string[]; revision: number; created_at_ms: number; updated_at_ms: number; decided_at_ms: number | null }

/**
 * Exactly the facts a request in this project carries to the model.
 */
export type MemoryContextView = { project_id: ProjectId; items: MemoryItemView[]; max_items: number; max_chars: number }

export type McpServerId = string

export type McpToolView = { name: string; title: string | null; description: string; parameters: string[]; schema_json: string; read_only_hint: boolean | null; destructive_hint: boolean | null; definition_sha256: string; enabled: boolean; changed: boolean; available: boolean; blocked_reason: string | null }

export type McpServerView = { server_id: McpServerId; name: string; url: string; has_key: boolean; checked_at_ms: number | null; last_error: string | null; tools: McpToolView[] }

/**
 * What the user says a model costs, in US dollars per million tokens.
 */
export type ModelPrice = { input_per_million: number; output_per_million: number }

export type ModelUsageView = { provider: string; model: string; calls_today: number; prompt_tokens_today: number; output_tokens_today: number; free_tier: boolean; price: ModelPrice | null; estimated_cost_today: number | null }

export type UsageView = { day_start_ms: number; daily_token_budget: number | null; tokens_today: number; models: ModelUsageView[]; estimated_cost_today: number | null }

export type AutomationId = string

export type RegisterId = string

export type RecordId = string

export type FieldKind = "Text" | "LongText" | "Url" | "Date" | "Choice" | "Number"

export type FieldDef = { key: string; label: string; kind: FieldKind; required: boolean; choices: string[]; max_chars: number; help: string; min?: number | null; max?: number | null }

export type RegisterView = { register_id: RegisterId; key: string; name: string; item_name: string; fields: FieldDef[]; status_field: string; closed_statuses: string[]; follow_up_field: string | null; title_fields: string[]; record_count: number; follow_ups_due: number }

export type RecordActor = "User" | "Agent"

export type RecordChangeKind = "Created" | "Updated" | "Undo"

export type RecordView = { record_id: RecordId; register_id: RegisterId; values: { [key: string]: string }; revision: number; created_by: RecordActor; created_at_ms: number; updated_at_ms: number; follow_up_due: boolean }

export type FieldChange = { key: string; before: string | null; after: string | null }

export type RecordChangeView = { change_id: number; kind: RecordChangeKind; actor: RecordActor; at_ms: number; fields: FieldChange[]; conversation_id: ConversationId | null; read_hosts: string[]; undone: boolean; can_undo: boolean }

export type RecordDetailView = { record: RecordView; history: RecordChangeView[] }

export type RecordQuery = { register_id: RegisterId; statuses: string[]; text: string | null; follow_up_due: boolean; limit: number }

export type SaveRecordRequest = { register_id: RegisterId; record_id: RecordId | null; revision: number; values: { [key: string]: string } }

export type FolderId = string

export type FolderView = { folder_id: FolderId; label: string; path: string; can_write: boolean; available: boolean; problem: string | null; revision: number; created_at_ms: number }

export type SaveFolderRequest = { folder_id: FolderId | null; revision: number; path: string; label: string; can_write: boolean }

/**
 * One file or folder inside a shared folder.
 */
export type FolderEntryView = { path: string; is_dir: boolean; size_bytes: number; modified_ms: number; readable: boolean }

export type SkillId = string

export type SkillView = { skill_id: SkillId; name: string; description: string; license: string | null; compatibility: string | null; allowed_tools: string | null; has_scripts: boolean; skipped: string[]; files: SkillFileView[]; enabled: boolean; changed_since_enabled: boolean; content_sha256: string; revision: number; created_at_ms: number; updated_at_ms: number }

export type SkillFileView = { path: string; chars: number }

/**
 * A skill with every file's text, to read before turning it on.
 */
export type SkillContentView = { skill: SkillView; files: SkillFileText[] }

export type SkillFileText = { path: string; text: string }

export type SetSkillEnabledRequest = { skill_id: SkillId; revision: number; enabled: boolean }

export type PackView = { pack_id: string; name: string; version: number; description: string; registers: string[]; skills: PackSkillView[]; intake: PackFlowView | null }

/**
 * A skill of a pack: part of Nexus, read-only.
 */
export type PackSkillView = { name: string; description: string; text: string }

/**
 * The tasks a pack's intake starts, one per new file, and what they may do
 * beyond the user's own rules.
 */
export type PackFlowView = { label: string; skill: string; turned_off: string[]; record_writes_without_asking: number | null }

/**
 * A register written to an Excel file in a shared folder.
 */
export type ExportView = { path: string; rows: number }

/**
 * What an Excel or CSV file would import, before anything is added.
 */
export type ImportPreviewView = { file: string; sheet: string; columns: string[]; mapping: (string | null)[]; rows: number; sample: string[][]; truncated: boolean }

export type ImportRequest = { register_id: RegisterId; folder_id: FolderId; path: string; mapping: (string | null)[] }

export type ImportSkipView = { row: number; reason: string }

export type ImportResultView = { added: number; adjusted: number; skipped: ImportSkipView[] }

export type InboxFileStatus = "Waiting" | "Started" | "Done" | "Failed" | "Skipped"

export type InboxFileView = { file_id: number; path: string; status: InboxFileStatus; reason: string | null; conversation_id: ConversationId | null; seen_at_ms: number; started_at_ms: number | null }

/**
 * A register's inbox: a shared folder whose new documents the agent reads
 * and records on its own, one task per file.
 */
export type InboxView = { register_id: RegisterId; folder_id: FolderId | null; folder_label: string | null; enabled: boolean; revision: number; cv_folder_id: FolderId | null; cv_folder_label: string | null; cv_path: string | null; waiting: number; files: InboxFileView[] }

/**
 * Applications sent in one week (Monday first, local time).
 */
export type WeekCountView = { week_start: string; sent: number }

export type ScoreBand = "High" | "Low" | "Unscored"

/**
 * Sent applications in one group, and how many of them reached an
 * interview.
 */
export type GroupCountView = { sent: number; interviews: number }

export type ScoreBandView = { band: ScoreBand; counts: GroupCountView }

export type SourceCountView = { source: string; counts: GroupCountView }

/**
 * A failure cause (ADR-TASK-001's closed list, or `NotStarted` when the
 * task could not even start) and how many ads it stopped.
 */
export type CauseCountView = { cause: string; count: number }

/**
 * Whether the user kept what the agent did in a turn (ADR-TASK-001): a
 * turn that succeeded is accepted once its review window (7 days) passes
 * with nothing of it rejected, undone, corrected or stopped.
 */
export type Acceptance = "NotApplicable" | "InReview" | "Accepted" | "NotAccepted"

/**
 * One turn of the agent: a request and every reply to it.
 */
export type TurnView = { run_id: RunId; status: RunStatus; failure_cause: string | null; acceptance: Acceptance; review_until_ms: number | null; interventions: string[] }

/**
 * How the agent handled the job ads from the inbox (ADR-TASK-001),
 * counted from each ad's task and its runs; the cost is an estimate from
 * the prices the user entered.
 */
export type AdsHandlingView = { seen: number; finished: number; recorded: number; nothing_recorded: number; failed: number; waiting: number; corrected: number; accepted: number; in_review: number; failure_causes: CauseCountView[]; prompt_tokens: number; output_tokens: number; estimated_cost_usd: number | null; median_seconds: number | null }

/**
 * What the job applications led to (`PLAN_NEXUS_RO.md` §3a), counted from
 * the applications and their history; nothing is estimated. "Sent" is an
 * application that was applied for (Applied, Interviewing, Offer,
 * Rejected, or withdrawn after applying); a stage counts once it was
 * reached, even if the status moved on, unless the user undid it.
 */
export type JobResultsView = { sent: number; to_apply: number; answered: number; interviews: number; offers: number; waiting: number; waiting_long: number; median_days_to_answer: number | null; weeks: WeekCountView[]; undated: number; by_score: ScoreBandView[]; by_source: SourceCountView[]; added_by_nexus: number; ads_read: number; nexus_changes: number; undone_changes: number; partial: boolean; ads: AdsHandlingView }

export type SaveInboxRequest = { register_id: RegisterId; revision: number; folder_id: FolderId | null; enabled: boolean; cv_folder_id: FolderId | null; cv_path: string | null }

/**
 * When an automation runs, in this PC's local time.
 */
export type Schedule = { kind: "Once"; at_ms: number } | { kind: "Weekly"; days: number[]; hour: number; minute: number }

export type AutomationRunStatus = "Started" | "Skipped" | "Missed" | "Failed"

export type AutomationRunView = { scheduled_for_ms: number; at_ms: number; status: AutomationRunStatus; reason: string | null; conversation_id: ConversationId | null; catch_up: boolean; manual: boolean }

export type AutomationView = { automation_id: AutomationId; name: string; prompt: string; project_id: ProjectId | null; schedule: Schedule; enabled: boolean; catch_up: boolean; next_run_at_ms: number | null; last_run: AutomationRunView | null; revision: number; created_at_ms: number }

