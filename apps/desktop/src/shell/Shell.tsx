import { useCallback, useState } from "react";
import type { CSSProperties, ReactElement } from "react";
import type { HostStatus } from "@nexus/contracts";
import { uptime } from "../model/format.ts";
import { usePref } from "../state/prefs";
import { useDerived, useNexus } from "../state/store";
import {
  AgentIcon,
  AutomationIcon,
  CoreIcon,
  FolderIcon,
  HexIcon,
  MemoryIcon,
  PulseIcon,
  RailIcon,
  RouteIcon,
  SettingsIcon,
  TasksIcon,
  ToolIcon,
} from "../icons";
import { CommandBar } from "./CommandBar";
import { ExecutionPanel } from "./ExecutionPanel";
import { NavContext } from "./nav";
import type { NavTarget, Section } from "./nav";
import { Overview } from "../views/Overview";
import { TasksView } from "../views/Tasks";
import { AgentsView } from "../views/Agents";
import { ModelsView } from "../views/Models";
import { MemoryView } from "../views/Memory";
import { ToolsView } from "../views/Tools";
import { AutomationsView } from "../views/Automations";
import { ProjectsView } from "../views/Projects";
import { ActivityView } from "../views/Activity";
import { SettingsView } from "../views/Settings";

const SECTIONS: { id: Section; label: string; icon: (p: { size?: number }) => ReactElement; hint: string }[] = [
  { id: "overview", label: "Overview", icon: CoreIcon, hint: "What Nexus is doing now" },
  { id: "agents", label: "Agents", icon: AgentIcon, hint: "Who does the work, with which rights" },
  { id: "tasks", label: "Tasks", icon: TasksIcon, hint: "Requests, their runs, steps and actions" },
  { id: "projects", label: "Projects", icon: FolderIcon, hint: "Workspaces" },
  { id: "memory", label: "Memory", icon: MemoryIcon, hint: "What Nexus knows and sends" },
  { id: "automations", label: "Automations", icon: AutomationIcon, hint: "Scheduled and conditional work" },
  { id: "tools", label: "Tools", icon: ToolIcon, hint: "What Nexus can use, and how" },
  { id: "models", label: "Models", icon: RouteIcon, hint: "Providers, models and routing" },
  { id: "activity", label: "Activity", icon: PulseIcon, hint: "Everything that happened, in order" },
  { id: "settings", label: "Settings", icon: SettingsIcon, hint: "Host, appearance, persona, data" },
];

type Connected = Extract<HostStatus, { state: "Connected" }>;

export function Shell({ host }: { host: Connected }) {
  const [target, setTarget] = useState<NavTarget>({ section: "overview" });
  const [railCollapsed, setRailCollapsed] = usePref("railCollapsed");
  const [panelCollapsed, setPanelCollapsed] = usePref("panelCollapsed");
  const [panelWidth, setPanelWidth] = usePref("panelWidth");
  const goTo = useCallback((t: NavTarget) => setTarget(t), []);
  const section = SECTIONS.find((s) => s.id === target.section) ?? SECTIONS[0];

  const style = { "--panel-w": `${panelWidth}px` } as CSSProperties;

  return (
    <NavContext.Provider value={goTo}>
      <div
        className={`app${railCollapsed ? " rail-collapsed" : ""}${panelCollapsed ? " panel-collapsed" : ""}`}
        style={style}
      >
        <NavRail
          current={target.section}
          collapsed={railCollapsed}
          onToggle={() => setRailCollapsed(!railCollapsed)}
          onGo={(id) => goTo({ section: id })}
          host={host}
        />
        <main className="workspace" aria-labelledby="section-title">
          <header className="workspace-head">
            <div>
              <h1 id="section-title">{section.label}</h1>
              <p className="workspace-hint">{section.hint}</p>
            </div>
          </header>
          {host.paused && (
            <p className="banner banner-bad" role="alert">
              Restored from a backup: requests are recorded, but nothing runs until you review this profile and delete
              restored.json from it.
            </p>
          )}
          <div className={`workspace-body workspace-${target.section}`}>
            <SectionView target={target} />
          </div>
        </main>
        <ExecutionPanel
          collapsed={panelCollapsed}
          onToggle={() => setPanelCollapsed(!panelCollapsed)}
          width={panelWidth}
          onResize={setPanelWidth}
        />
        <CommandBar />
      </div>
    </NavContext.Provider>
  );
}

function SectionView({ target }: { target: NavTarget }) {
  switch (target.section) {
    case "overview":
      return <Overview />;
    case "tasks":
      return <TasksView key={String(target.taskId)} initialTaskId={target.taskId} />;
    case "agents":
      return <AgentsView />;
    case "models":
      return <ModelsView />;
    case "memory":
      return <MemoryView />;
    case "tools":
      return <ToolsView key={target.tool ?? "web"} initialTool={target.tool ?? "web"} />;
    case "automations":
      return <AutomationsView />;
    case "projects":
      return <ProjectsView />;
    case "activity":
      return <ActivityView />;
    case "settings":
      return <SettingsView />;
  }
}

function NavRail({
  current,
  collapsed,
  onToggle,
  onGo,
  host,
}: {
  current: Section;
  collapsed: boolean;
  onToggle: () => void;
  onGo: (s: Section) => void;
  host: Connected;
}) {
  const { system, steps } = useNexus();
  const { pending } = useDerived();
  const working = steps?.some((s) => s.status === "Running") ?? false;
  return (
    <aside className="rail" aria-label="Navigation">
      <div className="rail-brand">
        <HexIcon size={20} className="rail-mark" />
        <span className="rail-name">NEXUS</span>
        <button
          type="button"
          className="icon-button rail-toggle"
          onClick={onToggle}
          aria-label={collapsed ? "Expand navigation" : "Collapse navigation"}
          aria-expanded={!collapsed}
        >
          <RailIcon size={16} />
        </button>
      </div>
      <nav className="rail-nav" aria-label="Sections">
        {SECTIONS.map((s) => {
          const Icon = s.icon;
          const badge =
            s.id === "overview" && pending.length > 0 ? (
              <span className="rail-badge attention" aria-label={`${pending.length} waiting for approval`}>
                {pending.length}
              </span>
            ) : s.id === "tasks" && working ? (
              <span className="rail-badge live" aria-label="working" />
            ) : null;
          return (
            <button
              type="button"
              key={s.id}
              className={`rail-item${current === s.id ? " selected" : ""}`}
              aria-current={current === s.id ? "page" : undefined}
              aria-label={s.label}
              title={s.label}
              onClick={() => onGo(s.id)}
            >
              <Icon size={17} />
              <span className="rail-label">{s.label}</span>
              {badge}
            </button>
          );
        })}
      </nav>
      <HostBlock host={host} version={system?.host_version ?? null} />
    </aside>
  );
}

function HostBlock({ host, version }: { host: Connected; version: string | null }) {
  return (
    <div className="rail-host" title={`${host.host_identity} · protocol v${host.protocol_version}`}>
      <span className={`dot ${host.paused ? "dot-attention" : "dot-good"}`} aria-hidden="true" />
      <div className="rail-host-text">
        <p className="rail-host-name">Nexus.Host</p>
        <p className="rail-host-sub">
          {host.paused ? "Paused" : "Connected"} · Local{version ? ` · v${version}` : ""}
        </p>
        <p className="rail-host-sub">up {uptime(host.uptime_seconds)}</p>
      </div>
    </div>
  );
}
