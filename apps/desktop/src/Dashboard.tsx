import { useCallback, useEffect, useState } from "react";
import type { ConnectionView, ConversationId, ConversationSummary, HostStatus, RunId, RunView } from "@nexus/contracts";
import { api, describeError } from "./api";
import { StatusBadge, time } from "./Notes";
import {
  AlertIcon,
  ChatIcon,
  CheckCircleIcon,
  ChevronRightIcon,
  NotesIcon,
  PlugIcon,
  PlusIcon,
  ShieldIcon,
} from "./icons";

const POLL_MS = 3000;
const RECENT_COUNT = 5;

function greeting(): string {
  const h = new Date().getHours();
  if (h < 5) return "Still up";
  if (h < 12) return "Good morning";
  if (h < 18) return "Good afternoon";
  return "Good evening";
}

export function Dashboard({
  hostStatus,
  onGoToChat,
  onGoToNotes,
  onGoToSettings,
}: {
  hostStatus: HostStatus;
  onGoToChat: (conversationId?: ConversationId) => void;
  onGoToNotes: (runId?: RunId) => void;
  onGoToSettings: () => void;
}) {
  const [runs, setRuns] = useState<RunView[] | null>(null);
  const [conversations, setConversations] = useState<ConversationSummary[] | null>(null);
  const [connections, setConnections] = useState<ConnectionView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const [snapshot, convs, conns] = await Promise.all([
        api.listRuns(),
        api.listConversations(),
        api.listConnections(),
      ]);
      setRuns(snapshot.runs);
      setConversations(convs);
      setConnections(conns);
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      if (!document.hidden) void load();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  const pending = runs?.filter((r) => r.status === "AwaitingApproval") ?? [];
  const recentRuns = (runs ?? []).slice(0, RECENT_COUNT);
  const recentConversations = (conversations ?? []).slice(0, RECENT_COUNT);
  const active = connections?.find((c) => c.active) ?? null;
  const ready = active !== null && active.model !== null && active.has_key;
  const hostOk = hostStatus.state === "Connected";

  const statusLine = !hostOk
    ? "The Host is not connected."
    : pending.length > 0
      ? `${pending.length} item${pending.length === 1 ? "" : "s"} waiting for your approval.`
      : "Idle and ready for your request.";

  function reviewApprovals() {
    if (pending.length === 1) onGoToNotes(pending[0].run_id);
    else onGoToNotes();
  }

  return (
    <>
      <section className="hero">
        <div className="hero-text">
          <p className="hero-greeting">{`${greeting()}.`}</p>
          <h2 className="hero-title">{statusLine}</h2>
          <p className="hero-tagline">Nexus — your personal AI agent.</p>
        </div>
        <div className="hero-actions">
          <button type="button" onClick={() => onGoToChat()}>
            <PlusIcon size={14} /> New chat
          </button>
          <button type="button" className="secondary" onClick={() => onGoToNotes()}>
            <PlusIcon size={14} /> New note
          </button>
          <button
            type="button"
            className="secondary"
            onClick={reviewApprovals}
            disabled={pending.length === 0}
          >
            <ShieldIcon size={14} /> Review approvals{pending.length > 0 ? ` (${pending.length})` : ""}
          </button>
          <button type="button" className="secondary" onClick={onGoToSettings}>
            <PlugIcon size={14} /> Settings
          </button>
        </div>
      </section>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="dashboard-grid">
        <section className="card" aria-labelledby="approvals-heading">
          <h2 id="approvals-heading">
            <ShieldIcon /> Needs your approval
          </h2>
          {pending.length === 0 ? (
            <p className="muted empty-row">
              <CheckCircleIcon size={14} /> Nothing is waiting for you.
            </p>
          ) : (
            <ul className="dash-list">
              {pending.map((run) => (
                <li key={run.run_id}>
                  <button type="button" className="dash-row" onClick={() => onGoToNotes(run.run_id)}>
                    <AlertIcon size={16} />
                    <span className="run-title">{run.title}</span>
                    <StatusBadge status={run.status} />
                    <ChevronRightIcon size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="card" aria-labelledby="connection-heading">
          <h2 id="connection-heading">
            <PlugIcon /> AI connection
          </h2>
          {connections === null ? (
            <p className="muted">Loading…</p>
          ) : active === null ? (
            <div className="empty-row-stack">
              <p className="muted">No AI connection yet.</p>
              <button type="button" className="secondary" onClick={onGoToSettings}>
                Add one in Settings
              </button>
            </div>
          ) : (
            <>
              <dl className="facts">
                <dt>Connection</dt>
                <dd>
                  {active.display_name}{" "}
                  {active.free_tier && <span className="badge badge-attention">Free tier</span>}
                </dd>
                <dt>Model</dt>
                <dd>{active.model ?? "not chosen"}</dd>
                <dt>Status</dt>
                <dd>{ready ? <span className="badge badge-good">Ready</span> : <span className="badge badge-bad">Needs setup</span>}</dd>
              </dl>
              <div className="actions">
                <button type="button" className="secondary" onClick={onGoToSettings}>
                  Manage
                </button>
              </div>
            </>
          )}
        </section>

        <section className="card" aria-labelledby="conversations-heading">
          <h2 id="conversations-heading">
            <ChatIcon /> Recent conversations
          </h2>
          {recentConversations.length === 0 ? (
            <p className="muted">No conversations yet. Start one from Chat.</p>
          ) : (
            <ul className="dash-list">
              {recentConversations.map((c) => (
                <li key={c.conversation_id}>
                  <button
                    type="button"
                    className="dash-row"
                    onClick={() => onGoToChat(c.conversation_id)}
                  >
                    <span className="run-title">{c.title}</span>
                    <span className="muted">{time(c.updated_at_ms)}</span>
                    <ChevronRightIcon size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>

        <section className="card" aria-labelledby="recent-notes-heading">
          <h2 id="recent-notes-heading">
            <NotesIcon /> Recent notes
          </h2>
          {recentRuns.length === 0 ? (
            <p className="muted">No notes yet.</p>
          ) : (
            <ul className="dash-list">
              {recentRuns.map((run) => (
                <li key={run.run_id}>
                  <button type="button" className="dash-row" onClick={() => onGoToNotes(run.run_id)}>
                    <span className="run-title">{run.title}</span>
                    <StatusBadge status={run.status} />
                    <ChevronRightIcon size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
      </div>
    </>
  );
}
