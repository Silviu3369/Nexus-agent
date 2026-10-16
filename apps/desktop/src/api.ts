import { invoke } from "@tauri-apps/api/core";
import type {
  AgentStepView,
  ApprovalId,
  ArtifactId,
  BackupModelView,
  BackupSummary,
  BridgeError,
  ConnectionId,
  ConnectionView,
  ConversationId,
  ConversationSummary,
  ConversationView,
  DeletedView,
  ErrorCode,
  EventsPage,
  HostStatus,
  MemoryContextView,
  MemoryId,
  MemoryItemView,
  MemoryScope,
  MemoryStatus,
  MessageId,
  ModelView,
  PersonaView,
  ProjectId,
  ProjectView,
  ProviderChoice,
  RunId,
  RunView,
  SendMessageResponse,
  Snapshot,
  SubmitNoteResponse,
  SystemStatusView,
  ThinkingLevel,
} from "@nexus/contracts";

export const api = {
  hostStatus: () => invoke<HostStatus>("host_status"),
  startHost: () => invoke<null>("start_host"),
  submitNote: (idempotencyKey: string, title: string, body: string) =>
    invoke<SubmitNoteResponse>("submit_note", { idempotencyKey, title, body }),
  listRuns: () => invoke<Snapshot>("list_runs"),
  getRun: (runId: RunId) => invoke<RunView>("get_run", { runId }),
  decide: (approvalId: ApprovalId, manifestSha256: string, approve: boolean) =>
    invoke<RunView>("decide_approval", { approvalId, manifestSha256, approve }),
  cancelRun: (runId: RunId) => invoke<RunView>("cancel_run", { runId }),
  artifactContent: (artifactId: ArtifactId) => invoke<string>("artifact_content", { artifactId }),
  revealArtifact: (runId: RunId) => invoke<null>("reveal_artifact", { runId }),
  createBackup: () => invoke<BackupSummary>("create_backup"),
  /** Durable events after the cursor; they say what changed, nothing more. */
  getEvents: (after: number) => invoke<EventsPage>("get_events", { after }),
  getSystemStatus: () => invoke<SystemStatusView>("get_system_status"),

  listConnections: () => invoke<ConnectionView[]>("list_connections"),
  /** The key goes to the host's Windows vault; `null` keeps the stored one.
   *  `provider: null` lets the host read it from the key's format. */
  saveConnection: (
    connectionId: ConnectionId | null,
    displayName: string | null,
    provider: ProviderChoice | null,
    freeTier: boolean,
    apiKey: string | null,
  ) =>
    invoke<ConnectionView>("save_connection", { connectionId, displayName, provider, freeTier, apiKey }),
  getBackupModel: () => invoke<BackupModelView | null>("get_backup_model"),
  setBackupModel: (connectionId: ConnectionId | null, model: string | null) =>
    invoke<BackupModelView | null>("set_backup_model", { connectionId, model }),
  removeConnection: (connectionId: ConnectionId) =>
    invoke<null>("remove_connection", { connectionId }),
  listModels: (connectionId: ConnectionId) => invoke<ModelView[]>("list_models", { connectionId }),
  selectModel: (connectionId: ConnectionId, model: string) =>
    invoke<ConnectionView>("select_model", { connectionId, model }),
  setThinkingLevel: (connectionId: ConnectionId, level: ThinkingLevel) =>
    invoke<ConnectionView>("set_thinking_level", { connectionId, level }),
  getPersona: () => invoke<PersonaView>("get_persona"),
  savePersona: (name: string, tone: string, style: string) =>
    invoke<PersonaView>("save_persona", { name, tone, style }),
  listConversations: () => invoke<ConversationSummary[]>("list_conversations"),
  getConversation: (conversationId: ConversationId) =>
    invoke<ConversationView>("get_conversation", { conversationId }),
  /** `projectId` places a new task; a follow-up stays in its task's project,
   *  and `null` lets the Host use the active project. */
  sendMessage: (conversationId: ConversationId | null, projectId: ProjectId | null, idempotencyKey: string, text: string) =>
    invoke<SendMessageResponse>("send_message", { conversationId, projectId, idempotencyKey, text }),
  cancelReply: (messageId: MessageId) => invoke<null>("cancel_reply", { messageId }),
  /** For good: the task, its messages and steps. Facts and notes stay. */
  deleteConversation: (conversationId: ConversationId) =>
    invoke<DeletedView>("delete_conversation", { conversationId }),
  /** One request with its reply (either id); the only one takes the task. */
  deleteExchange: (messageId: MessageId) => invoke<DeletedView>("delete_exchange", { messageId }),
  getActivity: () => invoke<AgentStepView[]>("get_activity"),

  listProjects: () => invoke<ProjectView[]>("list_projects"),
  /** `projectId: null` creates a project; creating one does not switch to it. */
  saveProject: (projectId: ProjectId | null, name: string, description: string) =>
    invoke<ProjectView>("save_project", { projectId, name, description }),
  setActiveProject: (projectId: ProjectId) => invoke<ProjectView>("set_active_project", { projectId }),
  /** `projectId` narrows to that project's facts plus the personal ones. */
  listMemory: (query: {
    text?: string | null;
    scope?: MemoryScope | null;
    projectId?: ProjectId | null;
    status?: MemoryStatus | null;
    limit: number;
  }) =>
    invoke<MemoryItemView[]>("list_memory", {
      text: query.text ?? null,
      scope: query.scope ?? null,
      projectId: query.projectId ?? null,
      status: query.status ?? null,
      limit: query.limit,
    }),
  /** A fact the user writes is active at once. */
  addMemory: (scope: MemoryScope, projectId: ProjectId | null, content: string) =>
    invoke<MemoryItemView>("add_memory", { scope, projectId, content }),
  editMemory: (memoryId: MemoryId, revision: number, content: string) =>
    invoke<MemoryItemView>("edit_memory", { memoryId, revision, content }),
  decideMemory: (memoryId: MemoryId, revision: number, approve: boolean) =>
    invoke<MemoryItemView>("decide_memory", { memoryId, revision, approve }),
  deleteMemory: (memoryId: MemoryId) => invoke<null>("delete_memory", { memoryId }),
  /** Exactly the approved facts the Host sends with a request in this project. */
  memoryContext: (projectId: ProjectId) => invoke<MemoryContextView>("memory_context", { projectId }),
};

const CODE_TEXT: Partial<Record<ErrorCode, string>> = {
  ApprovalExpired: "The approval expired.",
  ApprovalInvalidated: "The approval is no longer valid.",
  InvalidRequest: "The request is not valid.",
  RevisionConflict: "Conflict with an earlier request.",
  ResourceChanged: "The file was changed outside Nexus.",
  NotFound: "Not found.",
  Unauthenticated: "Authentication was refused.",
  Forbidden: "This operation is not allowed here.",
  QuotaExceeded: "The AI provider's quota has been reached.",
  DeadlineExceeded: "The AI provider did not respond in time.",
  ProviderUnavailable: "The AI provider is unavailable right now.",
};

/** The Host's stable error code of a rejected bridge call, if it has one. */
export function errorCode(err: unknown): ErrorCode | null {
  if (typeof err === "object" && err !== null && "code" in err) return (err as BridgeError).code ?? null;
  return null;
}

/** Turns a rejected bridge call into a short English message plus detail. */
export function describeError(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    const bridge = err as BridgeError;
    const headline = bridge.code ? CODE_TEXT[bridge.code] : undefined;
    return headline ? `${headline} (${bridge.message})` : bridge.message;
  }
  return String(err);
}
