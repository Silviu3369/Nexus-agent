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
  stepStatus,
  taskRows,
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
  assert.match(nextLine(run), /Nothing is written until you approve/);

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
  assert.equal(run.proposedRunId, "r-1");
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
  assert.deepEqual([...invalidatedBy([ev("AgentStepFinished")])].sort(), ["focus", "steps"]);
  assert.deepEqual([...invalidatedBy([ev("ConnectionModelSelected")])], ["connections"]);
  assert.deepEqual([...invalidatedBy([ev("PersonaSaved")])], ["persona"]);
  assert.deepEqual([...invalidatedBy([ev("ProjectCreated")])], ["projects"]);
  assert.deepEqual([...invalidatedBy([ev("ActiveProjectChanged")])], ["projects"]);
  assert.deepEqual([...invalidatedBy([ev("MemoryDecided")])].sort(), ["focus", "memory", "projects"]);
  assert.ok(invalidatedBy([ev("MessageSent")]).has("projects"), "a new task changes its project's count");
  assert.deepEqual(
    [...invalidatedBy([ev("ConversationDeleted")])].sort(),
    ["conversations", "focus", "memory", "projects", "runs", "steps"],
    "a deleted task changes the lists, and the notes and facts that linked to it",
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
