// The approval card. It is deliberately not a notification: it states the
// exact action, who asked and why, what it touches, and asks for a decision
// that the Host binds to the manifest hash shown here.

import { useEffect, useMemo, useState } from "react";
import type { RunView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { confirmedInWindows } from "../model/execution.ts";
import { kib, timeLeft } from "../model/format.ts";
import { useNexus, useNexusActions, useNow } from "../state/store";
import { ErrorLine } from "../ui";
import { CheckIcon, EyeIcon, LockIcon, XIcon } from "../icons";

function requestBefore(messages: { message_id: string; role: string; text: string }[], reply: string): string | null {
  const at = messages.findIndex((m) => m.message_id === reply);
  for (let i = at - 1; i >= 0; i--) if (messages[i].role === "User") return messages[i].text;
  return null;
}

export function ApprovalCard({ run, agentName, compact = false }: { run: RunView; agentName: string; compact?: boolean }) {
  const { focus } = useNexus();
  const { refresh } = useNexusActions();
  const [detail, setDetail] = useState<RunView | null>(null);
  const [inspect, setInspect] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const now = useNow(1000);

  // The preview always comes from the Host's current manifest.
  useEffect(() => {
    let alive = true;
    api
      .getRun(run.run_id)
      .then((v) => alive && setDetail(v))
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [run.run_id, run.revision]);

  // Why: the request the agent was working on when it proposed this.
  const origin = run.origin;
  const originMessage = origin?.message_id ?? null;
  const originTask = origin?.conversation_id ?? null;
  const focusMatch = originTask !== null && focus?.conversation_id === originTask ? focus : null;
  const fromFocus = useMemo(
    () => (focusMatch && originMessage ? requestBefore(focusMatch.messages, originMessage) : null),
    [focusMatch, originMessage],
  );
  const [fetched, setFetched] = useState<string | null>(null);
  const needFetch = originTask !== null && focusMatch === null;
  useEffect(() => {
    if (!needFetch || !originTask || !originMessage) return;
    let alive = true;
    api
      .getConversation(originTask)
      .then((c) => alive && setFetched(requestBefore(c.messages, originMessage)))
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, [needFetch, originTask, originMessage]);
  const request = fromFocus ?? fetched;

  const pending = detail?.pending_approval ?? null;
  const preview = pending?.preview ?? null;
  // A call to an MCP tool that can change things, shown as the Host will make it.
  const call = run.kind === "McpCall" ? (detail?.call ?? run.call) : null;
  // A new file in a folder you shared: shown like a note, with its folder.
  const isDocument = run.kind === "Document";
  const callPreview = pending?.call ?? null;
  const ready = call ? callPreview !== null : preview !== null;
  const left = (pending?.expires_at_ms ?? run.pending_approval?.expires_at_ms ?? now) - now;

  async function decide(approve: boolean) {
    if (!pending) return;
    setBusy(true);
    setError(null);
    try {
      if (approve) await api.approve(run, pending.approval_id, pending.manifest_sha256);
      else await api.decide(pending.approval_id, pending.manifest_sha256, false);
      await refresh(["runs", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <article className={`approval${compact ? " approval-compact" : ""}`} aria-label={`Approval required: ${run.title}`}>
      <header className="approval-head">
        <span className="approval-flag">
          <LockIcon size={14} /> Approval required
        </span>
        <span className={`approval-timer${left < 60_000 ? " soon" : ""}`} title="Time left to decide; the request waits for you up to 24 hours">
          {timeLeft(left)}
        </span>
      </header>
      <dl className="kv approval-facts">
        <div className="kv-row">
          <dt>Action</dt>
          <dd>{call ? `Call ${call.tool} on ${call.server}` : isDocument ? `Create file ${run.title}` : `Create note “${run.title}”`}</dd>
        </div>
        <div className="kv-row">
          <dt>Requested by</dt>
          <dd>{origin ? `${agentName} · task “${origin.conversation_title}”` : "You · Tools → Notes"}</dd>
        </div>
        <div className="kv-row">
          <dt>Reason</dt>
          <dd className="clamp-2">
            {origin
              ? request
                ? `Proposed while answering: “${request}”`
                : "Proposed by the agent while answering this task"
              : isDocument
                ? "You asked for this file"
                : "You asked for this note"}
          </dd>
        </div>
        <div className="kv-row">
          <dt>Affected</dt>
          <dd>
            {call
              ? callPreview
                ? `Whatever this tool does on ${call.server}${callPreview.destructive_hint ? " · the server marks it destructive" : ""} · sent once, only if you approve`
                : "Loading the exact call…"
              : preview
                ? `1 new file · ${kib(preview.size_bytes)} · nothing overwritten`
                : "Loading the exact preview…"}
          </dd>
        </div>
        {!compact && (call ? (
          <div className="kv-row">
            <dt>Server</dt>
            <dd className="mono path">{call.url}</dd>
          </div>
        ) : (
          preview && (
            <div className="kv-row">
              <dt>Destination</dt>
              <dd className="mono path">{preview.destination}</dd>
            </div>
          )
        ))}
      </dl>
      {inspect && call && callPreview && (
        <div className="approval-inspect">
          <p className="muted">
            The server says: <span className="approval-claim">{callPreview.description || "no description"}</span>
          </p>
          {compact && <p className="mono path">{call.url}</p>}
          <pre className="content" aria-label="Exact arguments of the call">
            {call.arguments_json}
          </pre>
          <p className="mono hash">Tool definition SHA-256 {callPreview.definition_sha256}</p>
        </div>
      )}
      {inspect && preview && (
        <div className="approval-inspect">
          <p className="mono hash">SHA-256 {preview.sha256}</p>
          {compact && <p className="mono path">{preview.destination}</p>}
          <pre className="content" aria-label={isDocument ? "Exact content of the file" : "Exact content of the note"}>
            {preview.content}
          </pre>
        </div>
      )}
      <div className="approval-actions">
        <button type="button" className="ghost" aria-expanded={inspect} disabled={!ready} onClick={() => setInspect(!inspect)}>
          <EyeIcon size={14} /> {inspect ? "Hide" : "Inspect"}
        </button>
        <span className="spacer" />
        <button type="button" className="secondary" disabled={busy || !pending} onClick={() => decide(false)}>
          <XIcon size={14} /> Reject
        </button>
        <button type="button" className="approve" disabled={busy || !ready || left <= 0} onClick={() => decide(true)}>
          <CheckIcon size={14} /> Approve
        </button>
      </div>
      {confirmedInWindows(run.effect_class) && (
        <p className="muted small approval-note">Windows asks you to confirm: this changes something outside Nexus.</p>
      )}
      <ErrorLine error={error} />
    </article>
  );
}
