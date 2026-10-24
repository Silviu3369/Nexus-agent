// Unit tests for the execution model. Run with `pnpm --filter @nexus/desktop test`
// (Node's own test runner with type stripping; no extra dependency).

import { test } from "node:test";
import assert from "node:assert/strict";
import type { AgentStepView, ChatMessageView, ConversationView, RunView } from "@nexus/contracts";
import {
  buildActivity,
  buildRuns,
  deriveCore,
  followedTask,
  invalidatedBy,
  modelStats,
  nextLine,
  noteItems,
  stepStatus,
  taskRows,
  effectText,
  turnLine,
  actorText,
  actorsLine,
} from "../src/model/execution.ts";

const T0 = 1_800_000_000_000;
const CONV = "c-1";
const REPLY = "m-2";

let seq = 0;
function step(detail: AgentStepView["detail"], status: AgentStepView["status"], start: number, end: number | null): AgentStepView {
  seq += 1;
  return {
    step_id: `s-${seq}`,
    message_id: REPLY,
    conversation_id: CONV,
    conversation_title: "Check the site",
    seq,
    status,
    detail,
    started_at_ms: start,
    finished_at_ms: end,
  };
}

const model = (
  status: AgentStepView["status"],
  start: number,
  end: number | null,
  extra: Partial<{ first: number; total: number; error: string; attempt: number; backup: boolean; name: string }> = {},
) =>
  step(
    {
      kind: "ModelCall",
      model: extra.name ?? "gemini-3.8-flash",
      first_byte_ms: extra.first ?? null,
      duration_ms: extra.total ?? null,
      error: extra.error ?? null,
      attempt: extra.attempt ?? null,
      backup: extra.backup ?? null,
    },
    status,
    start,
    end,
  );

const fetch = (status: AgentStepView["status"], start: number, end: number | null) =>
  step(
    { kind: "Fetch", url: "https://example.com/a", final_url: null, http_status: status === "Done" ? 200 : null, text_bytes: status === "Done" ? 2048 : null, truncated: false, error: null },
    status,
    start,
    end,
  );

function message(role: "User" | "Model", id: string, steps: AgentStepView[], extra: Partial<ChatMessageView> = {}): ChatMessageView {
  return {
    message_id: id,
    role,
    text: role === "User" ? "Check example.com" : "",
    status: role === "User" ? "Complete" : "Streaming",
    model: role === "Model" ? "gemini-3.8-flash" : null,
    persona_version: 1,
    failure: null,
    proposed_run_id: null,
    prompt_tokens: null,
    output_tokens: null,
    created_at_ms: T0,
    steps,
    ...extra,
  };
}

function conversation(reply: ChatMessageView): ConversationView {
  return { conversation_id: CONV, title: "Check the site", messages: [message("User", "m-1", []), reply] };
}

function noteRun(status: RunView["status"], extra: Partial<RunView> = {}): RunView {
  return {
    run_id: "r-1",
    title: "Plan",
    status,
    revision: 2,
    failure: null,
    pending_approval:
      status === "AwaitingApproval"
        ? { approval_id: "a-1", manifest_sha256: "abc", expires_at_ms: T0 + 600_000, preview: null }
        : null,
    artifact: null,
    origin: { message_id: REPLY, conversation_id: CONV, conversation_title: "Check the site" },
    kind: "Note",
    call: null,
    created_at_ms: T0 + 3000,
    updated_at_ms: T0 + 3500,
    ...extra,
  };
}

test("a reply thinking after a web read maps to a running run with the path it took", () => {
  const steps = [model("Done", T0, T0 + 1200, { first: 800, total: 1200 }), fetch("Done", T0 + 1200, T0 + 1500), model("Running", T0 + 1500, null)];
  const [run] = buildRuns(conversation(message("Model", REPLY, steps)), new Map(), "Nexus");
  assert.equal(run.request, "Check example.com");
  assert.equal(run.status, "running");
  assert.deepEqual(run.items.map((i) => [i.kind, i.status]), [["model", "completed"], ["web", "completed"], ["model", "running"]]);
  assert.equal(run.items[1].title, "Read example.com");
  assert.ok(run.items[1].facts.some(([k, v]) => k === "Action" && v === "GET https://example.com/a"));

  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 2000 });
  assert.equal(core.state, "THINKING");
  assert.deepEqual(core.path, ["model", "web", "model"]);
  assert.deepEqual(core.nodes.map((n) => n.id), ["model", "web"]);
  assert.equal(core.active, "model");
  assert.equal(core.nodes[0].status, "active");
  assert.deepEqual(core.visual, {
    state: "THINKING",
    activeTool: "model",
    activeModel: "gemini-3.8-flash",
    activityIntensity: 0.5,
    hasPendingApproval: false,
    lastResult: null,
  }, "the core's visual state comes only from the recorded steps");
});

test("a running web read is shown as using a tool", () => {
  const [run] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), fetch("Running", T0 + 900, null)])), new Map(), "Nexus");
  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 1000 });
  assert.equal(core.state, "USING_TOOL");
  assert.equal(core.active, "web");
  assert.equal(core.visual.activeTool, "web", "the flow goes to the tool in use");
  assert.match(nextLine(run), /page text goes back to the model/);
});

test("a proposed note waits on the approval, then the write, from the real run state", () => {
  const proposal = step({ kind: "NoteProposed", title: "Plan", run_id: "r-1", error: null }, "Done", T0 + 2000, T0 + 2100);
  const reply = message("Model", REPLY, [model("Done", T0, T0 + 2000), proposal], { status: "Complete", proposed_run_id: "r-1" });
  const waiting = new Map([["r-1", noteRun("AwaitingApproval")]]);
  const [run] = buildRuns(conversation(reply), waiting, "Nexus");
  assert.equal(run.status, "approval_required");
  assert.deepEqual(run.items.map((i) => [i.kind, i.status]), [["model", "completed"], ["note", "completed"], ["approval", "approval_required"], ["write", "queued"]]);
  const core = deriveCore({ run, pendingApprovals: 1, composing: true, now: T0 + 999_999 });
  assert.equal(core.state, "WAITING_APPROVAL", "approval outranks typing, and keeps the nodes on screen");
  assert.equal(core.active, "approval");
  assert.equal(core.visual.hasPendingApproval, true);
  assert.equal(core.nodes.find((n) => n.id === "files")?.proposed, true, "the note is only an intent until it is written");
  assert.equal(core.outcome, null, "a run waiting on you has no outcome yet");
  assert.equal(core.visual.activityIntensity, 0, "nothing runs while it waits");
  assert.equal(core.nodes.find((n) => n.id === "approval")?.status, "waiting");
  assert.match(nextLine(run), /Nothing is written or sent until you approve/);

  const writing = new Map([["r-1", noteRun("Verifying")]]);
  const [after] = buildRuns(conversation(reply), writing, "Nexus");
  const executing = deriveCore({ run: after, pendingApprovals: 0, composing: false, now: T0 + 4000 });
  assert.equal(executing.state, "EXECUTING");
  assert.equal(executing.nodes.find((n) => n.id === "files")?.proposed, false, "once written it is an executed effect");

  const denied = new Map([["r-1", noteRun("Denied")]]);
  const [rejected] = buildRuns(conversation(reply), denied, "Nexus");
  assert.deepEqual(rejected.items.slice(2).map((i) => [i.kind, i.status, i.outcome]), [
    ["approval", "cancelled", "Rejected by you"],
    ["write", "cancelled", "Not written"],
  ]);
});

test("the note run is found from the proposal step before the reply settles", () => {
  const proposal = step({ kind: "NoteProposed", title: "Plan", run_id: "r-1", error: null }, "Done", T0, T0 + 10);
  const [run] = buildRuns(conversation(message("Model", REPLY, [proposal])), new Map([["r-1", noteRun("AwaitingApproval")]]), "Nexus");
  assert.deepEqual(run.proposedRunIds, ["r-1"]);
  assert.ok(run.items.some((i) => i.kind === "approval"));
});

test("success pulses briefly, errors linger a little, and finished runs leave the stage", () => {
  const done = message("Model", REPLY, [model("Done", T0, T0 + 1000)], { status: "Complete", text: "Hi" });
  const [run] = buildRuns(conversation(done), new Map(), "Nexus");
  assert.equal(run.finishedAt, T0 + 1000);
  assert.equal(deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 2000 }).state, "SUCCESS");
  assert.equal(deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 2000 }).visual.lastResult, "success");
  const later = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 5000 });
  assert.equal(later.state, "IDLE");
  assert.deepEqual(later.outcome, { result: "completed", text: "Last task completed", at: T0 + 1000 }, "the picture settles, the outcome stays");
  assert.equal(deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 3_600_000 }).outcome?.result, "completed");
  assert.equal(later.nodes.length, 1, "a recent run keeps its nodes");
  assert.equal(deriveCore({ run, pendingApprovals: 0, composing: true, now: T0 + 5000 }).state, "LISTENING");
  assert.equal(deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 200_000 }).nodes.length, 0);

  const failed = message("Model", REPLY, [model("Failed", T0, T0 + 500, { error: "quota" })], {
    status: "Failed",
    failure: { code: "QuotaExceeded", message: "quota" },
  });
  const [bad] = buildRuns(conversation(failed), new Map(), "Nexus");
  const core = deriveCore({ run: bad, pendingApprovals: 0, composing: false, now: T0 + 3000 });
  assert.equal(core.state, "ERROR");
  assert.equal(core.visual.lastResult, "error");
  assert.equal(core.outcome?.text, "Last task failed: quota");
  assert.equal(core.label, "quota");
  assert.equal(core.nodes[0].status, "failed");
});

test("an outage that was tried again, then handed to the backup, reads as such", () => {
  const steps = [
    model("Failed", T0, T0 + 500, { error: "the provider is unavailable (503)" }),
    model("Failed", T0 + 600, T0 + 900, { error: "the provider is unavailable (503)", attempt: 2 }),
    model("Failed", T0 + 1000, T0 + 1200, { error: "the provider is unavailable (503)", attempt: 3 }),
    model("Done", T0 + 1300, T0 + 2000, { backup: true, name: "claude-x" }),
  ];
  const [run] = buildRuns(conversation(message("Model", REPLY, steps, { status: "Complete", text: "ok" })), new Map(), "Nexus");
  assert.deepEqual(run.items.map((i) => i.title), ["Model call", "Model call · try 2", "Model call · try 3", "Backup model call"]);
  assert.match(run.items[0].outcome ?? "", /^Temporary outage — tried again/);
  assert.match(run.items[2].outcome ?? "", /^Temporary outage — the backup model took over/);
  assert.equal(run.items[3].tool, "Backup");
  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 3000 });
  assert.equal(core.nodes[0].sublabel, "backup model");
});

test("a step stopped by the user is cancelled, an interrupted one failed", () => {
  assert.equal(stepStatus(model("Failed", T0, T0 + 1, { error: "stopped by the user; the provider may still have processed the request" })), "cancelled");
  assert.equal(stepStatus(model("Failed", T0, T0 + 1, { error: "the host stopped before this step finished" })), "failed");
});

test("the live panel follows the working task, then a waiting approval, then the latest", () => {
  const other = { ...model("Done", T0, T0 + 1), conversation_id: "c-2" };
  const running = model("Running", T0 + 5, null);
  assert.equal(followedTask([running, other], []), CONV);
  assert.equal(followedTask([other], [noteRun("AwaitingApproval")]), CONV);
  assert.equal(followedTask([other], []), "c-2");
  assert.equal(followedTask([], []), null);
});

test("task rows take their status from steps and pending approvals", () => {
  const rows = taskRows(
    [
      { conversation_id: CONV, title: "Check the site", updated_at_ms: T0 },
      { conversation_id: "c-2", title: "Old", updated_at_ms: T0 - 1 },
    ],
    [model("Done", T0, T0 + 1)],
    [noteRun("AwaitingApproval")],
  );
  assert.deepEqual(rows.map((r) => r.status), ["approval_required", null]);
});

test("model latency is the median of finished calls", () => {
  const stats = modelStats([
    model("Done", T0, T0 + 1, { first: 100, total: 1000 }),
    model("Done", T0 + 10, T0 + 11, { first: 300, total: 3000 }),
    model("Failed", T0 + 20, T0 + 21, { error: "x" }),
  ]);
  assert.equal(stats.length, 1);
  assert.equal(stats[0].calls, 3);
  assert.equal(stats[0].failures, 1);
  assert.equal(stats[0].medianFirstByteMs, 200);
  assert.equal(stats[0].medianDurationMs, 2000);
  assert.equal(stats[0].lastUsedAt, T0 + 20);
});

test("the activity log names tasks and runs, and skips events the steps already show", () => {
  const run = noteRun("Succeeded", { artifact: { artifact_id: "art-1", path: "C:/n.md", sha256: "f", size_bytes: 10, created_at_ms: T0 } });
  const entries = buildActivity({
    steps: [model("Done", T0, T0 + 1)],
    events: [
      { seq: 1, kind: "AgentStepStarted", entity_id: "s-x", created_at_ms: T0 },
      { seq: 2, kind: "ArtifactCreated", entity_id: "art-1", created_at_ms: T0 + 50 },
      { seq: 3, kind: "MessageSettled", entity_id: CONV, created_at_ms: T0 + 40 },
      { seq: 4, kind: "SomethingNew", entity_id: "x", created_at_ms: T0 + 30 },
    ],
    runs: [run],
    conversations: [{ conversation_id: CONV, title: "Check the site", updated_at_ms: T0 }],
    agent: "Nexus",
  });
  assert.deepEqual(entries.map((e) => e.title), [
    "Note written and verified",
    "Reply settled",
    "Something new",
    "Model gemini-3.8-flash",
  ]);
  assert.equal(entries[0].detail, '"Plan"');
  assert.equal(entries[0].taskTitle, "Check the site");
  assert.equal(entries[1].taskId, CONV);
});

test("events invalidate only what they describe", () => {
  const ev = (kind: string) => ({ seq: 1, kind, entity_id: "x", created_at_ms: T0 });
  assert.deepEqual([...invalidatedBy([ev("ApprovalRequested")])].sort(), ["focus", "runs"]);
  assert.deepEqual([...invalidatedBy([ev("AgentStepFinished")])].sort(), ["focus", "steps", "usage"]);
  assert.deepEqual([...invalidatedBy([ev("ConnectionModelSelected")])].sort(), ["connections", "usage"]);
  assert.deepEqual([...invalidatedBy([ev("DailyTokenBudgetSet")])], ["usage"]);
  assert.deepEqual([...invalidatedBy([ev("ModelPriceSet")])], ["usage"]);
  assert.deepEqual([...invalidatedBy([ev("PersonaSaved")])], ["persona"]);
  assert.deepEqual([...invalidatedBy([ev("AutomationRan")])], ["automations"]);
  assert.deepEqual([...invalidatedBy([ev("AutomationCreated")])], ["automations"]);
  assert.deepEqual([...invalidatedBy([ev("ProjectCreated")])], ["projects"]);
  assert.deepEqual([...invalidatedBy([ev("ActiveProjectChanged")])], ["projects"]);
  assert.deepEqual([...invalidatedBy([ev("MemoryDecided")])].sort(), ["focus", "memory", "projects"]);
  assert.ok(invalidatedBy([ev("MessageSent")]).has("projects"), "a new task changes its project's count");
  assert.deepEqual(
    [...invalidatedBy([ev("ConversationDeleted")])].sort(),
    ["automations", "conversations", "focus", "memory", "projects", "runs", "steps"],
    "a deleted task changes the lists, and the notes, facts and automation runs that linked to it",
  );
  assert.deepEqual([...invalidatedBy([ev("ExchangeDeleted")])].sort(), [...invalidatedBy([ev("ConversationDeleted")])].sort());
  assert.equal(invalidatedBy([ev("SomethingElse")]).size, 0);
});

const memoryStep = (memoryId: string | null, error: string | null = null) =>
  step({ kind: "MemoryProposed", content: "The user prefers short answers.", scope: "Personal", memory_id: memoryId, error }, "Done", T0 + 1000, T0 + 1100);

test("a proposed fact waits for review only while the Host lists it as proposed", () => {
  const usedMemory = step(
    { kind: "ModelCall", model: "gemini-3.8-flash", first_byte_ms: null, duration_ms: 900, error: null, attempt: null, backup: null, memory_items: 2 },
    "Done",
    T0,
    T0 + 900,
  );
  const reply = message("Model", REPLY, [usedMemory, memoryStep("f-1")], { status: "Complete" });
  const [pending] = buildRuns(conversation(reply), new Map(), "Nexus", new Set(["f-1"]));
  assert.equal(
    deriveCore({ run: pending, pendingApprovals: 0, composing: false, now: T0 + 1200 }).nodes.find((n) => n.id === "memory")?.proposed,
    true,
    "a fact waiting for review is only proposed",
  );
  assert.equal(pending.status, "completed", "a fact to review does not hold the reply open");
  assert.deepEqual(pending.items.map((i) => [i.kind, i.status]), [["model", "completed"], ["memory", "waiting"]]);
  assert.ok(pending.items[0].facts.some(([k, v]) => k === "Approved memory sent" && v === "2 facts"));
  assert.match(nextLine(pending), /Nothing is remembered until you approve/);

  const core = deriveCore({ run: pending, pendingApprovals: 0, composing: false, now: T0 + 1200 });
  assert.deepEqual(core.path, ["memory", "model", "memory"], "memory is read before the call, then proposed after it");
  const node = core.nodes.find((n) => n.id === "memory");
  assert.equal(node?.status, "waiting");
  assert.equal(node?.label, "Memory");

  const [decided] = buildRuns(conversation(reply), new Map(), "Nexus", new Set());
  assert.equal(decided.items[1].status, "completed");
  assert.match(decided.items[1].outcome ?? "", /decision is in Memory/);

  const [refused] = buildRuns(conversation(message("Model", REPLY, [memoryStep(null, "a fact must have 1..=1000 characters")], { status: "Complete" })), new Map(), "Nexus");
  assert.equal(refused.items[0].outcome, "a fact must have 1..=1000 characters");
});

test("activity and tasks carry the project of their task", () => {
  const conversations = [
    { conversation_id: CONV, project_id: "p-1", title: "Check the site", updated_at_ms: T0 },
    { conversation_id: "c-2", project_id: "p-2", title: "Other", updated_at_ms: T0 - 10 },
  ];
  const entries = buildActivity({
    steps: [memoryStep("f-1")],
    events: [
      { seq: 1, kind: "MemoryAdded", entity_id: "f-2", created_at_ms: T0 + 5 },
      { seq: 2, kind: "MemoryProposed", entity_id: "f-1", created_at_ms: T0 + 6 },
    ],
    runs: [],
    conversations,
    agent: "Nexus",
  });
  assert.deepEqual(entries.map((e) => [e.title, e.tool, e.projectId]), [
    ['Proposed to remember "The user prefers short answers."', "Memory", "p-1"],
    ["Fact added by you", "Memory", null],
  ], "newest first; the proposal event repeats the step, so only the step is listed");
  assert.deepEqual(taskRows(conversations, [], []).map((r) => r.projectId), ["p-1", "p-2"]);
});

const mcpCall = (status: AgentStepView["status"], start: number, end: number | null, error: string | null = null) =>
  step(
    {
      kind: "McpCall",
      server: "Microsoft Learn",
      tool: "microsoft_docs_search",
      arguments: '{"query":"named pipes"}',
      result_chars: status === "Done" ? 19304 : null,
      truncated: status === "Done" ? false : null,
      is_error: status === "Done" ? false : null,
      duration_ms: status === "Done" ? 820 : null,
      error,
    },
    status,
    start,
    end,
  );

test("an MCP call is a tool step on its own node, named by server and tool", () => {
  const steps = [model("Done", T0, T0 + 900), mcpCall("Running", T0 + 900, null)];
  const [run] = buildRuns(conversation(message("Model", REPLY, steps)), new Map(), "Nexus");
  assert.deepEqual(run.items.map((i) => [i.kind, i.status]), [["model", "completed"], ["mcp", "running"]]);
  assert.equal(run.items[1].tool, "MCP");
  assert.match(nextLine(run), /answer goes back to the model as data/);

  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 1000 });
  assert.equal(core.state, "USING_TOOL");
  assert.equal(core.label, "Using microsoft_docs_search on Microsoft Learn");
  assert.equal(core.active, "mcp");
  assert.deepEqual(core.path, ["model", "mcp"]);
  const node = core.nodes.find((n) => n.id === "mcp");
  assert.equal(node?.label, "Microsoft Learn");
  assert.equal(node?.sublabel, "microsoft_docs_search");
  assert.equal(node?.proposed, false, "a read-only call is executed, not proposed");

  const [done] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), mcpCall("Done", T0 + 900, T0 + 1720)], { status: "Complete" })), new Map(), "Nexus");
  const item = done.items[1];
  assert.equal(item.durationMs, 820);
  assert.match(item.outcome ?? "", /19,304 characters back/);
  assert.ok(item.facts.some(([k, v]) => k === "Arguments sent" && v === '{"query":"named pipes"}'));

  const [failed] = buildRuns(conversation(message("Model", REPLY, [mcpCall("Failed", T0, T0 + 30, "this tool is not enabled any more")], { status: "Complete" })), new Map(), "Nexus");
  assert.equal(failed.items[0].status, "failed");
  assert.equal(failed.items[0].outcome, "this tool is not enabled any more");

  const activity = buildActivity({
    steps: [mcpCall("Done", T0, T0 + 820)],
    events: [{ seq: 9, kind: "McpToolEnabled", entity_id: "srv", created_at_ms: T0 - 10 }],
    runs: [],
    conversations: [],
    agent: "Nexus",
  });
  assert.deepEqual(activity.map((e) => [e.tool, e.title]), [
    ["MCP", "Used microsoft_docs_search on Microsoft Learn"],
    ["MCP", "MCP tool enabled"],
  ]);
});

test("MCP events refresh the servers only", () => {
  const ev = (kind: string) => ({ seq: 1, kind, entity_id: "x", created_at_ms: T0 });
  for (const kind of ["McpServerAdded", "McpToolsListed", "McpServerCheckFailed", "McpToolEnabled", "McpToolDisabled", "McpServerRemoved"]) {
    assert.deepEqual([...invalidatedBy([ev(kind)])], ["mcp"], kind);
  }
});

test("a task that continues after the user's decision stays one run", () => {
  const proposal = step({ kind: "NoteProposed", title: "Plan", run_id: "r-1", error: null }, "Done", T0 + 900, T0 + 910);
  const first = message("Model", REPLY, [model("Done", T0, T0 + 900), proposal], {
    status: "Complete",
    text: "Here is the plan.",
    proposed_run_id: "r-1",
    prompt_tokens: 10,
    output_tokens: 5,
  });
  const continued = message("Model", "m-3", [model("Running", T0 + 5000, null)], { text: "Saved" });
  const view: ConversationView = {
    conversation_id: CONV,
    title: "Check the site",
    messages: [message("User", "m-1", []), first, continued],
  };
  const written = noteRun("Succeeded", { updated_at_ms: T0 + 4000 });
  const runs = buildRuns(view, new Map([["r-1", written]]), "Nexus");
  assert.equal(runs.length, 1, "the continuation is not a new run");
  const [run] = runs;
  assert.deepEqual(run.items.map((i) => [i.kind, i.status]), [
    ["model", "completed"],
    ["note", "completed"],
    ["approval", "completed"],
    ["write", "completed"],
    ["model", "running"],
  ]);
  assert.equal(run.status, "running");
  assert.equal(run.text, "Here is the plan.\n\nSaved");
  assert.equal(run.messageId, "m-3", "Stop names the reply being written");
  assert.equal(run.id, REPLY, "the run keeps its identity");
  assert.equal(run.request, "Check example.com");
  assert.deepEqual(run.proposedRunIds, ["r-1"]);
  assert.equal(run.promptTokens, 10);
  assert.equal(nextLine(run).startsWith("The model is answering"), true);

  const waiting = buildRuns(
    { ...view, messages: [message("User", "m-1", []), first] },
    new Map([["r-1", noteRun("AwaitingApproval")]]),
    "Nexus",
  )[0];
  assert.match(nextLine(waiting), /after your decision, Nexus continues the task/);
});

test("a call that can change things waits for approval, then is made once", () => {
  const proposal = step(
    {
      kind: "McpCall",
      server: "Docs",
      tool: "delete_page",
      arguments: '{"q":"page 7"}',
      result_chars: null,
      truncated: null,
      is_error: null,
      duration_ms: null,
      error: null,
      run_id: "r-1",
    },
    "Done",
    T0 + 900,
    T0 + 910,
  );
  const reply = message("Model", REPLY, [model("Done", T0, T0 + 900), proposal], { status: "Complete", proposed_run_id: "r-1" });
  const call = {
    server: "Docs",
    url: "https://docs.example/mcp",
    tool: "delete_page",
    arguments_json: JSON.stringify({ q: "page 7" }, null, 2),
    result_chars: null,
    truncated: null,
    duration_ms: null,
  };
  const waiting = noteRun("AwaitingApproval", { kind: "McpCall", call, title: "delete_page on Docs" });
  const [run] = buildRuns(conversation(reply), new Map([["r-1", waiting]]), "Nexus");
  assert.deepEqual(run.items.map((i) => [i.kind, i.status]), [
    ["model", "completed"],
    ["mcp", "completed"],
    ["approval", "approval_required"],
    ["mcp", "queued"],
  ]);
  assert.equal(run.items[1].title, "Propose calling delete_page · Docs");
  assert.ok(run.items[2].facts.some(([k, v]) => k === "Action" && v === "Call delete_page on Docs"));
  const core = deriveCore({ run, pendingApprovals: 1, composing: false, now: T0 + 1000 });
  assert.equal(core.state, "WAITING_APPROVAL");
  const node = core.nodes.find((n) => n.id === "mcp");
  assert.equal(node?.proposed, true, "nothing was sent yet");
  assert.equal(node?.label, "Docs");

  const made = noteRun("Succeeded", { kind: "McpCall", call: { ...call, result_chars: 15, truncated: false, duration_ms: 40 } });
  const [done] = buildRuns(conversation(reply), new Map([["r-1", made]]), "Nexus");
  assert.deepEqual(done.items.slice(2).map((i) => [i.kind, i.status, i.outcome]), [
    ["approval", "completed", "Approved"],
    ["mcp", "completed", "Made once; 15 characters back"],
  ]);
  const after = deriveCore({ run: done, pendingApprovals: 0, composing: false, now: T0 + 5000 });
  assert.equal(after.nodes.find((n) => n.id === "mcp")?.proposed, false, "made after approval");

  const review = noteRun("NeedsReview", { kind: "McpCall", call, failure: { code: "OutcomeUnknown", message: "x" } });
  const [uncertain] = buildRuns(conversation(reply), new Map([["r-1", review]]), "Nexus");
  assert.match(uncertain.items[3].outcome ?? "", /may have reached the server/);

  const activity = buildActivity({
    steps: [],
    events: [{ seq: 4, kind: "RunCreated", entity_id: "r-1", created_at_ms: T0 }],
    runs: [made],
    conversations: [],
    agent: "Nexus",
  });
  assert.deepEqual(activity.map((e) => [e.tool, e.title]), [["MCP", "MCP call requested"]]);
});

test("a model call says which MCP servers it could use, and a run that did not use them says so", () => {
  const offered = step(
    {
      kind: "ModelCall",
      model: "gemini-3.8-flash",
      first_byte_ms: 3800,
      duration_ms: 8600,
      error: null,
      attempt: null,
      backup: null,
      memory_items: 0,
      mcp_servers: ["Microsoft Learn"],
      mcp_tools: 3,
    },
    "Done",
    T0,
    T0 + 8600,
  );
  const [unused] = buildRuns(conversation(message("Model", REPLY, [offered], { status: "Complete" })), new Map(), "Nexus");
  const item = unused.items[0];
  assert.ok(item.facts.some(([k, v]) => k === "MCP tools available" && v === "Microsoft Learn (3 tools)"));
  assert.match(item.outcome ?? "", /Microsoft Learn available; answered without it$/);

  const [used] = buildRuns(
    conversation(message("Model", REPLY, [offered, mcpCall("Done", T0 + 8600, T0 + 9000)], { status: "Complete" })),
    new Map(),
    "Nexus",
  );
  assert.doesNotMatch(used.items[0].outcome ?? "", /answered without it/);
});

test("a model call shows the tokens the provider reported", () => {
  const call = step(
    {
      kind: "ModelCall",
      model: "gemini-3.8-flash",
      first_byte_ms: 100,
      duration_ms: 900,
      error: null,
      attempt: null,
      backup: null,
      memory_items: 0,
      mcp_servers: null,
      mcp_tools: null,
      prompt_tokens: 1234,
      output_tokens: 56,
    },
    "Done",
    T0,
    T0 + 900,
  );
  const [run] = buildRuns(conversation(message("Model", REPLY, [call], { status: "Complete" })), new Map(), "Nexus");
  assert.ok(run.items[0].facts.some(([k, v]) => k === "Tokens" && v === "1,234 sent · 56 received"));
});

test("register steps say what was looked up and saved, and an undone change says so", () => {
  const found = step(
    { kind: "RecordsFound", register: "Job applications", query: "“Acme”", count: 0, error: null },
    "Done",
    T0 + 900,
    T0 + 905,
  );
  const added = step(
    {
      kind: "RecordChanged",
      register: "Job applications",
      action: "added",
      title: "Acme · Service Desk Analyst",
      fields: ["company", "role", "status"],
      record_id: "rec-1",
      change_id: 7,
      undone: false,
      error: null,
    },
    "Done",
    T0 + 1900,
    T0 + 1910,
  );
  const [run] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), found, added])), new Map(), "Nexus");
  const records = run.items.filter((i) => i.kind === "record");
  assert.deepEqual(
    records.map((i) => [i.title, i.outcome, i.tool]),
    [
      ["Looked up Job applications", "0 found · “Acme”", "Records"],
      ["Added Acme · Service Desk Analyst in Job applications", "company, role, status · saved; you can undo it", "Records"],
    ],
  );
  assert.deepEqual(records[1].record, { changeId: 7, recordId: "rec-1", undone: false });
  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 2000 });
  assert.ok(core.nodes.some((n) => n.id === "records" && n.label === "Records" && n.sublabel === "saved"));

  const undone = { ...added, detail: { ...added.detail, undone: true } } as AgentStepView;
  const [after] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), undone])), new Map(), "Nexus");
  const item = after.items.find((i) => i.kind === "record");
  assert.equal(item?.status, "cancelled");
  assert.equal(item?.outcome, "undone by you");

  const failed = step(
    {
      kind: "RecordChanged",
      register: "Job applications",
      action: "added",
      title: "",
      fields: [],
      record_id: null,
      change_id: null,
      undone: false,
      error: "Applied on must be a date as YYYY-MM-DD",
    },
    "Failed",
    T0,
    T0 + 1,
  );
  const [bad] = buildRuns(conversation(message("Model", REPLY, [failed])), new Map(), "Nexus");
  assert.equal(bad.items[0].title, "Could not save to Job applications");
});

test("record events refresh the registers, and an undo the task's steps", () => {
  const ev = (kind: string) => ({ seq: 1, kind, entity_id: "x", created_at_ms: T0 });
  for (const kind of ["RecordAdded", "RecordUpdated", "RecordDeleted"]) {
    assert.deepEqual([...invalidatedBy([ev(kind)])], ["registers"], kind);
  }
  assert.deepEqual([...invalidatedBy([ev("RecordChangeUndone")])].sort(), ["focus", "registers", "steps"]);
});

test("document steps say what was read and proposed, and a document run is a file write", () => {
  const listed = step({ kind: "FilesListed", folder: "Jobs", path: "", count: 3, error: null }, "Done", T0 + 900, T0 + 905);
  const read = step(
    { kind: "DocumentRead", folder: "Jobs", path: "CV.docx", format: "Word", chars: 1200, truncated: false, error: null },
    "Done",
    T0 + 906,
    T0 + 950,
  );
  const outside = step(
    { kind: "DocumentRead", folder: "Jobs", path: "../x.txt", format: null, chars: null, truncated: null, error: "\"..\" is not a valid name" },
    "Failed",
    T0 + 951,
    T0 + 952,
  );
  const proposed = step(
    { kind: "DocumentProposed", folder: "Letters", file_name: "Cover letter.docx", run_id: "r-9", error: null },
    "Done",
    T0 + 1900,
    T0 + 1910,
  );
  const [run] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), listed, read, outside, proposed])), new Map(), "Nexus");
  const docs = run.items.filter((i) => i.kind === "doc");
  assert.deepEqual(
    docs.map((i) => [i.title, i.outcome, i.status]),
    [
      ["Looked in Jobs", "3 items", "completed"],
      ["Read CV.docx in Jobs", "Word · 1,200 characters to the model", "completed"],
      ["Read ../x.txt in Jobs", "\"..\" is not a valid name", "failed"],
      ["Propose Cover letter.docx in Letters", "Proposal recorded; nothing written yet", "completed"],
    ],
  );
  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 2000 });
  assert.ok(core.nodes.some((n) => n.id === "files" && n.label === "Documents"));

  const documentRun: RunView = { ...noteRun("Succeeded"), kind: "Document", title: "Cover letter.docx in Letters" };
  const items = noteItems(documentRun);
  assert.deepEqual(
    items.map((i) => [i.title, i.tool]),
    [
      ["Your approval", null],
      ["Write Cover letter.docx in Letters", "Documents"],
    ],
  );
});

test("folder and document events refresh what they describe", () => {
  const ev = (kind: string) => ({ seq: 1, kind, entity_id: "x", created_at_ms: T0 });
  for (const kind of ["FolderShared", "FolderUpdated", "FolderUnshared"]) {
    assert.deepEqual([...invalidatedBy([ev(kind)])], ["folders"], kind);
  }
  assert.deepEqual([...invalidatedBy([ev("DocumentCreated")])].sort(), ["focus", "runs"]);
  assert.deepEqual([...invalidatedBy([ev("RecordsImported")])], ["registers"]);
});

test("inbox events refresh the registers, so the inbox panel reads again", () => {
  const ev = (kind: string) => ({ seq: 1, kind, entity_id: "x", created_at_ms: T0 });
  for (const kind of ["InboxUpdated", "InboxFileSeen", "InboxFileHandled", "InboxFileRetried"]) {
    assert.deepEqual([...invalidatedBy([ev(kind)])], ["registers"], kind);
  }
});

test("a turn's line says whether the user kept what the agent did", () => {
  const base = { run_id: "r", status: "Succeeded", failure_cause: null, review_until_ms: null, interventions: [] } as const;
  assert.equal(turnLine(null), null);
  assert.equal(turnLine({ ...base, acceptance: "NotApplicable" }), null);
  assert.equal(turnLine({ ...base, acceptance: "Accepted" }), "Accepted: kept for 7 days without being undone or corrected");
  assert.match(
    turnLine({ ...base, acceptance: "InReview", review_until_ms: Date.UTC(2026, 9, 27, 12) }) ?? "",
    /^In review until 27 Oct/,
  );
  assert.equal(
    turnLine({ ...base, acceptance: "NotAccepted", interventions: ["Approved", "Correction", "Undo", "Correction"] }),
    "Not accepted: you corrected, undid what it did",
  );
});

test("a run says what kind of effect it has and how its success is proven", () => {
  assert.equal(
    effectText("ExternalReversible", "Verified"),
    "Changes something outside Nexus; can be undone · proven by reading the result back",
  );
  assert.equal(
    effectText("ExternalIrreversible", "Reported"),
    "Changes something outside Nexus; cannot be undone · as the service reported it, not checked independently",
  );
  assert.equal(effectText(null, "Verified"), null);
  assert.equal(effectText("InternalReversible", null), "Changes only Nexus's own data; can be undone");
});

test("a run and a task say who asked for them and who decided", () => {
  const you = { kind: "User", name: "Local user" };
  const agent = { kind: "Agent", name: "Nexus" };
  assert.equal(actorText(you, "Ada"), "you");
  assert.equal(actorText(agent, "Ada"), "Ada", "the agent goes by the name the user gave it");
  assert.equal(actorText({ kind: "Automation", name: "Morning" }, "Ada"), "the automation “Morning”");
  assert.equal(actorText({ kind: "Intake", name: "Inbox" }, "Ada"), "Inbox");
  assert.equal(actorText(null, "Ada"), null);

  const proposed = noteRun("AwaitingApproval", { requested_by: agent, decided_by: null });
  assert.equal(actorsLine(proposed, "Ada"), "Asked by Ada");
  assert.equal(actorsLine({ ...proposed, status: "Succeeded", decided_by: you }, "Ada"), "Asked by Ada · approved by you");
  assert.equal(actorsLine({ ...proposed, status: "Denied", decided_by: you }, "Ada"), "Asked by Ada · rejected by you");
  assert.equal(actorsLine(noteRun("Queued", { requested_by: null, decided_by: null }), "Ada"), null);
});
