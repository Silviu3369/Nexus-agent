import { invoke } from "@tauri-apps/api/core";
import type {
  ApprovalId,
  ArtifactId,
  BackupSummary,
  BridgeError,
  ConnectionId,
  ConnectionView,
  ConversationId,
  ConversationSummary,
  ConversationView,
  ErrorCode,
  HostStatus,
  MessageId,
  ModelView,
  PersonaView,
  RunId,
  RunView,
  SendMessageResponse,
  Snapshot,
  SubmitNoteResponse,
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

  listConnections: () => invoke<ConnectionView[]>("list_connections"),
  /** The key goes to the host's Windows vault; `null` keeps the stored one. */
  saveGeminiConnection: (
    connectionId: ConnectionId | null,
    displayName: string,
    freeTier: boolean,
    apiKey: string | null,
  ) =>
    invoke<ConnectionView>("save_gemini_connection", { connectionId, displayName, freeTier, apiKey }),
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
  sendMessage: (conversationId: ConversationId | null, idempotencyKey: string, text: string) =>
    invoke<SendMessageResponse>("send_message", { conversationId, idempotencyKey, text }),
  cancelReply: (messageId: MessageId) => invoke<null>("cancel_reply", { messageId }),
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

/** Turns a rejected bridge call into a short English message plus detail. */
export function describeError(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    const bridge = err as BridgeError;
    const headline = bridge.code ? CODE_TEXT[bridge.code] : undefined;
    return headline ? `${headline} (${bridge.message})` : bridge.message;
  }
  return String(err);
}
