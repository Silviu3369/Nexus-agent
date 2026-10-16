import { Panel, Unavailable } from "../ui";
import { AutomationIcon } from "../icons";

/** The scheduler does not exist yet; this says so and what it will show. */
export function AutomationsView() {
  return (
    <div className="single">
      <Panel title="Scheduler" icon={<AutomationIcon size={15} />} id="scheduler">
        <Unavailable
          title="No automations can run in this build."
          reason="Nexus has no scheduler yet, so nothing starts on a timer or a condition. Every task starts from you."
          planned="Planned with M4: scheduled and condition-based tasks with their trigger, instructions, agent, model, tools and permissions, the next and last execution, and the full history — each one under the same approval rules."
        />
      </Panel>
    </div>
  );
}
