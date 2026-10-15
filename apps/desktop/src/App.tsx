import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { HostStatus } from "@nexus/contracts";
import "./App.css";

const POLL_MS = 3000;

type View =
  | { kind: "checking" }
  | { kind: "status"; status: HostStatus }
  | { kind: "bridgeError"; message: string };

function formatUptime(totalSeconds: number): string {
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  if (h > 0) return `${h} h ${m} min`;
  if (m > 0) return `${m} min ${s} s`;
  return `${s} s`;
}

function App() {
  const [view, setView] = useState<View>({ kind: "checking" });
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const status = await invoke<HostStatus>("host_status");
      setView({ kind: "status", status });
    } catch (err) {
      setView({ kind: "bridgeError", message: String(err) });
    } finally {
      inFlight.current = false;
    }
  }, []);

  useEffect(() => {
    void refresh();
    const timer = window.setInterval(() => {
      if (!document.hidden) void refresh();
    }, POLL_MS);
    return () => window.clearInterval(timer);
  }, [refresh]);

  async function startHost() {
    setStarting(true);
    setStartError(null);
    try {
      await invoke("start_host");
      window.setTimeout(() => void refresh(), 500);
    } catch (err) {
      setStartError(String(err));
    } finally {
      setStarting(false);
    }
  }

  return (
    <main className="shell">
      <header className="header">
        <h1>Nexus</h1>
        <p className="subtitle">Panou de control</p>
      </header>

      <section className="card" aria-labelledby="host-heading">
        <h2 id="host-heading">Host</h2>
        <div role="status" aria-live="polite">
          <HostView view={view} />
        </div>

        <div className="actions">
          {view.kind === "status" && view.status.state === "NotRunning" && (
            <button type="button" onClick={startHost} disabled={starting}>
              {starting ? "Se pornește…" : "Pornește Hostul"}
            </button>
          )}
          {(view.kind === "bridgeError" ||
            (view.kind === "status" &&
              (view.status.state === "Unreachable" || view.status.state === "Rejected"))) && (
            <button type="button" onClick={() => void refresh()}>
              Reîncearcă
            </button>
          )}
        </div>

        {startError && (
          <p className="error" role="alert">
            Hostul nu a putut fi pornit: {startError}
          </p>
        )}
      </section>

      <p className="note">
        Închiderea acestei ferestre nu oprește Hostul și nici sarcinile acceptate.
      </p>
    </main>
  );
}

function HostView({ view }: { view: View }) {
  if (view.kind === "checking") {
    return <p className="muted">Se verifică Hostul…</p>;
  }
  if (view.kind === "bridgeError") {
    return (
      <>
        <Badge tone="bad">Eroare</Badge>
        <p className="error">Puntea către Host a eșuat: {view.message}</p>
      </>
    );
  }

  const { status } = view;
  switch (status.state) {
    case "Connected":
      return (
        <>
          <Badge tone="good">Conectat</Badge>
          <dl className="facts">
            <dt>Identitate</dt>
            <dd>{status.host_identity}</dd>
            <dt>Protocol</dt>
            <dd>v{status.protocol_version}</dd>
            <dt>Rulează de</dt>
            <dd>{formatUptime(status.uptime_seconds)}</dd>
          </dl>
        </>
      );
    case "NotRunning":
      return (
        <>
          <Badge tone="neutral">Oprit</Badge>
          <p>Hostul Nexus nu rulează pentru acest utilizator Windows.</p>
        </>
      );
    case "Unreachable":
      return (
        <>
          <Badge tone="bad">Nu răspunde</Badge>
          <p className="error">{status.reason}</p>
        </>
      );
    case "Rejected":
      return (
        <>
          <Badge tone="bad">Refuzat</Badge>
          <p className="error">
            Hostul a refuzat conexiunea ({status.error.code}): {status.error.message}
          </p>
        </>
      );
  }
}

function Badge({ tone, children }: { tone: "good" | "bad" | "neutral"; children: string }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export default App;
