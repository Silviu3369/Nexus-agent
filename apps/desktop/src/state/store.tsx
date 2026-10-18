// One store for everything the dashboard shows, kept in step with the Host.
// The Host's durable event log is the trigger: each tick reads the events
// after the cursor and refetches only the collections they describe. Text
// that is still streaming is not evented, so a streaming reply is read
// directly while it lasts. Nothing here is optimistic: every change shown
// comes back from the Host.

import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";
import type { ReactNode } from "react";
import type {
  AgentStepView,
  AutomationView,
  ConnectionView,
  ConversationId,
  ConversationSummary,
  ConversationView,
  EventView,
  FolderView,
  HostStatus,
  MemoryItemView,
  McpServerView,
  PersonaView,
  ProjectView,
  RegisterView,
  RunView,
  SystemStatusView,
  UsageView,
} from "@nexus/contracts";
import { api, describeError, errorCode } from "../api";
import { buildRuns, followedTask, invalidatedBy } from "../model/execution.ts";
import type { Collection, RunVM } from "../model/execution.ts";

const IDLE_TICK_MS = 1000;
const BUSY_TICK_MS = 400;
const HOST_CHECK_MS = 3000;
const SYSTEM_CHECK_MS = 3000;
/** Events kept for the activity log (the Host pages them 200 at a time). */
const EVENT_HISTORY = 400;
/** Proposed facts read for review; the Memory view pages the rest. */
const PROPOSALS_SHOWN = 50;

export interface NexusData {
  host: HostStatus | null;
  bridgeError: string | null;
  system: SystemStatusView | null;
  connections: ConnectionView[] | null;
  runs: RunView[] | null;
  steps: AgentStepView[] | null;
  conversations: ConversationSummary[] | null;
  persona: PersonaView | null;
  projects: ProjectView[] | null;
  /** Facts the agent proposed that wait for the user's decision. */
  proposals: MemoryItemView[] | null;
  /** Bumped whenever memory changes, so views with their own queries reread. */
  memoryRev: number;
  /** Remote MCP servers the user added, with their tools. */
  mcp: McpServerView[] | null;
  /** Today's tokens, the daily budget and cost estimates. */
  usage: UsageView | null;
  /** Scheduled instructions with their next time and last outcome. */
  automations: AutomationView[] | null;
  /** The user's registers with their counts. */
  registers: RegisterView[] | null;
  /** Folders the user shared with Nexus. */
  folders: FolderView[] | null;
  /** Bumped whenever records change, so views with their own lists reread. */
  recordsRev: number;
  events: EventView[];
  /** The task the live panel follows. */
  focusId: ConversationId | null;
  focus: ConversationView | null;
  syncError: string | null;
  composing: boolean;
}

const EMPTY: NexusData = {
  host: null,
  bridgeError: null,
  system: null,
  connections: null,
  runs: null,
  steps: null,
  conversations: null,
  persona: null,
  projects: null,
  proposals: null,
  memoryRev: 0,
  mcp: null,
  usage: null,
  automations: null,
  registers: null,
  folders: null,
  recordsRev: 0,
  events: [],
  focusId: null,
  focus: null,
  syncError: null,
  composing: false,
};

interface NexusActions {
  /** Refetch now (after a command), instead of waiting for the next tick. */
  refresh: (what?: Collection[]) => Promise<void>;
  checkHost: () => Promise<void>;
  /** Follow this task in the live panel until another one starts working. */
  follow: (id: ConversationId) => void;
  /** A task that no longer exists: stop following it. */
  forget: (id: ConversationId) => void;
  setComposing: (on: boolean) => void;
}

const DataContext = createContext<NexusData>(EMPTY);
const ActionsContext = createContext<NexusActions | null>(null);

export function NexusProvider({ children }: { children: ReactNode }) {
  const [data, setData] = useState<NexusData>(EMPTY);
  const latest = useRef(data);
  latest.current = data;
  const cursor = useRef<number | null>(null);
  const pinned = useRef<ConversationId | null>(null);
  const busy = useRef(false);

  const patch = useCallback((p: Partial<NexusData>) => setData((d) => ({ ...d, ...p })), []);

  const loadFocus = useCallback(
    async (id: ConversationId | null) => {
      if (!id) {
        patch({ focus: null });
        return;
      }
      let view: ConversationView;
      try {
        view = await api.getConversation(id);
      } catch (err) {
        if (errorCode(err) !== "NotFound") throw err;
        // Deleted meanwhile: follow nothing until the next change picks a task.
        if (pinned.current === id) pinned.current = null;
        setData((d) => (d.focusId === id ? { ...d, focusId: null, focus: null } : d));
        return;
      }
      // A slower answer for a task no longer followed is dropped.
      setData((d) => (d.focusId === id ? { ...d, focus: view } : d));
    },
    [patch],
  );

  const chooseFocus = useCallback((steps: AgentStepView[], runs: RunView[]) => {
    const running = steps.find((s) => s.status === "Running");
    // A pinned task stays followed until a different one starts working.
    if (pinned.current && running && running.conversation_id !== pinned.current) pinned.current = null;
    return pinned.current ?? followedTask(steps, runs);
  }, []);

  const fetchCollections = useCallback(
    async (what: Set<Collection>) => {
      const [runs, steps, conversations, connections, persona, projects, proposals, mcp, usage, automations, registers, folders] =
        await Promise.all([
        what.has("runs") ? api.listRuns().then((s) => s.runs) : Promise.resolve(undefined),
        what.has("steps") ? api.getActivity() : Promise.resolve(undefined),
        what.has("conversations") ? api.listConversations() : Promise.resolve(undefined),
        what.has("connections") ? api.listConnections() : Promise.resolve(undefined),
        what.has("persona") ? api.getPersona() : Promise.resolve(undefined),
        what.has("projects") ? api.listProjects() : Promise.resolve(undefined),
        what.has("memory") ? listProposals() : Promise.resolve(undefined),
        what.has("mcp") ? api.listMcpServers() : Promise.resolve(undefined),
        what.has("usage") ? api.getUsage() : Promise.resolve(undefined),
        what.has("automations") ? api.listAutomations() : Promise.resolve(undefined),
        what.has("registers") ? api.listRegisters() : Promise.resolve(undefined),
        what.has("folders") ? api.listFolders() : Promise.resolve(undefined),
      ]);
      const current = latest.current;
      const nextSteps = steps ?? current.steps ?? [];
      const nextRuns = runs ?? current.runs ?? [];
      const focusId = chooseFocus(nextSteps, nextRuns);
      setData((d) => ({
        ...d,
        runs: runs ?? d.runs,
        steps: steps ?? d.steps,
        conversations: conversations ?? d.conversations,
        connections: connections ?? d.connections,
        persona: persona ?? d.persona,
        projects: projects ?? d.projects,
        proposals: proposals ?? d.proposals,
        memoryRev: proposals ? d.memoryRev + 1 : d.memoryRev,
        mcp: mcp ?? d.mcp,
        usage: usage ?? d.usage,
        automations: automations ?? d.automations,
        registers: registers ?? d.registers,
        folders: folders ?? d.folders,
        recordsRev: registers ? d.recordsRev + 1 : d.recordsRev,
        focusId,
        focus: focusId === d.focusId ? d.focus : null,
        syncError: null,
      }));
      if (what.has("focus") || focusId !== current.focusId) await loadFocus(focusId);
    },
    [chooseFocus, loadFocus],
  );

  const bootstrap = useCallback(async () => {
    const [snapshot, steps, conversations, connections, persona, projects, proposals, mcp, usage, automations, registers, folders] =
      await Promise.all([
      api.listRuns(),
      api.getActivity(),
      api.listConversations(),
      api.listConnections(),
      api.getPersona(),
      api.listProjects(),
      listProposals(),
      api.listMcpServers(),
      api.getUsage(),
      api.listAutomations(),
      api.listRegisters(),
      api.listFolders(),
    ]);
    // Everything after the snapshot's watermark arrives through the events.
    const history = await api.getEvents(Math.max(0, snapshot.watermark - 200));
    cursor.current = snapshot.watermark;
    const focusId = chooseFocus(steps, snapshot.runs);
    setData((d) => ({
      ...d,
      runs: snapshot.runs,
      steps,
      conversations,
      connections,
      persona,
      projects,
      proposals,
      memoryRev: d.memoryRev + 1,
      mcp,
      usage,
      automations,
      registers,
      folders,
      recordsRev: d.recordsRev + 1,
      events: history.events.filter((e) => e.seq <= snapshot.watermark),
      focusId,
      focus: focusId === d.focusId ? d.focus : null,
      syncError: null,
    }));
    await loadFocus(focusId);
  }, [chooseFocus, loadFocus]);

  const tick = useCallback(async () => {
    if (cursor.current === null) {
      await bootstrap();
      return;
    }
    let after = cursor.current;
    const invalid = new Set<Collection>();
    const fresh: EventView[] = [];
    // Drain every page so a burst of events is never half-applied.
    for (;;) {
      const page = await api.getEvents(after);
      fresh.push(...page.events);
      for (const c of invalidatedBy(page.events)) invalid.add(c);
      after = page.next_after;
      if (page.events.length < 200) break;
    }
    cursor.current = after;
    if (fresh.length > 0) {
      setData((d) => ({ ...d, events: [...d.events, ...fresh].slice(-EVENT_HISTORY) }));
    }
    const streaming = latest.current.focus?.messages.some((m) => m.status === "Streaming") ?? false;
    if (invalid.size > 0) await fetchCollections(invalid);
    else if (streaming) await loadFocus(latest.current.focusId);
  }, [bootstrap, fetchCollections, loadFocus]);

  const checkHost = useCallback(async () => {
    try {
      const host = await api.hostStatus();
      patch({ host, bridgeError: null });
      if (host.state !== "Connected") cursor.current = null;
    } catch (err) {
      patch({ bridgeError: describeError(err) });
    }
  }, [patch]);

  // Host status: decides between the gate screen and the shell.
  useEffect(() => {
    void checkHost();
    const timer = window.setInterval(() => {
      if (!document.hidden) void checkHost();
    }, HOST_CHECK_MS);
    return () => window.clearInterval(timer);
  }, [checkHost]);

  const connected = data.host?.state === "Connected";

  // The sync loop, paced by how busy the agent is; paused while hidden.
  useEffect(() => {
    if (!connected) return;
    let alive = true;
    let timer = 0;
    const loop = async () => {
      if (!alive) return;
      if (!document.hidden && !busy.current) {
        busy.current = true;
        try {
          await tick();
        } catch (err) {
          patch({ syncError: describeError(err) });
        } finally {
          busy.current = false;
        }
      }
      const d = latest.current;
      const working =
        (d.steps?.some((s) => s.status === "Running") ?? false) ||
        (d.focus?.messages.some((m) => m.status === "Streaming") ?? false);
      timer = window.setTimeout(loop, working ? BUSY_TICK_MS : IDLE_TICK_MS);
    };
    void loop();
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [connected, tick, patch]);

  // Machine figures: secondary, read every few seconds while visible.
  useEffect(() => {
    if (!connected) return;
    const read = () => {
      if (document.hidden) return;
      api
        .getSystemStatus()
        .then((system) => patch({ system }))
        .catch(() => patch({ system: null }));
    };
    read();
    const timer = window.setInterval(read, SYSTEM_CHECK_MS);
    return () => window.clearInterval(timer);
  }, [connected, patch]);

  const actions = useMemo<NexusActions>(
    () => ({
      refresh: async (what) => {
        try {
          await fetchCollections(
            new Set(what ?? ["runs", "steps", "conversations", "connections", "projects", "memory", "mcp", "usage", "automations", "registers", "folders", "focus"]),
          );
        } catch (err) {
          patch({ syncError: describeError(err) });
        }
      },
      checkHost,
      follow: (id) => {
        pinned.current = id;
        setData((d) => ({ ...d, focusId: id, focus: d.focusId === id ? d.focus : null }));
        void loadFocus(id).catch((err) => patch({ syncError: describeError(err) }));
      },
      forget: (id) => {
        if (pinned.current === id) pinned.current = null;
        setData((d) => (d.focusId === id ? { ...d, focusId: null, focus: null } : d));
      },
      setComposing: (on) => setData((d) => (d.composing === on ? d : { ...d, composing: on })),
    }),
    [fetchCollections, checkHost, loadFocus, patch],
  );

  return (
    <ActionsContext.Provider value={actions}>
      <DataContext.Provider value={data}>{children}</DataContext.Provider>
    </ActionsContext.Provider>
  );
}

function listProposals(): Promise<MemoryItemView[]> {
  return api.listMemory({ status: "Proposed", limit: PROPOSALS_SHOWN });
}

export function useNexus(): NexusData {
  return useContext(DataContext);
}

export function useNexusActions(): NexusActions {
  const actions = useContext(ActionsContext);
  if (!actions) throw new Error("useNexusActions outside NexusProvider");
  return actions;
}

/** Derived, memoised facts most views need. */
export function useDerived() {
  const d = useNexus();
  return useMemo(() => {
    const runs = d.runs ?? [];
    const runsById = new Map(runs.map((r) => [r.run_id as string, r]));
    const agentName = d.persona?.name ?? "Assistant";
    const pendingMemory = new Set<string>((d.proposals ?? []).map((m) => m.memory_id));
    const focusRuns: RunVM[] = d.focus ? buildRuns(d.focus, runsById, agentName, pendingMemory) : [];
    const focusRun = focusRuns.length > 0 ? focusRuns[focusRuns.length - 1] : null;
    const pending = runs.filter((r) => r.status === "AwaitingApproval");
    const active = d.connections?.find((c) => c.active) ?? null;
    const ready = active !== null && active.model !== null && active.has_key;
    const runningStep = d.steps?.find((s) => s.status === "Running") ?? null;
    const activeProject = d.projects?.find((p) => p.active) ?? null;
    const projectsById = new Map((d.projects ?? []).map((p) => [p.project_id as string, p]));
    return {
      runsById,
      agentName,
      focusRuns,
      focusRun,
      pending,
      pendingMemory,
      active,
      ready,
      runningStep,
      activeProject,
      projectsById,
    };
  }, [d.runs, d.persona, d.focus, d.connections, d.steps, d.proposals, d.projects]);
}

/** A clock for time-based states (success pulse, expiry countdowns). */
export function useNow(intervalMs: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), intervalMs);
    return () => window.clearInterval(timer);
  }, [intervalMs]);
  return now;
}
