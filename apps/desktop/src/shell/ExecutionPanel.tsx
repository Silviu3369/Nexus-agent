// The persistent right panel: the approvals waiting for you, then the run
// Nexus is on (or last finished), step by step, with what comes next.

import { useRef } from "react";
import type { KeyboardEvent, PointerEvent } from "react";
import { ApprovalCard } from "../components/ApprovalCard";
import { Timeline } from "../components/Timeline";
import { nextLine } from "../model/execution.ts";
import { clock, duration } from "../model/format.ts";
import { useDerived, useNow } from "../state/store";
import { replyFailure } from "../text";
import { Empty, StatusPill } from "../ui";
import { LockIcon, PanelIcon } from "../icons";
import { useGoTo } from "./nav";

const MIN_W = 280;
const MAX_W = 560;

export function ExecutionPanel({
  collapsed,
  onToggle,
  width,
  onResize,
}: {
  collapsed: boolean;
  onToggle: () => void;
  width: number;
  onResize: (w: number) => void;
}) {
  const { focusRun, pending, agentName } = useDerived();
  const working = focusRun?.status === "running";
  const now = useNow(working ? 250 : 1000);
  const goTo = useGoTo();
  const drag = useRef<{ x: number; w: number } | null>(null);

  if (collapsed) {
    return (
      <aside className="xpanel xpanel-collapsed" aria-label="Live execution">
        <button type="button" className="xpanel-open" onClick={onToggle} aria-label="Expand live execution">
          <PanelIcon size={16} />
          {pending.length > 0 && (
            <span className="rail-badge attention" aria-label={`${pending.length} waiting for approval`}>
              {pending.length}
            </span>
          )}
          {working && <span className="pulse-dot" aria-label="working" />}
          <span className="xpanel-vertical">Live execution</span>
        </button>
      </aside>
    );
  }

  const clamp = (w: number) => Math.max(MIN_W, Math.min(MAX_W, Math.round(w)));
  const onPointerDown = (e: PointerEvent<HTMLDivElement>) => {
    drag.current = { x: e.clientX, w: width };
    e.currentTarget.setPointerCapture(e.pointerId);
  };
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!drag.current) return;
    onResize(clamp(drag.current.w + (drag.current.x - e.clientX)));
  };
  const onPointerUp = () => {
    drag.current = null;
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowLeft") onResize(clamp(width + 20));
    if (e.key === "ArrowRight") onResize(clamp(width - 20));
  };

  const elapsed = focusRun ? (focusRun.finishedAt ?? now) - focusRun.startedAt : null;

  return (
    <aside className="xpanel" aria-label="Live execution">
      <div
        className="xpanel-resize"
        role="separator"
        aria-orientation="vertical"
        aria-label="Resize live execution"
        aria-valuenow={width}
        aria-valuemin={MIN_W}
        aria-valuemax={MAX_W}
        tabIndex={0}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onKeyDown={onKey}
      />
      <header className="xpanel-head">
        <h2>Live execution</h2>
        <button type="button" className="icon-button" onClick={onToggle} aria-label="Collapse live execution">
          <PanelIcon size={16} />
        </button>
      </header>
      <div className="xpanel-body">
        {pending.length > 0 && (
          <section className="xpanel-approvals" aria-label="Waiting for your approval">
            {pending.map((run) => (
              <ApprovalCard key={run.run_id} run={run} agentName={agentName} compact />
            ))}
          </section>
        )}

        {focusRun ? (
          <section className="xtask" aria-label="Current task">
            <p className="eyebrow">Task</p>
            <h3 className="xtask-title">{focusRun.taskTitle}</h3>
            {focusRun.request && <p className="xtask-request clamp-3">{focusRun.request}</p>}
            <div className="xtask-meta">
              <StatusPill status={focusRun.status} />
              <span>{agentName}</span>
              {focusRun.model && <span className="mono">{focusRun.model}</span>}
              <span>{clock(focusRun.startedAt)}</span>
              {elapsed !== null && <span>{duration(elapsed)}</span>}
            </div>

            <p className="eyebrow">Steps</p>
            {focusRun.items.length > 0 ? (
              <Timeline items={focusRun.items} now={now} label="Steps of the current run" />
            ) : (
              <Empty>{focusRun.status === "running" ? "Starting…" : "No steps were recorded for this run."}</Empty>
            )}

            <div className="xnext">
              <p className="eyebrow">Next</p>
              <p>
                {focusRun.items.some((i) => i.status === "approval_required") && <LockIcon size={13} />} {nextLine(focusRun)}
              </p>
            </div>

            {focusRun.failure && (
              <p className={focusRun.status === "cancelled" ? "muted" : "error"}>{replyFailure(focusRun.failure)}</p>
            )}
            {focusRun.text && (
              <div className="xoutput">
                <p className="eyebrow">Output</p>
                <p className="clamp-6">{focusRun.text}</p>
              </div>
            )}
            <div className="xactions">
              <button type="button" className="ghost" onClick={() => goTo({ section: "tasks", taskId: focusRun.taskId })}>
                Open task
              </button>
            </div>
          </section>
        ) : (
          <Empty>Nexus is idle. Give it a task below; each step appears here as it happens.</Empty>
        )}
      </div>
    </aside>
  );
}
