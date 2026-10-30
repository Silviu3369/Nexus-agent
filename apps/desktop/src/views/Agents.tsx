// Agents: Nexus, the default agent shaped by the persona — with its model,
// tools, rights and recent work — and the user's own agents (ADR-AGENT-001),
// each a configuration that only takes away from Nexus's permissions.

import { useMemo, useState } from "react";
import type { AgentView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { AgentForm } from "../components/AgentForm";
import { ASSISTANT, TOOLS } from "../model/catalog.ts";
import { describeStep, stepStatus } from "../model/execution.ts";
import type { ExecStatus } from "../model/execution.ts";
import { ago, clock } from "../model/format.ts";
import { PersonaCard } from "../Settings";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNexusActions, useNow } from "../state/store";
import { AgentGlyph, Empty, ErrorLine, Facts, Panel, StatusPill } from "../ui";
import { AgentIcon, AlertIcon, PlusIcon } from "../icons";

export function AgentsView() {
  const d = useNexus();
  const { agentName, active, runningStep, pending } = useDerived();
  const now = useNow(5000);
  const goTo = useGoTo();
  const [tab, setTab] = useState<"overview" | "config">("overview");

  const ownPending = pending.filter((r) => r.origin);
  const status = runningStep ? "running" : ownPending.length > 0 ? "approval_required" : null;
  const statusLabel = runningStep ? "Active" : ownPending.length > 0 ? "Waiting for you" : "Idle";
  const last = d.steps?.[0] ?? null;
  const executions = useMemo(() => {
    // A run's outcome is its last step's; a refused web read earlier in a
    // run that still answered does not make the run failed.
    const byReply = new Map<string, { title: string; taskId: string; at: number; steps: number; lastSeq: number; status: ExecStatus }>();
    for (const s of d.steps ?? []) {
      const e = byReply.get(s.message_id) ?? { title: s.conversation_title, taskId: s.conversation_id, at: s.started_at_ms, steps: 0, lastSeq: -1, status: "completed" as ExecStatus };
      e.steps += 1;
      e.at = Math.min(e.at, s.started_at_ms);
      if (s.status === "Running") e.status = "running";
      else if (s.seq > e.lastSeq && e.status !== "running") e.status = stepStatus(s);
      e.lastSeq = Math.max(e.lastSeq, s.seq);
      byReply.set(s.message_id, e);
    }
    return [...byReply.values()].sort((a, b) => b.at - a.at).slice(0, 8);
  }, [d.steps]);
  const errors = (d.steps ?? []).filter((s) => stepStatus(s) === "failed").slice(0, 6);

  return (
    <div className="agents">
      <div className="agent-grid">
        <article className={`agent-card selected${runningStep ? " live" : ""}`} aria-label={`Agent ${agentName}`}>
          <AgentGlyph id={ASSISTANT.id} size={40} active={runningStep !== null} />
          <div className="agent-card-main">
            <p className="agent-name">{agentName}</p>
            <p className="agent-role">{ASSISTANT.role}</p>
          </div>
          <StatusPill status={status} label={statusLabel} />
          <Facts
            rows={[
              ["Current task", runningStep ? runningStep.conversation_title : ownPending[0]?.origin?.conversation_title ?? "—"],
              ["Model", active?.model ? <span className="mono">{active.model}</span> : "not chosen"],
              ["Tools", ASSISTANT.tools.map((t) => TOOLS.find((x) => x.id === t)!.name).join(", ")],
              ["Last activity", last ? ago(last.started_at_ms, now) : "never"],
            ]}
          />
        </article>
        <YourAgents defaultName={agentName} />
      </div>

      <section className="panel inspector" aria-labelledby="inspector-heading">
        <header className="panel-head">
          <h2 id="inspector-heading">
            <AgentGlyph id={ASSISTANT.id} size={18} /> Inspector · {agentName}
          </h2>
          <div className="segmented" role="group" aria-label="Inspector section">
            <button type="button" aria-pressed={tab === "overview"} onClick={() => setTab("overview")}>
              Execution
            </button>
            <button type="button" aria-pressed={tab === "config"} onClick={() => setTab("config")}>
              Configuration
            </button>
          </div>
        </header>
        {tab === "config" ? (
          <div className="panel-body inspector-config">
            <PersonaCard />
          </div>
        ) : (
          <div className="panel-body inspector-grid">
            <div>
              <h3>Model routing</h3>
              <Facts
                rows={[
                  ["Model", active?.model ? <span className="mono">{active.model}</span> : "not chosen"],
                  ["Connection", active ? active.display_name : "none"],
                  ["Reasoning effort", active?.thinking_level ?? "—"],
                  ["Why this model", "You chose it. Nexus does not pick models on its own; only a backup model you choose in Models can take over, as a visible step."],
                ]}
              />
              <button type="button" className="link" onClick={() => goTo({ section: "models" })}>
                Change in Models
              </button>
              <h3>Permissions</h3>
              <ul className="bullets">
                {ASSISTANT.permissions.map((p) => (
                  <li key={p}>{p}</li>
                ))}
              </ul>
              <h3>Tools</h3>
              <ul className="bullets">
                {ASSISTANT.tools.map((id) => {
                  const t = TOOLS.find((x) => x.id === id)!;
                  return (
                    <li key={id}>
                      <b>{t.name}</b> <span className="mono small">{t.hostTool}</span> — {t.permission}
                    </li>
                  );
                })}
              </ul>
            </div>
            <div>
              <h3>Current execution</h3>
              {runningStep ? (
                <p>
                  {`${describeStep(runningStep).title} · task “${runningStep.conversation_title}”`}
                </p>
              ) : (
                <p className="muted">Not working right now.</p>
              )}
              <h3>Recent executions</h3>
              {executions.length === 0 ? (
                <Empty>No runs recorded yet.</Empty>
              ) : (
                <ul className="row-list">
                  {executions.map((e, i) => (
                    <li key={i}>
                      <button type="button" className="row" onClick={() => goTo({ section: "tasks", taskId: e.taskId })}>
                        <span className="row-title">{e.title}</span>
                        <span className="row-time">
                          {e.steps} {e.steps === 1 ? "step" : "steps"}
                        </span>
                        <StatusPill status={e.status} />
                        <span className="row-time">{clock(e.at)}</span>
                      </button>
                    </li>
                  ))}
                </ul>
              )}
              <h3>Errors</h3>
              {errors.length === 0 ? (
                <Empty>No failed steps in the recent window.</Empty>
              ) : (
                <ul className="error-list">
                  {errors.map((s) => (
                    <li key={s.step_id}>
                      <AlertIcon size={13} />
                      <span>
                        <b>{describeStep(s).title}</b> · {s.detail.error} <small>{clock(s.started_at_ms)}</small>
                      </span>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </div>
        )}
      </section>
    </div>
  );
}

/** The user's agents: each one with what it is for, its model and project,
 *  and how many tasks it started; created, edited and archived here. */
function YourAgents({ defaultName }: { defaultName: string }) {
  const { agents, connections, projects } = useNexus();
  const { refresh } = useNexusActions();
  // `"new"` or the agent being edited.
  const [editing, setEditing] = useState<AgentView | "new" | null>(null);
  const [archiving, setArchiving] = useState<AgentView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function archive(agent: AgentView) {
    setBusy(true);
    setError(null);
    try {
      await api.archiveAgent(agent.agent_id);
      setArchiving(null);
      await refresh(["agents"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const modelOf = (a: AgentView) =>
    a.connection_id === null
      ? "the active model"
      : `${a.model ?? "?"} · ${connections?.find((c) => c.connection_id === a.connection_id)?.display_name ?? "connection"}`;
  const projectOf = (a: AgentView) =>
    a.project_id === null ? "the active project" : (projects?.find((p) => p.project_id === a.project_id)?.name ?? "a project");

  return (
    <Panel
      title="Your agents"
      icon={<AgentIcon size={15} />}
      id="your-agents"
      actions={
        editing === null && (
          <button type="button" className="ghost" onClick={() => setEditing("new")}>
            <PlusIcon size={13} /> New agent
          </button>
        )
      }
    >
      {editing !== null ? (
        <AgentForm
          agent={editing === "new" ? null : editing}
          onDone={(saved) => {
            setEditing(null);
            if (saved) void refresh(["agents"]);
          }}
        />
      ) : (
        <>
          <p className="muted small">
            {`A new task starts with ${defaultName} unless you pick an agent in the command bar. An agent uses its own instructions, skills, model and project, and only the tools you allow it.`}
          </p>
          {agents === null ? (
            <p className="muted">Loading…</p>
          ) : agents.length === 0 ? (
            <Empty>No agent yet. Create one for each kind of work, for example your job search and your helpdesk.</Empty>
          ) : (
            <ul className="planned-list agent-list">
              {agents.map((a) => (
                <li key={a.agent_id}>
                  <AgentGlyph id={a.name} size={24} />
                  <span>
                    <b>{a.name}</b>
                    <small>{a.description || "No description"}</small>
                    <small>{`Model: ${modelOf(a)} · Project: ${projectOf(a)} · ${a.tasks} ${a.tasks === 1 ? "task" : "tasks"}`}</small>
                  </span>
                  <span className="actions">
                    <button type="button" className="ghost" onClick={() => setEditing(a)}>
                      Edit
                    </button>
                    <button type="button" className="ghost" onClick={() => setArchiving(a)}>
                      Archive
                    </button>
                  </span>
                </li>
              ))}
            </ul>
          )}
          {archiving && (
            <div className="confirm-bar" role="alert">
              <p>{`Archive “${archiving.name}”? No new task starts with it; its tasks and what it wrote keep its name.`}</p>
              <div className="actions">
                <button type="button" className="danger" disabled={busy} onClick={() => void archive(archiving)}>
                  Archive now
                </button>
                <button type="button" className="ghost" disabled={busy} onClick={() => setArchiving(null)}>
                  Keep
                </button>
              </div>
            </div>
          )}
          <ErrorLine error={error} />
        </>
      )}
    </Panel>
  );
}
