import { useState } from "react";
import { api, describeError } from "../api";
import { useGoTo } from "../shell/nav";
import { useNexusActions } from "../state/store";
import type { TimelineItem } from "../model/execution.ts";
import { clock, seconds } from "../model/format.ts";
import { StatusPill } from "../ui";
import {
  AlertIcon,
  BriefcaseIcon,
  CheckIcon,
  ChevronDownIcon,
  FolderIcon,
  GlobeIcon,
  LockIcon,
  MemoryIcon,
  NotesIcon,
  PlugIcon,
  SparkleIcon,
} from "../icons";

function KindIcon({ item }: { item: TimelineItem }) {
  switch (item.kind) {
    case "model":
      return <SparkleIcon size={14} />;
    case "web":
      return <GlobeIcon size={14} />;
    case "note":
    case "write":
      return <NotesIcon size={14} />;
    case "memory":
      return <MemoryIcon size={14} />;
    case "mcp":
      return <PlugIcon size={14} />;
    case "record":
      return <BriefcaseIcon size={14} />;
    case "doc":
      return <FolderIcon size={14} />;
    case "approval":
      return <LockIcon size={14} />;
  }
}

/** One numbered entry of a run: what, who, with which model or tool, how
 *  long, and — expanded — the recorded facts. */
export function TimelineEntry({ item, index, now }: { item: TimelineItem; index: number; now: number }) {
  const [open, setOpen] = useState(false);
  const elapsed = item.status === "running" ? now - item.startedAt : item.durationMs;
  const meta = [item.agent, item.model, item.tool, seconds(elapsed)].filter(Boolean).join(" · ");
  return (
    <li className={`tl tl-${item.status}`}>
      <button type="button" className="tl-row" aria-expanded={open} onClick={() => setOpen(!open)}>
        <span className="tl-index">{String(index).padStart(2, "0")}</span>
        <span className="tl-icon" aria-hidden="true">
          {item.status === "running" ? <span className="pulse-dot" /> : <KindIcon item={item} />}
        </span>
        <span className="tl-main">
          <span className="tl-title">{item.title}</span>
          <span className="tl-meta">{meta}</span>
        </span>
        <StatusPill status={item.status} />
        <ChevronDownIcon size={14} className="tl-chevron" />
      </button>
      {item.outcome && !open && <p className={`tl-outcome${item.status === "failed" ? " bad" : ""}`}>{item.outcome}</p>}
      {item.record && <RecordActions item={item} />}
      {open && (
        <div className="tl-detail">
          {item.outcome && <p className={`tl-outcome${item.status === "failed" ? " bad" : ""}`}>{item.outcome}</p>}
          <dl className="kv kv-compact">
            <div className="kv-row">
              <dt>Started</dt>
              <dd>{clock(item.startedAt)}</dd>
            </div>
            {item.finishedAt !== null && (
              <div className="kv-row">
                <dt>Finished</dt>
                <dd>{clock(item.finishedAt)}</dd>
              </div>
            )}
            {item.facts.map(([k, v]) => (
              <div className="kv-row" key={k}>
                <dt>{k}</dt>
                <dd className={k === "Action" || k === "File" || k === "Manifest" || k === "SHA-256" ? "mono" : undefined}>{v}</dd>
              </div>
            ))}
          </dl>
        </div>
      )}
    </li>
  );
}

/** A change the agent made to one of the user's registers: undo it, or
 *  open the record. The Host refuses an undo once the record changed again. */
function RecordActions({ item }: { item: TimelineItem }) {
  const { refresh } = useNexusActions();
  const goTo = useGoTo();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const record = item.record;
  if (!record || record.undone || record.changeId === null || item.status !== "completed") return null;
  const changeId = record.changeId;

  async function undo() {
    setBusy(true);
    setError(null);
    try {
      await api.undoRecordChange(changeId);
      await refresh(["registers", "steps", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="tl-actions">
      <button type="button" className="ghost" disabled={busy} aria-label={`Undo ${item.title}`} onClick={() => void undo()}>
        Undo
      </button>
      {record.recordId && (
        <button
          type="button"
          className="ghost"
          aria-label={`Open ${item.title} in Jobs`}
          onClick={() => goTo({ section: "jobs", recordId: record.recordId ?? undefined })}
        >
          Open in Jobs
        </button>
      )}
      {error && (
        <span className="error" role="alert">
          {error}
        </span>
      )}
    </div>
  );
}

export function Timeline({ items, now, label }: { items: TimelineItem[]; now: number; label: string }) {
  if (items.length === 0) return null;
  return (
    <ol className="timeline" aria-label={label}>
      {items.map((item, i) => (
        <TimelineEntry key={item.id} item={item} index={i + 1} now={now} />
      ))}
    </ol>
  );
}

/** Compact marker used in lists: done, failed, or working. */
export function StateMark({ status }: { status: TimelineItem["status"] }) {
  if (status === "running") return <span className="pulse-dot" aria-hidden="true" />;
  if (status === "failed") return <AlertIcon size={14} />;
  return <CheckIcon size={14} />;
}
