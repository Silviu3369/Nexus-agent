// Agents: the one agent this build has — the assistant, shaped by the
// persona — with its model, tools, rights and recent work. The specialised
// agents of the roadmap are listed as not available, with no controls.

import { useMemo, useState } from "react";
import { ASSISTANT, PLANNED_AGENTS, TOOLS } from "../model/catalog.ts";
import { describeStep, stepStatus } from "../model/execution.ts";
import type { ExecStatus } from "../model/execution.ts";
import { ago, clock } from "../model/format.ts";
import { PersonaCard } from "../Settings";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNow } from "../state/store";
import { AgentGlyph, Empty, Facts, Panel, StatusPill } from "../ui";
import { AgentIcon, AlertIcon } from "../icons";

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
        <Panel title="Planned agents" icon={<AgentIcon size={15} />} id="planned-agents" className="agent-planned">
          <p className="muted small">Named in the roadmap; none of them runs in this build, so none can be started.</p>
          <ul className="planned-list">
            {PLANNED_AGENTS.map((a) => (
              <li key={a.name}>
                <AgentGlyph id={a.name} size={24} />
                <span>
                  <b>{a.name}</b>
                  <small>{a.role}</small>
                </span>
                <span className="pill pill-unknown">{a.planned}</span>
              </li>
            ))}
          </ul>
        </Panel>
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
                  ["Thinking", active?.thinking_level ?? "—"],
                  ["Why this model", "You chose it. There is no automatic routing and no fallback to another provider."],
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
