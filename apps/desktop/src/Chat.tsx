import { useCallback, useEffect, useRef, useState } from "react";
import type {
  ChatMessageView,
  ConnectionView,
  ConversationId,
  ConversationSummary,
  ConversationView,
  RunId,
  RunView,
} from "@nexus/contracts";
import { api, describeError } from "./api";
import { RunDetail, time } from "./Notes";
import { StepList } from "./Steps";
import { ChatIcon, PlusIcon, SendIcon } from "./icons";

/** Fast while a reply streams; otherwise the list is refreshed on demand. */
const STREAM_POLL_MS = 400;
const RUN_POLL_MS = 2000;

const FAILURE_TEXT: Record<string, string> = {
  QuotaExceeded: "The provider's quota has been reached.",
  Unauthenticated: "The API key was rejected. Check the connection in Settings.",
  DeadlineExceeded: "The reply took too long and was stopped.",
  ProviderUnavailable: "The AI provider could not respond.",
  Forbidden: "The provider blocked the request.",
  InvalidRequest: "The provider rejected the request.",
  Interrupted: "The Host stopped before the reply finished.",
  Cancelled: "Stopped by you. The provider may still have processed the request.",
};

export function ChatPanel({
  onOpenSettings,
  initialConversationId,
}: {
  onOpenSettings: () => void;
  initialConversationId?: ConversationId | null;
}) {
  const [connections, setConnections] = useState<ConnectionView[] | null>(null);
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [selected, setSelected] = useState<ConversationId | null>(initialConversationId ?? null);
  const [conversation, setConversation] = useState<ConversationView | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  const loadList = useCallback(async () => {
    try {
      const [conns, convs] = await Promise.all([api.listConnections(), api.listConversations()]);
      setConnections(conns);
      setConversations(convs);
      setLoadError(null);
    } catch (err) {
      setLoadError(describeError(err));
    }
  }, []);

  const loadConversation = useCallback(async (id: ConversationId) => {
    try {
      setConversation(await api.getConversation(id));
      setLoadError(null);
    } catch (err) {
      setLoadError(describeError(err));
    }
  }, []);

  useEffect(() => {
    void loadList();
  }, [loadList]);

  useEffect(() => {
    if (selected) void loadConversation(selected);
    else setConversation(null);
  }, [selected, loadConversation]);

  const streaming = conversation?.messages.some((m) => m.status === "Streaming") ?? false;
  useEffect(() => {
    if (!streaming || !selected) return;
    const timer = window.setInterval(() => void loadConversation(selected), STREAM_POLL_MS);
    return () => window.clearInterval(timer);
  }, [streaming, selected, loadConversation]);

  // When a reply settles, the conversation list (titles, order) may change.
  const wasStreaming = useRef(false);
  useEffect(() => {
    if (wasStreaming.current && !streaming) void loadList();
    wasStreaming.current = streaming;
  }, [streaming, loadList]);

  const active = connections?.find((c) => c.active) ?? null;
  const ready = active !== null && active.model !== null && active.has_key;

  return (
    <>
      <section className="card" aria-labelledby="chat-heading">
        <h2 id="chat-heading">
          <ChatIcon /> Chat
        </h2>
        {connections === null && !loadError && <p className="muted">Loading…</p>}
        {loadError && (
          <p className="error" role="alert">
            {loadError}
          </p>
        )}
        {connections !== null && !ready && (
          <div className="callout">
            <p>
              {active === null
                ? "You don't have an AI connection yet."
                : !active.has_key
                  ? `Connection "${active.display_name}" no longer has an API key in the Windows vault.`
                  : `Choose a model for connection "${active.display_name}".`}{" "}
              Nexus sends nothing until you choose a connection and a model.
            </p>
            <div className="actions">
              <button type="button" onClick={onOpenSettings}>
                Open Settings
              </button>
            </div>
          </div>
        )}
        {ready && active && (
          <p className="muted connection-line">
            <span>{`${active.display_name} · ${active.model}`}</span>
            {active.free_tier && <span className="badge badge-attention">Free tier</span>}
          </p>
        )}
        {active?.free_tier && ready && (
          <p className="warning">
            Google AI Studio free tier: don't send personal data, passwords, or sensitive
            information. Outside the EEA, Switzerland and the UK, Google may use your conversations
            to improve its products, and human reviewers may read them.
          </p>
        )}

        <div className="conversations">
          <button
            type="button"
            className={`conversation-row${selected === null ? " selected" : ""}`}
            aria-current={selected === null ? "true" : undefined}
            onClick={() => setSelected(null)}
          >
            <PlusIcon size={14} /> New conversation
          </button>
          {conversations.map((c) => (
            <button
              type="button"
              key={c.conversation_id}
              className={`conversation-row${c.conversation_id === selected ? " selected" : ""}`}
              aria-current={c.conversation_id === selected ? "true" : undefined}
              onClick={() => setSelected(c.conversation_id)}
            >
              <span className="run-title">{c.title}</span>
              <span className="muted">{time(c.updated_at_ms)}</span>
            </button>
          ))}
        </div>
      </section>

      <section className="card" aria-labelledby="thread-heading">
        <h2 id="thread-heading">{conversation?.title ?? "New conversation"}</h2>
        {conversation && conversation.messages.length > 0 ? (
          <ol className="messages" aria-live="polite">
            {conversation.messages.map((m) => (
              <MessageItem
                key={m.message_id}
                message={m}
                onChanged={() => selected && void loadConversation(selected)}
              />
            ))}
          </ol>
        ) : (
          <p className="muted">Send the first message. Nexus replies in whatever language you write to it.</p>
        )}
        <Composer
          disabled={!ready || streaming}
          conversationId={selected}
          onSent={(conversationId) => {
            setSelected(conversationId);
            void loadConversation(conversationId);
            void loadList();
          }}
        />
      </section>
    </>
  );
}

function MessageItem({ message, onChanged }: { message: ChatMessageView; onChanged: () => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const mine = message.role === "User";

  async function stop() {
    setBusy(true);
    setError(null);
    try {
      await api.cancelReply(message.message_id);
      onChanged();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <li className={`message ${mine ? "message-user" : "message-model"}`}>
      <div className="message-meta">
        <span className="message-author">{mine ? "You" : "Assistant"}</span>
        <span className="muted">{time(message.created_at_ms)}</span>
        {!mine && message.model && (
          <span className="muted">
            {message.persona_version !== null
              ? `${message.model} · persona v${message.persona_version}`
              : message.model}
          </span>
        )}
      </div>
      {!mine && <StepList steps={message.steps} />}
      {message.text && <div className="message-text">{message.text}</div>}
      {message.status === "Streaming" && (
        <div className="actions">
          <span className="muted" role="status">
            Typing…
          </span>
          <button type="button" className="secondary" disabled={busy} onClick={stop}>
            Stop
          </button>
        </div>
      )}
      {message.failure && (
        <p className={message.status === "Cancelled" ? "muted" : "error"}>
          {FAILURE_TEXT[message.failure.code] ?? message.failure.code}{" "}
          <span className="muted">({message.failure.message})</span>
        </p>
      )}
      {message.status === "Complete" &&
        (message.prompt_tokens !== null || message.output_tokens !== null) && (
          <p className="muted tokens">
            {`${message.prompt_tokens ?? "?"} tokens sent · ${message.output_tokens ?? "?"} received`}
          </p>
        )}
      {message.proposed_run_id && <ProposedNote runId={message.proposed_run_id} />}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </li>
  );
}

/** A note the model proposed: shown with the same exact preview and
 *  approval as any other run. Nothing is written until the user approves. */
function ProposedNote({ runId }: { runId: RunId }) {
  const [run, setRun] = useState<RunView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setRun(await api.getRun(runId));
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, [runId]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      if (!document.hidden) void load();
    }, RUN_POLL_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  return (
    <div className="proposal">
      <p className="muted">The assistant proposed a note. Nothing is saved until you approve it:</p>
      {run && <RunDetail summary={run} onChanged={() => void load()} />}
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}

function Composer({
  disabled,
  conversationId,
  onSent,
}: {
  disabled: boolean;
  conversationId: ConversationId | null;
  onSent: (conversationId: ConversationId) => void;
}) {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One key per message: a retry after a lost answer is the same turn.
  const intentKey = useRef<string>(crypto.randomUUID());

  async function send() {
    if (text.trim() === "" || busy || disabled) return;
    setBusy(true);
    setError(null);
    try {
      const sent = await api.sendMessage(conversationId, intentKey.current, text);
      intentKey.current = crypto.randomUUID();
      setText("");
      onSent(sent.conversation_id);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="composer"
      onSubmit={(e) => {
        e.preventDefault();
        void send();
      }}
    >
      <label>
        Message
        <textarea
          value={text}
          rows={3}
          maxLength={16000}
          onChange={(e) => {
            setText(e.currentTarget.value);
            intentKey.current = crypto.randomUUID();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              void send();
            }
          }}
        />
      </label>
      <div className="actions">
        <button type="submit" disabled={disabled || busy || text.trim() === ""}>
          <SendIcon size={14} /> {busy ? "Sending…" : "Send"}
        </button>
        <span className="muted">Enter sends · Shift+Enter for a new line</span>
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
