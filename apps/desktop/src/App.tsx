import { useState } from "react";
import type { HostStatus } from "@nexus/contracts";
import { api, describeError } from "./api";
import { uptime } from "./model/format.ts";
import { Shell } from "./shell/Shell";
import { useThemeEffect } from "./state/prefs";
import { NexusProvider, useNexus, useNexusActions } from "./state/store";
import { HexIcon } from "./icons";
import "./App.css";

function App() {
  useThemeEffect();
  return (
    <NexusProvider>
      <Root />
    </NexusProvider>
  );
}

function Root() {
  const d = useNexus();
  if (d.host?.state === "Connected") return <Shell host={d.host} />;
  return <Gate host={d.host} bridgeError={d.bridgeError} />;
}

/** Before a Host connection exists: what is wrong, and the one action that
 *  can fix it from here. */
function Gate({ host, bridgeError }: { host: HostStatus | null; bridgeError: string | null }) {
  const { checkHost } = useNexusActions();
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);

  async function startHost() {
    setStarting(true);
    setStartError(null);
    try {
      await api.startHost();
      window.setTimeout(() => void checkHost(), 500);
    } catch (err) {
      setStartError(describeError(err));
    } finally {
      setStarting(false);
    }
  }

  const canRetry =
    bridgeError !== null || host?.state === "Unreachable" || host?.state === "Rejected" || host?.state === "Untrusted";

  return (
    <main className="gate">
      <header className="gate-brand">
        <HexIcon size={22} className="rail-mark" />
        <h1>NEXUS</h1>
      </header>
      <section className="panel gate-panel" aria-labelledby="host-heading">
        <header className="panel-head">
          <h2 id="host-heading">Host</h2>
        </header>
        <div className="panel-body">
          <div role="status" aria-live="polite">
            <HostView host={host} bridgeError={bridgeError} />
          </div>
          <div className="actions">
            {host?.state === "NotRunning" && (
              <button type="button" className="primary" onClick={startHost} disabled={starting}>
                {starting ? "Starting…" : "Start Host"}
              </button>
            )}
            {canRetry && (
              <button type="button" className="secondary" onClick={() => void checkHost()}>
                Retry
              </button>
            )}
          </div>
          {startError && (
            <p className="error" role="alert">
              The Host could not be started: {startError}
            </p>
          )}
        </div>
      </section>
      <p className="muted small">Closing this window does not stop the Host or any accepted tasks.</p>
    </main>
  );
}

function HostView({ host, bridgeError }: { host: HostStatus | null; bridgeError: string | null }) {
  if (bridgeError) {
    return (
      <>
        <Badge tone="bad">Error</Badge>
        <p className="error">The bridge to the Host failed: {bridgeError}</p>
      </>
    );
  }
  if (!host) return <p className="muted">Checking the Host…</p>;
  switch (host.state) {
    case "Connected":
      return (
        <>
          <Badge tone="good">Connected</Badge>
          <p className="muted">
            {host.host_identity} · v{host.protocol_version} · up {uptime(host.uptime_seconds)}
          </p>
        </>
      );
    case "NotRunning":
      return (
        <>
          <Badge tone="neutral">Stopped</Badge>
          <p>The Nexus Host is not running for this Windows account.</p>
        </>
      );
    case "Unreachable":
      return (
        <>
          <Badge tone="bad">Unreachable</Badge>
          <p className="error">{host.reason}</p>
        </>
      );
    case "Rejected":
      return (
        <>
          <Badge tone="bad">Rejected</Badge>
          <p className="error">
            The Host refused the connection ({host.error.code}): {host.error.message}
          </p>
        </>
      );
    case "Untrusted":
      return (
        <>
          <Badge tone="bad">Untrusted</Badge>
          <p className="error">
            The process holding the Nexus connection is not running under your Windows account, so your credentials were
            not sent.
          </p>
          <p className="muted">{host.reason}</p>
        </>
      );
  }
}

function Badge({ tone, children }: { tone: "good" | "bad" | "neutral"; children: string }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export default App;
