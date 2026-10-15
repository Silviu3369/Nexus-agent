import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactElement } from "react";
import type { ConnectionView, ConversationId, HostStatus, RunId } from "@nexus/contracts";
import { api, describeError } from "./api";
import { ChatPanel } from "./Chat";
import { Dashboard } from "./Dashboard";
import { NotesPanel } from "./Notes";
import { BackupCard, ConnectionsCard, PersonaCard } from "./Settings";
import { ChatIcon, DashboardIcon, NotesIcon, PlugIcon, SettingsIcon, ShieldIcon } from "./icons";
import "./App.css";

const POLL_MS = 3000;

type Tab = "dashboard" | "chat" | "notes" | "settings";
const TABS: { id: Tab; label: string; icon: (props: { size?: number }) => ReactElement }[] = [
  { id: "dashboard", label: "Dashboard", icon: DashboardIcon },
  { id: "chat", label: "Chat", icon: ChatIcon },
  { id: "notes", label: "Notes", icon: NotesIcon },
  { id: "settings", label: "Settings", icon: SettingsIcon },
];

type View =
  | { kind: "checking" }
  | { kind: "status"; status: HostStatus }
  | { kind: "bridgeError"; message: string };

function formatUptime(totalSeconds: number): string {
  const h = Math.floor(totalSeconds / 3600);
  const m = Math.floor((totalSeconds % 3600) / 60);
  const s = totalSeconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

function Clock() {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 1000);
    return () => window.clearInterval(timer);
  }, []);
  return (
    <div className="clock">
      <span className="clock-time">{now.toLocaleTimeString("en-US", { hour: "2-digit", minute: "2-digit" })}</span>
      <span className="clock-date">{now.toLocaleDateString("en-US", { weekday: "short", month: "short", day: "numeric" })}</span>
    </div>
  );
}

function App() {
  const [view, setView] = useState<View>({ kind: "checking" });
  const [tab, setTab] = useState<Tab>("dashboard");
  const [connections, setConnections] = useState<ConnectionView[]>([]);
  const [pendingCount, setPendingCount] = useState(0);
  const [focusRunId, setFocusRunId] = useState<RunId | null>(null);
  const [focusConversationId, setFocusConversationId] = useState<ConversationId | null>(null);
  const [starting, setStarting] = useState(false);
  const [startError, setStartError] = useState<string | null>(null);
  const inFlight = useRef(false);

  const refresh = useCallback(async () => {
    if (inFlight.current) return;
    inFlight.current = true;
    try {
      const status = await api.hostStatus();
      setView({ kind: "status", status });
      if (status.state === "Connected") {
        const [conns, snapshot] = await Promise.all([api.listConnections(), api.listRuns()]);
        setConnections(conns);
        setPendingCount(snapshot.runs.filter((r) => r.status === "AwaitingApproval").length);
      }
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

  function goToChat(conversationId?: ConversationId) {
    setFocusConversationId(conversationId ?? null);
    setTab("chat");
  }
  function goToNotes(runId?: RunId) {
    setFocusRunId(runId ?? null);
    setTab("notes");
  }

  if (view.kind !== "status" || view.status.state !== "Connected") {
    return (
      <main className="shell shell-gate">
        <header className="header">
          <h1>NEXUS</h1>
          <p className="subtitle">Personal AI agent</p>
        </header>
        <section className="card" aria-labelledby="host-heading">
          <h2 id="host-heading">Host</h2>
          <div role="status" aria-live="polite">
            <HostView view={view} />
          </div>
          <div className="actions">
            {view.kind === "status" && view.status.state === "NotRunning" && (
              <button type="button" onClick={startHost} disabled={starting}>
                {starting ? "Starting…" : "Start Host"}
              </button>
            )}
            {(view.kind === "bridgeError" ||
              (view.kind === "status" &&
                (view.status.state === "Unreachable" ||
                  view.status.state === "Rejected" ||
                  view.status.state === "Untrusted"))) && (
              <button type="button" onClick={() => void refresh()}>
                Retry
              </button>
            )}
          </div>
          {startError && (
            <p className="error" role="alert">
              The Host could not be started: {startError}
            </p>
          )}
        </section>
        <p className="note">Closing this window does not stop the Host or any accepted tasks.</p>
      </main>
    );
  }

  const status = view.status;
  const active = connections.find((c) => c.active) ?? null;

  return (
    <div className="shell-app">
      <aside className="sidebar">
        <div className="brand">
          <span className="brand-mark">NEXUS</span>
          <span className="brand-tagline">Personal AI agent</span>
        </div>
        <nav aria-label="Sections">
          {TABS.map((t) => {
            const TabIcon = t.icon;
            return (
              <button
                type="button"
                key={t.id}
                className={`nav-item${tab === t.id ? " selected" : ""}`}
                aria-current={tab === t.id ? "page" : undefined}
                onClick={() => setTab(t.id)}
              >
                <TabIcon />
                {t.label}
                {t.id === "dashboard" && pendingCount > 0 && (
                  <span className="nav-count"> {pendingCount}</span>
                )}
              </button>
            );
          })}
        </nav>
        <div className="sidebar-connections">
          <p className="sidebar-heading">AI connections</p>
          {connections.length === 0 ? (
            <p className="muted sidebar-empty">None yet</p>
          ) : (
            connections.map((c) => (
              <div className="sidebar-connection" key={c.connection_id}>
                <span className={`dot ${c.active && c.has_key ? "dot-good" : "dot-neutral"}`} />
                {c.display_name}
              </div>
            ))
          )}
        </div>
        <div className="sidebar-host">
          <span className="dot dot-good" />
          <div>
            <p className="sidebar-host-title">Host connected</p>
            <p className="muted sidebar-host-sub">{formatUptime(status.uptime_seconds)}</p>
          </div>
        </div>
      </aside>

      <div className="main">
        <header className="topbar">
          <div className="topbar-chips">
            <button type="button" className="chip" onClick={() => setTab("settings")}>
              <span className="dot dot-good" /> Host online
            </button>
            <button type="button" className="chip" onClick={() => setTab("settings")}>
              <PlugIcon size={14} />
              {active ? `${active.display_name}${active.model ? ` · ${active.model}` : ""}` : "No AI connection"}
            </button>
            <button
              type="button"
              className={`chip${pendingCount > 0 ? " chip-attention" : ""}`}
              onClick={() => (pendingCount > 0 ? goToNotes() : setTab("dashboard"))}
            >
              <ShieldIcon size={14} />
              {pendingCount > 0 ? `${pendingCount} waiting` : "All clear"}
            </button>
          </div>
          <Clock />
        </header>

        {status.paused && (
          <p className="error banner">
            Restored from a backup: requests are recorded, but nothing runs until you review this
            profile and delete restored.json from it.
          </p>
        )}

        <div className="main-content">
          {tab === "dashboard" && (
            <Dashboard
              hostStatus={status}
              onGoToChat={goToChat}
              onGoToNotes={goToNotes}
              onGoToSettings={() => setTab("settings")}
            />
          )}
          {tab === "chat" && (
            <ChatPanel
              key={focusConversationId ?? "new"}
              initialConversationId={focusConversationId}
              onOpenSettings={() => setTab("settings")}
            />
          )}
          {tab === "notes" && <NotesPanel key={focusRunId ?? "new"} initialRunId={focusRunId} />}
          {tab === "settings" && (
            <>
              <HostCard status={status} />
              <ConnectionsCard />
              <PersonaCard />
              <BackupCard />
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function HostCard({ status }: { status: Extract<HostStatus, { state: "Connected" }> }) {
  return (
    <section className="card" aria-labelledby="host-heading">
      <h2 id="host-heading">Host</h2>
      <Badge tone="good">Connected</Badge>
      <dl className="facts">
        <dt>Identity</dt>
        <dd>{status.host_identity}</dd>
        <dt>Protocol</dt>
        <dd>v{status.protocol_version}</dd>
        <dt>Running for</dt>
        <dd>{formatUptime(status.uptime_seconds)}</dd>
      </dl>
    </section>
  );
}

function HostView({ view }: { view: View }) {
  if (view.kind === "checking") {
    return <p className="muted">Checking the Host…</p>;
  }
  if (view.kind === "bridgeError") {
    return (
      <>
        <Badge tone="bad">Error</Badge>
        <p className="error">The bridge to the Host failed: {view.message}</p>
      </>
    );
  }

  const { status } = view;
  switch (status.state) {
    case "Connected":
      return (
        <>
          <Badge tone="good">Connected</Badge>
          <dl className="facts">
            <dt>Identity</dt>
            <dd>{status.host_identity}</dd>
            <dt>Protocol</dt>
            <dd>v{status.protocol_version}</dd>
            <dt>Running for</dt>
            <dd>{formatUptime(status.uptime_seconds)}</dd>
          </dl>
          {status.paused && (
            <p className="error">
              Restored from a backup: requests are recorded, but nothing runs until you review this
              profile and delete restored.json from it.
            </p>
          )}
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
          <p className="error">{status.reason}</p>
        </>
      );
    case "Rejected":
      return (
        <>
          <Badge tone="bad">Rejected</Badge>
          <p className="error">
            The Host refused the connection ({status.error.code}): {status.error.message}
          </p>
        </>
      );
    case "Untrusted":
      return (
        <>
          <Badge tone="bad">Untrusted</Badge>
          <p className="error">
            The process holding the Nexus connection is not running under your Windows account, so
            your credentials were not sent.
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
