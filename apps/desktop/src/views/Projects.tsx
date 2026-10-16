// Projects: the workspaces tasks and memory belong to. New tasks go to the
// active project; a follow-up stays in its task's project. A project's
// facts are sent only with its own requests.

import { useState } from "react";
import type { ProjectView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago } from "../model/format.ts";
import { useGoTo } from "../shell/nav";
import { useNexus, useNexusActions, useNow } from "../state/store";
import { Empty, ErrorLine, Panel, Unavailable } from "../ui";
import { FolderIcon, PlusIcon } from "../icons";

const MAX_NAME = 100;
const MAX_DESCRIPTION = 1000;

export function ProjectsView() {
  const d = useNexus();
  return (
    <div className="projects">
      <Panel title="Projects" icon={<FolderIcon size={15} />} id="projects">
        {d.projects === null ? (
          <Empty>Loading…</Empty>
        ) : (
          <ul className="project-list" aria-label="Projects">
            {d.projects.map((p) => (
              <li key={p.project_id}>
                <ProjectCard project={p} />
              </li>
            ))}
          </ul>
        )}
      </Panel>
      <div className="projects-side">
        <NewProject />
        <Panel title="What a project holds" icon={<FolderIcon size={15} />} id="project-scope">
          <ul className="bullets">
            <li>Tasks: every new task goes to the active project; a follow-up stays in its task's project.</li>
            <li>Memory: project facts are sent only with that project's requests; personal facts with all of them.</li>
          </ul>
          <Unavailable
            reason="Files, a repository, instructions, agents and model preferences per project are not built yet."
            planned="They come with the coding and document milestones (M3, M5)."
          />
        </Panel>
      </div>
    </div>
  );
}

function ProjectCard({ project }: { project: ProjectView }) {
  const { refresh } = useNexusActions();
  const goTo = useGoTo();
  const now = useNow(60_000);
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(project.name);
  const [description, setDescription] = useState(project.description);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      setEditing(false);
      await refresh(["projects", "conversations"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const nameOk = name.trim() !== "" && name.trim().length <= MAX_NAME;

  return (
    <article className={`project${project.active ? " project-active" : ""}`} aria-label={`Project ${project.name}`}>
      {editing ? (
        <form
          className="stack"
          onSubmit={(e) => {
            e.preventDefault();
            if (nameOk) void run(() => api.saveProject(project.project_id, name.trim(), description.trim()));
          }}
        >
          <label>
            Name
            <input value={name} maxLength={MAX_NAME} onChange={(e) => setName(e.currentTarget.value)} />
          </label>
          <label>
            Description
            <textarea
              value={description}
              rows={2}
              maxLength={MAX_DESCRIPTION}
              onChange={(e) => setDescription(e.currentTarget.value)}
            />
          </label>
          <div className="actions">
            <button type="submit" className="primary" disabled={busy || !nameOk}>
              Save
            </button>
            <button
              type="button"
              className="ghost"
              disabled={busy}
              onClick={() => {
                setEditing(false);
                setName(project.name);
                setDescription(project.description);
              }}
            >
              Cancel
            </button>
          </div>
        </form>
      ) : (
        <>
          <header className="project-head">
            <h3>{project.name}</h3>
            {project.active && <span className="pill pill-completed">Active — new tasks go here</span>}
          </header>
          {project.description && <p className="project-description">{project.description}</p>}
          <p className="project-meta">
            <button type="button" className="link" onClick={() => goTo({ section: "tasks", projectId: project.project_id })}>
              {project.conversations} {project.conversations === 1 ? "task" : "tasks"}
            </button>
            {" · "}
            <button type="button" className="link" onClick={() => goTo({ section: "memory", projectId: project.project_id })}>
              {project.memory_items} project {project.memory_items === 1 ? "fact" : "facts"}
            </button>
            {" · created "}
            {ago(project.created_at_ms, now)}
          </p>
          <div className="actions">
            {!project.active && (
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => void run(() => api.setActiveProject(project.project_id))}
              >
                Make active
              </button>
            )}
            <button type="button" className="ghost" disabled={busy} onClick={() => setEditing(true)}>
              Edit
            </button>
          </div>
        </>
      )}
      <ErrorLine error={error} />
    </article>
  );
}

function NewProject() {
  const { refresh } = useNexusActions();
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [activate, setActivate] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ok = name.trim() !== "" && name.trim().length <= MAX_NAME;

  async function create() {
    if (!ok || busy) return;
    setBusy(true);
    setError(null);
    try {
      const created = await api.saveProject(null, name.trim(), description.trim());
      if (activate) await api.setActiveProject(created.project_id);
      setName("");
      setDescription("");
      await refresh(["projects"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="New project" icon={<PlusIcon size={15} />} id="new-project">
      <form
        className="stack"
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <label>
          Name
          <input value={name} maxLength={MAX_NAME} placeholder="e.g. Thesis" onChange={(e) => setName(e.currentTarget.value)} />
        </label>
        <label>
          Description
          <textarea
            value={description}
            rows={2}
            maxLength={MAX_DESCRIPTION}
            placeholder="What this project is about (optional)"
            onChange={(e) => setDescription(e.currentTarget.value)}
          />
        </label>
        <label className="check">
          <input type="checkbox" checked={activate} onChange={(e) => setActivate(e.currentTarget.checked)} />
          Make it the active project
        </label>
        <div className="actions">
          <button type="submit" className="primary" disabled={!ok || busy}>
            {busy ? "Creating…" : "Create project"}
          </button>
        </div>
        <ErrorLine error={error} />
      </form>
    </Panel>
  );
}
