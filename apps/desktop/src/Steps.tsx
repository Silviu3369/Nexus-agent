import type { AgentStepView } from "@nexus/contracts";
import { AlertIcon, CheckCircleIcon, GlobeIcon, NotesIcon, SparkleIcon } from "./icons";

function seconds(ms: number | null): string | null {
  if (ms === null) return null;
  return ms < 10_000 ? `${(ms / 1000).toFixed(1)} s` : `${Math.round(ms / 1000)} s`;
}

function host(url: string): string {
  try {
    return new URL(url).host || url;
  } catch {
    return url;
  }
}

/** One line saying what the step is, and one saying how it went. Nothing
 *  here comes from page content — only the step's own metadata. */
export function describeStep(step: AgentStepView): { title: string; detail: string | null } {
  const d = step.detail;
  const running = step.status === "Running";
  switch (d.kind) {
    case "ModelCall": {
      const title = running ? `Thinking — ${d.model}` : `Model ${d.model}`;
      if (d.error) return { title, detail: d.error };
      const total = seconds(d.duration_ms);
      const first = seconds(d.first_byte_ms);
      if (!total) return { title, detail: null };
      return { title, detail: first ? `${total} (first response after ${first})` : total };
    }
    case "Fetch": {
      const title = `${running ? "Reading" : "Read"} ${host(d.url)}`;
      if (d.error) return { title, detail: d.error };
      if (d.http_status === null) return { title, detail: null };
      const kb = d.text_bytes === null ? null : `${(d.text_bytes / 1024).toFixed(1)} KB of text`;
      const parts = [`HTTP ${d.http_status}`, kb, d.truncated ? "truncated" : null].filter(Boolean);
      return { title, detail: parts.join(" · ") };
    }
    case "NoteProposed": {
      const title = `Proposed note "${d.title}"`;
      if (d.error) return { title, detail: d.error };
      return { title, detail: running ? null : "waiting for your approval" };
    }
  }
}

function KindIcon({ step }: { step: AgentStepView }) {
  switch (step.detail.kind) {
    case "ModelCall":
      return <SparkleIcon size={14} />;
    case "Fetch":
      return <GlobeIcon size={14} />;
    case "NoteProposed":
      return <NotesIcon size={14} />;
  }
}

const STATUS_LABEL = { Running: "in progress", Done: "done", Failed: "failed" } as const;

function StatusMark({ step }: { step: AgentStepView }) {
  if (step.status === "Running") return <span className="pulse-dot" />;
  if (step.status === "Failed") return <AlertIcon size={14} />;
  return <CheckCircleIcon size={14} />;
}

export function StepItem({
  step,
  onOpen,
}: {
  step: AgentStepView;
  /** When set, the step shows its conversation and opens it. */
  onOpen?: () => void;
}) {
  const { title, detail } = describeStep(step);
  const body = (
    <>
      <span className="step-kind">
        <KindIcon step={step} />
      </span>
      <span className="step-text">
        <span className="step-title" title={step.detail.kind === "Fetch" ? step.detail.url : undefined}>
          {title}
        </span>
        {detail && <span className="step-detail">{detail}</span>}
        {onOpen && <span className="step-conversation">{step.conversation_title}</span>}
      </span>
      <span className="step-status" role="img" aria-label={STATUS_LABEL[step.status]}>
        <StatusMark step={step} />
      </span>
    </>
  );
  return (
    <li className={`step step-${step.status.toLowerCase()}`}>
      {onOpen ? (
        <button type="button" className="step-row" onClick={onOpen}>
          {body}
        </button>
      ) : (
        <div className="step-row">{body}</div>
      )}
    </li>
  );
}

export function StepList({ steps }: { steps: AgentStepView[] }) {
  if (steps.length === 0) return null;
  return (
    <ol className="steps" aria-label="What the assistant did">
      {steps.map((s) => (
        <StepItem key={s.step_id} step={s} />
      ))}
    </ol>
  );
}
