import { useCallback, useEffect, useRef, useState } from "react";
import type { BackupSummary, HostStatus } from "@nexus/contracts";
import { api, describeError } from "./api";
import { NotesPanel } from "./Notes";
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
      const status = await api.hostStatus();
      setView({ kind: "status", status });
    } catch (err) {
      setView({ kind: "bridgeError", message: describeError(err) });
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
      await api.startHost();
      window.setTimeout(() => void refresh(), 500);
    } catch (err) {
      setStartError(describeError(err));
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
              (view.status.state === "Unreachable" ||
                view.status.state === "Rejected" ||
                view.status.state === "Untrusted"))) && (
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

      {view.kind === "status" && view.status.state === "Connected" && (
        <>
          <NotesPanel />
          <BackupCard />
        </>
      )}

      <p className="note">
        Închiderea acestei ferestre nu oprește Hostul și nici sarcinile acceptate.
      </p>
    </main>
  );
}

function BackupCard() {
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<BackupSummary | null>(null);
  const [error, setError] = useState<string | null>(null);

  async function backup() {
    setBusy(true);
    setError(null);
    try {
      setResult(await api.createBackup());
    } catch (err) {
      setError(describeError(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <section className="card" aria-labelledby="backup-heading">
      <h2 id="backup-heading">Date</h2>
      <p className="muted">
        Backup-ul copiază baza de date și notele verificate, cu un manifest de hash-uri. Se poate
        restaura doar într-un profil nou, unde nimic nu se execută automat.
      </p>
      <div className="actions">
        <button type="button" onClick={backup} disabled={busy}>
          {busy ? "Se face backup…" : "Fă backup"}
        </button>
      </div>
      {result && (
        <div role="status">
          <p>{`Backup creat: ${result.artifacts} ${result.artifacts === 1 ? "notă" : "note"}.`}</p>
          <p className="path">{result.path}</p>
          {result.unmatched_artifacts > 0 && (
            <p className="error">
              {result.unmatched_artifacts} notă/note fuseseră modificate în afara Nexus și au fost
              copiate așa cum erau.
            </p>
          )}
        </div>
      )}
      {error && (
        <p className="error" role="alert">
          Backup-ul a eșuat: {error}
        </p>
      )}
    </section>
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
          {status.paused && (
            <p className="error">
              Profil restaurat dintr-un backup: cererile se înregistrează, dar nimic nu se execută până
              nu verifici profilul și ștergi fișierul restored.json din el.
            </p>
          )}
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
    case "Untrusted":
      return (
        <>
          <Badge tone="bad">Neverificat</Badge>
          <p className="error">
            Procesul care deține conexiunea Nexus nu rulează sub contul tău Windows, așa că
            nu i-am trimis datele de acces.
          </p>
          <p className="muted">{status.reason}</p>
        </>
      );
  }
}

function Badge({ tone, children }: { tone: "good" | "bad" | "neutral"; children: string }) {
  return <span className={`badge badge-${tone}`}>{children}</span>;
}

export default App;
