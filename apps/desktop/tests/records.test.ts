import { test } from "node:test";
import assert from "node:assert/strict";
import type { FieldDef, RecordChangeView, RecordView, RegisterView } from "@nexus/contracts";
import {
  changeLines,
  changeText,
  dateText,
  filterRecords,
  formValues,
  missingRequired,
  recordTitle,
  statusCounts,
  statusTone,
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
