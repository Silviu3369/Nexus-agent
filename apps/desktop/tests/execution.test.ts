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

const model = (status: AgentStepView["status"], start: number, end: number | null, extra: Partial<{ first: number; total: number; error: string }> = {}) =>
  step(
    { kind: "ModelCall", model: "gemini-3.8-flash", first_byte_ms: extra.first ?? null, duration_ms: extra.total ?? null, error: extra.error ?? null },
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
});

test("a running web read is shown as using a tool", () => {
  const [run] = buildRuns(conversation(message("Model", REPLY, [model("Done", T0, T0 + 900), fetch("Running", T0 + 900, null)])), new Map(), "Nexus");
  const core = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 1000 });
  assert.equal(core.state, "USING_TOOL");
  assert.equal(core.active, "web");
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
  assert.equal(core.nodes.find((n) => n.id === "approval")?.status, "waiting");
  assert.match(nextLine(run), /Nothing is written until you approve/);

  const writing = new Map([["r-1", noteRun("Verifying")]]);
  const [after] = buildRuns(conversation(reply), writing, "Nexus");
  assert.equal(deriveCore({ run: after, pendingApprovals: 0, composing: false, now: T0 + 4000 }).state, "EXECUTING");

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
  const later = deriveCore({ run, pendingApprovals: 0, composing: false, now: T0 + 5000 });
  assert.equal(later.state, "IDLE");
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
  assert.equal(core.label, "quota");
  assert.equal(core.nodes[0].status, "failed");
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
  assert.equal(invalidatedBy([ev("ProjectCreated")]).size, 0);
});
