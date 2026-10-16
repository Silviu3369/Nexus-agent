// Models: which providers exist, which model does what and why, and how
// fast it has actually been — measured from the recorded model calls.

import { useMemo } from "react";
import { PROVIDERS } from "../model/catalog.ts";
import { modelStats } from "../model/execution.ts";
import { ago, seconds } from "../model/format.ts";
import { ConnectionsCard } from "../Settings";
import { useDerived, useNexus, useNow } from "../state/store";
import { Empty, Panel, StatusPill } from "../ui";
import { PlugIcon, PulseIcon, RouteIcon } from "../icons";

export function ModelsView() {
  const d = useNexus();
  const { active, ready } = useDerived();
  const now = useNow(5000);
  const stats = useMemo(() => modelStats(d.steps ?? []), [d.steps]);
  const connected = (d.connections ?? []).filter((c) => c.provider.toLowerCase() === "gemini");

  return (
    <div className="models">
      <div className="models-top">
        <Panel title="Providers" icon={<PlugIcon size={15} />} id="providers">
          <table className="table">
            <thead>
              <tr>
                <th>Provider</th>
                <th>Status</th>
                <th>Selected model</th>
                <th>Notes</th>
              </tr>
            </thead>
            <tbody>
              {PROVIDERS.map((p) => {
                const conn = p.id === "gemini" ? (connected.find((c) => c.active) ?? connected[0]) : undefined;
                const status = !p.adapter
                  ? <span className="pill pill-unknown">No adapter</span>
                  : !conn
                    ? <span className="pill pill-unknown">Not connected</span>
                    : !conn.has_key
                      ? <StatusPill status="failed" label="Key missing" />
                      : conn.active && conn.model
                        ? <StatusPill status="completed" label="Active" />
                        : <StatusPill status="waiting" label="Needs a model" />;
                return (
                  <tr key={p.id} className={p.adapter ? undefined : "dim"}>
                    <td>{p.name}</td>
                    <td>{status}</td>
                    <td className="mono">{conn?.model ?? "—"}</td>
                    <td className="muted">
                      {p.note}
                      {conn?.free_tier && <span className="badge badge-attention"> Free tier</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </Panel>

        <Panel title="Routing" icon={<RouteIcon size={15} />} id="routing">
          <table className="table">
            <thead>
              <tr>
                <th>Role</th>
                <th>Model</th>
                <th>Why</th>
              </tr>
            </thead>
            <tbody>
              <tr>
                <td>Answers, web reading, note proposals</td>
                <td className="mono">{ready && active ? active.model : "—"}</td>
                <td className="muted">
                  {ready && active
                    ? `Chosen by you on “${active.display_name}”, thinking ${active.thinking_level}.`
                    : "No model is chosen, so nothing is sent."}
                </td>
              </tr>
            </tbody>
          </table>
          <p className="muted small">
            Nexus does not route between models yet: every request uses the model you chose, and there is no fallback to
            another provider when it fails.
          </p>
        </Panel>
      </div>

      <Panel title="Measured latency" icon={<PulseIcon size={15} />} id="latency">
        {stats.length === 0 ? (
          <Empty>No model calls recorded yet.</Empty>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>Model</th>
                <th>Now</th>
                <th>Calls</th>
                <th>First response (median)</th>
                <th>Full call (median)</th>
                <th>Failed</th>
                <th>Last used</th>
              </tr>
            </thead>
            <tbody>
              {stats.map((s) => (
                <tr key={s.model}>
                  <td className="mono">{s.model}</td>
                  <td>{s.running ? <StatusPill status="running" label="Answering" /> : <span className="muted">idle</span>}</td>
                  <td>{s.calls}</td>
                  <td>{seconds(s.medianFirstByteMs) ?? "—"}</td>
                  <td>{seconds(s.medianDurationMs) ?? "—"}</td>
                  <td className={s.failures > 0 ? "bad" : undefined}>{s.failures}</td>
                  <td>{ago(s.lastUsedAt, now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <p className="muted small">From the last {d.steps?.length ?? 0} recorded steps. First response is mostly the model's own thinking time.</p>
      </Panel>

      <ConnectionsCard />
    </div>
  );
}
