import { useDerived, useNexus } from "../state/store";
import { Facts, Panel, StatusPill, Unavailable } from "../ui";
import { FolderIcon } from "../icons";

/** One workspace holds everything today; separate projects are planned. */
export function ProjectsView() {
  const d = useNexus();
  const { runningStep, pending } = useDerived();
  const notes = (d.runs ?? []).filter((r) => r.status === "Succeeded").length;
  return (
    <div className="single">
      <Panel title="Default workspace" icon={<FolderIcon size={15} />} id="workspace">
        <p className="muted small">All tasks and notes currently live in a single workspace inside your Nexus profile.</p>
        <Facts
          rows={[
            [
              "Activity",
              runningStep ? (
                <StatusPill status="running" label="Working" />
              ) : pending.length > 0 ? (
                <StatusPill status="approval_required" label="Waiting for you" />
              ) : (
                <span className="muted">idle</span>
              ),
            ],
            ["Tasks", String(d.conversations?.length ?? "—")],
            ["Notes written", String(notes)],
            ["Note runs", String(d.runs?.length ?? "—")],
            ["Model preference", "the active model (Models)"],
          ]}
        />
      </Panel>
      <Panel title="Projects" icon={<FolderIcon size={15} />} id="projects">
        <Unavailable
          reason="Separate projects are not built yet."
          planned="Each project will have its own files, repository, instructions, memory, agents, tasks, configuration and model preferences, with its health and current activity here."
        />
      </Panel>
    </div>
  );
}
