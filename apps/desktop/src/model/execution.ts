// The dashboard's view of autonomous work, derived from what the Host
// actually records. Pure functions only (no React, no IPC), so the mapping
// can be tested on its own.
//
// Hierarchy shown to the user, and where each level comes from today:
//   Task   — a conversation (work for the agent), or a note the user
//            submitted directly (an M1 Task).
//   Run    — one reply of the model to a request in that task, or an M1 run.
//   Step   — one recorded agent step: a model call, a web read, a proposed
//            note or a proposed fact to remember.
//   Action — the effect a step leads to: the GET of a web read, or the
//            CreateMarkdownArtifact action with its approval and write.
// The Host does not yet record agent work as Task/Run rows of its own; this
// module is the single place that maps it, so it can be replaced when it does.

import type {
  ActorView,
  AgentStepView,
  ChatMessageView,
  ConversationSummary,
  ConversationView,
  EventView,
  ProjectId,
  RunId,
  RunStatus,
  RunView,
  TurnView,
} from "@nexus/contracts";
import { hostOf, kib, median, seconds } from "./format.ts";

export type ExecStatus =
  | "queued"
  | "running"
  | "completed"
  | "failed"
  | "waiting"
  | "approval_required"
  | "cancelled";

export const STATUS_LABEL: Record<ExecStatus, string> = {
  queued: "Queued",
  running: "Running",
  completed: "Completed",
  failed: "Failed",
  waiting: "Waiting",
  approval_required: "Approval required",
  cancelled: "Cancelled",
};

/** The visual states of the Nexus Core. PLANNING is supported by the
 *  visual but never derived: the Host has no planning step yet. */
export type CoreState =
  | "IDLE"
  | "LISTENING"
  | "THINKING"
  | "PLANNING"
  | "USING_TOOL"
  | "EXECUTING"
  | "WAITING_APPROVAL"
  | "SUCCESS"
  | "ERROR";

export type ItemKind = "model" | "web" | "note" | "doc" | "memory" | "mcp" | "record" | "approval" | "write";

export interface TimelineItem {
  id: string;
  kind: ItemKind;
  title: string;
  status: ExecStatus;
  /** Who performed it: the assistant's persona name, or "You". */
  agent: string;
  model: string | null;
  tool: string | null;
  startedAt: number;
  finishedAt: number | null;
  durationMs: number | null;
  /** One line on how it went. */
  outcome: string | null;
  /** Shown when the item is expanded. Metadata only, never page text. */
  facts: [string, string][];
  /** Model calls: how many approved facts the Host sent with the request. */
  memoryItems?: number;
  /** MCP calls: which tool of which server. */
  mcp?: { server: string; tool: string };
  /** Only proposed: it waits for the user before anything is done. */
  proposal?: boolean;
  /** Model calls: MCP servers whose tools the model could call. */
  mcpAvailable?: string[];
  /** A change the agent made to one of the user's registers, which the
   *  user can undo. */
  record?: { changeId: number | null; recordId: string | null; undone: boolean };
}

export interface RunVM {
  id: string;
  taskId: string;
  taskTitle: string;
  /** The request this run answers (the user's message). */
  request: string | null;
  status: ExecStatus;
  model: string | null;
  personaVersion: number | null;
  startedAt: number;
  finishedAt: number | null;
  items: TimelineItem[];
  text: string;
  failure: { code: string; message: string } | null;
  promptTokens: number | null;
  outputTokens: number | null;
  /** Notes the run proposed, in order (one per reply that stopped on one). */
  proposedRunIds: RunId[];
  /** The latest reply of the run: the one a Stop or a deletion names. */
  messageId: string;
  streaming: boolean;
  /** How the turn ended and whether the user kept it (ADR-TASK-001). */
  turn: TurnView | null;
}

export const STOPPED_BY_USER = "stopped by the user";

const RUN_TO_STATUS: Record<RunStatus, ExecStatus> = {
  Queued: "queued",
  Running: "running",
  AwaitingApproval: "approval_required",
  Verifying: "running",
  CancellationRequested: "waiting",
  NeedsReconciliation: "waiting",
  NeedsReview: "waiting",
  Succeeded: "completed",
  Failed: "failed",
  Cancelled: "cancelled",
  Denied: "cancelled",
};

export function runStatus(status: RunStatus): ExecStatus {
  return RUN_TO_STATUS[status];
}

export function stepStatus(step: AgentStepView): ExecStatus {
  if (step.status === "Running") return "running";
  if (step.status === "Done") return "completed";
  const error = step.detail.error ?? "";
  return error.startsWith(STOPPED_BY_USER) ? "cancelled" : "failed";
}

/** One line saying what the step is, and one saying how it went. */
export function describeStep(step: AgentStepView): { title: string; detail: string | null } {
  const d = step.detail;
  const running = step.status === "Running";
  switch (d.kind) {
    case "ModelCall": {
      const which = d.backup ? " (backup)" : d.attempt ? ` (try ${d.attempt})` : "";
      const title = running ? `Thinking — ${d.model}${which}` : `Model ${d.model}${which}`;
      if (d.error) return { title, detail: d.error };
      const total = seconds(d.duration_ms);
      const first = seconds(d.first_byte_ms);
      if (!total) return { title, detail: null };
      return { title, detail: first ? `${total} (first response after ${first})` : total };
    }
    case "Fetch": {
      const title = `${running ? "Reading" : "Read"} ${hostOf(d.url)}`;
      if (d.error) return { title, detail: d.error };
      if (d.http_status === null) return { title, detail: null };
      const size = d.text_bytes === null ? null : `${kib(d.text_bytes)} of text`;
      const parts = [`HTTP ${d.http_status}`, size, d.truncated ? "truncated" : null].filter(Boolean);
      return { title, detail: parts.join(" · ") };
    }
    case "NoteProposed": {
      const title = `Proposed note "${d.title}"`;
      if (d.error) return { title, detail: d.error };
      return { title, detail: running ? null : "waiting for your approval" };
    }
    case "MemoryProposed": {
      const title = `Proposed to remember "${clip(d.content, 60)}"`;
      if (d.error) return { title, detail: d.error };
      return { title, detail: running ? null : "remembered only if you approve it" };
    }
    case "FilesListed": {
      const where = d.path ? `${d.folder}/${d.path}` : d.folder;
      const title = `${running ? "Looking in" : "Looked in"} ${where}`;
      if (d.error) return { title, detail: d.error };
      return { title, detail: d.count === null ? null : `${d.count} ${d.count === 1 ? "item" : "items"}` };
    }
    case "DocumentRead": {
      const title = `${running ? "Reading" : "Read"} ${d.path} in ${d.folder}`;
      if (d.error) return { title, detail: d.error };
      if (d.chars === null) return { title, detail: null };
      const parts = [d.format, `${d.chars.toLocaleString("en-US")} characters to the model`, d.truncated ? "cut" : null].filter(Boolean);
      return { title, detail: parts.join(" · ") };
    }
    case "DocumentProposed": {
      const title = `Proposed ${d.file_name || "a file"} in ${d.folder}`;
      if (d.error) return { title, detail: d.error };
      return { title, detail: running ? null : "written only if you approve it" };
    }
    case "RecordsFound": {
      const title = `${running ? "Looking up" : "Looked up"} ${d.register}`;
      if (d.error) return { title, detail: d.error };
      if (d.count === null) return { title, detail: null };
      return { title, detail: `${d.count} found · ${d.query}` };
    }
    case "RecordChanged": {
      const what = d.title || "a record";
      const title = `${d.action === "added" ? "Added" : "Updated"} ${what} in ${d.register}`;
      if (d.error) return { title: `Could not save to ${d.register}`, detail: d.error };
      if (running) return { title, detail: null };
      if (d.undone) return { title, detail: "undone by you" };
      return { title, detail: d.fields.length > 0 ? `${d.fields.join(", ")} · saved; you can undo it` : "nothing changed" };
    }
    case "McpCall": {
      if (d.run_id) {
        // A tool that can change things: the call waits for the user.
        const title = `Proposed calling ${d.tool} on ${d.server}`;
        if (d.error) return { title, detail: d.error };
        return { title, detail: running ? null : "sent only if you approve it" };
      }
      const title = `${running ? "Using" : "Used"} ${d.tool} on ${d.server}`;
      if (d.error) return { title, detail: d.error };
      if (d.result_chars === null) return { title, detail: null };
      const parts = [
        `${d.result_chars.toLocaleString("en-US")} characters back`,
        d.truncated ? "truncated" : null,
        d.is_error ? "the tool reported an error" : null,
        seconds(d.duration_ms),
      ].filter(Boolean);
      return { title, detail: parts.join(" · ") };
    }
  }
}

function clip(text: string, max: number): string {
  return text.length > max ? `${text.slice(0, max - 1)}…` : text;
}

function stepItem(step: AgentStepView, agent: string, pendingMemory: ReadonlySet<string>): TimelineItem {
  const d = step.detail;
  const status = stepStatus(step);
  const finishedAt = step.finished_at_ms;
  const base = {
    id: step.step_id,
    status,
    agent,
    startedAt: step.started_at_ms,
    finishedAt,
    durationMs: finishedAt === null ? null : finishedAt - step.started_at_ms,
  };
  const { detail } = describeStep(step);
  switch (d.kind) {
    case "ModelCall":
      return {
        ...base,
        kind: "model",
        title:
          (status === "running" ? "Thinking" : d.backup ? "Backup model call" : "Model call") +
          (d.attempt ? ` · try ${d.attempt}` : ""),
        model: d.model,
        tool: d.backup ? "Backup" : null,
        durationMs: d.duration_ms ?? base.durationMs,
        outcome: detail,
        memoryItems: d.memory_items ?? 0,
        mcpAvailable: d.mcp_servers ?? undefined,
        facts: compact([
          ["Model", d.model],
          ["Approved memory sent", d.memory_items ? `${d.memory_items} ${d.memory_items === 1 ? "fact" : "facts"}` : null],
          [
            "Tokens",
            d.prompt_tokens !== null || d.output_tokens !== null
              ? `${(d.prompt_tokens ?? 0).toLocaleString("en-US")} sent · ${(d.output_tokens ?? 0).toLocaleString("en-US")} received`
              : null,
          ],
          [
            "MCP tools available",
            d.mcp_servers?.length
              ? `${d.mcp_servers.join(", ")} (${d.mcp_tools ?? 0} ${d.mcp_tools === 1 ? "tool" : "tools"})`
              : null,
          ],
          ["Try", d.attempt ? String(d.attempt) : null],
          ["Backup model", d.backup ? "yes — the active model stayed unavailable" : null],
          ["First response", seconds(d.first_byte_ms)],
          ["Duration", seconds(d.duration_ms)],
          ["Error", d.error],
        ]),
      };
    case "Fetch":
      return {
        ...base,
        kind: "web",
        title: `${status === "running" ? "Reading" : "Read"} ${hostOf(d.url)}`,
        model: null,
        tool: "Web read",
        outcome: detail,
        facts: compact([
          ["Action", `GET ${d.url}`],
          ["Final address", d.final_url !== d.url ? d.final_url : null],
          ["HTTP status", d.http_status === null ? null : String(d.http_status)],
          ["Text given to the model", d.text_bytes === null ? null : kib(d.text_bytes)],
          ["Truncated", d.truncated ? "yes" : null],
          ["Error", d.error],
        ]),
      };
    case "NoteProposed":
      return {
        ...base,
        kind: "note",
        title: `Propose note "${d.title}"`,
        model: null,
        tool: "Notes",
        outcome: d.error ?? (status === "completed" ? "Proposal recorded; nothing written yet" : null),
        facts: compact([
          ["Title", d.title],
          ["Run", d.run_id],
          ["Error", d.error],
        ]),
      };
    case "MemoryProposed": {
      const waiting = status === "completed" && d.memory_id !== null && pendingMemory.has(d.memory_id);
      return {
        ...base,
        kind: "memory",
        status: waiting ? "waiting" : status,
        title: `Propose to remember "${clip(d.content, 48)}"`,
        model: null,
        tool: "Memory",
        outcome:
          d.error ??
          (waiting
            ? "Waiting for your review; nothing is remembered until you approve"
            : status === "completed"
              ? "Proposal recorded; its decision is in Memory"
              : null),
        facts: compact([
          ["Fact", d.content],
          ["Scope", d.scope === "Project" ? "this project" : d.scope === "Personal" ? "personal (every project)" : null],
          ["Memory item", d.memory_id],
          ["Error", d.error],
        ]),
      };
    }
    case "FilesListed":
    case "DocumentRead":
      return {
        ...base,
        kind: "doc",
        title: describeStep(step).title,
        model: null,
        tool: "Documents",
        outcome: detail,
        facts: compact([
          ["Folder", d.folder],
          ["Path", d.path || null],
          ["Format", d.kind === "DocumentRead" ? d.format : null],
          ["Error", d.error],
        ]),
      };
    case "DocumentProposed":
      return {
        ...base,
        kind: "doc",
        title: `Propose ${d.file_name || "a file"} in ${d.folder}`,
        model: null,
        tool: "Documents",
        outcome: d.error ?? (status === "completed" ? "Proposal recorded; nothing written yet" : null),
        proposal: true,
        facts: compact([
          ["Folder", d.folder],
          ["File", d.file_name || null],
          ["Run", d.run_id],
          ["Error", d.error],
        ]),
      };
    case "RecordsFound":
      return {
        ...base,
        kind: "record",
        title: `${status === "running" ? "Looking up" : "Looked up"} ${d.register}`,
        model: null,
        tool: "Records",
        outcome: detail,
        facts: compact([
          ["Register", d.register],
          ["Asked for", d.query],
          ["Found", d.count === null ? null : String(d.count)],
          ["Error", d.error],
        ]),
      };
    case "RecordChanged":
      return {
        ...base,
        kind: "record",
        status: d.undone ? "cancelled" : status,
        title: describeStep(step).title,
        model: null,
        tool: "Records",
        outcome: detail,
        record: { changeId: d.change_id, recordId: d.record_id, undone: d.undone },
        facts: compact([
          ["Register", d.register],
          ["Record", d.title || null],
          ["Fields", d.fields.length > 0 ? d.fields.join(", ") : null],
          ["Undone", d.undone ? "yes, by you" : null],
          ["Error", d.error],
        ]),
      };
    case "McpCall":
      if (d.run_id)
        return {
          ...base,
          kind: "mcp",
          title: `Propose calling ${d.tool} · ${d.server}`,
          model: null,
          tool: "MCP",
          outcome: "Proposal recorded; nothing sent yet",
          mcp: { server: d.server, tool: d.tool },
          proposal: true,
          facts: compact([
            ["Server", d.server],
            ["Tool", d.tool],
            ["Arguments proposed", d.arguments || null],
            ["Run", d.run_id],
          ]),
        };
      return {
        ...base,
        kind: "mcp",
        title: `${status === "running" ? "Using" : "Used"} ${d.tool} · ${d.server}`,
        model: null,
        tool: "MCP",
        durationMs: d.duration_ms ?? base.durationMs,
        outcome: detail,
        mcp: { server: d.server, tool: d.tool },
        facts: compact([
          ["Server", d.server],
          ["Tool", d.tool],
          ["Arguments sent", d.arguments || null],
          ["Result given to the model", d.result_chars === null ? null : `${d.result_chars.toLocaleString("en-US")} characters`],
          ["Truncated", d.truncated ? "yes" : null],
          ["Tool reported an error", d.is_error ? "yes" : null],
          ["Duration", seconds(d.duration_ms)],
          ["Error", d.error],
        ]),
      };
  }
}

/** The approval and the call that follow a proposed call to an MCP tool
 *  that can change things, read from the real run. */
function callItems(run: RunView): TimelineItem[] {
  const call = run.call;
  const at = run.updated_at_ms;
  const what = call ? `Call ${call.tool} on ${call.server}` : run.title;
  const mcp = call ? { server: call.server, tool: call.tool } : undefined;
  const approval = (status: ExecStatus, outcome: string): TimelineItem => ({
    id: `${run.run_id}:approval`,
    kind: "approval",
    title: "Your approval",
    status,
    agent: "You",
    model: null,
    tool: null,
    startedAt: run.created_at_ms,
    finishedAt: status === "approval_required" ? null : at,
    durationMs: null,
    outcome,
    facts: compact([
      ["Action", what],
      ["Expires", run.pending_approval ? new Date(run.pending_approval.expires_at_ms).toLocaleTimeString("en-US") : null],
      ["Manifest", run.pending_approval?.manifest_sha256 ?? null],
    ]),
  });
  const made = (status: ExecStatus, outcome: string): TimelineItem => ({
    id: `${run.run_id}:call`,
    kind: "mcp",
    title: call ? `Call ${call.tool} · ${call.server}` : "Call",
    status,
    agent: "Host",
    model: null,
    tool: "MCP",
    startedAt: at,
    finishedAt: status === "completed" || status === "failed" || status === "cancelled" ? at : null,
    durationMs: call?.duration_ms ?? null,
    outcome,
    mcp,
    facts: compact([
      ["Server", call?.url ?? null],
      ["Arguments", call?.arguments_json ?? null],
      ["Answer", call?.result_chars != null ? `${call.result_chars.toLocaleString("en-US")} characters, given to the model, not kept` : null],
      ["Error", run.failure ? `${run.failure.code}: ${run.failure.message}` : null],
    ]),
  });
  switch (run.status) {
    case "AwaitingApproval":
      return [
        approval("approval_required", "Nothing is sent until you approve this exact call"),
        made("queued", "Runs once after your approval"),
      ];
    case "Queued":
      return [made("queued", "Preparing")];
    case "Running":
    case "Verifying":
      return [approval("completed", "Approved"), made("running", "Calling the server")];
    case "Succeeded":
      return [
        approval("completed", "Approved"),
        made(
          "completed",
          call?.result_chars != null ? `Made once; ${call.result_chars.toLocaleString("en-US")} characters back` : "Made once",
        ),
      ];
    case "Denied":
      return [approval("cancelled", "Rejected by you"), made("cancelled", "Not sent")];
    case "Failed":
      if (run.failure?.code === "ApprovalExpired")
        return [approval("failed", "Expired before a decision"), made("cancelled", "Not sent")];
      return [approval("completed", "Approved"), made("failed", run.failure?.message ?? "Failed")];
    case "Cancelled":
      return [made("cancelled", "Cancelled; not sent")];
    case "CancellationRequested":
      return [made("waiting", "Cancelling")];
    case "NeedsReconciliation":
    case "NeedsReview":
      return [
        approval("completed", "Approved"),
        made("waiting", "Needs your review: the call may have reached the server; it is not sent again"),
      ];
  }
}

/** The approval and the write that follow a note proposal, read from the
 *  real run. Only states the run proves are shown. */
export function noteItems(run: RunView): TimelineItem[] {
  if (run.kind === "McpCall") return callItems(run);
  const at = run.updated_at_ms;
  const isDocument = run.kind === "Document";
  const approval = (status: ExecStatus, outcome: string): TimelineItem => ({
    id: `${run.run_id}:approval`,
    kind: "approval",
    title: "Your approval",
    status,
    agent: "You",
    model: null,
    tool: null,
    startedAt: run.created_at_ms,
    finishedAt: status === "approval_required" ? null : at,
    durationMs: null,
    outcome,
    facts: compact([
      ["Action", isDocument ? `Create file ${run.title}` : `Create note "${run.title}"`],
      ["Expires", run.pending_approval ? new Date(run.pending_approval.expires_at_ms).toLocaleTimeString("en-US") : null],
      ["Manifest", run.pending_approval?.manifest_sha256 ?? null],
    ]),
  });
  const write = (status: ExecStatus, outcome: string): TimelineItem => ({
    id: `${run.run_id}:write`,
    kind: "write",
    title: isDocument ? `Write ${run.title}` : "Write note",
    status,
    agent: "Host",
    model: null,
    tool: isDocument ? "Documents" : "Files",
    startedAt: at,
    finishedAt: status === "completed" || status === "failed" || status === "cancelled" ? at : null,
    durationMs: null,
    outcome,
    facts: compact([
      ["Action", isDocument ? "CreateDocument" : "CreateMarkdownArtifact"],
      ["File", run.artifact?.path ?? null],
      ["SHA-256", run.artifact?.sha256 ?? null],
      ["Size", run.artifact ? kib(run.artifact.size_bytes) : null],
      ["Error", run.failure ? `${run.failure.code}: ${run.failure.message}` : null],
    ]),
  });
  switch (run.status) {
    case "AwaitingApproval":
      return [
        approval("approval_required", "Nothing is written until you approve the exact preview"),
        write("queued", "Runs after your approval"),
      ];
    case "Queued":
    case "Running":
      return [write("running", "Preparing")];
    case "Verifying":
      return [approval("completed", "Approved"), write("running", "Verifying the written file")];
    case "Succeeded":
      return [approval("completed", "Approved"), write("completed", "Written and verified against its SHA-256")];
    case "Denied":
      return [approval("cancelled", "Rejected by you"), write("cancelled", "Not written")];
    case "Failed":
      return run.failure?.code === "ApprovalExpired"
        ? [approval("failed", "Expired before a decision"), write("cancelled", "Not written")]
        : [write("failed", run.failure?.message ?? "Failed")];
    case "Cancelled":
      return [write("cancelled", "Cancelled")];
    case "CancellationRequested":
      return [write("waiting", "Cancelling")];
    case "NeedsReconciliation":
    case "NeedsReview":
      return [write("waiting", "Needs your review: the outcome could not be proven")];
  }
}

function compact(rows: [string, string | null | undefined][]): [string, string][] {
  return rows.filter((r): r is [string, string] => typeof r[1] === "string" && r[1] !== "");
}

function replyStatus(reply: ChatMessageView, note: RunView | undefined): ExecStatus {
  switch (reply.status) {
    case "Streaming":
      return "running";
    case "Failed":
      return "failed";
    case "Cancelled":
      return "cancelled";
    case "Complete": {
      if (!note) return "completed";
      const s = runStatus(note.status);
      // The reply is done; its task still waits on the note it proposed.
      return s === "approval_required" || s === "running" || s === "queued" || s === "waiting" ? s : "completed";
    }
  }
}

export function buildRuns(
  conversation: ConversationView,
  runsById: ReadonlyMap<string, RunView>,
  agent: string,
  /** Ids of the facts still waiting for the user's decision. */
  pendingMemory: ReadonlySet<string> = new Set(),
): RunVM[] {
  const out: RunVM[] = [];
  let request: string | null = null;
  // The run a model message continues: after the user decided on what the
  // previous reply proposed, the task goes on as more of the same run.
  let current: RunVM | null = null;
  for (const m of conversation.messages) {
    if (m.role === "User") {
      request = m.text;
      current = null;
      continue;
    }
    const note = m.proposed_run_id ? runsById.get(m.proposed_run_id) : proposedRun(m, runsById);
    const items = markRetries(m.steps.map((s) => stepItem(s, agent, pendingMemory)));
    if (note) items.push(...noteItems(note));
    const lastStep = m.steps.length > 0 ? m.steps[m.steps.length - 1] : null;
    const status = replyStatus(m, note);
    const settledAt = lastStep?.finished_at_ms ?? null;
    const finishedAt = status === "running" || status === "approval_required" ? null : (note?.updated_at_ms ?? settledAt);
    const proposed = m.proposed_run_id ?? note?.run_id ?? null;
    if (current) {
      current.items.push(...items);
      current.text = [current.text, m.text].filter(Boolean).join("\n\n");
      current.status = status;
      current.finishedAt = finishedAt;
      current.failure = m.failure;
      current.model = m.model ?? current.model;
      current.promptTokens = sum(current.promptTokens, m.prompt_tokens);
      current.outputTokens = sum(current.outputTokens, m.output_tokens);
      current.messageId = m.message_id;
      current.streaming = m.status === "Streaming";
      current.turn = m.turn ?? current.turn;
      if (proposed) current.proposedRunIds.push(proposed);
      continue;
    }
    current = {
      id: m.message_id,
      messageId: m.message_id,
      taskId: conversation.conversation_id,
      taskTitle: conversation.title,
      request,
      status,
      model: m.model,
      personaVersion: m.persona_version,
      startedAt: m.created_at_ms,
      finishedAt,
      items,
      text: m.text,
      failure: m.failure,
      promptTokens: m.prompt_tokens,
      outputTokens: m.output_tokens,
      proposedRunIds: proposed ? [proposed] : [],
      streaming: m.status === "Streaming",
      turn: m.turn,
    };
    out.push(current);
    request = null;
  }
  for (const run of out) noteUnusedMcp(run);
  return out;
}

/** A finished run whose model could call MCP tools but did not: said on
 *  its last model call, so "available but unused" is visible. */
function noteUnusedMcp(run: RunVM): void {
  if (run.status === "running" || run.items.some((i) => i.kind === "mcp")) return;
  const last = [...run.items].reverse().find((i) => i.kind === "model" && i.mcpAvailable?.length);
  if (!last?.mcpAvailable) return;
  const note = `${last.mcpAvailable.join(", ")} available; answered without it`;
  last.outcome = last.outcome ? `${last.outcome} · ${note}` : note;
}

function sum(a: number | null, b: number | null): number | null {
  return a === null && b === null ? null : (a ?? 0) + (b ?? 0);
}

/** A model call that failed and was followed by another model call was a
 *  temporary outage the Host tried again (or handed to the backup model). */
function markRetries(items: TimelineItem[]): TimelineItem[] {
  return items.map((item, i) => {
    const next = items[i + 1];
    if (item.kind !== "model" || item.status !== "failed" || next?.kind !== "model") return item;
    const how = next.tool === "Backup" ? "the backup model took over" : "tried again";
    return { ...item, outcome: `Temporary outage — ${how}. ${item.outcome ?? ""}`.trim() };
  });
}

/** While a reply is still settling, only its proposal step names the run. */
function proposedRun(m: ChatMessageView, runsById: ReadonlyMap<string, RunView>): RunView | undefined {
  for (const s of m.steps) {
    if ((s.detail.kind === "NoteProposed" || s.detail.kind === "McpCall" || s.detail.kind === "DocumentProposed") && s.detail.run_id)
      return runsById.get(s.detail.run_id);
  }
  return undefined;
}

/** What happens next, stated only from facts the Host has recorded. */
export function nextLine(run: RunVM | null): string {
  if (!run) return "Nothing is queued. Give Nexus a task below.";
  const running = run.items.find((i) => i.status === "running");
  if (running?.kind === "model")
    return "The model is answering. It may read public web pages, use the MCP tools you enabled, propose a note or propose a fact to remember; anything that writes or is remembered waits for you.";
  if (running?.kind === "web")
    return "The page text goes back to the model, which then continues the answer.";
  if (running?.kind === "mcp")
    return "The tool's answer goes back to the model as data, which then continues the answer.";
  if (running?.kind === "write") return "The Host writes the note, then verifies the file against its SHA-256.";
  if (run.items.some((i) => i.status === "approval_required"))
    return "Waiting for your approval. Nothing is written or sent until you approve; after your decision, Nexus continues the task.";
  if (run.items.some((i) => i.kind === "memory" && i.status === "waiting"))
    return "A proposed fact waits for your review. Nothing is remembered until you approve it.";
  if (run.status === "running") return "The reply is still streaming.";
  return "Nothing is queued. Give Nexus a task below.";
}

// ---------- Core state and the context nodes around it ----------

export type NodeKind = "model" | "web" | "files" | "memory" | "mcp" | "records" | "approval";

export interface ContextNode {
  id: NodeKind;
  label: string;
  sublabel: string | null;
  status: "active" | "waiting" | "failed" | "done";
  /** Only proposed so far: nothing has been executed on this node yet (a
   *  note waiting for approval, a fact waiting for review). */
  proposed: boolean;
}

/** How the followed run ended. It stays until the next run starts, so the
 *  outcome is never lost when the core's picture settles. */
export interface CoreOutcome {
  result: "completed" | "failed" | "cancelled";
  text: string;
  at: number;
}

/** What the Nexus Core draws, derived only from state the dashboard already
 *  has. New sources (more agents, a planner) extend this, not the drawing. */
export interface NexusCoreVisualState {
  state: CoreState;
  /** The context node the work flows to now: the tool in use, the model, or
   *  the approval the core waits on. */
  activeTool: NodeKind | null;
  activeModel: string | null;
  /** 0..1 while working: how many steps the current run has taken. */
  activityIntensity: number;
  hasPendingApproval: boolean;
  /** How the followed run ended, while that is recent. */
  lastResult: "success" | "error" | null;
}

export interface CoreView {
  state: CoreState;
  /** Nodes involved in the current run, in the order they were used. */
  nodes: ContextNode[];
  /** Node ids in execution order, from the core and back to it. */
  path: NodeKind[];
  active: NodeKind | null;
  label: string;
  visual: NexusCoreVisualState;
  outcome: CoreOutcome | null;
}

const NODE_OF: Record<ItemKind, NodeKind> = {
  model: "model",
  web: "web",
  note: "files",
  memory: "memory",
  mcp: "mcp",
  record: "records",
  doc: "files",
  write: "files",
  approval: "approval",
};

/** A finished run keeps its nodes on screen this long. */
export const RECENT_MS = 90_000;
const SUCCESS_MS = 2_500;
const ERROR_MS = 8_000;

export function deriveCore(input: {
  run: RunVM | null;
  pendingApprovals: number;
  composing: boolean;
  now: number;
}): CoreView {
  const { run, now } = input;
  const relevant =
    run !== null &&
    (run.finishedAt === null || now - run.finishedAt < RECENT_MS || run.items.some((i) => i.status === "approval_required"));
  const items = relevant && run ? run.items : [];

  const nodes: ContextNode[] = [];
  const path: NodeKind[] = [];
  for (const item of items) {
    // Approved memory is read by the Host before the request it goes with.
    if (item.memoryItems) {
      if (path[path.length - 1] !== "memory") path.push("memory");
      let memory = nodes.find((n) => n.id === "memory");
      if (!memory) {
        memory = { id: "memory", label: "Memory", sublabel: null, status: "done", proposed: false };
        nodes.push(memory);
      }
      memory.sublabel = `${item.memoryItems} ${item.memoryItems === 1 ? "fact" : "facts"} sent`;
    }
    const id = NODE_OF[item.kind];
    if (path[path.length - 1] !== id) path.push(id);
    let node = nodes.find((n) => n.id === id);
    if (!node) {
      node = { id, label: nodeLabel(id, item), sublabel: null, status: "done", proposed: false };
      nodes.push(node);
    }
    // Intent and execution are told apart: a proposal stays a proposal
    // until the Host executes it (or it is decided).
    if (item.kind === "note") node.proposed = true;
    else if (item.kind === "doc") node.proposed = item.proposal === true;
    else if (item.kind === "write") node.proposed = item.status === "queued";
    else if (item.kind === "memory") node.proposed = item.status === "waiting";
    else if (item.kind === "mcp") node.proposed = item.proposal === true || item.status === "queued";
    node.sublabel = nodeSublabel(id, item) ?? node.sublabel;
    // The model node names whichever model answered last (e.g. the backup),
    // the MCP node whichever server was used last.
    if (id === "model" || id === "mcp") node.label = nodeLabel(id, item);
    node.status = mergeNodeStatus(node.status, item.status);
  }

  const running = items.find((i) => i.status === "running");
  const waiting = items.find((i) => i.status === "approval_required");
  const active = running ? NODE_OF[running.kind] : waiting ? "approval" : null;

  let state: CoreState = "IDLE";
  let label = "Idle";
  if (running) {
    if (running.kind === "web") {
      state = "USING_TOOL";
      label = running.title;
    } else if (running.kind === "write") {
      state = "EXECUTING";
      label = "Writing the approved note";
    } else if (running.kind === "memory") {
      state = "USING_TOOL";
      label = "Proposing a fact to remember";
    } else if (running.kind === "doc") {
      state = "USING_TOOL";
      label = running.title;
    } else if (running.kind === "record") {
      state = "USING_TOOL";
      label = running.title;
    } else if (running.kind === "mcp") {
      state = "USING_TOOL";
      label = running.mcp ? `Using ${running.mcp.tool} on ${running.mcp.server}` : running.title;
    } else {
      state = "THINKING";
      label = running.model ? `Thinking — ${running.model}` : "Thinking";
    }
  } else if (waiting || input.pendingApprovals > 0) {
    state = "WAITING_APPROVAL";
    label = input.pendingApprovals > 1 ? `${input.pendingApprovals} approvals waiting` : "Waiting for your approval";
  } else if (run?.status === "running") {
    state = "THINKING";
    label = "Answering";
  } else if (input.composing) {
    state = "LISTENING";
    label = "Listening";
  } else if (run && run.finishedAt !== null && run.status === "completed" && now - run.finishedAt < SUCCESS_MS) {
    state = "SUCCESS";
    label = "Done";
  } else if (run && run.finishedAt !== null && run.status === "failed" && now - run.finishedAt < ERROR_MS) {
    state = "ERROR";
    label = run.failure?.message ?? "The last run failed";
  }
  const finished = relevant && run !== null && run.finishedAt !== null;
  const visual: NexusCoreVisualState = {
    state,
    activeTool: active,
    activeModel: running?.model ?? (relevant ? (run?.model ?? null) : null),
    activityIntensity: running ? Math.min(1, items.length / 6) : 0,
    hasPendingApproval: input.pendingApprovals > 0 || waiting !== undefined,
    lastResult: finished && run?.status === "completed" ? "success" : finished && run?.status === "failed" ? "error" : null,
  };
  return { state, nodes, path, active, label, visual, outcome: outcomeOf(run) };
}

function outcomeOf(run: RunVM | null): CoreOutcome | null {
  if (!run || run.finishedAt === null) return null;
  switch (run.status) {
    case "completed":
      return { result: "completed", text: "Last task completed", at: run.finishedAt };
    case "failed":
      return { result: "failed", text: `Last task failed: ${run.failure?.message ?? "see the task"}`, at: run.finishedAt };
    case "cancelled":
      return { result: "cancelled", text: "Last task stopped", at: run.finishedAt };
    default:
      return null;
  }
}

function nodeLabel(id: NodeKind, item: TimelineItem): string {
  switch (id) {
    case "model":
      return item.model ?? "Model";
    case "web":
      return "Web read";
    case "files":
      return item.kind === "doc" || item.tool === "Documents" ? "Documents" : "Notes";
    case "memory":
      return "Memory";
    case "mcp":
      return item.mcp?.server ?? "MCP";
    case "records":
      return "Records";
    case "approval":
      return "You";
  }
}

function nodeSublabel(id: NodeKind, item: TimelineItem): string | null {
  switch (id) {
    case "model":
      return item.tool === "Backup" ? "backup model" : "model";
    case "web":
      return item.title.replace(/^(Reading|Read) /, "");
    case "files":
      if (item.kind === "doc") return item.proposal ? "proposal" : "read";
      return item.kind === "write" ? "write" : "proposal";
    case "memory":
      return item.status === "waiting" ? "proposal · your review" : "proposal";
    case "mcp":
      return item.mcp?.tool ?? "MCP tool";
    case "records":
      return item.record ? (item.record.undone ? "change undone" : "saved") : "lookup";
    case "approval":
      return "approval";
  }
}

const NODE_RANK: Record<ContextNode["status"], number> = { done: 0, failed: 1, waiting: 2, active: 3 };

function mergeNodeStatus(current: ContextNode["status"], item: ExecStatus): ContextNode["status"] {
  const next: ContextNode["status"] =
    item === "running"
      ? "active"
      : item === "approval_required" || item === "queued" || item === "waiting"
        ? "waiting"
        : item === "failed"
          ? "failed"
          : "done";
  return NODE_RANK[next] > NODE_RANK[current] ? next : current;
}

// ---------- Tasks ----------

export interface TaskRow {
  id: string;
  projectId: ProjectId | null;
  title: string;
  updatedAt: number;
  /** Known only for tasks with recent steps; `null` when not loaded. */
  status: ExecStatus | null;
}

/** Status of each task from the steps and runs already on hand. */
export function taskRows(
  conversations: ConversationSummary[],
  steps: AgentStepView[],
  runs: RunView[],
): TaskRow[] {
  const pendingByConversation = new Set(
    runs.filter((r) => r.status === "AwaitingApproval" && r.origin).map((r) => r.origin!.conversation_id),
  );
  return conversations.map((c) => {
    const own = steps.filter((s) => s.conversation_id === c.conversation_id);
    let status: ExecStatus | null = null;
    if (own.some((s) => s.status === "Running")) status = "running";
    else if (pendingByConversation.has(c.conversation_id)) status = "approval_required";
    else if (own.length > 0) status = stepStatus(own[0]) === "failed" ? "failed" : "completed";
    return { id: c.conversation_id, projectId: c.project_id, title: c.title, updatedAt: c.updated_at_ms, status };
  });
}

/** Which task the live panel follows: the one working now, else the most
 *  recently active one. */
export function followedTask(steps: AgentStepView[], runs: RunView[]): string | null {
  const running = steps.find((s) => s.status === "Running");
  if (running) return running.conversation_id;
  const waiting = runs.find((r) => r.status === "AwaitingApproval" && r.origin);
  if (waiting?.origin) return waiting.origin.conversation_id;
  return steps.length > 0 ? steps[0].conversation_id : null;
}

// ---------- Models ----------

export interface ModelStats {
  model: string;
  calls: number;
  failures: number;
  medianFirstByteMs: number | null;
  medianDurationMs: number | null;
  lastUsedAt: number;
  running: boolean;
}

export function modelStats(steps: AgentStepView[]): ModelStats[] {
  const by = new Map<string, AgentStepView[]>();
  for (const s of steps) {
    if (s.detail.kind !== "ModelCall") continue;
    const list = by.get(s.detail.model) ?? [];
    list.push(s);
    by.set(s.detail.model, list);
  }
  return [...by.entries()].map(([model, list]) => {
    const firsts: number[] = [];
    const totals: number[] = [];
    for (const s of list) {
      if (s.detail.kind !== "ModelCall" || s.status !== "Done") continue;
      if (s.detail.first_byte_ms !== null) firsts.push(s.detail.first_byte_ms);
      if (s.detail.duration_ms !== null) totals.push(s.detail.duration_ms);
    }
    return {
      model,
      calls: list.length,
      failures: list.filter((s) => s.status === "Failed").length,
      medianFirstByteMs: median(firsts),
      medianDurationMs: median(totals),
      lastUsedAt: Math.max(...list.map((s) => s.started_at_ms)),
      running: list.some((s) => s.status === "Running"),
    };
  });
}

// ---------- Activity log ----------

export type ActivityTool = "Model" | "Web read" | "Notes" | "Documents" | "Memory" | "Records" | "MCP" | "Automations" | "System";

export interface ActivityEntry {
  id: string;
  at: number;
  source: "agent" | "host";
  title: string;
  detail: string | null;
  taskId: string | null;
  taskTitle: string | null;
  /** The task's project; `null` for entries tied to no task. */
  projectId: ProjectId | null;
  tool: ActivityTool;
  model: string | null;
  status: ExecStatus | null;
}

const EVENT_TEXT: Record<string, { title: string; tool: ActivityTool; status?: ExecStatus }> = {
  RunCreated: { title: "Note requested", tool: "Notes", status: "queued" },
  ApprovalRequested: { title: "Approval requested", tool: "Notes", status: "approval_required" },
  ApprovalDecided: { title: "Approval decided", tool: "Notes" },
  ApprovalExpired: { title: "Approval expired", tool: "Notes", status: "failed" },
  ApprovalRevoked: { title: "Approval revoked", tool: "Notes", status: "cancelled" },
  OperationIntended: { title: "Write intent recorded before writing", tool: "Notes" },
  OperationRetried: { title: "Write retried after a restart", tool: "Notes" },
  ArtifactCreated: { title: "Note written and verified", tool: "Notes", status: "completed" },
  MessageSent: { title: "Task request recorded", tool: "System" },
  MessageSettled: { title: "Reply settled", tool: "System" },
  MessagesInterrupted: { title: "Replies interrupted by a Host stop", tool: "System", status: "failed" },
  ConversationDeleted: { title: "Task deleted with its messages", tool: "System" },
  ExchangeDeleted: { title: "A request and its reply deleted", tool: "System" },
  AgentStepsInterrupted: { title: "Unfinished steps closed (stop or restart)", tool: "System" },
  ConnectionSaved: { title: "AI connection saved", tool: "System" },
  ConnectionRemoved: { title: "AI connection removed", tool: "System" },
  ConnectionModelSelected: { title: "Model selected", tool: "System" },
  ConnectionThinkingLevelSet: { title: "Thinking level changed", tool: "System" },
  ActiveConnectionChanged: { title: "Active connection changed", tool: "System" },
  PersonaSaved: { title: "Persona saved", tool: "System" },
  ProjectCreated: { title: "Project created", tool: "System" },
  ProjectUpdated: { title: "Project renamed or described", tool: "System" },
  ActiveProjectChanged: { title: "Active project changed", tool: "System" },
  MemoryAdded: { title: "Fact added by you", tool: "Memory", status: "completed" },
  MemoryEdited: { title: "Fact corrected", tool: "Memory" },
  MemoryDecided: { title: "Proposed fact decided", tool: "Memory" },
  MemoryDeleted: { title: "Fact deleted", tool: "Memory" },
  DailyTokenBudgetSet: { title: "Daily token budget changed", tool: "System" },
  ModelPriceSet: { title: "Model price changed", tool: "System" },
  McpServerAdded: { title: "MCP server added", tool: "MCP" },
  McpServerUpdated: { title: "MCP server changed", tool: "MCP" },
  McpServerRemoved: { title: "MCP server removed with its tools and key", tool: "MCP" },
  McpToolsListed: { title: "MCP server's tools listed", tool: "MCP" },
  McpServerCheckFailed: { title: "MCP server could not be asked for its tools", tool: "MCP", status: "failed" },
  McpToolEnabled: { title: "MCP tool enabled", tool: "MCP" },
  McpToolDisabled: { title: "MCP tool turned off", tool: "MCP" },
  RemoteCallCompleted: { title: "Approved MCP call made once", tool: "MCP", status: "completed" },
  FolderShared: { title: "Folder shared with Nexus", tool: "Documents" },
  FolderUpdated: { title: "Shared folder changed", tool: "Documents" },
  FolderUnshared: { title: "Folder no longer shared", tool: "Documents" },
  DocumentCreated: { title: "Approved file written and verified", tool: "Documents", status: "completed" },
  RecordsImported: { title: "Records imported from a file", tool: "Records", status: "completed" },
  InboxUpdated: { title: "Inbox folder changed", tool: "Records" },
  InboxFileSeen: { title: "New file seen in the inbox", tool: "Records" },
  InboxFileHandled: { title: "Inbox file sent to the agent", tool: "Records" },
  InboxFileRetried: { title: "Inbox file put back in line", tool: "Records" },
  InterventionRecorded: { title: "Your decision about the agent's work recorded", tool: "System" },
  InboxFileSettled: { title: "Inbox file's task ended", tool: "Records" },
  RecordAdded: { title: "Record added", tool: "Records" },
  RecordUpdated: { title: "Record changed", tool: "Records" },
  RecordDeleted: { title: "Record deleted with its history", tool: "Records" },
  RecordChangeUndone: { title: "A record change undone", tool: "Records" },
  AutomationCreated: { title: "Automation created", tool: "Automations" },
  AutomationUpdated: { title: "Automation changed", tool: "Automations" },
  AutomationDeleted: { title: "Automation deleted (its tasks stay)", tool: "Automations" },
  AutomationRan: { title: "Automation time recorded (started, skipped, missed or not started)", tool: "Automations" },
};

/** Run events read differently when the run is a call to an MCP tool. */
const CALL_EVENT_TEXT: Record<string, string> = {
  RunCreated: "MCP call requested",
  OperationIntended: "Call intent recorded before calling",
};

/** Events whose facts the agent steps already show in more detail, or that
 *  only restate a change already listed. */
const SKIPPED_EVENTS = new Set([
  "MemoryProposed",
  "AgentStepStarted",
  "AgentStepFinished",
  "RunStateChanged",
  "ActionStateChanged",
  "OperationStateChanged",
]);

export function buildActivity(input: {
  steps: AgentStepView[];
  events: EventView[];
  runs: RunView[];
  conversations: ConversationSummary[];
  agent: string;
}): ActivityEntry[] {
  const projectOf = new Map(input.conversations.map((c) => [c.conversation_id as string, c.project_id]));
  const entries: ActivityEntry[] = input.steps.map((s) => {
    const { title, detail } = describeStep(s);
    const tool: ActivityTool = TOOL_OF_STEP[s.detail.kind];
    return {
      id: s.step_id,
      at: s.started_at_ms,
      source: "agent",
      title,
      detail,
      taskId: s.conversation_id,
      taskTitle: s.conversation_title,
      projectId: projectOf.get(s.conversation_id) ?? null,
      tool,
      model: s.detail.kind === "ModelCall" ? s.detail.model : null,
      status: stepStatus(s),
    };
  });

  const titles = new Map(input.conversations.map((c) => [c.conversation_id as string, c.title]));
  const runOf = new Map<string, RunView>();
  for (const r of input.runs) {
    runOf.set(r.run_id, r);
    if (r.artifact) runOf.set(r.artifact.artifact_id, r);
    if (r.pending_approval) runOf.set(r.pending_approval.approval_id, r);
  }
  for (const e of input.events) {
    if (SKIPPED_EVENTS.has(e.kind)) continue;
    const run = runOf.get(e.entity_id);
    const known =
      run?.kind === "McpCall" && EVENT_TEXT[e.kind]
        ? { ...EVENT_TEXT[e.kind], tool: "MCP" as ActivityTool, title: CALL_EVENT_TEXT[e.kind] ?? EVENT_TEXT[e.kind].title }
        : EVENT_TEXT[e.kind];
    const conversationTitle = titles.get(e.entity_id) ?? null;
    const taskId = conversationTitle ? e.entity_id : (run?.origin?.conversation_id ?? null);
    entries.push({
      id: `event-${e.seq}`,
      at: e.created_at_ms,
      source: "host",
      title: known?.title ?? splitWords(e.kind),
      detail: run ? `"${run.title}"` : null,
      taskId,
      taskTitle: conversationTitle ?? run?.origin?.conversation_title ?? null,
      projectId: taskId ? (projectOf.get(taskId) ?? null) : null,
      tool: known?.tool ?? "System",
      model: null,
      status: known?.status ?? null,
    });
  }
  return entries.sort((a, b) => b.at - a.at);
}

const TOOL_OF_STEP: Record<AgentStepView["detail"]["kind"], ActivityTool> = {
  ModelCall: "Model",
  Fetch: "Web read",
  NoteProposed: "Notes",
  MemoryProposed: "Memory",
  McpCall: "MCP",
  RecordsFound: "Records",
  RecordChanged: "Records",
  FilesListed: "Documents",
  DocumentRead: "Documents",
  DocumentProposed: "Documents",
};

function splitWords(kind: string): string {
  const words = kind.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

// ---------- Event-driven refresh ----------

export type Collection =
  | "runs"
  | "steps"
  | "conversations"
  | "connections"
  | "persona"
  | "projects"
  | "memory"
  | "mcp"
  | "usage"
  | "automations"
  | "registers"
  | "folders"
  | "focus";

/** What each durable event invalidates. Events are notifications only;
 *  the UI refetches the authoritative state, it never applies them. */
export function invalidatedBy(events: EventView[]): Set<Collection> {
  const out = new Set<Collection>();
  for (const e of events) {
    const k = e.kind;
    if (k.startsWith("Run") || k.startsWith("Approval") || k.startsWith("Action") || k.startsWith("Operation") || k.startsWith("Artifact")) {
      out.add("runs");
      out.add("focus");
    } else if (k.startsWith("AgentStep")) {
      out.add("steps");
      out.add("focus");
      // A finished model call adds to today's tokens.
      out.add("usage");
    } else if (k.startsWith("Message") || k === "InterventionRecorded") {
      out.add("conversations");
      out.add("steps");
      out.add("focus");
      // A new task changes its project's task count.
      out.add("projects");
    } else if (k === "ConversationDeleted" || k === "ExchangeDeleted") {
      out.add("conversations");
      out.add("steps");
      out.add("focus");
      out.add("projects");
      // Notes and facts it led to lose their link to it.
      out.add("runs");
      out.add("memory");
      // An automation's run keeps its entry without the task.
      out.add("automations");
    } else if (k.startsWith("Memory")) {
      out.add("memory");
      out.add("projects");
      // A pending proposal shown in a task's steps changes with its decision.
      out.add("focus");
    } else if (k.startsWith("Project") || k === "ActiveProjectChanged") {
      out.add("projects");
    } else if (k.startsWith("Connection") || k === "ActiveConnectionChanged") {
      out.add("connections");
      // The chosen models are listed with their usage.
      out.add("usage");
    } else if (k === "PersonaSaved") {
      out.add("persona");
    } else if (k.startsWith("Mcp")) {
      out.add("mcp");
    } else if (k === "DailyTokenBudgetSet" || k === "ModelPriceSet") {
      out.add("usage");
    } else if (k.startsWith("Automation")) {
      out.add("automations");
    } else if (k.startsWith("Folder")) {
      out.add("folders");
    } else if (k === "DocumentCreated") {
      out.add("runs");
      out.add("focus");
    } else if (k.startsWith("Inbox")) {
      out.add("registers");
    } else if (k.startsWith("Record")) {
      out.add("registers");
      // An undone change shows on the agent's step in its task.
      if (k === "RecordChangeUndone") {
        out.add("steps");
        out.add("focus");
      }
    }
  }
  return out;
}

const INTERVENTION_TEXT: Record<string, string> = {
  Rejected: "rejected",
  Undo: "undid",
  Correction: "corrected",
  Stopped: "stopped",
};

/** One line on how a turn stands (ADR-TASK-001): in review until a date,
 *  accepted, or not accepted because the user rejected, undid, corrected
 *  or stopped what it did. `null` for a turn still running or waiting. */
export function turnLine(turn: TurnView | null): string | null {
  if (!turn) return null;
  switch (turn.acceptance) {
    case "InReview":
      return turn.review_until_ms === null
        ? "In review"
        : `In review until ${new Date(turn.review_until_ms).toLocaleDateString("en-GB", { day: "numeric", month: "short" })}: undo or correct it before then and it does not count as accepted`;
    case "Accepted":
      return "Accepted: kept for 7 days without being undone or corrected";
    case "NotAccepted": {
      const did = [...new Set(turn.interventions.map((k) => INTERVENTION_TEXT[k]).filter(Boolean))];
      return `Not accepted: you ${did.join(", ") || "changed"} what it did`;
    }
    default:
      return null;
  }
}

const EFFECT_TEXT: Record<string, string> = {
  Read: "Only reads",
  InternalReversible: "Changes only Nexus's own data; can be undone",
  ExternalReversible: "Changes something outside Nexus; can be undone",
  ExternalIrreversible: "Changes something outside Nexus; cannot be undone",
};

const EVIDENCE_TEXT: Record<string, string> = {
  Verified: "proven by reading the result back",
  Reported: "as the service reported it, not checked independently",
  Unverifiable: "the result cannot be checked",
};

/** What kind of effect a run has and how its success is proven
 *  (ADR-ACTION-001), in one line; `null` when the run does not say. */
export function effectText(effectClass: string | null, evidence: string | null): string | null {
  const effect = effectClass ? EFFECT_TEXT[effectClass] : undefined;
  if (!effect) return null;
  const proof = evidence ? EVIDENCE_TEXT[evidence] : undefined;
  return proof ? `${effect} · ${proof}` : effect;
}

/** Who did or asked for something, as the user reads it (ADR-IDENTITY-001). */
export function actorText(actor: ActorView | null, agentName: string): string | null {
  if (!actor) return null;
  switch (actor.kind) {
    case "User":
      return "you";
    case "Agent":
      return agentName;
    case "Automation":
      return `the automation “${actor.name}”`;
    case "System":
      return "the Nexus Host";
    default:
      return actor.name;
  }
}

/** Who asked for an effect and who decided it: “Asked by Nexus · approved by you”. */
export function actorsLine(run: RunView, agentName: string): string | null {
  const asked = actorText(run.requested_by, agentName);
  const decided = actorText(run.decided_by, agentName);
  const parts: string[] = [];
  if (asked) parts.push(`Asked by ${asked}`);
  if (decided) parts.push(`${run.status === "Denied" ? "rejected" : "approved"} by ${decided}`);
  if (parts.length === 0) return null;
  const line = parts.join(" · ");
  return line.charAt(0).toUpperCase() + line.slice(1);
}
