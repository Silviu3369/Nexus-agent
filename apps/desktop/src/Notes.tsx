import { useCallback, useEffect, useRef, useState } from "react";
import type { RunId, RunStatus, RunView } from "@nexus/contracts";
import { api, describeError } from "./api";

const POLL_MS = 2000;

const STATUS_LABEL: Record<RunStatus, string> = {
  Queued: "În coadă",
  Running: "Se pregătește",
  AwaitingApproval: "Așteaptă aprobarea ta",
  Verifying: "Se verifică",
  CancellationRequested: "Se anulează",
  NeedsReconciliation: "Se reconciliază",
  NeedsReview: "Necesită verificarea ta",
  Succeeded: "Creată",
  Failed: "Eșuată",
  Cancelled: "Anulată",
  Denied: "Refuzată",
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
  Denied: "Ai refuzat crearea notei.",
  ApprovalExpired: "Aprobarea a expirat înainte de o decizie.",
  ApprovalInvalidated: "Aprobarea nu mai corespundea acțiunii.",
  Cancelled: "Anulată.",
  ResourceChanged: "Destinația nu mai era neatinsă, așa că nota nu a fost scrisă.",
  EffectFailed: "Scrierea nu a reușit; nu s-a creat niciun fișier.",
  OutcomeUnknown: "Rezultat nesigur: există un fișier care nu poate fi dovedit ca fiind al acestei note.",
  InvalidRequest: "Cererea nu era validă.",
};

const CANCELLABLE: RunStatus[] = ["Queued", "Running", "AwaitingApproval"];

function time(ms: number): string {
  return new Date(ms).toLocaleTimeString("ro-RO");
}

export function StatusBadge({ status }: { status: RunStatus }) {
  return <span className={`badge badge-${statusTone(status)}`}>{STATUS_LABEL[status]}</span>;
}

export function NotesPanel() {
  const [runs, setRuns] = useState<RunView[]>([]);
  const [selected, setSelected] = useState<RunId | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const snapshot = await api.listRuns();
      setRuns(snapshot.runs);
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
        <h2 id="activity-heading">Activitate</h2>
        {listError && (
          <p className="error" role="alert">
            Lista nu a putut fi încărcată: {listError}
          </p>
        )}
        {runs.length === 0 && !listError ? (
          <p className="muted">Nicio notă încă.</p>
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
      <h2 id="new-note-heading">Notă nouă</h2>
      <form className="note-form" onSubmit={submit}>
        <label>
          Titlu
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
          Conținut
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
            {busy ? "Se trimite…" : "Pregătește nota"}
          </button>
        </div>
        <p className="muted">Nimic nu se scrie pe disc până nu aprobi conținutul exact.</p>
        {error && (
          <p className="error" role="alert">
            {error}
          </p>
        )}
      </form>
    </section>
  );
}

function RunDetail({ summary, onChanged }: { summary: RunView; onChanged: () => void }) {
  const [detail, setDetail] = useState<RunView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [content, setContent] = useState<string | null>(null);

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
  const pending = view.pending_approval;
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
    <section className="card" aria-labelledby="detail-heading">
      <h2 id="detail-heading">{view.title}</h2>
      <div role="status" aria-live="polite">
        <StatusBadge status={view.status} />
      </div>

      {view.failure && (
        <p className={view.status === "Cancelled" ? "muted" : "error"}>
          {FAILURE_TEXT[view.failure.code] ?? view.failure.code} ({view.failure.message})
        </p>
      )}

      {pending && (
        <div className="preview">
          <h3>Ce se va scrie</h3>
          {preview ? (
            <>
              <dl className="facts">
                <dt>Destinație</dt>
                <dd className="path">{preview.destination}</dd>
                <dt>Mărime</dt>
                <dd>{preview.size_bytes} octeți</dd>
                <dt>SHA-256</dt>
                <dd className="hash">{preview.sha256}</dd>
                <dt>Expiră</dt>
                <dd>{time(pending.expires_at_ms)}</dd>
              </dl>
              <pre className="content" aria-label="Conținutul exact al notei">
                {preview.content}
              </pre>
              <div className="actions">
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => act(() => api.decide(pending.approval_id, pending.manifest_sha256, true))}
                >
                  Aprobă și creează
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={busy}
                  onClick={() => act(() => api.decide(pending.approval_id, pending.manifest_sha256, false))}
                >
                  Refuză
                </button>
              </div>
            </>
          ) : (
            <p className="muted">Se încarcă previzualizarea exactă…</p>
          )}
        </div>
      )}

      {view.status === "Succeeded" && view.artifact && (
        <div className="result">
          <dl className="facts">
            <dt>Fișier</dt>
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
              Arată conținutul
            </button>
            <button type="button" className="secondary" disabled={busy} onClick={() => act(() => api.revealArtifact(view.run_id))}>
              Deschide locația
            </button>
          </div>
          {content !== null && (
            <pre className="content" aria-label="Conținutul verificat al notei">
              {content}
            </pre>
          )}
        </div>
      )}

      {CANCELLABLE.includes(view.status) && (
        <div className="actions">
          <button type="button" className="secondary" disabled={busy} onClick={() => act(() => api.cancelRun(view.run_id))}>
            Anulează
          </button>
        </div>
      )}

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
