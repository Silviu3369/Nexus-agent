import { test } from "node:test";
import assert from "node:assert/strict";
import type { FieldDef, RecordChangeView, RecordView, RegisterView } from "@nexus/contracts";
import {
  bandText,
  causeText,
  changeLines,
  changeText,
  dateText,
  durationText,
  filterRecords,
  formValues,
  groupText,
  matchText,
  missingRequired,
  percentText,
  previewLines,
  recordTitle,
  sortRecords,
  statusCounts,
  statusTone,
  usdText,
  weekText,
} from "../src/model/records.ts";

const field = (key: string, label: string, kind: FieldDef["kind"], extra: Partial<FieldDef> = {}): FieldDef => ({
  key,
  label,
  kind,
  required: false,
  choices: [],
  max_chars: 200,
  help: "",
  ...extra,
});

const STATUSES = ["To apply", "Applied", "Interviewing", "Offer", "Rejected", "Withdrawn"];

const register: RegisterView = {
  register_id: "reg-1",
  key: "job_applications",
  name: "Job applications",
  item_name: "application",
  fields: [
    field("company", "Company", "Text", { required: true }),
    field("role", "Role", "Text", { required: true }),
    field("status", "Status", "Choice", { required: true, choices: STATUSES }),
    field("applied_on", "Applied on", "Date"),
    field("follow_up_on", "Follow up on", "Date"),
    field("notes", "Notes", "LongText"),
  ],
  status_field: "status",
  closed_statuses: ["Rejected", "Withdrawn"],
  follow_up_field: "follow_up_on",
  title_fields: ["company", "role"],
  record_count: 0,
  follow_ups_due: 0,
};

const record = (id: string, values: Record<string, string>, due = false): RecordView => ({
  record_id: id,
  register_id: "reg-1",
  values,
  revision: 1,
  created_by: "User",
  created_at_ms: 0,
  updated_at_ms: 0,
  follow_up_due: due,
});

test("a record is named, counted and filtered as its register says", () => {
  const all = [
    record("a", { company: "Acme", role: "Service Desk", status: "Applied", notes: "Referral from Ana" }, true),
    record("b", { company: "Beta", role: "IT Support", status: "Rejected" }),
    record("c", { company: "Gamma", role: "Sysadmin", status: "Applied" }),
  ];
  assert.equal(recordTitle(register, all[0].values), "Acme · Service Desk");
  assert.equal(recordTitle(register, {}), "Untitled application");
  assert.deepEqual(statusCounts(register, all).slice(0, 2), [["To apply", 0], ["Applied", 2]]);
  const ids = (f: Parameters<typeof filterRecords>[2]) => filterRecords(register, all, f).map((r) => r.record_id);
  assert.deepEqual(ids({ status: "Applied", dueOnly: false, text: "" }), ["a", "c"]);
  assert.deepEqual(ids({ status: null, dueOnly: true, text: "" }), ["a"]);
  assert.deepEqual(ids({ status: null, dueOnly: false, text: "ana" }), ["a"], "text is found in any field");
});

test("statuses read as open, in progress, good or closed", () => {
  assert.equal(statusTone(register, "To apply"), "queued");
  assert.equal(statusTone(register, "Interviewing"), "running");
  assert.equal(statusTone(register, "Offer"), "completed");
  assert.equal(statusTone(register, "Rejected"), "cancelled");
  assert.equal(dateText("2026-10-18"), "18 Oct 2026");
  assert.equal(dateText("soon"), "soon");
});

test("the history says who changed what", () => {
  const change = (over: Partial<RecordChangeView>): RecordChangeView => ({
    change_id: 1,
    kind: "Updated",
    actor: "Agent",
    at_ms: 0,
    fields: [
      { key: "status", before: "Applied", after: "Interviewing" },
      { key: "follow_up_on", before: null, after: "2026-10-27" },
    ],
    conversation_id: "c-1",
    read_hosts: [],
    undone: false,
    can_undo: true,
    ...over,
  });
  assert.equal(changeText(register, change({}), "Nexus"), "Nexus changed status, follow up on");
  assert.equal(changeText(register, change({ kind: "Created" }), "Nexus"), "Nexus added it");
  assert.equal(changeText(register, change({ kind: "Undo", actor: "User" }), "Nexus"), "You undid a change");
  assert.deepEqual(changeLines(register, change({})), ["Status: Applied → Interviewing", "Follow up on: — → 27 Oct 2026"]);
});

test("a new record starts at the first status and the form names what is missing", () => {
  const values = formValues(register, null);
  assert.equal(values.status, "To apply");
  assert.equal(values.company, "");
  assert.deepEqual(missingRequired(register, values), ["Company", "Role"]);
  assert.deepEqual(missingRequired(register, { ...values, company: "Acme", role: "IT" }), []);
});

test("records sort by last change, best match or the nearest follow-up", () => {
  const r = (id: string, values: Record<string, string>, updated: number): RecordView => ({
    ...record(id, values),
    updated_at_ms: updated,
  });
  const all = [
    r("a", { company: "A", role: "x", status: "Applied", match_score: "55", follow_up_on: "2026-10-30" }, 3),
    r("b", { company: "B", role: "x", status: "Applied", match_score: "90" }, 1),
    r("c", { company: "C", role: "x", status: "Applied", follow_up_on: "2026-10-20" }, 2),
  ];
  const order = (s: Parameters<typeof sortRecords>[2]) => sortRecords(register, all, s).map((x) => x.record_id);
  assert.deepEqual(order("changed"), ["a", "c", "b"]);
  assert.deepEqual(order("match"), ["b", "a", "c"], "unscored last");
  assert.deepEqual(order("follow_up"), ["c", "a", "b"], "no date last");
  assert.equal(matchText(all[1]), "Match 90");
  assert.equal(matchText(all[2]), null);
});

test("results read as plain numbers, with no percentage of nothing", () => {
  assert.equal(percentText(1, 3), "33%");
  assert.equal(percentText(0, 0), "—");
  assert.equal(weekText("2026-10-19"), "19 Oct");
  assert.equal(bandText("High"), "Match 70 or more");
  assert.equal(groupText({ sent: 3, interviews: 1 }), "3 sent · 1 interview (33%)");
  assert.equal(groupText({ sent: 2, interviews: 0 }), "2 sent · 0 interviews (0%)");
  assert.equal(groupText({ sent: 0, interviews: 0 }), "0 sent · 0 interviews");
});

test("how the ads were handled reads in plain words", () => {
  assert.equal(causeText("QuotaExceeded"), "the AI provider's quota was used up");
  assert.equal(causeText("NotStarted"), "could not start");
  assert.equal(causeText("Something new"), "Something new");
  assert.equal(usdText(0), "$0");
  assert.equal(usdText(0.0042), "$0.0042");
  assert.equal(usdText(2.8), "$2.80");
  assert.equal(durationText(34), "34 s");
  assert.equal(durationText(240), "4 min");
  assert.equal(durationText(7200), "2 h");
});

test("a change waiting in a batch reads as its fields, before → after", () => {
  const register = {
    key: "job_applications",
    fields: [
      { key: "company", label: "Company", kind: "Text" },
      { key: "status", label: "Status", kind: "Choice" },
    ],
  } as unknown as RegisterView;
  const added = { register_key: "job_applications", register_name: "Jobs", record_id: null, title: "Acme · IT", fields: [{ key: "company", before: null, after: "Acme" }] };
  assert.deepEqual(previewLines(register, added), ["Company: Acme"]);
  const changed = { ...added, record_id: "r-1", fields: [{ key: "status", before: "Applied", after: "Interviewing" }] };
  assert.deepEqual(previewLines(register, changed), ["Status: Applied → Interviewing"]);
  assert.deepEqual(previewLines(undefined, changed), ["status: Applied → Interviewing"], "an unknown register shows the key");
});
