import { useCallback, useEffect, useRef, useState } from "react";
import type {
  AgentStepView,
  ConnectionView,
  ConversationId,
  ConversationSummary,
  ConversationView,
  HostStatus,
  RunId,
  RunView,
} from "@nexus/contracts";
import { api, describeError } from "./api";
import { StatusBadge, time } from "./Notes";
import { StepItem, StepList, describeStep } from "./Steps";
import {
  AlertIcon,
  ChatIcon,
  CheckCircleIcon,
  ChevronRightIcon,
  NotesIcon,
  PlugIcon,
  PlusIcon,
  SendIcon,
  ShieldIcon,
  SparkleIcon,
} from "./icons";

const POLL_MS = 3000;
/** While the agent is working, the feed refreshes this often. */
const BUSY_POLL_MS = 600;
const LIVE_POLL_MS = 400;
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
  const [activity, setActivity] = useState<AgentStepView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  /** The conversation started from the command bar, followed live here. */
  const [live, setLive] = useState<ConversationId | null>(null);

  const load = useCallback(async () => {
    try {
      const [snapshot, convs, conns, steps] = await Promise.all([
        api.listRuns(),
        api.listConversations(),
        api.listConnections(),
        api.getActivity(),
      ]);
      setRuns(snapshot.runs);
      setConversations(convs);
      setConnections(conns);
      setActivity(steps);
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, []);

  const running = activity?.find((s) => s.status === "Running") ?? null;
  const busy = running !== null;

  useEffect(() => {
    void load();
    const timer = window.setInterval(
      () => {
        if (!document.hidden) void load();
      },
      busy ? BUSY_POLL_MS : POLL_MS,
    );
    return () => window.clearInterval(timer);
  }, [load, busy]);

  const pending = runs?.filter((r) => r.status === "AwaitingApproval") ?? [];
  const recentRuns = (runs ?? []).slice(0, RECENT_COUNT);
  const recentConversations = (conversations ?? []).slice(0, RECENT_COUNT);
  const active = connections?.find((c) => c.active) ?? null;
  const ready = active !== null && active.model !== null && active.has_key;
  const hostOk = hostStatus.state === "Connected";

  const statusLine = !hostOk
    ? "The Host is not connected."
    : running
      ? `Working: ${describeStep(running).title}…`
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
          <h2 className="hero-title" aria-live="polite">
            {statusLine}
          </h2>
        </div>
        <CommandBar
          ready={ready}
          known={connections !== null}
          onStarted={(conversationId) => {
            setLive(conversationId);
            void load();
          }}
          onSetup={onGoToSettings}
        />
        <div className="hero-actions">
          <button type="button" className="secondary" onClick={() => onGoToChat()}>
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
        </div>
      </section>

      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}

      <div className="dashboard-columns">
        <div className="dashboard-main">
          {live && (
            <LiveResult
              key={live}
              conversationId={live}
              onOpen={() => onGoToChat(live)}
              onReviewNote={(run) => onGoToNotes(run)}
              onClear={() => setLive(null)}
              onSettled={() => void load()}
            />
          )}

          <section className="card" aria-labelledby="activity-heading">
            <h2 id="activity-heading">
              <SparkleIcon /> Live activity
            </h2>
            {activity === null ? (
              <p className="muted">Loading…</p>
            ) : activity.length === 0 ? (
              <p className="muted">
                Nothing yet. Ask Nexus something above — every step it takes shows up here as it
                happens.
              </p>
            ) : (
              <ol className="steps activity-feed">
                {activity.map((s) => (
                  <StepItem key={s.step_id} step={s} onOpen={() => onGoToChat(s.conversation_id)} />
                ))}
              </ol>
            )}
          </section>
        </div>

        <div className="dashboard-side">
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
                  <dt>Thinking</dt>
                  <dd>{active.thinking_level}</dd>
                  <dt>Status</dt>
                  <dd>
                    {ready ? (
                      <span className="badge badge-good">Ready</span>
                    ) : (
                      <span className="badge badge-bad">Needs setup</span>
                    )}
                  </dd>
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
              <p className="muted">No conversations yet.</p>
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
      </div>
    </>
  );
}

/** Starts a new conversation from the dashboard. Nothing is sent until a
 *  connection and model are chosen — there is no default provider. */
function CommandBar({
  ready,
  known,
  onStarted,
  onSetup,
}: {
  ready: boolean;
  known: boolean;
  onStarted: (conversationId: ConversationId) => void;
  onSetup: () => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One key per intent: a retry after a lost answer is the same request.
  const intentKey = useRef<string>(crypto.randomUUID());

  async function run(event: React.FormEvent) {
    event.preventDefault();
    if (!ready || busy || text.trim() === "") return;
    setBusy(true);
    setError(null);
    try {
      const sent = await api.sendMessage(null, intentKey.current, text);
      intentKey.current = crypto.randomUUID();
      setText("");
      onStarted(sent.conversation_id);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form className="command-bar" onSubmit={run} role="search">
      <div className="command-row">
        <input
          aria-label="Ask Nexus"
          placeholder={ready ? "Ask Nexus or give it a task…" : "Choose an AI connection first"}
          value={text}
          maxLength={16000}
          disabled={!ready}
          onChange={(e) => {
            setText(e.currentTarget.value);
            intentKey.current = crypto.randomUUID();
          }}
        />
        <button type="submit" disabled={!ready || busy || text.trim() === ""}>
          <SendIcon size={14} /> {busy ? "Starting…" : "Run"}
        </button>
      </div>
      {known && !ready && (
        <p className="muted command-hint">
          Nexus sends nothing until you choose a connection and a model.{" "}
          <button type="button" className="link" onClick={onSetup}>
            Set up a connection
          </button>
        </p>
      )}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

/** The latest reply of the conversation started here, followed live: its
 *  steps as they happen, then its text. */
function LiveResult({
  conversationId,
  onOpen,
  onReviewNote,
  onClear,
  onSettled,
}: {
  conversationId: ConversationId;
  onOpen: () => void;
  onReviewNote: (run: RunId) => void;
  onClear: () => void;
  onSettled: () => void;
}) {
  const [view, setView] = useState<ConversationView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stopping, setStopping] = useState(false);

  const load = useCallback(async () => {
    try {
      setView(await api.getConversation(conversationId));
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, [conversationId]);

  const replies = view?.messages.filter((m) => m.role === "Model") ?? [];
  const reply = replies.length > 0 ? replies[replies.length - 1] : null;
  const streaming = reply?.status === "Streaming";

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!streaming) return;
    const timer = window.setInterval(() => void load(), LIVE_POLL_MS);
    return () => window.clearInterval(timer);
  }, [streaming, load]);

  const wasStreaming = useRef(false);
  useEffect(() => {
    if (wasStreaming.current && !streaming) onSettled();
    wasStreaming.current = streaming;
  }, [streaming, onSettled]);

  async function stop() {
    if (!reply) return;
    setStopping(true);
    try {
      await api.cancelReply(reply.message_id);
      await load();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setStopping(false);
    }
  }

  return (
    <section className="card live-result" aria-labelledby="live-heading">
      <h2 id="live-heading">
        {streaming && <span className="pulse-dot" aria-hidden="true" />}
        {view?.title ?? "Starting…"}
      </h2>
      {reply && <StepList steps={reply.steps} />}
      {reply?.text && <div className="message-text">{reply.text}</div>}
      {reply?.failure && (
        <p className={reply.status === "Cancelled" ? "muted" : "error"}>
          {reply.failure.message}
        </p>
      )}
      <div className="actions">
        {streaming && (
          <button type="button" className="secondary" disabled={stopping} onClick={stop}>
            Stop
          </button>
        )}
        {reply?.proposed_run_id && (
          <button type="button" onClick={() => onReviewNote(reply.proposed_run_id!)}>
            Review the proposed note
          </button>
        )}
        <button type="button" className="secondary" onClick={onOpen}>
          Open in Chat
        </button>
        <button type="button" className="secondary" onClick={onClear}>
          Clear
        </button>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </section>
  );
}
