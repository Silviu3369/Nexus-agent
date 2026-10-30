// Creating or editing one of the user's agents (ADR-AGENT-001). Every part
// only takes away from what Nexus has: tools the user turned off in
// Permissions stay off, skills must be on to count, and the instructions
// grant nothing. The Host checks each part again when it is saved.

import { useEffect, useState } from "react";
import type { AgentView, McpServerView, PermissionView, SkillView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { useNexus } from "../state/store";
import { ErrorLine } from "../ui";

export function AgentForm({
  agent,
  onDone,
}: {
  /** `null`: a new agent. */
  agent: AgentView | null;
  onDone: (saved: AgentView | null) => void;
}) {
  const { connections, projects, mcp } = useNexus();
  const [name, setName] = useState(agent?.name ?? "");
  const [description, setDescription] = useState(agent?.description ?? "");
  const [instructions, setInstructions] = useState(agent?.instructions ?? "");
  const [connectionId, setConnectionId] = useState<string | null>(agent?.connection_id ?? null);
  const [model, setModel] = useState(agent?.model ?? "");
  const [projectId, setProjectId] = useState<string | null>(agent?.project_id ?? null);
  const [skills, setSkills] = useState<string[]>(agent?.skills ?? []);
  const [servers, setServers] = useState<string[]>(agent?.mcp_servers ?? []);
  const [toolsOff, setToolsOff] = useState<string[]>(agent?.tools_off ?? []);
  const [allSkills, setAllSkills] = useState<SkillView[] | null>(null);
  const [tools, setTools] = useState<PermissionView[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    void (async () => {
      try {
        const [s, t] = await Promise.all([api.listSkills(), api.listPermissions()]);
        setAllSkills(s);
        setTools(t);
      } catch (err) {
        setError(describeError(err));
      }
    })();
  }, []);

  const toggle = (list: string[], item: string, on: boolean) =>
    on ? [...list.filter((x) => x !== item), item] : list.filter((x) => x !== item);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveAgent({
        agent_id: agent?.agent_id ?? null,
        revision: agent?.revision ?? 0,
        name,
        description,
        instructions,
        tools_off: toolsOff,
        skills,
        mcp_servers: servers,
        connection_id: connectionId,
        model: connectionId === null ? null : model,
        project_id: projectId,
      });
      onDone(saved);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="stack agent-form"
      aria-label={agent ? `Edit agent ${agent.name}` : "New agent"}
      onSubmit={(e) => {
        e.preventDefault();
        void save();
      }}
    >
      <label>
        Agent name
        <input value={name} maxLength={60} onChange={(e) => setName(e.currentTarget.value)} placeholder="A short name, e.g. Research" />
      </label>
      <label>
        What it is for
        <input
          value={description}
          maxLength={500}
          onChange={(e) => setDescription(e.currentTarget.value)}
          placeholder="The kind of work it does"
        />
      </label>
      <label>
        Agent instructions
        <textarea
          value={instructions}
          rows={5}
          maxLength={8000}
          onChange={(e) => setInstructions(e.currentTarget.value)}
          placeholder="How this agent works: language, tone, what to check first…"
        />
      </label>
      <p className="muted small">
        Added after Nexus's persona, for this agent's tasks only. Instructions shape how it works; they grant no
        permissions.
      </p>
      <div className="form-row">
        <label>
          Model
          <select
            aria-label="Agent model connection"
            value={connectionId ?? ""}
            onChange={(e) => {
              const id = e.currentTarget.value === "" ? null : e.currentTarget.value;
              setConnectionId(id);
              setModel(connections?.find((c) => c.connection_id === id)?.model ?? "");
            }}
          >
            <option value="">The active model</option>
            {(connections ?? []).map((c) => (
              <option key={c.connection_id} value={c.connection_id}>
                {c.display_name}
              </option>
            ))}
          </select>
        </label>
        {connectionId !== null && (
          <label>
            Model name
            <input value={model} maxLength={200} spellCheck={false} onChange={(e) => setModel(e.currentTarget.value)} />
          </label>
        )}
        <label>
          Project
          <select
            aria-label="Agent project"
            value={projectId ?? ""}
            onChange={(e) => setProjectId(e.currentTarget.value === "" ? null : e.currentTarget.value)}
          >
            <option value="">The active project</option>
            {(projects ?? []).map((p) => (
              <option key={p.project_id} value={p.project_id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <fieldset>
        <legend>Skills it may use</legend>
        {allSkills === null ? (
          <p className="muted small">Loading…</p>
        ) : allSkills.length === 0 ? (
          <p className="muted small">No skill imported yet (Tools → Skills).</p>
        ) : (
          allSkills.map((s) => (
            <label key={s.skill_id} className="check">
              <input
                type="checkbox"
                checked={skills.includes(s.name)}
                onChange={(e) => setSkills(toggle(skills, s.name, e.currentTarget.checked))}
              />
              {s.name}
              {!s.enabled && <span className="muted small"> (off: turn it on in Tools → Skills)</span>}
            </label>
          ))
        )}
      </fieldset>
      <fieldset>
        <legend>MCP servers it may use</legend>
        {(mcp ?? []).length === 0 ? (
          <p className="muted small">No MCP server added.</p>
        ) : (
          (mcp ?? []).map((m: McpServerView) => (
            <label key={m.server_id} className="check">
              <input
                type="checkbox"
                checked={servers.includes(m.server_id)}
                onChange={(e) => setServers(toggle(servers, m.server_id, e.currentTarget.checked))}
              />
              {m.name}
            </label>
          ))
        )}
      </fieldset>
      <fieldset>
        <legend>Tools it uses</legend>
        {tools === null ? (
          <p className="muted small">Loading…</p>
        ) : (
          tools.map((t) => (
            <label key={t.tool_id} className="check">
              <input
                type="checkbox"
                checked={t.enabled && !toolsOff.includes(t.tool_id)}
                disabled={!t.enabled}
                onChange={(e) => setToolsOff(toggle(toolsOff, t.tool_id, !e.currentTarget.checked))}
              />
              {t.label}
              {!t.enabled && <span className="muted small"> (off in Permissions)</span>}
            </label>
          ))
        )}
      </fieldset>
      <div className="actions">
        <button type="submit" className="primary" disabled={busy || name.trim() === ""}>
          {agent ? "Save agent" : "Create agent"}
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={() => onDone(null)}>
          Cancel
        </button>
      </div>
      <ErrorLine error={error} />
    </form>
  );
}
