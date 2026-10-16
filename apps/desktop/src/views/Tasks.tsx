// Tasks: every request given to the agent, with its runs, steps and the
// actions they led to. A task continues with follow-up messages.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ConversationView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ApprovalCard } from "../components/ApprovalCard";
import { Timeline } from "../components/Timeline";
import { buildRuns, taskRows } from "../model/execution.ts";
import type { ExecStatus, RunVM } from "../model/execution.ts";
import { ago, clock, duration } from "../model/format.ts";
import { RunDetail } from "../Notes";
import { useDerived, useNexus, useNexusActions, useNow } from "../state/store";
import { replyFailure } from "../text";
import { Empty, ErrorLine, StatusPill } from "../ui";
import { PlusIcon, SendIcon, StopIcon } from "../icons";

const STREAM_POLL_MS = 400;

type Filter = "all" | "running" | "approval" | "failed";
const FILTERS: { id: Filter; label: string; match: (s: ExecStatus | null) => boolean }[] = [
  { id: "all", label: "All", match: () => true },
  { id: "running", label: "Running", match: (s) => s === "running" },
  { id: "approval", label: "Needs approval", match: (s) => s === "approval_required" },
  { id: "failed", label: "Failed", match: (s) => s === "failed" },
];

export function TasksView({ initialTaskId }: { initialTaskId?: string | null }) {
  const d = useNexus();
  const [selected, setSelected] = useState<string | null>(() =>
    initialTaskId === undefined ? (d.focusId ?? d.conversations?.[0]?.conversation_id ?? null) : initialTaskId,
  );
  const [filter, setFilter] = useState<Filter>("all");
  const now = useNow(5000);
  const rows = useMemo(() => taskRows(d.conversations ?? [], d.steps ?? [], d.runs ?? []), [d.conversations, d.steps, d.runs]);
  const match = FILTERS.find((f) => f.id === filter)!.match;
  const shown = rows.filter((r) => match(r.status));

  return (
    <div className="split">
      <section className="split-list panel" aria-label="Tasks">
        <header className="panel-head">
          <div className="segmented" role="group" aria-label="Filter tasks">
            {FILTERS.map((f) => (
              <button type="button" key={f.id} aria-pressed={filter === f.id} onClick={() => setFilter(f.id)}>
                {f.label}
              </button>
            ))}
          </div>
        </header>
        <div className="panel-body">
          <button
            type="button"
            className={`row row-new${selected === null ? " selected" : ""}`}
            aria-current={selected === null ? "true" : undefined}
            onClick={() => setSelected(null)}
          >
            <PlusIcon size={14} /> New task
          </button>
          {d.conversations === null ? (
            <Empty>Loading…</Empty>
          ) : shown.length === 0 ? (
            <Empty>{rows.length === 0 ? "No tasks yet." : "No task matches this filter."}</Empty>
          ) : (
            <ul className="row-list">
              {shown.map((r) => (
                <li key={r.id}>
                  <button
                    type="button"
                    className={`row${r.id === selected ? " selected" : ""}`}
                    aria-current={r.id === selected ? "true" : undefined}
                    onClick={() => setSelected(r.id)}
                  >
                    <span className="row-title">{r.title}</span>
                    <StatusPill status={r.status} />
                    <span className="row-time">{ago(r.updatedAt, now)}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
      </section>
      <TaskDetail key={selected ?? "new"} taskId={selected} onCreated={setSelected} />
    </div>
  );
}

/** The conversation of a task: the store's live copy when it is the one
 *  being followed, otherwise read here (and re-read while it streams). */
function useTask(taskId: string | null) {
  const d = useNexus();
  const live = taskId !== null && d.focus?.conversation_id === taskId ? d.focus : null;
  const [own, setOwn] = useState<ConversationView | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(async () => {
    if (!taskId || live) return;
    try {
      setOwn(await api.getConversation(taskId));
      setError(null);
    } catch (err) {
      setError(describeError(err));
    }
  }, [taskId, live]);

  useEffect(() => {
    void load();
  }, [load, d.steps, d.runs]);

  const view = live ?? own;
  const streaming = view?.messages.some((m) => m.status === "Streaming") ?? false;
  useEffect(() => {
    if (!streaming || live) return;
    const timer = window.setInterval(() => void load(), STREAM_POLL_MS);
    return () => window.clearInterval(timer);
  }, [streaming, live, load]);

  return { view, error, streaming, reload: load };
}

function TaskDetail({ taskId, onCreated }: { taskId: string | null; onCreated: (id: string) => void }) {
  const { runsById, agentName, ready } = useDerived();
  const { follow } = useNexusActions();
  const { view, error, streaming } = useTask(taskId);
  const runs = useMemo(() => (view ? buildRuns(view, runsById, agentName) : []), [view, runsById, agentName]);
  const last = runs.length > 0 ? runs[runs.length - 1] : null;

  return (
    <section className="split-detail panel" aria-labelledby="task-heading">
      <header className="panel-head">
        <h2 id="task-heading">{taskId === null ? "New task" : (view?.title ?? "Loading…")}</h2>
        <div className="panel-actions">
          {last && <StatusPill status={last.status} />}
          {taskId !== null && (
            <button type="button" className="ghost" onClick={() => follow(taskId)}>
              Follow live
            </button>
          )}
        </div>
      </header>
      <div className="panel-body task-body">
        <ErrorLine error={error} />
        {taskId === null && (
          <Empty>Describe what Nexus should do. It replies in the language you write in, can read public web pages, and asks before writing anything.</Empty>
        )}
        {runs.map((run, i) => (
          <RunBlock key={run.id} run={run} index={i + 1} agentName={agentName} />
        ))}
        <Composer
          taskId={taskId}
          disabled={!ready || streaming}
          onSent={(id) => {
            if (taskId === null) onCreated(id);
            follow(id);
          }}
        />
      </div>
    </section>
  );
}

function RunBlock({ run, index, agentName }: { run: RunVM; index: number; agentName: string }) {
  const { runsById } = useDerived();
  const { refresh } = useNexusActions();
  const now = useNow(run.status === "running" ? 250 : 5000);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const note = run.proposedRunId ? runsById.get(run.proposedRunId) : undefined;
  const elapsed = (run.finishedAt ?? now) - run.startedAt;

  async function stop() {
    setStopping(true);
    setError(null);
    try {
      await api.cancelReply(run.messageId);
      await refresh(["steps", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setStopping(false);
    }
  }

  return (
    <article className={`run run-${run.status}`} aria-label={`Run ${index}`}>
      {run.request && (
        <div className="run-request">
          <span className="eyebrow">You</span>
          <p>{run.request}</p>
        </div>
      )}
      <header className="run-head">
        <span className="eyebrow">Run {index}</span>
        <StatusPill status={run.status} />
        <span className="run-meta">
          {[agentName, run.model, run.personaVersion !== null ? `persona v${run.personaVersion}` : null, clock(run.startedAt), duration(elapsed)]
            .filter(Boolean)
            .join(" · ")}
          {run.promptTokens !== null && ` · ${run.promptTokens} tokens sent · ${run.outputTokens ?? "?"} received`}
        </span>
      </header>
      <Timeline items={run.items} now={now} label={`Steps of run ${index}`} />
      {run.text && <div className="message-text">{run.text}</div>}
      {run.streaming && (
        <div className="actions">
          <span className="muted" role="status">
            Writing…
          </span>
          <button type="button" className="secondary" disabled={stopping} onClick={stop}>
            <StopIcon size={14} /> Stop
          </button>
        </div>
      )}
      {run.failure && <p className={run.status === "cancelled" ? "muted" : "error"}>{replyFailure(run.failure)}</p>}
      {note &&
        (note.status === "AwaitingApproval" ? (
          <ApprovalCard run={note} agentName={agentName} />
        ) : (
          <RunDetail summary={note} onChanged={() => void refresh(["runs"])} />
        ))}
      <ErrorLine error={error} />
    </article>
  );
}

function Composer({
  taskId,
  disabled,
  onSent,
}: {
  taskId: string | null;
  disabled: boolean;
  onSent: (taskId: string) => void;
}) {
  const { refresh } = useNexusActions();
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
      const sent = await api.sendMessage(taskId, intentKey.current, text);
      intentKey.current = crypto.randomUUID();
      setText("");
      onSent(sent.conversation_id);
      await refresh(["steps", "conversations", "focus"]);
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
          placeholder={taskId === null ? "Describe a new task…" : "Continue this task…"}
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
        <button type="submit" className="primary" disabled={disabled || busy || text.trim() === ""}>
          <SendIcon size={14} /> {busy ? "Sending…" : "Send"}
        </button>
        <span className="muted">Enter sends · Shift+Enter for a new line</span>
      </div>
      <ErrorLine error={error} />
    </form>
  );
}
