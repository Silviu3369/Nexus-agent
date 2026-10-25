import { actorsLine, effectText } from "./model/execution.ts";
import { weekdayClock } from "./model/format.ts";
import { useCallback, useEffect, useId, useRef, useState } from "react";
import type { RunId, RunStatus, RunView } from "@nexus/contracts";
import { api, describeError } from "./api";
import { CheckCircleIcon, NotesIcon } from "./icons";
import { useDerived } from "./state/store";

const POLL_MS = 2000;

const STATUS_LABEL: Record<RunStatus, string> = {
  Queued: "Queued",
  Running: "Preparing",
  AwaitingApproval: "Awaiting your approval",
  Verifying: "Verifying",
  CancellationRequested: "Cancelling",
  NeedsReconciliation: "Reconciling",
  NeedsReview: "Needs your review",
  Succeeded: "Created",
  Failed: "Failed",
  Cancelled: "Cancelled",
  Denied: "Denied",
};

function statusTone(status: RunStatus): "good" | "bad" | "neutral" | "attention" {
  switch (status) {
    case "Succeeded":
      return "good";
    case "Failed":
    case "Denied":
    case "NeedsReview":
      return "bad";
    case "AwaitingApproval":
      return "attention";
    default:
      return "neutral";
  }
}

const FAILURE_TEXT: Record<string, string> = {
  Denied: "You denied creating the note.",
  ApprovalExpired: "The approval expired before a decision.",
  ApprovalInvalidated: "The approval no longer matched the action.",
  Cancelled: "Cancelled.",
  ResourceChanged: "The destination was no longer untouched, so the note was not written.",
  EffectFailed: "The write failed; no file was created.",
  OutcomeUnknown: "Uncertain outcome: there is a file that cannot be proven to belong to this note.",
  InvalidRequest: "The request was not valid.",
};

const CANCELLABLE: RunStatus[] = ["Queued", "Running", "AwaitingApproval"];

export function time(ms: number): string {
  return new Date(ms).toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit" });
}

/** How a call to an MCP tool ended, where it differs from a note. */
const CALL_STATUS_LABEL: Partial<Record<RunStatus, string>> = {
  Running: "Calling",
  Succeeded: "Called",
};

const CALL_FAILURE_TEXT: Record<string, string> = {
  Denied: "You rejected the call; nothing was sent.",
  EffectFailed: "The call failed; the server did nothing.",
  ToolReportedError: "The tool reported an error; check on the server whether anything changed.",
  OutcomeUnknown: "Uncertain: the call may have reached the server. Check there; Nexus does not send it again.",
  ApprovalInvalidated: "The server or the tool changed after you approved, so the call was not sent.",
};

export function StatusBadge({ status, call = false }: { status: RunStatus; call?: boolean }) {
  const label = (call ? CALL_STATUS_LABEL[status] : undefined) ?? STATUS_LABEL[status];
  return <span className={`badge badge-${statusTone(status)}`}>{label}</span>;
}

export function NotesPanel({ initialRunId }: { initialRunId?: RunId | null }) {
  const [runs, setRuns] = useState<RunView[]>([]);
  const [selected, setSelected] = useState<RunId | null>(initialRunId ?? null);
  const [listError, setListError] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const snapshot = await api.listRuns();
      // Calls to MCP tools are runs too; this panel lists notes.
      setRuns(snapshot.runs.filter((r) => r.kind === "Note"));
      setListError(null);
    } catch (err) {
      setListError(describeError(err));
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  const selectedSummary = runs.find((r) => r.run_id === selected) ?? null;

  return (
    <>
      <NewNoteForm
        onSubmitted={(runId) => {
          setSelected(runId);
          void refresh();
        }}
      />

      <section className="card" aria-labelledby="activity-heading">
        <h2 id="activity-heading">
          <NotesIcon /> Notes
        </h2>
        {listError && (
          <p className="error" role="alert">
            The list could not be loaded: {listError}
          </p>
        )}
        {runs.length === 0 && !listError ? (
          <p className="muted">No notes yet.</p>
        ) : (
          <ul className="runs">
            {runs.map((run) => (
              <li key={run.run_id}>
                <button
                  type="button"
                  className={`run-row${run.run_id === selected ? " selected" : ""}`}
                  aria-current={run.run_id === selected ? "true" : undefined}
                  onClick={() => setSelected(run.run_id)}
                >
                  <span className="run-title">{run.title}</span>
                  <StatusBadge status={run.status} />
                  <span className="muted">{time(run.updated_at_ms)}</span>
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>

      {selectedSummary && (
        <RunDetail key={selectedSummary.run_id} summary={selectedSummary} onChanged={() => void refresh()} />
      )}
    </>
  );
}

function NewNoteForm({ onSubmitted }: { onSubmitted: (runId: RunId) => void }) {
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One key per intent: a retry after a lost answer reuses it, so the host
  // returns the same run instead of creating a second one (RUN-03).
  const intentKey = useRef<string>(crypto.randomUUID());

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const result = await api.submitNote(intentKey.current, title, body);
      intentKey.current = crypto.randomUUID();
      setTitle("");
      setBody("");
      onSubmitted(result.run_id);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby="new-note-heading">
      <h2 id="new-note-heading">New note</h2>
      <form className="note-form" onSubmit={submit}>
        <label>
          Title
          <input
            value={title}
            onChange={(e) => {
              setTitle(e.currentTarget.value);
              intentKey.current = crypto.randomUUID();
            }}
            maxLength={200}
            required
          />
        </label>
        <label>
          Content
          <textarea
            value={body}
            onChange={(e) => {
              setBody(e.currentTarget.value);
              intentKey.current = crypto.randomUUID();
            }}
            rows={5}
          />
        </label>
        <div className="actions">
          <button type="submit" disabled={busy || title.trim() === ""}>
            {busy ? "Sending…" : "Prepare note"}
          </button>
        </div>
        <p className="muted">Nothing is written to disk until you approve the exact content.</p>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </form>
    </section>
  );
}

export function RunDetail({ summary, onChanged }: { summary: RunView; onChanged: () => void }) {
  const [detail, setDetail] = useState<RunView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [confirmUndo, setConfirmUndo] = useState(false);
  const { agentName } = useDerived();
  // Several details can be on screen (chat proposals), so ids are unique.
  const headingId = useId();

  // Refetch whenever the run changes, so the preview always comes from the
  // host's current manifest.
  useEffect(() => {
    let alive = true;
    api
      .getRun(summary.run_id)
      .then((view) => alive && setDetail(view))
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [summary.run_id, summary.revision]);

  const view = detail ?? summary;
  const call = view.kind === "McpCall" ? view.call : null;
  const pending = call ? null : view.pending_approval;
  const preview = pending?.preview ?? null;

  async function act(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      onChanged();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby={headingId}>
      <h2 id={headingId}>{view.title}</h2>
      <div role="status" aria-live="polite">
        <StatusBadge status={view.status} call={call !== null} />
      </div>
      {effectText(view.effect_class, view.evidence) && (
        <p className="muted effect-line">{effectText(view.effect_class, view.evidence)}</p>
      )}
      {actorsLine(view, agentName) && <p className="muted actors-line">{actorsLine(view, agentName)}</p>}

      {view.failure && (
        <p className={view.status === "Cancelled" ? "muted" : "error"}>
          {(call ? CALL_FAILURE_TEXT[view.failure.code] : undefined) ?? FAILURE_TEXT[view.failure.code] ?? view.failure.code} (
          {view.failure.message})
        </p>
      )}

      {call && (
        <dl className="facts">
          <dt>Server</dt>
          <dd className="path">{call.url}</dd>
          <dt>Tool</dt>
          <dd className="mono">{call.tool}</dd>
          <dt>Arguments</dt>
          <dd>
            <pre className="content" aria-label="Arguments of the call">
              {call.arguments_json}
            </pre>
          </dd>
          {call.result_chars !== null && (
            <>
              <dt>Answer</dt>
              <dd>
                {call.result_chars.toLocaleString("en-US")} characters{call.truncated ? " (shortened for the model)" : ""}
                {call.duration_ms !== null ? ` · ${call.duration_ms} ms` : ""} · given to the model, not kept
              </dd>
            </>
          )}
        </dl>
      )}

      {pending && (
        <div className="preview">
          <h3>What will be written</h3>
          {preview ? (
            <>
              <dl className="facts">
                <dt>Destination</dt>
                <dd className="path">{preview.destination}</dd>
                <dt>Size</dt>
                <dd>{preview.size_bytes} bytes</dd>
                <dt>SHA-256</dt>
                <dd className="hash">{preview.sha256}</dd>
                <dt>Waits until</dt>
                <dd>{weekdayClock(pending.expires_at_ms)}</dd>
              </dl>
              <pre className="content" aria-label="Exact content of the note">
                {preview.content}
              </pre>
              <div className="actions">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => act(() => api.decide(pending.approval_id, pending.manifest_sha256, true))}
                >
                  Approve and create
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={busy}
                  onClick={() => act(() => api.decide(pending.approval_id, pending.manifest_sha256, false))}
                >
                  Deny
                </button>
              </div>
            </>
          ) : (
            <p className="muted">Loading the exact preview…</p>
          )}
        </div>
      )}

      {view.status === "Succeeded" && view.artifact && (
        <div className="result">
          <dl className="facts">
            <dt>File</dt>
            <dd className="path">{view.artifact.path}</dd>
            <dt>SHA-256</dt>
            <dd className="hash">{view.artifact.sha256}</dd>
          </dl>
          <div className="actions">
            <button
              type="button"
              disabled={busy}
              onClick={() =>
                act(async () => setContent(await api.artifactContent(view.artifact!.artifact_id)))
              }
            >
              Show content
            </button>
            <button type="button" className="secondary" disabled={busy} onClick={() => act(() => api.revealArtifact(view.run_id))}>
              Open location
            </button>
          </div>
          {content !== null && (
            <pre className="content" aria-label="Verified content of the note">
              {content}
            </pre>
          )}
        </div>
      )}

      {view.undone_at_ms !== null && (
        <p className="muted" role="status">{`Undone at ${time(view.undone_at_ms)}: the file Nexus created was deleted.`}</p>
      )}
      {view.can_undo &&
        (confirmUndo ? (
          <div className="confirm-bar" role="alert">
            <p>{`Delete ${view.title}? Nexus deletes it only if it is unchanged since it was written.`}</p>
            <div className="actions">
              <button
                type="button"
                className="danger"
                disabled={busy}
                onClick={() =>
                  act(async () => {
                    await api.undoEffect(view.run_id);
                    setConfirmUndo(false);
                  })
                }
              >
                Delete the file
              </button>
              <button type="button" className="ghost" disabled={busy} onClick={() => setConfirmUndo(false)}>
                Keep
              </button>
            </div>
          </div>
        ) : (
          <div className="actions">
            <button type="button" className="secondary" disabled={busy} onClick={() => setConfirmUndo(true)}>
              Undo
            </button>
          </div>
        ))}

      {CANCELLABLE.includes(view.status) && (
        <div className="actions">
          <button type="button" className="secondary" disabled={busy} onClick={() => act(() => api.cancelRun(view.run_id))}>
            Cancel
          </button>
        </div>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {view.status === "Succeeded" && !view.failure && (
        <p className="muted result-footer">
          <CheckCircleIcon size={14} />{" "}
          {call ? "Made once, after your approval." : "Verified against its SHA-256 hash."}
        </p>
      )}
    </section>
  );
}
