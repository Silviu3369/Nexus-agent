// Jobs: the applications the user saved or sent, kept by Nexus in its job
// applications register. The agent adds and updates them when the user
// tells it in a task (or pastes a job ad link); every change it makes shows
// in that task and here, and can be undone. The user adds, edits and
// deletes them here too. Every value is checked by the Host.

import { useCallback, useEffect, useMemo, useState } from "react";
import type { FieldDef, RecordDetailView, RecordView, RegisterView } from "@nexus/contracts";
import { api, describeError, errorCode } from "../api";
import { dateTimeText } from "../model/automation.ts";
import {
  changeLines,
  changeText,
  dateText,
  filterRecords,
  formValues,
  missingRequired,
  recordTitle,
  statusCounts,
  statusOf,
  statusTone,
  valueText,
} from "../model/records.ts";
import type { RecordFilterState } from "../model/records.ts";
import { useGoTo } from "../shell/nav";
import { useDerived, useNexus, useNexusActions } from "../state/store";
import { Empty, ErrorLine, Panel, StatusPill } from "../ui";
import { BriefcaseIcon, PlusIcon } from "../icons";

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
  const [selected, setSelected] = useState<string | null>(initialRecordId);
  const [adding, setAdding] = useState(false);
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

  const shown = useMemo(() => (records ? filterRecords(register, records, filter) : []), [register, records, filter]);
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
          <button
            type="button"
            className="ghost"
            disabled={adding}
            onClick={() => {
              setAdding(true);
              setSelected(null);
            }}
          >
            <PlusIcon size={14} /> New {item}
          </button>
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
  const meta = [applied ? `Applied ${dateText(applied)}` : null, record.values.source ?? null, record.values.location ?? null]
    .filter(Boolean)
    .join(" · ");
  return (
    <button type="button" className={`record-row${selected ? " record-row-on" : ""}`} aria-current={selected} aria-label={title} onClick={onSelect}>
      <span className="record-row-main">
        <span className="record-row-title">{title}</span>
        {meta && <span className="record-row-meta">{meta}</span>}
      </span>
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
