// Tools: what Nexus can use, with which permission, what it is doing with
// it now and when it last did. Files opens the notes workspace.

import { useMemo, useState } from "react";
import type { ReactElement } from "react";
import { TOOLS } from "../model/catalog.ts";
import type { ToolId, ToolInfo } from "../model/catalog.ts";
import { describeStep, stepStatus } from "../model/execution.ts";
import { ago, clock } from "../model/format.ts";
import { NotesPanel } from "../Notes";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNow } from "../state/store";
import { Empty, Facts, Panel, StatusPill, Unavailable } from "../ui";
import { ApiIcon, BrowserIcon, FolderIcon, GitIcon, GlobeIcon, MemoryIcon, MonitorIcon, PlugIcon, TerminalIcon } from "../icons";

const ICON: Record<ToolId, (p: { size?: number }) => ReactElement> = {
  web: GlobeIcon,
  files: FolderIcon,
  memory: MemoryIcon,
  browser: BrowserIcon,
  terminal: TerminalIcon,
  computer: MonitorIcon,
  git: GitIcon,
  apis: ApiIcon,
  mcp: PlugIcon,
};

export function ToolsView({ initialTool }: { initialTool: ToolId }) {
  const [selected, setSelected] = useState<ToolId>(initialTool);
  const usage = useToolUsage();
  const tool = TOOLS.find((t) => t.id === selected)!;

  return (
    <div className="split tools">
      <section className="split-list panel" aria-label="Tools">
        <div className="panel-body">
          <ul className="row-list">
            {TOOLS.map((t) => {
              const Icon = ICON[t.id];
              const u = usage[t.id];
              return (
                <li key={t.id}>
                  <button
                    type="button"
                    className={`row tool-row${t.id === selected ? " selected" : ""}${t.available ? "" : " dim"}`}
                    aria-current={t.id === selected ? "true" : undefined}
                    onClick={() => setSelected(t.id)}
                  >
                    <Icon size={16} />
                    <span className="row-title">{t.name}</span>
                    {u?.running ? (
                      <StatusPill status="running" label="In use" />
                    ) : t.available ? (
                      <span className="pill pill-completed">Available</span>
                    ) : (
                      <span className="pill pill-unknown">Not available</span>
                    )}
                  </button>
                </li>
              );
            })}
          </ul>
        </div>
      </section>
      <div className="split-detail tool-detail">
        <ToolDetail tool={tool} />
      </div>
    </div>
  );
}

interface Usage {
  running: boolean;
  lastAt: number | null;
  count: number;
}

function useToolUsage(): Partial<Record<ToolId, Usage>> {
  const d = useNexus();
  return useMemo(() => {
    const steps = d.steps ?? [];
    const runs = d.runs ?? [];
    const reads = steps.filter((s) => s.detail.kind === "Fetch");
    const proposals = steps.filter((s) => s.detail.kind === "NoteProposed");
    const facts = steps.filter((s) => s.detail.kind === "MemoryProposed");
    const artifactTimes = runs.flatMap((r) => (r.artifact ? [r.artifact.created_at_ms] : []));
    const fileTimes = [...proposals.map((s) => s.started_at_ms), ...artifactTimes];
    return {
      web: { running: reads.some((s) => s.status === "Running"), lastAt: reads[0]?.started_at_ms ?? null, count: reads.length },
      files: {
        running: proposals.some((s) => s.status === "Running") || runs.some((r) => r.status === "Running" || r.status === "Verifying"),
        lastAt: fileTimes.length > 0 ? Math.max(...fileTimes) : null,
        count: proposals.length + runs.length,
      },
      memory: { running: facts.some((s) => s.status === "Running"), lastAt: facts[0]?.started_at_ms ?? null, count: facts.length },
    };
  }, [d.steps, d.runs]);
}

function ToolDetail({ tool }: { tool: ToolInfo }) {
  const usage = useToolUsage()[tool.id];
  const { agentName } = useDerived();
  const goTo = useGoTo();
  const now = useNow(5000);
  const Icon = ICON[tool.id];
  return (
    <>
      <Panel title={tool.name} icon={<Icon size={15} />} id={`tool-${tool.id}`}>
        {!tool.available ? (
          <Unavailable reason={tool.reason ?? "Not available."} planned={tool.planned} />
        ) : (
          <>
            <Facts
              rows={[
                ["Availability", <span className="pill pill-completed">Available</span>],
                ["Permission", tool.permission],
                ["Model tool", <span className="mono">{tool.hostTool}</span>],
                ["Connected agent", agentName],
                ["Current activity", usage?.running ? <StatusPill status="running" label="In use now" /> : "idle"],
                ["Last use", usage?.lastAt ? ago(usage.lastAt, now) : "never"],
              ]}
            />
            <h3>Rules enforced by the Host</h3>
            <ul className="bullets">
              {tool.limits.map((l) => (
                <li key={l}>{l}</li>
              ))}
            </ul>
            {tool.id === "memory" && (
              <button type="button" className="ghost" onClick={() => goTo({ section: "memory" })}>
                Open Memory
              </button>
            )}
          </>
        )}
      </Panel>
      {tool.id === "web" && <RecentReads />}
      {tool.id === "files" && (
        <div className="notes-workspace">
          <NotesPanel />
        </div>
      )}
    </>
  );
}

function RecentReads() {
  const d = useNexus();
  const goTo = useGoTo();
  const reads = (d.steps ?? []).filter((s) => s.detail.kind === "Fetch").slice(0, 12);
  return (
    <Panel title="Recent reads" icon={<GlobeIcon size={15} />} id="recent-reads">
      {reads.length === 0 ? (
        <Empty>No web reads yet.</Empty>
      ) : (
        <ul className="row-list">
          {reads.map((s) => {
            const { title, detail } = describeStep(s);
            return (
              <li key={s.step_id}>
                <button type="button" className="row" onClick={() => goTo({ section: "tasks", taskId: s.conversation_id })}>
                  <span className="row-title" title={s.detail.kind === "Fetch" ? s.detail.url : undefined}>
                    {title}
                    {detail && <small> · {detail}</small>}
                  </span>
                  <StatusPill status={stepStatus(s)} />
                  <span className="row-time">{clock(s.started_at_ms)}</span>
                </button>
              </li>
            );
          })}
        </ul>
      )}
    </Panel>
  );
}
