import { useMemo } from "react";
import type { AgentStepView } from "@nexus/contracts";
import { CoreStage } from "../core/CoreStage";
import { deriveCore, stepStatus, taskRows } from "../model/execution.ts";
import type { NodeKind } from "../model/execution.ts";
import { ago, seconds, uptime } from "../model/format.ts";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNow } from "../state/store";
import { Empty, Panel, StatusPill } from "../ui";
import { CoreIcon, PulseIcon, TasksIcon } from "../icons";

const WINDOW_MS = 30 * 60_000;

export function Overview() {
  const d = useNexus();
  const { focusRun, pending } = useDerived();
  const now = useNow(1000);
  const goTo = useGoTo();
  const core = deriveCore({ run: focusRun, pendingApprovals: pending.length, composing: d.composing, now });

  function openNode(id: NodeKind) {
    if (id === "model") goTo({ section: "models" });
    else if (id === "web") goTo({ section: "tools", tool: "web" });
    else if (id === "files") goTo({ section: "tools", tool: "files" });
    else if (id === "memory") goTo({ section: "memory" });
    else goTo({ section: "tasks", taskId: focusRun?.taskId ?? null });
  }

  return (
    <div className="overview">
      <div className="overview-stage">
        <div className="mission">
          <p className="eyebrow">Now</p>
          {focusRun ? (
            <button type="button" className="mission-task" onClick={() => goTo({ section: "tasks", taskId: focusRun.taskId })}>
              <span className="mission-title">{focusRun.taskTitle}</span>
              <StatusPill status={focusRun.status} />
            </button>
          ) : (
            <p className="mission-title muted">No task yet</p>
          )}
        </div>
        <CoreStage view={core} onNode={openNode} />
      </div>
      <div className="overview-strip">
        <RecentSummary steps={d.steps} pending={pending.length + (d.proposals?.length ?? 0)} now={now} />
        <RecentTasks now={now} />
        <SystemPanel />
      </div>
    </div>
  );
}

function RecentSummary({ steps, pending, now }: { steps: AgentStepView[] | null; pending: number; now: number }) {
  const s = useMemo(() => {
    const recent = (steps ?? []).filter((x) => now - x.started_at_ms < WINDOW_MS);
    const calls = recent.filter((x) => x.detail.kind === "ModelCall");
    const reads = recent.filter((x) => x.detail.kind === "Fetch");
    const notes = recent.filter((x) => x.detail.kind === "NoteProposed");
    const facts = recent.filter((x) => x.detail.kind === "MemoryProposed");
    const modelMs = calls.reduce((sum, x) => sum + (x.detail.kind === "ModelCall" ? (x.detail.duration_ms ?? 0) : 0), 0);
    return {
      any: recent.length > 0,
      replies: new Set(recent.map((x) => x.message_id)).size,
      calls: calls.length,
      callFails: calls.filter((x) => stepStatus(x) === "failed").length,
      modelMs,
      reads: reads.length,
      readFails: reads.filter((x) => stepStatus(x) === "failed").length,
      notes: notes.length,
      facts: facts.length,
    };
  }, [steps, now]);
  return (
    <Panel title="Last 30 minutes" icon={<PulseIcon size={15} />} id="summary">
      {!s.any ? (
        <Empty>No agent activity in the last 30 minutes.</Empty>
      ) : (
        <ul className="stat-list">
          <li>
            <span>Replies</span>
            <b>{s.replies}</b>
          </li>
          <li>
            <span>Model calls</span>
            <b>
              {s.calls}
              {s.modelMs > 0 && <small> · {seconds(s.modelMs)}</small>}
              {s.callFails > 0 && <small className="bad"> · {s.callFails} failed</small>}
            </b>
          </li>
          <li>
            <span>Web reads</span>
            <b>
              {s.reads}
              {s.readFails > 0 && <small className="bad"> · {s.readFails} refused or failed</small>}
            </b>
          </li>
          <li>
            <span>Notes proposed</span>
            <b>{s.notes}</b>
          </li>
          <li>
            <span>Facts proposed</span>
            <b>{s.facts}</b>
          </li>
          <li>
            <span>Waiting for you</span>
            <b className={pending > 0 ? "attention" : undefined}>{pending}</b>
          </li>
        </ul>
      )}
    </Panel>
  );
}

function RecentTasks({ now }: { now: number }) {
  const d = useNexus();
  const goTo = useGoTo();
  const rows = useMemo(() => taskRows(d.conversations ?? [], d.steps ?? [], d.runs ?? []).slice(0, 5), [d.conversations, d.steps, d.runs]);
  return (
    <Panel
      title="Recent tasks"
      icon={<TasksIcon size={15} />}
      id="recent-tasks"
      actions={
        <button type="button" className="link" onClick={() => goTo({ section: "tasks" })}>
          All tasks
        </button>
      }
    >
      {d.conversations === null ? (
        <Empty>Loading…</Empty>
      ) : rows.length === 0 ? (
        <Empty>No tasks yet.</Empty>
      ) : (
        <ul className="row-list">
          {rows.map((r) => (
            <li key={r.id}>
              <button type="button" className="row" onClick={() => goTo({ section: "tasks", taskId: r.id })}>
                <span className="row-title">{r.title}</span>
                <StatusPill status={r.status} />
                <span className="row-time">{ago(r.updatedAt, now)}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </Panel>
  );
}

function Meter({ value, max, label }: { value: number | null; max: number; label: string }) {
  const pct = value === null ? 0 : Math.max(0, Math.min(100, (value / max) * 100));
  return (
    <div className="meter" role="meter" aria-label={label} aria-valuemin={0} aria-valuemax={max} aria-valuenow={value ?? undefined}>
      <span style={{ width: `${pct}%` }} />
    </div>
  );
}

export function SystemPanel() {
  const d = useNexus();
  const { runningStep } = useDerived();
  const sys = d.system;
  const host = d.host?.state === "Connected" ? d.host : null;
  const writing = (d.runs ?? []).filter((r) => r.status === "Running" || r.status === "Verifying").length;
  const runningTasks = new Set((d.steps ?? []).filter((s) => s.status === "Running").map((s) => s.conversation_id)).size + writing;
  const gb = (mib: number) => (mib / 1024).toFixed(1);
  return (
    <Panel title="System" icon={<CoreIcon size={15} />} id="system">
      <ul className="sys-list">
        <li>
          <span>CPU</span>
          <Meter value={sys?.cpu_percent ?? null} max={100} label="CPU load" />
          <b>{sys?.cpu_percent != null ? `${Math.round(sys.cpu_percent)}%` : "—"}</b>
        </li>
        <li>
          <span>RAM</span>
          <Meter value={sys?.memory_used_mib ?? null} max={sys?.memory_total_mib ?? 1} label="Memory in use" />
          <b>{sys?.memory_used_mib != null && sys.memory_total_mib != null ? `${gb(sys.memory_used_mib)} / ${gb(sys.memory_total_mib)} GB` : "—"}</b>
        </li>
        <li>
          <span>Nexus.Host</span>
          <b title={host ? `up ${uptime(host.uptime_seconds)}` : undefined}>{host ? (host.paused ? "Paused" : "Connected") : "—"}</b>
        </li>
        <li>
          <span>Workers</span>
          <b title="Runs execute in the Host's own serial executor; no worker processes exist yet">None</b>
        </li>
        <li>
          <span>Active agents</span>
          <b>{runningStep ? "1 of 1" : "0 of 1"}</b>
        </li>
        <li>
          <span>Running tasks</span>
          <b>{runningTasks}</b>
        </li>
      </ul>
    </Panel>
  );
}
