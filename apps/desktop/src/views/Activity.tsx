// Activity: one chronological log of what the agent did (its recorded
// steps) and what the Host recorded (approvals, writes, settings), with
// filters on what each entry really carries.

import { useMemo, useState } from "react";
import { buildActivity } from "../model/execution.ts";
import type { ActivityTool, ExecStatus } from "../model/execution.ts";
import { clock } from "../model/format.ts";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus } from "../state/store";
import { Empty, StatusPill } from "../ui";

const TOOLS: (ActivityTool | "all")[] = ["all", "Model", "Web read", "Notes", "Memory", "MCP", "Automations", "System"];
const STATUSES: (ExecStatus | "all")[] = ["all", "running", "completed", "failed", "approval_required", "cancelled"];

export function ActivityView() {
  const d = useNexus();
  const { agentName } = useDerived();
  const goTo = useGoTo();
  const [task, setTask] = useState("all");
  const [project, setProject] = useState("all");
  const [tool, setTool] = useState<ActivityTool | "all">("all");
  const [model, setModel] = useState("all");
  const [status, setStatus] = useState<ExecStatus | "all">("all");
  const [source, setSource] = useState<"all" | "agent" | "host">("all");

  const entries = useMemo(
    () =>
      buildActivity({
        steps: d.steps ?? [],
        events: d.events,
        runs: d.runs ?? [],
        conversations: d.conversations ?? [],
        agent: agentName,
      }),
    [d.steps, d.events, d.runs, d.conversations, agentName],
  );
  const tasks = useMemo(() => {
    const seen = new Map<string, string>();
    for (const e of entries) if (e.taskId && e.taskTitle) seen.set(e.taskId, e.taskTitle);
    return [...seen.entries()];
  }, [entries]);
  const models = useMemo(() => [...new Set(entries.flatMap((e) => (e.model ? [e.model] : [])))], [entries]);

  const shown = entries.filter(
    (e) =>
      (project === "all" || e.projectId === project) &&
      (task === "all" || e.taskId === task) &&
      (tool === "all" || e.tool === tool) &&
      (model === "all" || e.model === model) &&
      (status === "all" || e.status === status) &&
      (source === "all" || e.source === source),
  );

  return (
    <div className="activity">
      <div className="filters" role="group" aria-label="Filter activity">
        <label>
          Source
          <select value={source} onChange={(e) => setSource(e.currentTarget.value as typeof source)}>
            <option value="all">Agent and Host</option>
            <option value="agent">Agent steps</option>
            <option value="host">Host events</option>
          </select>
        </label>
        <label>
          Project
          <select value={project} onChange={(e) => setProject(e.currentTarget.value)}>
            <option value="all">All projects</option>
            {(d.projects ?? []).map((p) => (
              <option key={p.project_id} value={p.project_id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Task
          <select value={task} onChange={(e) => setTask(e.currentTarget.value)}>
            <option value="all">All tasks</option>
            {tasks.map(([id, title]) => (
              <option key={id} value={id}>
                {title}
              </option>
            ))}
          </select>
        </label>
        <label>
          Tool
          <select value={tool} onChange={(e) => setTool(e.currentTarget.value as ActivityTool | "all")}>
            {TOOLS.map((t) => (
              <option key={t} value={t}>
                {t === "all" ? "All tools" : t}
              </option>
            ))}
          </select>
        </label>
        <label>
          Model
          <select value={model} onChange={(e) => setModel(e.currentTarget.value)}>
            <option value="all">All models</option>
            {models.map((m) => (
              <option key={m} value={m}>
                {m}
              </option>
            ))}
          </select>
        </label>
        <label>
          Status
          <select value={status} onChange={(e) => setStatus(e.currentTarget.value as ExecStatus | "all")}>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {s === "all" ? "Any status" : s.replace("_", " ")}
              </option>
            ))}
          </select>
        </label>
        <span className="filters-count">{shown.length} entries</span>
      </div>
      <p className="muted small">
        Agent: {agentName} (the only one). Covers the last {d.steps?.length ?? 0} agent steps and {d.events.length} Host
        events; entries tied to no task appear only under All projects.
      </p>
      {shown.length === 0 ? (
        <Empty>{entries.length === 0 ? "Nothing has happened yet." : "No entry matches these filters."}</Empty>
      ) : (
        <ol className="log" aria-label="Activity log">
          {shown.map((e) => (
            <li key={e.id} className={`log-row log-${e.source}`}>
              <span className="log-time mono">{clock(e.at)}</span>
              <span className="log-tool">{e.tool}</span>
              <span className="log-main">
                <span className="log-title">{e.title}</span>
                {e.detail && <span className="log-detail">{e.detail}</span>}
              </span>
              {e.taskId && e.taskTitle ? (
                <button type="button" className="link log-task" onClick={() => goTo({ section: "tasks", taskId: e.taskId })}>
                  {e.taskTitle}
                </button>
              ) : (
                <span className="log-task muted">—</span>
              )}
              <span className="log-status">{e.status ? <StatusPill status={e.status} /> : null}</span>
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}
