// Folders the user shares with Nexus. The agent reads documents only inside
// them, and proposes new files only where writing is allowed (each one is
// approved). The Host checks every path; nothing outside is visible.

import { useEffect, useState } from "react";
import type { FolderEntryView, FolderView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago, kib } from "../model/format.ts";
import { useNexus, useNexusActions, useNow } from "../state/store";
import { Empty, ErrorLine, Facts, Panel } from "../ui";
import { FolderIcon, PlusIcon } from "../icons";

const MAX_LABEL = 60;

export function SharedFolders() {
  const d = useNexus();
  return (
    <>
      <AddFolder />
      {d.folders === null ? (
        <p className="muted">Loading…</p>
      ) : d.folders.length === 0 ? (
        <Panel title="Shared folders" icon={<FolderIcon size={15} />} id="folders">
          <Empty>No folder is shared yet. Add one above, for example the folder with your CV and job ads.</Empty>
        </Panel>
      ) : (
        d.folders.map((f) => <FolderCard key={f.folder_id} folder={f} />)
      )}
    </>
  );
}

function AddFolder() {
  const { refresh } = useNexusActions();
  const [path, setPath] = useState("");
  const [label, setLabel] = useState("");
  const [canWrite, setCanWrite] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ok = path.trim() !== "" && label.trim() !== "";

  async function add() {
    setBusy(true);
    setError(null);
    try {
      await api.saveFolder({ folder_id: null, revision: 0, path: path.trim(), label: label.trim(), can_write: canWrite });
      setPath("");
      setLabel("");
      setCanWrite(false);
      await refresh(["folders"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Share a folder" icon={<PlusIcon size={15} />} id="folder-add">
      <form
        className="stack"
        aria-label="Share a folder"
        onSubmit={(e) => {
          e.preventDefault();
          if (ok) void add();
        }}
      >
        <label>
          Folder path
          <input
            value={path}
            spellCheck={false}
            maxLength={1024}
            placeholder="C:\Users\you\Documents\Jobs  ·  or G:\My Drive\Jobs"
            onChange={(e) => setPath(e.currentTarget.value)}
          />
        </label>
        <label>
          Name
          <input value={label} maxLength={MAX_LABEL} placeholder="e.g. Jobs" onChange={(e) => setLabel(e.currentTarget.value)} />
        </label>
        <label className="check">
          <input type="checkbox" checked={canWrite} onChange={(e) => setCanWrite(e.currentTarget.checked)} />
          Nexus may propose new files here (each one waits for your approval; nothing is overwritten)
        </label>
        <p className="muted">
          Nexus reads documents only inside the folders you share, never through links. A document's text goes to your AI
          provider when the agent reads it for a task. Copy the path from File Explorer's address bar.
        </p>
        <div className="actions">
          <button type="submit" className="primary" disabled={!ok || busy}>
            Share folder
          </button>
        </div>
        <ErrorLine error={error} />
      </form>
    </Panel>
  );
}

function FolderCard({ folder }: { folder: FolderView }) {
  const { refresh } = useNexusActions();
  const now = useNow(60_000);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [open, setOpen] = useState(false);
  const [entries, setEntries] = useState<FolderEntryView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    let alive = true;
    api
      .folderEntries(folder.folder_id)
      .then((e) => alive && setEntries(e))
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [open, folder.folder_id, folder.revision]);

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await refresh(["folders"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const readable = entries?.filter((e) => e.readable).length ?? 0;
  return (
    <Panel
      title={folder.label}
      icon={<FolderIcon size={15} />}
      id={`folder-${folder.folder_id}`}
      actions={
        <>
          <button type="button" className="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>
            {open ? "Hide files" : "Show files"}
          </button>
          <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(true)}>
            Stop sharing
          </button>
        </>
      }
    >
      {confirm && (
        <div className="confirm-bar" role="alert">
          <p>{`Stop sharing “${folder.label}”? Nothing in the folder is touched; Nexus just stops seeing it.`}</p>
          <div className="actions">
            <button type="button" className="danger" disabled={busy} onClick={() => void act(() => api.removeFolder(folder.folder_id))}>
              Stop sharing now
            </button>
            <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </button>
          </div>
        </div>
      )}
      <Facts
        rows={[
          ["Path", <span className="mono">{folder.path}</span>],
          ["State", folder.available ? "available" : (folder.problem ?? "not available")],
          ["Shared", ago(folder.created_at_ms, now)],
        ]}
      />
      <label className="check">
        <input
          type="checkbox"
          checked={folder.can_write}
          disabled={busy}
          aria-label={`Allow new files in ${folder.label}`}
          onChange={(e) =>
            void act(() =>
              api.saveFolder({
                folder_id: folder.folder_id,
                revision: folder.revision,
                path: folder.path,
                label: folder.label,
                can_write: e.currentTarget.checked,
              }),
            )
          }
        />
        {folder.can_write ? "New files allowed (each one approved by you)" : "Read only"}
      </label>
      {open &&
        (entries === null ? (
          <p className="muted">Loading…</p>
        ) : entries.length === 0 ? (
          <p className="muted">The folder is empty.</p>
        ) : (
          <>
            <p className="muted">{`${entries.length} items · ${readable} Nexus can read`}</p>
            <ul className="folder-entries" aria-label={`Files in ${folder.label}`}>
              {entries.map((e) => (
                <li key={e.path} className={e.readable || e.is_dir ? undefined : "dim"}>
                  <span className="mono">{e.is_dir ? `${e.path}/` : e.path}</span>
                  {!e.is_dir && <span className="muted">{kib(e.size_bytes)}</span>}
                </li>
              ))}
            </ul>
          </>
        ))}
      <ErrorLine error={error} />
    </Panel>
  );
}
