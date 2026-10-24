// Jobs: the applications the user saved or sent, kept by Nexus in its job
// applications register. The agent adds and updates them when the user
// tells it in a task (or pastes a job ad link); every change it makes shows
// in that task and here, and can be undone. The user adds, edits and
// deletes them here too. Every value is checked by the Host.

import { useCallback, useEffect, useMemo, useState } from "react";
import type {
  FieldDef,
  FolderEntryView,
  ImportPreviewView,
  ImportResultView,
  InboxView,
  JobResultsView,
  RecordDetailView,
  RecordView,
  RegisterView,
  WeekCountView,
} from "@nexus/contracts";
import { api, describeError, errorCode } from "../api";
import { dateTimeText } from "../model/automation.ts";
import {
  bandText,
  causeText,
  changeLines,
  changeText,
  dateText,
  durationText,
  filterRecords,
  formValues,
  groupText,
  matchText,
  missingRequired,
  percentText,
  recordTitle,
  statusCounts,
  statusOf,
  sortRecords,
  statusTone,
  usdText,
  valueText,
  weekText,
} from "../model/records.ts";
import type { RecordFilterState, RecordSort } from "../model/records.ts";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNexusActions } from "../state/store";
import { Empty, ErrorLine, Panel, StatusPill } from "../ui";
import { BriefcaseIcon, FolderIcon, PlusIcon, PulseIcon } from "../icons";

const REGISTER = "job_applications";
const LIST_LIMIT = 500;

export function JobsView({ initialRecordId }: { initialRecordId: string | null }) {
  const d = useNexus();
  if (d.registers === null) return <Empty>Loading…</Empty>;
  const register = d.registers.find((r) => r.key === REGISTER);
  if (!register) return <Empty>This Host has no job applications register.</Empty>;
  return <RegisterScreen register={register} initialRecordId={initialRecordId} />;
}

function RegisterScreen({ register, initialRecordId }: { register: RegisterView; initialRecordId: string | null }) {
  const d = useNexus();
  const [records, setRecords] = useState<RecordView[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [filter, setFilter] = useState<RecordFilterState>({ status: null, dueOnly: false, text: "" });
  const [sort, setSort] = useState<RecordSort>("changed");
  const [selected, setSelected] = useState<string | null>(initialRecordId);
  const [adding, setAdding] = useState(false);
  const [side, setSide] = useState<"results" | "import" | "export" | "inbox" | null>(null);
  const deselect = useCallback(() => setSelected(null), []);

  useEffect(() => {
    let alive = true;
    api
      .listRecords({ register_id: register.register_id, statuses: [], text: null, follow_up_due: false, limit: LIST_LIMIT })
      .then((r) => {
        if (!alive) return;
        setRecords(r);
        setError(null);
      })
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [register.register_id, d.recordsRev]);

  const shown = useMemo(
    () => (records ? sortRecords(register, filterRecords(register, records, filter), sort) : []),
    [register, records, filter, sort],
  );
  const counts = useMemo(() => (records ? statusCounts(register, records) : []), [register, records]);
  const due = records?.filter((r) => r.follow_up_due).length ?? 0;
  const item = register.item_name;

  return (
    <div className="records">
      <Panel
        title={register.name}
        icon={<BriefcaseIcon size={15} />}
        id="register"
        actions={
          <>
            <button type="button" className="ghost" aria-pressed={side === "results"} onClick={() => setSide(side === "results" ? null : "results")}>
              Results
            </button>
            <button type="button" className="ghost" aria-pressed={side === "inbox"} onClick={() => setSide(side === "inbox" ? null : "inbox")}>
              Inbox
            </button>
            <button type="button" className="ghost" aria-pressed={side === "import"} onClick={() => setSide(side === "import" ? null : "import")}>
              Import
            </button>
            <button type="button" className="ghost" aria-pressed={side === "export"} onClick={() => setSide(side === "export" ? null : "export")}>
              Export
            </button>
            <button
              type="button"
              className="ghost"
              disabled={adding}
              aria-label={`New ${item}`}
              onClick={() => {
                setAdding(true);
                setSelected(null);
                setSide(null);
              }}
            >
              <PlusIcon size={14} /> New
            </button>
          </>
        }
      >
        <div className="record-filters">
          <div className="record-chips" role="group" aria-label="Status">
            <button
              type="button"
              className={`chip${filter.status === null && !filter.dueOnly ? " chip-on" : ""}`}
              aria-pressed={filter.status === null && !filter.dueOnly}
              onClick={() => setFilter({ ...filter, status: null, dueOnly: false })}
            >
              All {records?.length ?? 0}
            </button>
            {counts.map(([status, n]) => (
              <button
                key={status}
                type="button"
                className={`chip${filter.status === status ? " chip-on" : ""}${n === 0 ? " chip-empty" : ""}`}
                aria-pressed={filter.status === status}
                onClick={() => setFilter({ ...filter, status, dueOnly: false })}
              >
                {status} {n}
              </button>
            ))}
          </div>
          {register.follow_up_field && (
            <button
              type="button"
              className={`chip${filter.dueOnly ? " chip-on" : ""}${due > 0 ? " chip-attention" : ""}`}
              aria-pressed={filter.dueOnly}
              onClick={() => setFilter({ ...filter, dueOnly: !filter.dueOnly, status: null })}
            >
              Follow-up due {due}
            </button>
          )}
          <select className="record-sort" aria-label="Order" value={sort} onChange={(e) => setSort(e.currentTarget.value as RecordSort)}>
            <option value="changed">Last changed</option>
            <option value="match">Best match</option>
            <option value="follow_up">Follow-up date</option>
          </select>
          <input
            className="record-search"
            type="search"
            value={filter.text}
            aria-label={`Search ${register.name.toLowerCase()}`}
            placeholder="Search company, role, notes…"
            onChange={(e) => setFilter({ ...filter, text: e.currentTarget.value })}
          />
        </div>
        <ErrorLine error={error} />
        {records === null ? (
          <Empty>Loading…</Empty>
        ) : records.length === 0 ? (
          <Empty>
            {`No ${item}s yet. Tell Nexus in a task, for example “I applied today at Acme for Service Desk Analyst on their site”, or paste a job ad link — or add one here.`}
          </Empty>
        ) : shown.length === 0 ? (
          <Empty>Nothing matches this filter.</Empty>
        ) : (
          <ul className="record-list" aria-label={register.name}>
            {shown.map((r) => (
              <li key={r.record_id}>
                <RecordRow
                  register={register}
                  record={r}
                  selected={r.record_id === selected}
                  onSelect={() => {
                    setSelected(r.record_id);
                    setAdding(false);
                  }}
                />
              </li>
            ))}
          </ul>
        )}
        <p className="muted record-hint">
          Nexus adds and updates these when you tell it in a task, or when you give it a job ad link. Each change it makes is
          shown in that task and in the history here, and can be undone.
        </p>
      </Panel>
      <div className="records-side">
        {side === "results" && <ResultsPanel register={register} onClose={() => setSide(null)} />}
        {side === "inbox" && <InboxPanel register={register} onClose={() => setSide(null)} />}
        {side === "export" && <ExportPanel register={register} onClose={() => setSide(null)} />}
        {side === "import" && <ImportPanel register={register} onClose={() => setSide(null)} />}
        {adding ? (
          <Panel title={`New ${item}`} icon={<PlusIcon size={15} />} id="record-new">
            <RecordForm
              register={register}
              record={null}
              onDone={(id) => {
                setAdding(false);
                if (id) setSelected(id);
              }}
            />
          </Panel>
        ) : selected ? (
          <RecordDetail key={selected} register={register} recordId={selected} onGone={deselect} />
        ) : (
          <Panel title="What Nexus keeps here" icon={<BriefcaseIcon size={15} />} id="record-help">
            <ul className="bullets">
              <li>Each {item}: company, role, status, when you applied, when to follow up, the link, contact and notes.</li>
              <li>Ask in a task: “Where did I apply this month?”, “Which ones haven’t answered in two weeks?” — the answer comes from this list.</li>
              <li>Follow-up dates that are due are counted above; an automation can remind you every morning.</li>
              <li>The model you use sees the {item}s it looks up, like the rest of the task.</li>
            </ul>
          </Panel>
        )}
      </div>
    </div>
  );
}

function RecordRow({
  register,
  record,
  selected,
  onSelect,
}: {
  register: RegisterView;
  record: RecordView;
  selected: boolean;
  onSelect: () => void;
}) {
  const status = statusOf(register, record);
  const title = recordTitle(register, record.values);
  const applied = record.values.applied_on;
  const followUp = register.follow_up_field ? record.values[register.follow_up_field] : undefined;
  const meta = [
    applied ? `Applied ${dateText(applied)}` : null,
    record.values.reference ? `Ref ${record.values.reference}` : null,
    record.values.source ?? null,
    record.values.location ?? null,
  ]
    .filter(Boolean)
    .join(" · ");
  return (
    <button type="button" className={`record-row${selected ? " record-row-on" : ""}`} aria-current={selected} aria-label={title} onClick={onSelect}>
      <span className="record-row-main">
        <span className="record-row-title">{title}</span>
        {meta && <span className="record-row-meta">{meta}</span>}
      </span>
      {matchText(record) && <span className={`record-match${Number(record.values.match_score) >= 70 ? " record-match-good" : ""}`}>{matchText(record)}</span>}
      {followUp && (
        <span className={`record-follow${record.follow_up_due ? " record-follow-due" : ""}`}>
          {record.follow_up_due ? "Follow up due" : "Follow up"} {dateText(followUp)}
        </span>
      )}
      <StatusPill status={statusTone(register, status)} label={status ?? "—"} />
    </button>
  );
}

function RecordDetail({ register, recordId, onGone }: { register: RegisterView; recordId: string; onGone: () => void }) {
  const d = useNexus();
  const { refresh } = useNexusActions();
  const { agentName } = useDerived();
  const goTo = useGoTo();
  const [detail, setDetail] = useState<RecordDetailView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [editing, setEditing] = useState(false);
  const [confirm, setConfirm] = useState(false);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    let alive = true;
    api
      .recordDetail(recordId)
      .then((r) => {
        if (!alive) return;
        setDetail(r);
        setError(null);
      })
      .catch((err) => {
        if (!alive) return;
        if (errorCode(err) === "NotFound") onGone();
        else setError(describeError(err));
      });
    return () => {
      alive = false;
    };
  }, [recordId, d.recordsRev, onGone]);

  async function act(work: () => Promise<unknown>) {
    setBusy(true);
    setError(null);
    try {
      await work();
      await refresh(["registers", "steps", "focus"]);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  if (!detail) {
    return (
      <Panel title={register.name} icon={<BriefcaseIcon size={15} />} id="record">
        {error ? <ErrorLine error={error} /> : <Empty>Loading…</Empty>}
      </Panel>
    );
  }
  const record = detail.record;
  const title = recordTitle(register, record.values);
  const status = statusOf(register, record);

  if (editing) {
    return (
      <Panel title={`Edit ${title}`} icon={<BriefcaseIcon size={15} />} id="record-edit">
        <RecordForm register={register} record={record} onDone={() => setEditing(false)} />
      </Panel>
    );
  }

  return (
    <Panel
      title={title}
      icon={<BriefcaseIcon size={15} />}
      id="record"
      className="record-detail"
      actions={
        <>
          <button type="button" className="ghost" disabled={busy} onClick={() => setEditing(true)}>
            Edit
          </button>
          <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(true)}>
            Delete
          </button>
        </>
      }
    >
      {confirm && (
        <div className="confirm-bar" role="alert">
          <p>{`Delete “${title}” with its history? This cannot be undone.`}</p>
          <div className="actions">
            <button
              type="button"
              className="danger"
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  await api.deleteRecord(record.record_id);
                  onGone();
                })
              }
            >
              Delete for good
            </button>
            <button type="button" className="ghost" disabled={busy} onClick={() => setConfirm(false)}>
              Keep
            </button>
          </div>
        </div>
      )}
      <div className="record-badges">
        <StatusPill status={statusTone(register, status)} label={status ?? "—"} />
        {record.follow_up_due && <span className="badge badge-attention">Follow-up due</span>}
        <span className="muted">{record.created_by === "Agent" ? `Added by ${agentName}` : "Added by you"}</span>
      </div>
      <dl className="kv">
        {register.fields
          .filter((f) => f.key !== register.status_field && record.values[f.key])
          .map((f) => (
            <div className="kv-row" key={f.key}>
              <dt>{f.label}</dt>
              <dd className={f.kind === "LongText" ? "record-long" : f.kind === "Url" ? "mono record-url" : undefined}>
                {valueText(f, record.values[f.key])}
              </dd>
            </div>
          ))}
      </dl>
      <h3 className="record-history-head">History</h3>
      <ol className="record-history" aria-label={`History of ${title}`}>
        {detail.history.map((c) => (
          <li key={c.change_id} className={c.undone ? "dim" : undefined}>
            <div className="record-change-head">
              <span className="record-change-what">{changeText(register, c, agentName)}</span>
              <span className="muted">{dateTimeText(c.at_ms)}</span>
              {c.undone && <span className="badge badge-neutral">undone</span>}
            </div>
            {changeLines(register, c).map((line) => (
              <p key={line} className="record-change-line">
                {line}
              </p>
            ))}
            {c.read_hosts.length > 0 && <p className="muted record-change-line">{`after reading ${c.read_hosts.join(", ")}`}</p>}
            <div className="record-change-actions">
              {c.conversation_id && (
                <button type="button" className="link" onClick={() => goTo({ section: "tasks", taskId: c.conversation_id })}>
                  Open task
                </button>
              )}
              {c.can_undo && (
                <button
                  type="button"
                  className="link"
                  disabled={busy}
                  aria-label={`Undo: ${changeText(register, c, agentName)}`}
                  onClick={() => void act(() => api.undoRecordChange(c.change_id))}
                >
                  Undo
                </button>
              )}
            </div>
          </li>
        ))}
      </ol>
      <ErrorLine error={error} />
    </Panel>
  );
}

function FieldInput({ field, value, onChange }: { field: FieldDef; value: string; onChange: (v: string) => void }) {
  const label = `${field.label}${field.required ? "" : " (optional)"}`;
  switch (field.kind) {
    case "LongText":
      return (
        <label>
          {label}
          <textarea value={value} rows={3} maxLength={field.max_chars} onChange={(e) => onChange(e.currentTarget.value)} />
        </label>
      );
    case "Number":
      return (
        <label className="record-date">
          {label}
          <input
            type="number"
            value={value}
            min={field.min ?? undefined}
            max={field.max ?? undefined}
            step={1}
            onChange={(e) => onChange(e.currentTarget.value)}
          />
        </label>
      );
    case "Date":
      return (
        <label className="record-date">
          {label}
          <input type="date" value={value} onChange={(e) => onChange(e.currentTarget.value)} />
        </label>
      );
    case "Choice":
      return (
        <label>
          {label}
          <select value={value} onChange={(e) => onChange(e.currentTarget.value)}>
            {!field.required && <option value="">—</option>}
            {field.choices.map((c) => (
              <option key={c} value={c}>
                {c}
              </option>
            ))}
          </select>
        </label>
      );
    default:
      return (
        <label>
          {label}
          <input
            value={value}
            maxLength={field.max_chars}
            spellCheck={field.kind !== "Url"}
            placeholder={field.kind === "Url" ? "https://…" : undefined}
            onChange={(e) => onChange(e.currentTarget.value)}
          />
        </label>
      );
  }
}

function RecordForm({
  register,
  record,
  onDone,
}: {
  register: RegisterView;
  record: RecordView | null;
  onDone: (savedId: string | null) => void;
}) {
  const { refresh } = useNexusActions();
  const [values, setValues] = useState<Record<string, string>>(() => formValues(register, record));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const missing = missingRequired(register, values);

  async function save() {
    setBusy(true);
    setError(null);
    try {
      const saved = await api.saveRecord({
        register_id: register.register_id,
        record_id: record?.record_id ?? null,
        revision: record?.revision ?? 0,
        values,
      });
      await refresh(["registers"]);
      onDone(saved.record_id);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="stack record-form"
      aria-label={record ? `Edit ${recordTitle(register, record.values)}` : `New ${register.item_name}`}
      onSubmit={(e) => {
        e.preventDefault();
        if (missing.length === 0) void save();
      }}
    >
      {register.fields.map((f) => (
        <FieldInput key={f.key} field={f} value={values[f.key] ?? ""} onChange={(v) => setValues({ ...values, [f.key]: v })} />
      ))}
      {missing.length > 0 && <p className="muted">{`Still needed: ${missing.join(", ")}.`}</p>}
      <div className="actions">
        <button type="submit" className="primary" disabled={busy || missing.length > 0}>
          {record ? "Save" : `Add ${register.item_name}`}
        </button>
        <button type="button" className="ghost" disabled={busy} onClick={() => onDone(null)}>
          Cancel
        </button>
      </div>
      <ErrorLine error={error} />
    </form>
  );
}

const SPREADSHEET = /\.(xlsx|xlsm|xls|ods|csv|tsv)$/i;

/** No shared folder fits: say where to share one. */
function NoFolder({ text }: { text: string }) {
  const goTo = useGoTo();
  return (
    <>
      <p className="muted">{text}</p>
      <button type="button" className="ghost" onClick={() => goTo({ section: "tools", tool: "documents" })}>
        <FolderIcon size={14} /> Share a folder
      </button>
    </>
  );
}

function ExportPanel({ register, onClose }: { register: RegisterView; onClose: () => void }) {
  const d = useNexus();
  const folders = (d.folders ?? []).filter((f) => f.can_write && f.available);
  const [folder, setFolder] = useState<string>("");
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const chosen = folder || folders[0]?.folder_id || "";

  async function run() {
    setBusy(true);
    setError(null);
    try {
      const out = await api.exportRegister(register.register_id, chosen);
      setDone(`Saved ${out.rows} ${out.rows === 1 ? register.item_name : `${register.item_name}s`} to ${out.path}`);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Export to Excel" icon={<FolderIcon size={15} />} id="record-export">
      {folders.length === 0 ? (
        <NoFolder text="Exports go to a folder you shared with new files allowed. None is shared that way yet." />
      ) : (
        <div className="stack">
          <label>
            Folder
            <select value={chosen} onChange={(e) => setFolder(e.currentTarget.value)}>
              {folders.map((f) => (
                <option key={f.folder_id} value={f.folder_id}>
                  {f.label}
                </option>
              ))}
            </select>
          </label>
          <p className="muted">A new .xlsx file with every field; an existing file is never replaced.</p>
          <div className="actions">
            <button type="button" className="primary" disabled={busy || !chosen} onClick={() => void run()}>
              Export to Excel
            </button>
            <button type="button" className="ghost" onClick={onClose}>
              Close
            </button>
          </div>
          {done && (
            <p className="record-done" role="status">
              {done}
            </p>
          )}
          <ErrorLine error={error} />
        </div>
      )}
    </Panel>
  );
}

function ImportPanel({ register, onClose }: { register: RegisterView; onClose: () => void }) {
  const d = useNexus();
  const { refresh } = useNexusActions();
  const folders = (d.folders ?? []).filter((f) => f.available);
  const [folder, setFolder] = useState<string>("");
  const [files, setFiles] = useState<FolderEntryView[] | null>(null);
  const [file, setFile] = useState<string>("");
  const [preview, setPreview] = useState<ImportPreviewView | null>(null);
  const [mapping, setMapping] = useState<(string | null)[]>([]);
  const [result, setResult] = useState<ImportResultView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const chosen = folder || folders[0]?.folder_id || "";

  useEffect(() => {
    if (!chosen) return;
    let alive = true;
    setFiles(null);
    setFile("");
    setPreview(null);
    api
      .folderEntries(chosen)
      .then((e) => {
        if (!alive) return;
        const sheets = e.filter((x) => !x.is_dir && SPREADSHEET.test(x.path));
        setFiles(sheets);
        // One file to choose from: it is the one.
        if (sheets.length === 1) setFile(sheets[0].path);
      })
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [chosen]);

  async function work(task: () => Promise<void>) {
    setBusy(true);
    setError(null);
    try {
      await task();
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const loadPreview = () =>
    work(async () => {
      setResult(null);
      const p = await api.importPreview(register.register_id, chosen, file);
      setPreview(p);
      setMapping(p.mapping);
    });

  const runImport = () =>
    work(async () => {
      if (!preview) return;
      const r = await api.importRecords({ register_id: register.register_id, folder_id: chosen, path: file, mapping });
      setResult(r);
      setPreview(null);
      await refresh(["registers"]);
    });

  const used = mapping.filter(Boolean);
  const duplicate = used.length !== new Set(used).size;
  return (
    <Panel title="Import from Excel or CSV" icon={<FolderIcon size={15} />} id="record-import">
      {folders.length === 0 ? (
        <NoFolder text="Imports read a file in a folder you shared. None is shared yet." />
      ) : (
        <div className="stack">
          <label>
            Folder
            <select value={chosen} onChange={(e) => setFolder(e.currentTarget.value)}>
              {folders.map((f) => (
                <option key={f.folder_id} value={f.folder_id}>
                  {f.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            File
            <select value={file} onChange={(e) => setFile(e.currentTarget.value)} disabled={!files || files.length === 0}>
              <option value="">
                {files === null ? "Loading…" : files.length === 0 ? "No Excel or CSV file here" : "Choose a file"}
              </option>
              {(files ?? []).map((f) => (
                <option key={f.path} value={f.path}>
                  {f.path}
                </option>
              ))}
            </select>
          </label>
          <div className="actions">
            <button type="button" className="secondary" disabled={busy || !file} onClick={() => void loadPreview()}>
              Preview
            </button>
            <button type="button" className="ghost" onClick={onClose}>
              Close
            </button>
          </div>
          {preview && (
            <>
              <p className="muted">
                {`${preview.rows} rows in ${preview.file}${preview.truncated ? " (only the first ones are imported)" : ""}. Check where each column goes; a value that cannot be used as is goes to the notes.`}
              </p>
              <table className="import-map" aria-label="Columns">
                <thead>
                  <tr>
                    <th>Column in the file</th>
                    <th>Goes to</th>
                    <th>First value</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.columns.map((c, i) => (
                    <tr key={`${c}-${i}`}>
                      <td>{c || <em>no name</em>}</td>
                      <td>
                        <select
                          aria-label={`Field for ${c || `column ${i + 1}`}`}
                          value={mapping[i] ?? ""}
                          onChange={(e) => {
                            const next = [...mapping];
                            next[i] = e.currentTarget.value || null;
                            setMapping(next);
                          }}
                        >
                          <option value="">— not imported</option>
                          {register.fields.map((f) => (
                            <option key={f.key} value={f.key}>
                              {f.label}
                            </option>
                          ))}
                        </select>
                      </td>
                      <td className="muted">{preview.sample[0]?.[i] ?? ""}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
              {duplicate && <p className="error">Two columns go to the same field; choose one.</p>}
              <div className="actions">
                <button type="button" className="primary" disabled={busy || duplicate || used.length === 0} onClick={() => void runImport()}>
                  {`Import ${preview.rows} rows`}
                </button>
              </div>
            </>
          )}
          {result && (
            <div className="record-done" role="status">
              <p>
                {`Imported ${result.added} ${result.added === 1 ? register.item_name : `${register.item_name}s`}${result.adjusted > 0 ? ` · ${result.adjusted} with a value kept in the notes` : ""} · ${result.skipped.length} skipped`}
              </p>
              {result.skipped.length > 0 && (
                <ul className="bullets">
                  {result.skipped.slice(0, 20).map((sk) => (
                    <li key={sk.row}>{`Row ${sk.row}: ${sk.reason}`}</li>
                  ))}
                </ul>
              )}
            </div>
          )}
          <ErrorLine error={error} />
        </div>
      )}
    </Panel>
  );
}

const INBOX_LABEL: Record<InboxView["files"][number]["status"], string> = {
  Waiting: "Waiting",
  Started: "Sent to the agent",
  Done: "Done",
  Failed: "Not started",
  Skipped: "Skipped",
};

const INBOX_PILL: Record<InboxView["files"][number]["status"], "queued" | "running" | "completed" | "failed" | "cancelled"> = {
  Waiting: "queued",
  Started: "running",
  Done: "completed",
  Failed: "failed",
  Skipped: "cancelled",
};

/** The register's inbox: a shared folder whose new job ads the agent reads
 *  and records on its own, one task per file. */
function InboxPanel({ register, onClose }: { register: RegisterView; onClose: () => void }) {
  const d = useNexus();
  const goTo = useGoTo();
  const folders = (d.folders ?? []).filter((f) => f.available);
  const [inbox, setInbox] = useState<InboxView | null>(null);
  const [folder, setFolder] = useState<string>("");
  const [cvFolder, setCvFolder] = useState<string | null>(null);
  const [cvPath, setCvPath] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .getInbox(register.register_id)
      .then((v) => {
        if (!alive) return;
        setInbox(v);
        setError(null);
      })
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [register.register_id, d.recordsRev]);

  const chosen = folder || inbox?.folder_id || folders[0]?.folder_id || "";
  const cvFolderShown = cvFolder ?? inbox?.cv_folder_id ?? "";
  const cvPathShown = cvPath ?? inbox?.cv_path ?? "";

  async function save(enabled: boolean, cv?: { folder: string | null; path: string | null }) {
    if (!inbox) return;
    setBusy(true);
    setError(null);
    const keep = cv ?? { folder: inbox.cv_folder_id, path: inbox.cv_path };
    try {
      setInbox(
        await api.saveInbox({
          register_id: register.register_id,
          revision: inbox.revision,
          folder_id: chosen || inbox.folder_id,
          enabled,
          cv_folder_id: keep.folder,
          cv_path: keep.path,
        }),
      );
      setCvFolder(null);
      setCvPath(null);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  async function retry(fileId: number) {
    setBusy(true);
    setError(null);
    try {
      setInbox(await api.retryInboxFile(register.register_id, fileId));
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Job ads inbox" icon={<FolderIcon size={15} />} id="record-inbox">
      {folders.length === 0 ? (
        <NoFolder text="The inbox reads a folder you shared. None is shared yet." />
      ) : inbox === null ? (
        error ? <ErrorLine error={error} /> : <Empty>Loading…</Empty>
      ) : (
        <div className="stack">
          <p className="muted">
            Put job ads (PDF, Word or text) in this folder. Each new one is read by Nexus and added as an application — with
            its reference number, platform and the rest — while Nexus.Host is running. Each file becomes a task you can
            open and undo; a file is read once; nothing in the folder is moved or changed. Best a folder of its own: every
            document in it except your CV is treated as an ad.
          </p>
          <label>
            Folder
            <select value={chosen} disabled={busy} onChange={(e) => setFolder(e.currentTarget.value)}>
              {folders.map((f) => (
                <option key={f.folder_id} value={f.folder_id}>
                  {f.label}
                </option>
              ))}
            </select>
          </label>
          <div className="actions">
            {inbox.enabled ? (
              <>
                <button type="button" className="secondary" disabled={busy} onClick={() => void save(false)}>
                  Turn inbox off
                </button>
                {chosen !== inbox.folder_id && (
                  <button type="button" className="primary" disabled={busy} onClick={() => void save(true)}>
                    Use this folder
                  </button>
                )}
              </>
            ) : (
              <button type="button" className="primary" disabled={busy || !chosen} onClick={() => void save(true)}>
                Turn inbox on
              </button>
            )}
            <button type="button" className="ghost" onClick={onClose}>
              Close
            </button>
          </div>
          <h3 className="record-history-head">CV for the match score</h3>
          <p className="muted">
            Name your CV and each new ad is compared with it: a match score (0–100) and what fits or is missing. Without a CV,
            ads are recorded without a score.
          </p>
          <label>
            CV folder
            <select value={cvFolderShown} disabled={busy} onChange={(e) => setCvFolder(e.currentTarget.value)}>
              <option value="">— no CV</option>
              {folders.map((f) => (
                <option key={f.folder_id} value={f.folder_id}>
                  {f.label}
                </option>
              ))}
            </select>
          </label>
          <label>
            CV file
            <input
              value={cvPathShown}
              spellCheck={false}
              disabled={busy || !cvFolderShown}
              placeholder="e.g. CV 2026.pdf"
              onChange={(e) => setCvPath(e.currentTarget.value)}
            />
          </label>
          <div className="actions">
            <button
              type="button"
              className="secondary"
              disabled={busy || (cvFolderShown !== "" && cvPathShown.trim() === "")}
              onClick={() =>
                void save(inbox.enabled, {
                  folder: cvFolderShown || null,
                  path: cvFolderShown ? cvPathShown.trim() : null,
                })
              }
            >
              Save CV
            </button>
          </div>
          {inbox.cv_path && (
            <p className="muted">{`Comparing with ${inbox.cv_folder_label ?? "?"}/${inbox.cv_path}`}</p>
          )}
          <p className={inbox.enabled ? "record-done" : "muted"} role="status">
            {inbox.enabled
              ? `On · reading “${inbox.folder_label ?? "?"}”${inbox.waiting > 0 ? ` · ${inbox.waiting} waiting` : ""}`
              : "Off: new files are not read"}
          </p>
          {inbox.files.length > 0 && (
            <ol className="automation-history" aria-label="Inbox files">
              <li className="automation-history-head">Files, newest first</li>
              {inbox.files.map((f) => (
                <li key={f.file_id}>
                  <span className="automation-run">
                    <StatusPill status={INBOX_PILL[f.status]} label={INBOX_LABEL[f.status]} />
                    <span className="mono">{f.path}</span>
                    {f.reason && <span className="muted">{f.reason}</span>}
                    {f.conversation_id && (
                      <button
                        type="button"
                        className="link"
                        onClick={() => goTo({ section: "tasks", taskId: f.conversation_id })}
                      >
                        Open task
                      </button>
                    )}
                    {f.status === "Failed" && (
                      <button type="button" className="link" disabled={busy} onClick={() => void retry(f.file_id)}>
                        Retry
                      </button>
                    )}
                  </span>
                </li>
              ))}
            </ol>
          )}
          <ErrorLine error={error} />
        </div>
      )}
    </Panel>
  );
}

/** What the applications led to, counted by the Host from them and their
 *  history; nothing here is estimated. */
function ResultsPanel({ register, onClose }: { register: RegisterView; onClose: () => void }) {
  const d = useNexus();
  const [results, setResults] = useState<JobResultsView | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api
      .jobResults(register.register_id)
      .then((v) => {
        if (!alive) return;
        setResults(v);
        setError(null);
      })
      .catch((err) => alive && setError(describeError(err)));
    return () => {
      alive = false;
    };
  }, [register.register_id, d.recordsRev]);

  const r = results;
  return (
    <Panel title="Results" icon={<PulseIcon size={15} />} id="record-results">
      {r === null ? (
        error ? <ErrorLine error={error} /> : <Empty>Loading…</Empty>
      ) : (
        <div className="stack">
          <p className="muted">Counted from your applications and their history; nothing is estimated.</p>
          <dl className="results-tiles">
            <div>
              <dt>Sent</dt>
              <dd>{r.sent}</dd>
            </div>
            <div>
              <dt>Answered</dt>
              <dd>
                {r.answered}
                <small>{percentText(r.answered, r.sent)}</small>
              </dd>
            </div>
            <div>
              <dt>Interviews</dt>
              <dd>
                {r.interviews}
                <small>{percentText(r.interviews, r.sent)}</small>
              </dd>
            </div>
            <div>
              <dt>Offers</dt>
              <dd>{r.offers}</dd>
            </div>
          </dl>
          <p>
            {`Waiting for an answer: ${r.waiting}`}
            {r.waiting_long > 0 && ` · ${r.waiting_long} for more than 14 days`}
            {r.median_days_to_answer !== null &&
              ` · usually answered in ${r.median_days_to_answer} ${r.median_days_to_answer === 1 ? "day" : "days"}`}
          </p>
          <WeeksChart weeks={r.weeks} undated={r.undated} />
          {r.sent > 0 && (
            <>
              <h3 className="record-history-head">Interviews by match score</h3>
              <table className="results-table">
                <tbody>
                  {r.by_score
                    .filter((b) => b.counts.sent > 0)
                    .map((b) => (
                      <tr key={b.band}>
                        <th scope="row">{bandText(b.band)}</th>
                        <td>{groupText(b.counts)}</td>
                      </tr>
                    ))}
                </tbody>
              </table>
              <h3 className="record-history-head">By source</h3>
              <table className="results-table">
                <tbody>
                  {r.by_source.map((s) => (
                    <tr key={s.source}>
                      <th scope="row">{s.source}</th>
                      <td>{groupText(s.counts)}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </>
          )}
          {r.ads.seen > 0 && (
            <>
              <h3 className="record-history-head">How Nexus handled the job ads</h3>
              <table className="results-table" aria-label="How Nexus handled the job ads">
                <tbody>
                  <tr>
                    <th scope="row">Finished</th>
                    <td>{`${r.ads.finished} of ${r.ads.seen}`}</td>
                  </tr>
                  <tr>
                    <th scope="row">Recorded an application</th>
                    <td>{`${r.ads.recorded} (${percentText(r.ads.recorded, r.ads.finished)})`}</td>
                  </tr>
                  <tr>
                    <th scope="row">Recorded nothing</th>
                    <td>{r.ads.nothing_recorded}</td>
                  </tr>
                  <tr>
                    <th scope="row">Accepted (kept 7 days)</th>
                    <td>{r.ads.accepted}</td>
                  </tr>
                  <tr>
                    <th scope="row">Still in review</th>
                    <td>{r.ads.in_review}</td>
                  </tr>
                  <tr>
                    <th scope="row">You rejected, undid or corrected</th>
                    <td>{r.ads.corrected}</td>
                  </tr>
                  {r.ads.failed > 0 && (
                    <tr>
                      <th scope="row">Failed</th>
                      <td>
                        {`${r.ads.failed}: ${r.ads.failure_causes.map((c) => `${causeText(c.cause)} (${c.count})`).join(", ")}`}
                      </td>
                    </tr>
                  )}
                  {r.ads.waiting > 0 && (
                    <tr>
                      <th scope="row">Waiting or in progress</th>
                      <td>{r.ads.waiting}</td>
                    </tr>
                  )}
                  <tr>
                    <th scope="row">Usually done in</th>
                    <td>{r.ads.median_seconds === null ? "—" : durationText(r.ads.median_seconds)}</td>
                  </tr>
                  <tr>
                    <th scope="row">Tokens used</th>
                    <td>{(r.ads.prompt_tokens + r.ads.output_tokens).toLocaleString("en-US")}</td>
                  </tr>
                  <tr>
                    <th scope="row">Estimated cost</th>
                    <td>
                      {r.ads.estimated_cost_usd === null
                        ? "set the model's prices in Models"
                        : r.ads.finished > 0
                          ? `${usdText(r.ads.estimated_cost_usd)} · ${usdText(r.ads.estimated_cost_usd / r.ads.finished)} per ad`
                          : usdText(r.ads.estimated_cost_usd)}
                    </td>
                  </tr>
                </tbody>
              </table>
            </>
          )}
          <h3 className="record-history-head">What Nexus did</h3>
          <table className="results-table">
            <tbody>
              <tr>
                <th scope="row">Applications it added</th>
                <td>{r.added_by_nexus}</td>
              </tr>
              <tr>
                <th scope="row">Job ads it read from the inbox</th>
                <td>{r.ads_read}</td>
              </tr>
              <tr>
                <th scope="row">Changes it made</th>
                <td>
                  {r.nexus_changes > 0
                    ? `${r.nexus_changes} · you undid ${r.undone_changes} (${percentText(r.undone_changes, r.nexus_changes)})`
                    : "0"}
                </td>
              </tr>
              <tr>
                <th scope="row">Still to apply</th>
                <td>{r.to_apply}</td>
              </tr>
            </tbody>
          </table>
          {r.partial && <p className="muted">Only the newest 20,000 applications were counted.</p>}
          <div className="actions">
            <button type="button" className="ghost" onClick={onClose}>
              Close
            </button>
          </div>
        </div>
      )}
    </Panel>
  );
}

const CHART_HEIGHT = 80;

/** Applications sent in each of the last 8 weeks; a bar's value shows on
 *  hover and keyboard focus, and the busiest and current weeks carry it on
 *  their cap. */
function WeeksChart({ weeks, undated }: { weeks: WeekCountView[]; undated: number }) {
  const [hover, setHover] = useState<number | null>(null);
  if (weeks.length === 0) return null;
  const last = weeks.length - 1;
  const max = Math.max(1, ...weeks.map((w) => w.sent));
  const peak = weeks.reduce((best, w, i) => (w.sent > weeks[best].sent ? i : best), 0);
  const label = (i: number) =>
    `${i === last ? "This week" : `Week of ${weekText(weeks[i].week_start)}`}: ${weeks[i].sent} sent`;
  return (
    <figure className="results-chart">
      <figcaption className="record-history-head">Sent per week</figcaption>
      <ol className="results-weeks" aria-label="Applications sent per week" onMouseLeave={() => setHover(null)}>
        {weeks.map((w, i) => (
          <li
            key={w.week_start}
            tabIndex={0}
            aria-label={label(i)}
            className={hover === i ? "active" : undefined}
            onMouseEnter={() => setHover(i)}
            onFocus={() => setHover(i)}
            onBlur={() => setHover(null)}
          >
            {(i === peak || i === last) && w.sent > 0 && (
              <span className="results-cap" aria-hidden="true">
                {w.sent}
              </span>
            )}
            <span className="results-bar" aria-hidden="true" style={{ height: Math.round((CHART_HEIGHT * w.sent) / max) }} />
          </li>
        ))}
      </ol>
      <div className="results-axis" aria-hidden="true">
        <span>{weekText(weeks[0].week_start)}</span>
        <span>This week</span>
      </div>
      <p className="results-readout" aria-hidden="true">
        {label(hover ?? last)}
      </p>
      {undated > 0 && <p className="muted">{`${undated} sent without a date ${undated === 1 ? "is" : "are"} not in the weeks.`}</p>}
    </figure>
  );
}
