// The dashboard's view of autonomous work, derived from what the Host
// actually records. Pure functions only (no React, no IPC), so the mapping
// can be tested on its own.
//
// Hierarchy shown to the user, and where each level comes from today:
//   Task   — a conversation (work for the agent), or a note the user
//            submitted directly (an M1 Task).
//   Run    — one reply of the model to a request in that task, or an M1 run.
//   Step   — one recorded agent step: a model call, a web read, a proposal.
//   Action — the effect a step leads to: the GET of a web read, or the
//            CreateMarkdownArtifact action with its approval and write.
// The Host does not yet record agent work as Task/Run rows of its own; this
// module is the single place that maps it, so it can be replaced when it does.

import type {
  AgentStepView,
  ChatMessageView,
  ConversationSummary,
  ConversationView,
  EventView,
  RunId,
  RunStatus,
  RunView,
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

export type ItemKind = "model" | "web" | "note" | "approval" | "write";

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
  proposedRunId: RunId | null;
  messageId: string;
  streaming: boolean;
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
  }
}

function stepItem(step: AgentStepView, agent: string): TimelineItem {
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
        facts: compact([
          ["Model", d.model],
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
  }
}

/** The approval and the write that follow a note proposal, read from the
 *  real run. Only states the run proves are shown. */
export function noteItems(run: RunView): TimelineItem[] {
  const at = run.updated_at_ms;
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
      ["Action", `Create note "${run.title}"`],
      ["Expires", run.pending_approval ? new Date(run.pending_approval.expires_at_ms).toLocaleTimeString("en-US") : null],
      ["Manifest", run.pending_approval?.manifest_sha256 ?? null],
    ]),
  });
  const write = (status: ExecStatus, outcome: string): TimelineItem => ({
    id: `${run.run_id}:write`,
    kind: "write",
    title: "Write note",
    status,
    agent: "Host",
    model: null,
    tool: "Files",
    startedAt: at,
    finishedAt: status === "completed" || status === "failed" || status === "cancelled" ? at : null,
    durationMs: null,
    outcome,
    facts: compact([
      ["Action", "CreateMarkdownArtifact"],
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
): RunVM[] {
  const out: RunVM[] = [];
  let request: string | null = null;
  for (const m of conversation.messages) {
    if (m.role === "User") {
      request = m.text;
      continue;
    }
    const note = m.proposed_run_id ? runsById.get(m.proposed_run_id) : proposedRun(m, runsById);
    const items = markRetries(m.steps.map((s) => stepItem(s, agent)));
    if (note) items.push(...noteItems(note));
    const lastStep = m.steps.length > 0 ? m.steps[m.steps.length - 1] : null;
    const status = replyStatus(m, note);
    const settledAt = lastStep?.finished_at_ms ?? null;
    out.push({
      id: m.message_id,
      messageId: m.message_id,
      taskId: conversation.conversation_id,
      taskTitle: conversation.title,
      request,
      status,
      model: m.model,
      personaVersion: m.persona_version,
      startedAt: m.created_at_ms,
      finishedAt: status === "running" || status === "approval_required" ? null : (note?.updated_at_ms ?? settledAt),
      items,
      text: m.text,
      failure: m.failure,
      promptTokens: m.prompt_tokens,
      outputTokens: m.output_tokens,
      proposedRunId: m.proposed_run_id ?? note?.run_id ?? null,
      streaming: m.status === "Streaming",
    });
    request = null;
  }
  return out;
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

/** While a reply is still settling, only its NoteProposed step names the run. */
function proposedRun(m: ChatMessageView, runsById: ReadonlyMap<string, RunView>): RunView | undefined {
  for (const s of m.steps) {
    if (s.detail.kind === "NoteProposed" && s.detail.run_id) return runsById.get(s.detail.run_id);
  }
  return undefined;
}

/** What happens next, stated only from facts the Host has recorded. */
export function nextLine(run: RunVM | null): string {
  if (!run) return "Nothing is queued. Give Nexus a task below.";
  const running = run.items.find((i) => i.status === "running");
  if (running?.kind === "model")
    return "The model is answering. It may read public web pages or propose a note; anything that writes waits for you.";
  if (running?.kind === "web")
    return "The page text goes back to the model, which then continues the answer.";
  if (running?.kind === "write") return "The Host writes the note, then verifies the file against its SHA-256.";
  if (run.items.some((i) => i.status === "approval_required"))
    return "Waiting for your approval. Nothing is written until you approve.";
  if (run.status === "running") return "The reply is still streaming.";
  return "Nothing is queued. Give Nexus a task below.";
}

// ---------- Core state and the context nodes around it ----------

export type NodeKind = "model" | "web" | "files" | "approval";

export interface ContextNode {
  id: NodeKind;
  label: string;
  sublabel: string | null;
  status: "active" | "waiting" | "failed" | "done";
}

export interface CoreView {
  state: CoreState;
  /** Nodes involved in the current run, in the order they were used. */
  nodes: ContextNode[];
  /** Node ids in execution order, from the core and back to it. */
  path: NodeKind[];
  active: NodeKind | null;
  label: string;
}

const NODE_OF: Record<ItemKind, NodeKind> = {
  model: "model",
  web: "web",
  note: "files",
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
    const id = NODE_OF[item.kind];
    if (path[path.length - 1] !== id) path.push(id);
    let node = nodes.find((n) => n.id === id);
    if (!node) {
      node = { id, label: nodeLabel(id, item), sublabel: null, status: "done" };
      nodes.push(node);
    }
    node.sublabel = nodeSublabel(id, item) ?? node.sublabel;
    // The model node names whichever model answered last (e.g. the backup).
    if (id === "model") node.label = nodeLabel(id, item);
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
  return { state, nodes, path, active, label };
}

function nodeLabel(id: NodeKind, item: TimelineItem): string {
  switch (id) {
    case "model":
      return item.model ?? "Model";
    case "web":
      return "Web read";
    case "files":
      return "Notes";
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
      return item.kind === "write" ? "write" : "proposal";
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
    return { id: c.conversation_id, title: c.title, updatedAt: c.updated_at_ms, status };
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

export type ActivityTool = "Model" | "Web read" | "Notes" | "System";

export interface ActivityEntry {
  id: string;
  at: number;
  source: "agent" | "host";
  title: string;
  detail: string | null;
  taskId: string | null;
  taskTitle: string | null;
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
  AgentStepsInterrupted: { title: "Unfinished steps closed (stop or restart)", tool: "System" },
  ConnectionSaved: { title: "AI connection saved", tool: "System" },
  ConnectionRemoved: { title: "AI connection removed", tool: "System" },
  ConnectionModelSelected: { title: "Model selected", tool: "System" },
  ConnectionThinkingLevelSet: { title: "Thinking level changed", tool: "System" },
  ActiveConnectionChanged: { title: "Active connection changed", tool: "System" },
  PersonaSaved: { title: "Persona saved", tool: "System" },
  ProjectCreated: { title: "Workspace created", tool: "System" },
};

/** Events whose facts the agent steps already show in more detail, or that
 *  only restate a change already listed. */
const SKIPPED_EVENTS = new Set([
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
  const entries: ActivityEntry[] = input.steps.map((s) => {
    const { title, detail } = describeStep(s);
    const tool: ActivityTool = s.detail.kind === "ModelCall" ? "Model" : s.detail.kind === "Fetch" ? "Web read" : "Notes";
    return {
      id: s.step_id,
      at: s.started_at_ms,
      source: "agent",
      title,
      detail,
      taskId: s.conversation_id,
      taskTitle: s.conversation_title,
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
    const known = EVENT_TEXT[e.kind];
    const run = runOf.get(e.entity_id);
    const conversationTitle = titles.get(e.entity_id) ?? null;
    entries.push({
      id: `event-${e.seq}`,
      at: e.created_at_ms,
      source: "host",
      title: known?.title ?? splitWords(e.kind),
      detail: run ? `"${run.title}"` : null,
      taskId: conversationTitle ? e.entity_id : (run?.origin?.conversation_id ?? null),
      taskTitle: conversationTitle ?? run?.origin?.conversation_title ?? null,
      tool: known?.tool ?? "System",
      model: null,
      status: known?.status ?? null,
    });
  }
  return entries.sort((a, b) => b.at - a.at);
}

function splitWords(kind: string): string {
  const words = kind.replace(/([a-z])([A-Z])/g, "$1 $2").toLowerCase();
  return words.charAt(0).toUpperCase() + words.slice(1);
}

// ---------- Event-driven refresh ----------

export type Collection = "runs" | "steps" | "conversations" | "connections" | "persona" | "focus";

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
    } else if (k.startsWith("Message")) {
      out.add("conversations");
      out.add("steps");
      out.add("focus");
    } else if (k.startsWith("Connection") || k === "ActiveConnectionChanged") {
      out.add("connections");
    } else if (k === "PersonaSaved") {
      out.add("persona");
    }
  }
  return out;
}
