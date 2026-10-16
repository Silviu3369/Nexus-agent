// One remembered fact with where it came from, and the decisions the Host
// accepts for its state: approve or reject a proposal, correct a fact,
// delete it. Every change names the revision shown, so a fact changed
// elsewhere in the meantime is refused instead of overwritten.

import { useState } from "react";
import type { MemoryItemView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago, clock } from "../model/format.ts";
import { useGoTo } from "../shell/nav";
import { useNexusActions, useNow } from "../state/store";
import { ErrorLine } from "../ui";
import { AlertIcon, CheckIcon, XIcon } from "../icons";

const MAX_FACT = 1000;

export function MemoryCard({
  item,
  agentName,
  compact = false,
}: {
  item: MemoryItemView;
  agentName: string;
  compact?: boolean;
}) {
  const { refresh } = useNexusActions();
  const goTo = useGoTo();
  const now = useNow(30_000);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(item.content);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await action();
      setEditing(false);
      setConfirmDelete(false);
      await refresh(["memory", "projects", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const proposed = item.status === "Proposed";
  const where = item.scope === "Personal" ? "Personal" : (item.project_name ?? "Project");
  const draftOk = draft.trim() !== "" && draft.trim().length <= MAX_FACT && draft.trim() !== item.content;

  return (
    <article
      className={`fact fact-${item.status.toLowerCase()}${compact ? " fact-compact" : ""}`}
      aria-label={proposed ? "Proposed fact" : item.status === "Rejected" ? "Rejected fact" : "Remembered fact"}
    >
      <div className="fact-tags">
        <span className={`tag tag-${item.scope.toLowerCase()}`}>{where}</span>
        {proposed && <span className="pill pill-waiting">Waiting for your review</span>}
        {item.status === "Rejected" && <span className="pill pill-cancelled">Rejected — never sent</span>}
        {item.edited_by_user && <span className="tag">corrected by you</span>}
      </div>

      {editing ? (
        <form
          className="fact-edit"
          onSubmit={(e) => {
            e.preventDefault();
            if (draftOk) void run(() => api.editMemory(item.memory_id, item.revision, draft.trim()));
          }}
        >
          <label>
            Fact
            <textarea value={draft} rows={3} maxLength={MAX_FACT} onChange={(e) => setDraft(e.currentTarget.value)} />
          </label>
          <div className="actions">
            <button type="submit" className="primary" disabled={busy || !draftOk}>
              Save
            </button>
            <button
              type="button"
              className="ghost"
              disabled={busy}
              onClick={() => {
                setEditing(false);
                setDraft(item.content);
              }}
            >
              Cancel
            </button>
            <span className="muted small">{draft.trim().length} / {MAX_FACT}</span>
          </div>
        </form>
      ) : (
        <p className={`fact-text${compact ? " clamp-3" : ""}`}>{item.content}</p>
      )}

      <p className="fact-source">
        {item.source === "User" ? (
          <>Added by you · {ago(item.created_at_ms, now)}</>
        ) : (
          <>
            Proposed by {agentName}
            {item.conversation_id && item.conversation_title ? (
              <>
                {" in "}
                <button
                  type="button"
                  className="link"
                  onClick={() => goTo({ section: "tasks", taskId: item.conversation_id })}
                >
                  {item.conversation_title}
                </button>
              </>
            ) : null}
            {" · "}
            {ago(item.created_at_ms, now)}
            {item.decided_at_ms !== null && !proposed && ` · decided ${clock(item.decided_at_ms)}`}
          </>
        )}
      </p>
      {item.influences.length > 0 && (
        <p className="fact-warning">
          <AlertIcon size={13} /> Proposed after reading {item.influences.join(", ")}. Check that it is about you or
          your project, not something a page asked to be remembered.
        </p>
      )}

      {!editing && (
        <div className="actions fact-actions">
          {proposed && (
            <>
              <button
                type="button"
                className="primary"
                disabled={busy}
                onClick={() => void run(() => api.decideMemory(item.memory_id, item.revision, true))}
              >
                <CheckIcon size={13} /> Remember
              </button>
              <button
                type="button"
                className="secondary"
                disabled={busy}
                onClick={() => void run(() => api.decideMemory(item.memory_id, item.revision, false))}
              >
                <XIcon size={13} /> Reject
              </button>
            </>
          )}
          {item.status !== "Rejected" && (
            <button type="button" className="ghost" disabled={busy} onClick={() => setEditing(true)}>
              {proposed ? "Edit first" : "Correct"}
            </button>
          )}
          {!proposed &&
            (confirmDelete ? (
              <>
                <button
                  type="button"
                  className="danger"
                  disabled={busy}
                  onClick={() => void run(() => api.deleteMemory(item.memory_id))}
                >
                  Delete for good
                </button>
                <button type="button" className="ghost" disabled={busy} onClick={() => setConfirmDelete(false)}>
                  Keep
                </button>
              </>
            ) : (
              <button type="button" className="ghost danger-ghost" disabled={busy} onClick={() => setConfirmDelete(true)}>
                Delete
              </button>
            ))}
        </div>
      )}
      <ErrorLine error={error} />
    </article>
  );
}
