// The Permissions page (ADR-POLICY-001): every tool the agent can use, what
// it does and sends, and what you may change within the fixed limits.
// Turning a tool off or letting fewer changes run without asking is saved
// from here; turning one back on or letting more run is confirmed in a
// Windows dialog the app shows itself.

import { useEffect, useState } from "react";
import type { PermissionView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { ago } from "../model/format.ts";
import { loosens, permissionText } from "../model/permissions.ts";
import { useNow } from "../state/store";
import { ErrorLine, Panel } from "../ui";
import { LockIcon } from "../icons";

function Row({ p, onChanged }: { p: PermissionView; onChanged: (all: PermissionView[]) => void }) {
  const now = useNow(30_000);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [count, setCount] = useState(String(p.max_auto ?? ""));

  useEffect(() => setCount(String(p.max_auto ?? "")), [p.max_auto]);

  async function save(enabled: boolean, maxAuto: number | null) {
    setBusy(true);
    setError(null);
    try {
      const all = loosens(p, enabled, maxAuto)
        ? await api.relaxPermission(p.tool_id, enabled, maxAuto)
        : await api.setPermission(p.tool_id, enabled, maxAuto);
      if (all) onChanged(all);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  const number = Number(count);
  const validCount = count.trim() !== "" && Number.isInteger(number) && number >= 0 && number <= 25;
  return (
    <li className="permission">
      <div className="permission-head">
        <label className="check">
          <input
            type="checkbox"
            checked={p.enabled}
            disabled={busy}
            onChange={(e) => void save(e.currentTarget.checked, p.max_auto)}
          />
          <strong>{p.label}</strong>
        </label>
        {p.changed_at_ms !== null && <span className="muted small">Changed {ago(p.changed_at_ms, now)}</span>}
      </div>
      <p className="small">{permissionText(p)}</p>
      <p className="muted small">Sends: {p.sends}</p>
      {p.max_auto !== null && p.enabled && (
        <div className="permission-count">
          <label>
            Changes per reply without asking (0 to 25)
            <input
              type="number"
              min={0}
              max={25}
              value={count}
              disabled={busy}
              onChange={(e) => setCount(e.currentTarget.value)}
            />
          </label>
          <button
            type="button"
            className="secondary"
            disabled={busy || !validCount || number === p.max_auto}
            onClick={() => void save(true, number)}
          >
            Save number
          </button>
          {p.default_max_auto !== null && <span className="muted small">Default {p.default_max_auto}</span>}
        </div>
      )}
      <ErrorLine error={error} />
    </li>
  );
}

export function PermissionsPanel() {
  const [all, setAll] = useState<PermissionView[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .listPermissions()
      .then(setAll)
      .catch((err) => setError(describeError(err)));
  }, []);

  return (
    <Panel title="Permissions" icon={<LockIcon size={15} />} id="permissions">
      <p className="muted small">
        What Nexus may do on its own. You can turn any tool off. Turning one back on, or letting more changes run without
        asking, is confirmed in a Windows dialog. Anything outside Nexus is always asked for.
      </p>
      {all === null ? (
        <p className="muted">Loading…</p>
      ) : (
        <ul className="permissions" aria-label="Permissions">
          {all.map((p) => (
            <Row key={p.tool_id} p={p} onChanged={setAll} />
          ))}
        </ul>
      )}
      <ErrorLine error={error} />
    </Panel>
  );
}
