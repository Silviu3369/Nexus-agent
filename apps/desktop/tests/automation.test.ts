import { test } from "node:test";
import assert from "node:assert/strict";
import type { AutomationRunView, AutomationView } from "@nexus/contracts";
import {
  fromLocalInput,
  nextText,
  parseTime,
  runText,
  scheduleText,
  toLocalInput,
  untilText,
} from "../src/model/automation.ts";

const weekly = (days: number[], hour = 9, minute = 0) => ({ kind: "Weekly" as const, days, hour, minute });

test("a schedule reads as days and a local time", () => {
  assert.equal(scheduleText(weekly([0, 1, 2, 3, 4, 5, 6])), "Every day at 09:00");
  assert.equal(scheduleText(weekly([4, 0, 1, 2, 3], 7, 5)), "Weekdays at 07:05");
  assert.equal(scheduleText(weekly([5, 6], 10, 30)), "Weekends at 10:30");
  assert.equal(scheduleText(weekly([0, 2, 4], 18)), "Mon, Wed, Fri at 18:00");
  const at = new Date(2026, 9, 25, 9, 0).getTime();
  assert.equal(scheduleText({ kind: "Once", at_ms: at }), "Once, Sun 25 Oct 2026, 09:00");
});

test("local date inputs round-trip and bad values are refused", () => {
  const at = new Date(2026, 9, 25, 2, 30).getTime();
  assert.equal(toLocalInput(at), "2026-10-25T02:30");
  assert.equal(fromLocalInput("2026-10-25T02:30"), at);
  assert.equal(fromLocalInput(""), null);
  assert.equal(fromLocalInput("25.10.2026 02:30"), null);
  assert.deepEqual(parseTime("09:30"), [9, 30]);
  assert.equal(parseTime("24:00"), null);
  assert.equal(parseTime("9:30"), null);
});

const automation = (over: Partial<AutomationView>): AutomationView => ({
  automation_id: "a-1",
  name: "Morning",
  prompt: "Summarise",
  project_id: null,
  schedule: weekly([0, 1, 2, 3, 4, 5, 6]),
  enabled: true,
  catch_up: true,
  next_run_at_ms: null,
  last_run: null,
  revision: 1,
  created_at_ms: 0,
  ...over,
});

test("the next time says when, or why there is none", () => {
  const now = new Date(2026, 9, 18, 12, 0).getTime();
  assert.equal(
    nextText(automation({ next_run_at_ms: now + 3 * 3_600_000 }), now),
    "Sun 18 Oct 2026, 15:00 · in 3 h",
  );
  assert.match(nextText(automation({ enabled: false }), now), /^Off/);
  assert.match(nextText(automation({ schedule: { kind: "Once", at_ms: now - 1 } }), now), /^Finished/);
  assert.equal(untilText(now + 30_000, now), "in under a minute");
  assert.equal(untilText(now + 45 * 60_000, now), "in 45 min");
  assert.equal(untilText(now + 3 * 86_400_000, now), "in 3 days");
});

const run = (over: Partial<AutomationRunView>): AutomationRunView => ({
  scheduled_for_ms: 0,
  at_ms: 0,
  status: "Started",
  reason: null,
  conversation_id: "c-1",
  catch_up: false,
  manual: false,
  ...over,
});

test("a run says how it started and why it did not", () => {
  assert.equal(runText(run({})), "");
  assert.equal(runText(run({ manual: true })), "Run now");
  assert.equal(runText(run({ catch_up: true })), "caught up late");
  assert.equal(runText(run({ conversation_id: null })), "its task was deleted");
  assert.equal(
    runText(run({ status: "Skipped", reason: "the previous run was still answering", conversation_id: null })),
    "the previous run was still answering",
  );
  assert.equal(
    runText(run({ status: "Failed", manual: true, reason: "choose an AI connection and model first", conversation_id: null })),
    "Run now · choose an AI connection and model first",
  );
});
