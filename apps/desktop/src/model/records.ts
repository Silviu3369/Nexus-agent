// Pure helpers for registers (job applications first): how a record is
// named, counted, filtered and how its history reads. The Host checks and
// stores every value; these only present what it returned.

import type { FieldDef, RecordChangeView, RecordView, RegisterView } from "@nexus/contracts";

/** "Acme · Service Desk Analyst". */
export function recordTitle(register: RegisterView, values: Record<string, string>): string {
  const parts = register.title_fields.map((k) => values[k]).filter((v): v is string => Boolean(v));
  return parts.length > 0 ? parts.join(" · ") : `Untitled ${register.item_name}`;
}

export function statusOf(register: RegisterView, record: RecordView): string | null {
  return record.values[register.status_field] ?? null;
}

export function statusChoices(register: RegisterView): string[] {
  return register.fields.find((f) => f.key === register.status_field)?.choices ?? [];
}

/** How many records have each status, in the register's own order. */
export function statusCounts(register: RegisterView, records: RecordView[]): [string, number][] {
  return statusChoices(register).map((s) => [s, records.filter((r) => statusOf(register, r) === s).length]);
}

export interface RecordFilterState {
  status: string | null;
  dueOnly: boolean;
  text: string;
}

export function filterRecords(register: RegisterView, records: RecordView[], f: RecordFilterState): RecordView[] {
  const text = f.text.trim().toLowerCase();
  return records.filter(
    (r) =>
      (f.status === null || statusOf(register, r) === f.status) &&
      (!f.dueOnly || r.follow_up_due) &&
      (text === "" || Object.values(r.values).some((v) => v.toLowerCase().includes(text))),
  );
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "2026-10-18" → "18 Oct 2026"; anything else as it is. */
export function dateText(value: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!m) return value;
  return `${Number(m[3])} ${MONTHS[Number(m[2]) - 1]} ${m[1]}`;
}

/** A value as the field shows it. */
export function valueText(field: FieldDef, value: string | undefined): string {
  if (value === undefined || value === "") return "—";
  return field.kind === "Date" ? dateText(value) : value;
}

/** The pill a status shows: open work, waiting, good or closed. */
export function statusTone(register: RegisterView, status: string | null): "queued" | "running" | "completed" | "cancelled" {
  if (status === null) return "queued";
  if (register.closed_statuses.includes(status)) return "cancelled";
  const choices = statusChoices(register);
  const i = choices.indexOf(status);
  if (i <= 0) return "queued";
  // The last open status before the closed ones is the good outcome.
  const open = choices.filter((c) => !register.closed_statuses.includes(c));
  return status === open[open.length - 1] ? "completed" : "running";
}

/** "Nexus added it", "You changed status, next step", "You undid a change". */
export function changeText(register: RegisterView, change: RecordChangeView, agent: string): string {
  const who = change.actor === "Agent" ? agent : "You";
  if (change.kind === "Created") return `${who} added it`;
  if (change.kind === "Undo") return `${who} undid a change`;
  const labels = change.fields.map((c) => register.fields.find((f) => f.key === c.key)?.label ?? c.key);
  return `${who} changed ${labels.join(", ").toLowerCase() || "nothing"}`;
}

/** Each changed field as "Status: Applied → Interviewing". */
export function changeLines(register: RegisterView, change: RecordChangeView): string[] {
  if (change.kind === "Created") return [];
  return change.fields.map((c) => {
    const field = register.fields.find((f) => f.key === c.key);
    const label = field?.label ?? c.key;
    const show = (v: string | null) => (v === null ? "—" : field ? valueText(field, v) : v);
    return `${label}: ${show(c.before)} → ${show(c.after)}`;
  });
}

/** The form's values for a record (every field, empty when unset). */
export function formValues(register: RegisterView, record: RecordView | null): Record<string, string> {
  const out: Record<string, string> = {};
  for (const f of register.fields) out[f.key] = record?.values[f.key] ?? "";
  if (!record) {
    const status = statusChoices(register)[0];
    if (status) out[register.status_field] = status;
  }
  return out;
}

/** What the form would refuse before asking the Host. */
export function missingRequired(register: RegisterView, values: Record<string, string>): string[] {
  return register.fields.filter((f) => f.required && (values[f.key] ?? "").trim() === "").map((f) => f.label);
}

export type RecordSort = "changed" | "match" | "follow_up";

/** The records in the order chosen: last changed, best match, or the
 *  nearest follow-up date first. Records without the value come last. */
export function sortRecords(register: RegisterView, records: RecordView[], sort: RecordSort): RecordView[] {
  const out = [...records];
  if (sort === "match") {
    const score = (r: RecordView) => {
      const n = Number(r.values.match_score);
      return r.values.match_score !== undefined && Number.isFinite(n) ? n : -1;
    };
    out.sort((a, b) => score(b) - score(a));
  } else if (sort === "follow_up" && register.follow_up_field) {
    const key = register.follow_up_field;
    out.sort((a, b) => (a.values[key] ?? "9999").localeCompare(b.values[key] ?? "9999"));
  } else {
    out.sort((a, b) => b.updated_at_ms - a.updated_at_ms);
  }
  return out;
}

/** "Match 78" for a scored record. */
export function matchText(record: RecordView): string | null {
  const v = record.values.match_score;
  return v === undefined ? null : `Match ${v}`;
}
