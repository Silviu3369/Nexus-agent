// Automations: an instruction you wrote, sent as a new task at the times
// you chose, in this PC's local time. Each run is a task like one you sent
// yourself: the active model answers, proposals wait for your approval and
// the daily token budget applies. The Host keeps the schedule, computes
// every next time and records each outcome; this view only shows them.

import { useEffect, useState } from "react";
import type { AutomationRunView, AutomationView, ProjectId, Schedule } from "@nexus/contracts";
import { api, describeError } from "../api";
import {
  DAY_NAMES,
  EVERY_DAY,
  RUN_LABEL,
  RUN_PILL,
  WEEKDAYS,
  dateTimeText,
  fromLocalInput,
  nextText,
  parseTime,
  runText,
  scheduleText,
  timeText,
  toLocalInput,
} from "../model/automation.ts";
import { useGoTo } from "../shell/nav";
import { useNexus, useNexusActions, useNow } from "../state/store";
import { Empty, ErrorLine, Facts, Panel, StatusPill } from "../ui";
import { AutomationIcon, PlayIcon, PlusIcon } from "../icons";

const MAX_NAME = 100;
const MAX_PROMPT_BYTES = 16 * 1024;
const HISTORY_SHOWN = 20;
const DAY_FULL = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];

export function AutomationsView() {
  const d = useNexus();
  const [creating, setCreating] = useState(false);
  return (
    <div className="automations">
      <Panel
        title="Automations"
        icon={<AutomationIcon size={15} />}
        id="automations"
        actions={
          <button type="button" className="ghost" disabled={creating} onClick={() => setCreating(true)}>
            <PlusIcon size={14} /> New automation
          </button>
        }
      >
        {creating && <AutomationForm onDone={() => setCreating(false)} />}
        {d.automations === null ? (
          <Empty>Loading…</Empty>
        ) : d.automations.length === 0 ? (
          !creating && <Empty>No automations yet. Create one to have an instruction sent as a task at the times you choose.</Empty>
        ) : (
          <ul className="automation-list" aria-label="Automations">
            {d.automations.map((a) => (
              <li key={a.automation_id}>
                <AutomationCard automation={a} />
              </li>
            ))}
          </ul>
        )}
      </Panel>
      <div className="automations-side">
        <Panel title="How they run" icon={<AutomationIcon size={15} />} id="automation-rules">
          <ul className="bullets">
            <li>Each time, the instruction is sent as a new task, exactly as if you sent it: the active model answers, anything it proposes waits for your approval, and the daily token budget applies.</li>
            <li>Times are this PC's local time; the summer/winter clock change is followed.</li>
            <li>Only while Nexus.Host is running. A time missed while it was off (or the PC slept) is caught up once at start-up, if you allow it and it is less than a day old; several missed times never run as a burst.</li>
            <li>A run whose reply is still being written is not overlapped: the next time is skipped and noted.</li>
            <li>Deleting an automation keeps the tasks it started.</li>
          </ul>
        </Panel>
      </div>
    </div>
  );
}

/** The next full hour, for a new one-time automation. */
function nextHour(now: number): number {
  const d = new Date(now);
  d.setMinutes(0, 0, 0);
  d.setHours(d.getHours() + 1);
  return d.getTime();
}

function AutomationForm({ initial, onDone }: { initial?: AutomationView; onDone: () => void }) {
  const d = useNexus();
  const { refresh } = useNexusActions();
  const start = initial?.schedule;
  const [name, setName] = useState(initial?.name ?? "");
  const [prompt, setPrompt] = useState(initial?.prompt ?? "");
  const [project, setProject] = useState<string>(initial?.project_id ?? "");
  const [kind, setKind] = useState<Schedule["kind"]>(start?.kind ?? "Weekly");
  const [days, setDays] = useState<number[]>(start?.kind === "Weekly" ? start.days : EVERY_DAY);
  const [time, setTime] = useState(start?.kind === "Weekly" ? timeText(start.hour, start.minute) : "09:00");
  const [at, setAt] = useState(toLocalInput(start?.kind === "Once" ? start.at_ms : nextHour(Date.now())));
  const [catchUp, setCatchUp] = useState(initial?.catch_up ?? true);
  const [enabled, setEnabled] = useState(initial?.enabled ?? true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const parsedTime = parseTime(time);
  const atMs = fromLocalInput(at);
  const schedule: Schedule | null =
    kind === "Weekly"
      ? parsedTime && days.length > 0
        ? { kind: "Weekly", days: [...days].sort((a, b) => a - b), hour: parsedTime[0], minute: parsedTime[1] }
        : null
      : atMs !== null
        ? { kind: "Once", at_ms: atMs }
        : null;
  const promptBytes = new TextEncoder().encode(prompt.trim()).length;
  const ok =
    name.trim() !== "" &&
    name.trim().length <= MAX_NAME &&
    promptBytes > 0 &&
    promptBytes <= MAX_PROMPT_BYTES &&
    schedule !== null;

  function toggleDay(day: number) {
    setDays((current) => (current.includes(day) ? current.filter((x) => x !== day) : [...current, day]));
  }

  async function save() {
    if (!schedule) return;
    setBusy(true);
    setError(null);
    try {
      await api.saveAutomation(
        initial?.automation_id ?? null,
        initial?.revision ?? 0,
        name.trim(),
        prompt.trim(),
        project === "" ? null : (project as ProjectId),
        schedule,
        enabled,
        catchUp,
      );
      await refresh(["automations"]);
      onDone();
    } catch (err) {
      setError(describeError(err));
      await refresh(["automations"]);
    } finally {
      setBusy(false);
    }
  }

  const label = initial ? `Edit automation ${initial.name}` : "New automation";
  return (
    <form
      className="stack automation-form"
      aria-label={label}
      onSubmit={(e) => {
        e.preventDefault();
        if (ok) void save();
      }}
    >
      <label>
        Name
        <input value={name} maxLength={MAX_NAME} placeholder="e.g. Morning summary" onChange={(e) => setName(e.currentTarget.value)} />
      </label>
      <label>
        Instruction
        <textarea
          value={prompt}
          rows={3}
          placeholder="What the agent should do each time, as you would write it in a task."
          onChange={(e) => setPrompt(e.currentTarget.value)}
        />
      </label>
      <label>
        Project
        <select value={project} onChange={(e) => setProject(e.currentTarget.value)}>
          <option value="">The active project at each run</option>
          {(d.projects ?? []).map((p) => (
            <option key={p.project_id} value={p.project_id}>
              {p.name}
            </option>
          ))}
        </select>
      </label>
      <div className="automation-when">
        <span className="field-label">When</span>
        <div className="segmented" role="group" aria-label="Schedule kind">
          <button type="button" aria-pressed={kind === "Weekly"} onClick={() => setKind("Weekly")}>
            Repeat
          </button>
          <button type="button" aria-pressed={kind === "Once"} onClick={() => setKind("Once")}>
            Once
          </button>
        </div>
      </div>
      {kind === "Weekly" ? (
        <>
          <div className="automation-repeat">
            <div className="segmented" role="group" aria-label="Days">
              {DAY_NAMES.map((n, i) => (
                <button key={n} type="button" aria-pressed={days.includes(i)} aria-label={DAY_FULL[i]} onClick={() => toggleDay(i)}>
                  {n}
                </button>
              ))}
            </div>
            <button type="button" className="link" onClick={() => setDays(EVERY_DAY)}>
              Every day
            </button>
            <button type="button" className="link" onClick={() => setDays(WEEKDAYS)}>
              Weekdays
            </button>
          </div>
          <label className="automation-time">
            Time
            <input type="time" value={time} onChange={(e) => setTime(e.currentTarget.value)} />
          </label>
        </>
      ) : (
        <label className="automation-time">
          Date and time
          <input type="datetime-local" value={at} onChange={(e) => setAt(e.currentTarget.value)} />
        </label>
      )}
      {schedule && <p className="muted">{`${scheduleText(schedule)} (this PC's local time).`}</p>}
      {kind === "Weekly" && days.length === 0 && <p className="muted">Choose at least one day.</p>}
      <label className="check">
        <input type="checkbox" checked={catchUp} onChange={(e) => setCatchUp(e.currentTarget.checked)} />
        If Nexus.Host was not running at that time, run it once when it starts (only within a day)
      </label>
      <label className="check">
        <input type="checkbox" checked={enabled} onChange={(e) => setEnabled(e.currentTarget.checked)} />
        On
      </label>
      <div className="actions">
        <button type="submit" className="primary" disabled={!ok || busy}>
          {initial ? "Save" : "Create automation"}
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={onDone}>
          Cancel
        </button>
      </div>
      <ErrorLine error={error} />
    </form>
  );
}

function AutomationCard({ automation: a }: { automation: AutomationView }) {
  const d = useNexus();
  const { refresh } = useNexusActions();
  const goTo = useGoTo();
  const now = useNow(30_000);
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [history, setHistory] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const project = a.project_id ? d.projects?.find((p) => p.project_id === a.project_id) : null;
  const state = a.next_run_at_ms !== null ? "On" : a.enabled ? "Finished" : "Off";

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await refresh(["automations"]);
    } catch (err) {
      setError(describeError(err));
      await refresh(["automations"]);
    } finally {
      setBusy(false);
    }
  }

  function setOn(on: boolean) {
    void act(() =>
      api.saveAutomation(a.automation_id, a.revision, a.name, a.prompt, a.project_id, a.schedule, on, a.catch_up),
    );
  }

  if (editing) {
    return (
      <article className="automation" aria-label={`Automation ${a.name}`}>
        <AutomationForm initial={a} onDone={() => setEditing(false)} />
      </article>
    );
  }

  return (
    <article className={`automation${state === "On" ? "" : " dim-soft"}`} aria-label={`Automation ${a.name}`}>
      <header className="project-head">
        <h3>{a.name}</h3>
        <span className={`badge ${state === "On" ? "badge-good" : "badge-neutral"}`}>{state}</span>
      </header>
      <p className="automation-prompt">{a.prompt}</p>
      <Facts
        rows={[
          ["Schedule", scheduleText(a.schedule)],
          ["Next", nextText(a, now)],
          [
            "Last",
            a.last_run ? (
              <RunLine run={a.last_run} onOpen={(id) => goTo({ section: "tasks", taskId: id })} />
            ) : (
              "not run yet"
            ),
          ],
          ["Project", project ? project.name : "the active project at each run"],
          ["If missed", a.catch_up ? "caught up once at start-up, within a day" : "not caught up"],
        ]}
      />
      {confirm && (
        <div className="confirm-bar" role="alert">
          <p>{`Delete “${a.name}” with its history? The tasks it started stay.`}</p>
          <div className="actions">
            <button type="button" className="danger" disabled={busy} onClick={() => void act(() => api.deleteAutomation(a.automation_id))}>
              Delete for good
            </button>
            <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </button>
          </div>
        </div>
      )}
      <div className="actions">
        <button
          type="button"
          className="secondary"
          disabled={busy}
          onClick={() => void act(() => api.runAutomationNow(a.automation_id))}
        >
          <PlayIcon size={14} /> Run now
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={() => setEditing(true)}>
          Edit
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={() => setOn(!a.enabled)}>
          {a.enabled ? "Turn off" : "Turn on"}
        </button>
        <button type="button" className="ghost" aria-expanded={history} onClick={() => setHistory(!history)}>
          {history ? "Hide history" : "History"}
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(true)}>
          Delete
        </button>
      </div>
      {history && <History automation={a} />}
      <ErrorLine error={error} />
    </article>
  );
}

function RunLine({ run, onOpen }: { run: AutomationRunView; onOpen: (task: string) => void }) {
  return (
    <span className="automation-run">
      <StatusPill status={RUN_PILL[run.status]} label={RUN_LABEL[run.status]} />
      <span>{dateTimeText(run.scheduled_for_ms)}</span>
      {runText(run) && <span className="muted">{runText(run)}</span>}
      {run.conversation_id && (
        <button type="button" className="link" onClick={() => onOpen(run.conversation_id as string)}>
          Open task
        </button>
      )}
    </span>
  );
}

function History({ automation: a }: { automation: AutomationView }) {
  const goTo = useGoTo();
  const [runs, setRuns] = useState<AutomationRunView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Reread when a new outcome or an edit arrives.
  const marker = `${a.revision}:${a.last_run?.at_ms ?? 0}`;
  useEffect(() => {
    let alive = true;
    api
      .automationRuns(a.automation_id, HISTORY_SHOWN)
      .then((r) => alive && setRuns(r))
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [a.automation_id, marker]);

  if (error) return <ErrorLine error={error} />;
  if (runs === null) return <p className="muted">Loading…</p>;
  if (runs.length === 0) return <p className="muted">No runs yet.</p>;
  return (
    <ol className="automation-history" aria-label={`History of ${a.name}`}>
      <li className="automation-history-head">History, newest first</li>
      {runs.map((r, i) => (
        <li key={`${r.at_ms}-${i}`}>
          <RunLine run={r} onOpen={(id) => goTo({ section: "tasks", taskId: id })} />
        </li>
      ))}
    </ol>
  );
}
