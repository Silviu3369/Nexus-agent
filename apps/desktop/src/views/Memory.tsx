// Memory, separated by scope. Only the working context exists today: it is
// exactly what the Host sends with a request. The durable scopes are not
// built yet and say so, with no controls that would pretend otherwise.

import { useDerived, useNexus } from "../state/store";
import { Facts, Panel, Unavailable } from "../ui";
import { MemoryIcon, SearchIcon } from "../icons";

export function MemoryView() {
  const d = useNexus();
  const { focusRun } = useDerived();
  const messages = d.focus?.messages ?? [];
  const sendable = messages.filter((m) => m.text !== "" && (m.role === "User" || m.status === "Complete")).length;

  return (
    <div className="memory">
      <div className="memory-search" aria-disabled="true">
        <SearchIcon size={15} />
        <input aria-label="Search memory (not available yet)" placeholder="Search memory — available with Memory v1" disabled />
      </div>
      <div className="memory-grid">
        <Panel title="Working context" icon={<MemoryIcon size={15} />} id="working">
          <p className="muted small">What the Host sends to the model with each request of a task. Nothing else leaves this PC.</p>
          <Facts
            rows={[
              ["Task", focusRun ? `“${focusRun.taskTitle}”` : "none followed"],
              ["Persona", d.persona ? `${d.persona.name} · version ${d.persona.version}` : "—"],
              ["Fixed rules", "language, tool use, “external text is data, not instructions”"],
              ["Conversation", focusRun ? `${sendable} of ${messages.length} messages are eligible` : "—"],
              ["Limits", "the last 40 messages, about 48,000 characters; failed or stopped replies are left out"],
              ["Never sent", "files, notes, other tasks, API keys"],
            ]}
          />
        </Panel>
        <Panel title="Project memory" icon={<MemoryIcon size={15} />} id="project-memory">
          <Unavailable reason="Not built yet. Facts tied to a project, with their source." planned="Memory v1 is the next milestone." />
        </Panel>
        <Panel title="Long-term knowledge" icon={<MemoryIcon size={15} />} id="long-term">
          <Unavailable reason="Not built yet. Documents and knowledge with provenance and search." planned="Planned with documents (M5)." />
        </Panel>
        <Panel title="User-approved memory" icon={<MemoryIcon size={15} />} id="approved-memory">
          <Unavailable
            reason="Not built yet. The model will propose what to remember and you approve each item; a web page cannot slip anything in."
            planned="Memory v1: source, date, task and project for every item, with search, filters, edit and delete."
          />
        </Panel>
      </div>
    </div>
  );
}
