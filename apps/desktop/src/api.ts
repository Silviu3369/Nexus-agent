import { invoke } from "@tauri-apps/api/core";
import type {
  ApprovalId,
  ArtifactId,
  BridgeError,
  ErrorCode,
  HostStatus,
  RunId,
  RunView,
  Snapshot,
  SubmitNoteResponse,
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
};

const CODE_TEXT: Partial<Record<ErrorCode, string>> = {
  ApprovalExpired: "Aprobarea a expirat.",
  ApprovalInvalidated: "Aprobarea nu mai este valabilă.",
  InvalidRequest: "Cererea nu este validă.",
  RevisionConflict: "Conflict cu o cerere anterioară.",
  ResourceChanged: "Fișierul a fost modificat în afara Nexus.",
  NotFound: "Nu a fost găsit.",
  Unauthenticated: "Conexiunea la Host nu este autentificată.",
};

/** Turns a rejected bridge call into a short Romanian message plus detail. */
export function describeError(err: unknown): string {
  if (typeof err === "object" && err !== null && "message" in err) {
    const bridge = err as BridgeError;
    const headline = bridge.code ? CODE_TEXT[bridge.code] : undefined;
    return headline ? `${headline} (${bridge.message})` : bridge.message;
  }
  return String(err);
}
