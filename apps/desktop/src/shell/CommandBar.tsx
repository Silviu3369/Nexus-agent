// The persistent command composer. A task given here starts new work for
// the agent; nothing is sent until a connection and a model are chosen.

import { useRef, useState } from "react";
import type { FormEvent } from "react";
import { api, describeError } from "../api";
import { deriveCore } from "../model/execution.ts";
import { seconds } from "../model/format.ts";
import { useDerived, useNexus, useNexusActions, useNow } from "../state/store";
import { MicIcon, PaperclipIcon, PauseIcon, SendIcon, StopIcon } from "../icons";
import { useGoTo } from "./nav";

export function CommandBar() {
  const { composing, connections, projects } = useNexus();
  const { focusRun, pending, ready, activeProject } = useDerived();
  const { follow, refresh, setComposing } = useNexusActions();
  const goTo = useGoTo();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [stopping, setStopping] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // One key per intent: a retry after a lost answer is the same request.
  const intentKey = useRef<string>(crypto.randomUUID());
  const now = useNow(focusRun?.status === "running" ? 250 : 1000);

  const core = deriveCore({ run: focusRun, pendingApprovals: pending.length, composing, now });
  const running = focusRun?.items.find((i) => i.status === "running") ?? null;
  const streaming = focusRun?.streaming ?? false;

  async function submit(event: FormEvent) {
    event.preventDefault();
    if (!ready || busy || text.trim() === "") return;
    setBusy(true);
    setError(null);
    try {
      const sent = await api.sendMessage(null, activeProject?.project_id ?? null, intentKey.current, text);
      intentKey.current = crypto.randomUUID();
      setText("");
      setComposing(false);
      follow(sent.conversation_id);
      await refresh(["steps", "conversations", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  async function chooseProject(id: string) {
    setError(null);
    try {
      await api.setActiveProject(id);
      await refresh(["projects"]);
    } catch (err) {
      setError(describeError(err));
    }
  }

  async function stop() {
    if (!focusRun || !streaming) return;
    setStopping(true);
    setError(null);
    try {
      await api.cancelReply(focusRun.messageId);
      await refresh(["steps", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setStopping(false);
    }
  }

  const stateText = running
    ? `${core.label}${seconds(now - running.startedAt) ? ` · ${seconds(now - running.startedAt)}` : ""}`
    : core.label;

  return (
    <form className="command" onSubmit={submit} aria-label="Command bar">
      <div className={`command-state core-${core.state.toLowerCase()}`} role="status" aria-live="polite">
        <span className="orb" aria-hidden="true" />
        <span className="command-state-text">{stateText}</span>
      </div>
      <div className="command-input">
        {projects && projects.length > 1 && (
          <select
            className="command-project"
            aria-label="Project for new tasks"
            title="New tasks go to this project"
            value={activeProject?.project_id ?? ""}
            onChange={(e) => void chooseProject(e.currentTarget.value)}
          >
            {projects.map((p) => (
              <option key={p.project_id} value={p.project_id}>
                {p.name}
              </option>
            ))}
          </select>
        )}
        <input
          aria-label="Give Nexus a task"
          placeholder={ready ? "Give Nexus a task…" : "Choose an AI connection and model first"}
          value={text}
          maxLength={16000}
          disabled={!ready}
          onChange={(e) => {
            setText(e.currentTarget.value);
            setComposing(e.currentTarget.value.trim() !== "");
            intentKey.current = crypto.randomUUID();
          }}
          onBlur={() => setComposing(false)}
          onFocus={() => setComposing(text.trim() !== "")}
        />
        <button type="button" className="icon-button" disabled title="Attachments are not supported yet" aria-label="Attach a file (not available yet)">
          <PaperclipIcon size={16} />
        </button>
        <button type="button" className="icon-button" disabled title="Voice input is not available yet" aria-label="Voice input (not available yet)">
          <MicIcon size={16} />
        </button>
      </div>
      <div className="command-controls">
        <button
          type="button"
          className="icon-button"
          disabled
          title="The Host cannot pause and resume a run yet"
          aria-label="Pause or resume (not available yet)"
        >
          <PauseIcon size={16} />
        </button>
        <button
          type="button"
          className="icon-button danger-ghost"
          disabled={!streaming || stopping}
          onClick={stop}
          title={streaming ? "Stop the reply in progress" : "Nothing is running"}
          aria-label="Stop"
        >
          <StopIcon size={16} />
        </button>
        <button type="submit" className="primary" disabled={!ready || busy || text.trim() === ""}>
          <SendIcon size={14} /> {busy ? "Starting…" : "Run"}
        </button>
      </div>
      {!ready && connections !== null && (
        <p className="command-hint">
          Nexus sends nothing until you choose a connection and a model.{" "}
          <button type="button" className="link" onClick={() => goTo({ section: "models" })}>
            Set up a model
          </button>
        </p>
      )}
      {error && (
        <p className="command-hint error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}
