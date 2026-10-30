// Which agent a new task starts with (ADR-AGENT-001): Nexus, the default,
// or one of the user's agents. A follow-up keeps its task's agent, so this
// is shown only for new tasks.

import type { AgentView } from "@nexus/contracts";

export function AgentSelect({
  agents,
  defaultName,
  value,
  onChange,
  className,
}: {
  agents: AgentView[];
  defaultName: string;
  value: string | null;
  onChange: (agentId: string | null) => void;
  className?: string;
}) {
  return (
    <select
      className={className}
      aria-label="Agent for new tasks"
      title="New tasks start with this agent"
      value={value ?? ""}
      onChange={(e) => onChange(e.currentTarget.value === "" ? null : e.currentTarget.value)}
    >
      <option value="">{defaultName}</option>
      {agents.map((a) => (
        <option key={a.agent_id} value={a.agent_id}>
          {a.name}
        </option>
      ))}
    </select>
  );
}
