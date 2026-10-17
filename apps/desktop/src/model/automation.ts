// Pure helpers for the Automations view: how a schedule, a next time and a
// past run read, and the local date/time inputs. Every time is computed by
// the Host; these only format what it returned (or what the user typed).

import type { AutomationRunView, AutomationView, Schedule } from "@nexus/contracts";

/** 0 = Monday … 6 = Sunday, as the Host counts them. */
export const DAY_NAMES = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"] as const;
export const EVERY_DAY = [0, 1, 2, 3, 4, 5, 6];
export const WEEKDAYS = [0, 1, 2, 3, 4];

const pad = (n: number) => String(n).padStart(2, "0");

export function timeText(hour: number, minute: number): string {
  return `${pad(hour)}:${pad(minute)}`;
}

/** "Mon 20 Oct 2026, 09:00" in this PC's local time. */
export function dateTimeText(ms: number): string {
  const d = new Date(ms);
  const day = DAY_NAMES[(d.getDay() + 6) % 7];
  const month = d.toLocaleDateString("en-GB", { month: "short" });
  return `${day} ${d.getDate()} ${month} ${d.getFullYear()}, ${timeText(d.getHours(), d.getMinutes())}`;
}

export function scheduleText(schedule: Schedule): string {
  if (schedule.kind === "Once") return `Once, ${dateTimeText(schedule.at_ms)}`;
  const days = [...schedule.days].sort((a, b) => a - b);
  const at = timeText(schedule.hour, schedule.minute);
  if (days.length === 7) return `Every day at ${at}`;
  if (days.join() === WEEKDAYS.join()) return `Weekdays at ${at}`;
  if (days.join() === "5,6") return `Weekends at ${at}`;
  return `${days.map((d) => DAY_NAMES[d]).join(", ")} at ${at}`;
}

/** "in 45 min", "in 3 h", "in 2 days". */
export function untilText(ms: number, now: number): string {
  const d = ms - now;
  if (d < 60_000) return "in under a minute";
  if (d < 3_600_000) return `in ${Math.round(d / 60_000)} min`;
  if (d < 2 * 86_400_000) return `in ${Math.round(d / 3_600_000)} h`;
  return `in ${Math.round(d / 86_400_000)} days`;
}

export function nextText(a: AutomationView, now: number): string {
  if (a.next_run_at_ms !== null) return `${dateTimeText(a.next_run_at_ms)} · ${untilText(a.next_run_at_ms, now)}`;
  if (!a.enabled) return "Off: it does not run until you turn it on";
  return "Finished: a one-time automation that already had its time";
}

export const RUN_LABEL: Record<AutomationRunView["status"], string> = {
  Started: "Started",
  Skipped: "Skipped",
  Missed: "Missed",
  Failed: "Not started",
};

/** The pill each outcome shows (the dashboard's status classes). */
export const RUN_PILL: Record<AutomationRunView["status"], "completed" | "cancelled" | "failed"> = {
  Started: "completed",
  Skipped: "cancelled",
  Missed: "cancelled",
  Failed: "failed",
};

/** How it started and why it did not, after its status and time. */
export function runText(run: AutomationRunView): string {
  const parts: string[] = [];
  if (run.manual) parts.push("Run now");
  else if (run.catch_up) parts.push("caught up late");
  if (run.reason) parts.push(run.reason);
  if (run.status === "Started" && run.conversation_id === null) parts.push("its task was deleted");
  return parts.join(" · ");
}

/** The value of an `<input type="datetime-local">` for a moment. */
export function toLocalInput(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${timeText(d.getHours(), d.getMinutes())}`;
}

/** The moment an `<input type="datetime-local">` names, or null. */
export function fromLocalInput(value: string): number | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/.exec(value);
  if (!m) return null;
  const [y, mo, d, h, mi] = m.slice(1).map(Number);
  const ms = new Date(y, mo - 1, d, h, mi).getTime();
  return Number.isNaN(ms) ? null : ms;
}

/** "09:30" → [9, 30], else null. */
export function parseTime(value: string): [number, number] | null {
  const m = /^(\d{2}):(\d{2})$/.exec(value);
  if (!m) return null;
  const h = Number(m[1]);
  const mi = Number(m[2]);
  return h <= 23 && mi <= 59 ? [h, mi] : null;
}
