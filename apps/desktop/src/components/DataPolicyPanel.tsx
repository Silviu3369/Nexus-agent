// Data sent to AI providers (ADR-PROVIDER-002): what each connection may
// receive and why, and each source's class. Keeping more data in is saved
// from here; letting more leave is confirmed in a Windows dialog the app
// shows itself.

import { useEffect, useState } from "react";
import type { DataChange, DataPolicyView } from "@nexus/contracts";
import { api, describeError } from "../api";
import { letsMoreLeave, offersConsent, receiveText } from "../model/data.ts";
import { useNexus } from "../state/store";
import { ErrorLine, Panel } from "../ui";
import { LockIcon } from "../icons";

const KIND_TEXT: Record<string, string> = { Folder: "Shared folder", Register: "Register", McpServer: "MCP server" };

export function DataPolicyPanel() {
  // Read again when a connection, folder or MCP server comes or goes.
  const { connections, folders, mcp } = useNexus();
  const [policy, setPolicy] = useState<DataPolicyView | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .getDataPolicy()
      .then(setPolicy)
      .catch((err) => setError(describeError(err)));
  }, [connections, folders, mcp]);

  async function apply(change: DataChange) {
    if (!policy) return;
    setBusy(true);
    setError(null);
    try {
      const next = letsMoreLeave(policy, change) ? await api.relaxDataPolicy(change) : await api.setDataPolicy(change);
      if (next) setPolicy(next);
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title="Data sent to AI providers" icon={<LockIcon size={15} />} id="data-policy">
      {policy === null ? (
        <p className="muted">Loading…</p>
      ) : (
        <>
          <label className="check">
            <input
              type="checkbox"
              checked={policy.in_eea}
              disabled={busy}
              onChange={(e) => void apply({ InEea: { in_eea: e.currentTarget.checked } })}
            />
            I use Nexus from the European Economic Area, Switzerland or the UK
          </label>
          <ul className="data-connections" aria-label="What each connection may receive">
            {policy.connections.map((c) => (
              <li key={c.connection_id}>
                <strong>{c.display_name}</strong> · {c.provider}
                {c.free_tier ? " (free tier)" : ""}
                <p className="small">Can receive: {receiveText(c.can_receive)}</p>
                <p className="muted small">{c.reason}</p>
                {c.terms_url && <p className="muted small mono">Terms: {c.terms_url}</p>}
                {c.dpa === "Offered" && (
                  <label className="check small">
                    <input
                      type="checkbox"
                      checked={c.dpa_signed}
                      disabled={busy}
                      onChange={(e) =>
                        void apply({
                          Connection: { connection_id: c.connection_id, dpa_signed: e.currentTarget.checked, allows_training: c.allows_training },
                        })
                      }
                    />
                    I signed {c.provider}'s data processing addendum
                  </label>
                )}
                {offersConsent(c) && (
                  <label className="check small">
                    <input
                      type="checkbox"
                      checked={c.allows_training}
                      disabled={busy}
                      onChange={(e) =>
                        void apply({
                          Connection: { connection_id: c.connection_id, dpa_signed: c.dpa_signed, allows_training: e.currentTarget.checked },
                        })
                      }
                    />
                    Let {c.provider} use my own data to improve its models
                  </label>
                )}
              </li>
            ))}
          </ul>
          {policy.sources.length > 0 && (
            <table className="table" aria-label="Whose data each source holds">
              <thead>
                <tr>
                  <th>Source</th>
                  <th>Kind</th>
                  <th>Holds</th>
                </tr>
              </thead>
              <tbody>
                {policy.sources.map((s) => (
                  <tr key={`${s.kind}:${s.id}`}>
                    <td>{s.name}</td>
                    <td className="muted">{KIND_TEXT[s.kind] ?? s.kind}</td>
                    <td>
                      <select
                        aria-label={`Whose data ${s.name} holds`}
                        value={s.data_class}
                        disabled={busy}
                        onChange={(e) => void apply({ Source: { kind: s.kind, id: s.id, data_class: e.currentTarget.value } })}
                      >
                        <option value="Personal">Your own data</option>
                        <option value="Business">Other people's data</option>
                      </select>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          <p className="muted small">
            Providers' terms checked {policy.terms_checked}. Letting more data leave is confirmed in a Windows dialog; keys never
            leave.
          </p>
        </>
      )}
      <ErrorLine error={error} />
    </Panel>
  );
}
