// DEV ONLY. Opened from `pnpm dev` at /core-preview.html; `vite build` builds
// index.html only, so this never ships and nothing imports it. It drives the
// Nexus Core with FORCED visual states so each can be reviewed, including
// PLANNING, which the Host does not emit yet, and EXECUTING, which lasts a
// moment in real use. No Host, no IPC; nothing here is shown as activity.
// `?state=THINKING&still=1` picks a state, and a still frame (reduced motion).

import { NexusCore } from "./NexusCore";
import type { CoreNodeInput, ScreenPoints } from "./NexusCore";
import type { CoreState, NexusCoreVisualState, NodeKind } from "../model/execution.ts";

const STATES: CoreState[] = [
  "IDLE",
  "LISTENING",
  "THINKING",
  "PLANNING",
  "USING_TOOL",
  "EXECUTING",
  "WAITING_APPROVAL",
  "SUCCESS",
  "ERROR",
];

const SCENES: Record<CoreState, { nodes: CoreNodeInput[]; active: NodeKind | null; intensity: number }> = {
  IDLE: { nodes: [], active: null, intensity: 0 },
  LISTENING: { nodes: [], active: null, intensity: 0 },
  THINKING: {
    nodes: [
      { id: "memory", status: "done" },
      { id: "model", status: "active" },
    ],
    active: "model",
    intensity: 0.5,
  },
  PLANNING: { nodes: [{ id: "model", status: "active" }], active: "model", intensity: 0.5 },
  USING_TOOL: {
    nodes: [
      { id: "model", status: "done" },
      { id: "web", status: "active" },
    ],
    active: "web",
    intensity: 0.6,
  },
  EXECUTING: {
    nodes: [
      { id: "model", status: "done" },
      { id: "approval", status: "done" },
      { id: "files", status: "active" },
    ],
    active: "files",
    intensity: 0.8,
  },
  WAITING_APPROVAL: {
    nodes: [
      { id: "model", status: "done" },
      { id: "files", status: "waiting" },
      { id: "approval", status: "waiting" },
    ],
    active: "approval",
    intensity: 0,
  },
  SUCCESS: { nodes: [{ id: "model", status: "done" }], active: null, intensity: 0 },
  ERROR: { nodes: [{ id: "model", status: "failed" }], active: null, intensity: 0 },
};

const LABELS: Record<string, string> = { model: "gemini (preview)", web: "Web read", files: "Notes", approval: "You", memory: "Memory" };

const params = new URLSearchParams(location.search);
const initial = (params.get("state") as CoreState | null) ?? "IDLE";
const still = params.get("still") === "1";

const stage = document.getElementById("stage")!;
const canvas = document.getElementById("core") as HTMLCanvasElement;
const caption = document.getElementById("caption")!;
const bar = document.getElementById("bar")!;
let labels: HTMLElement[] = [];

function layout(points: ScreenPoints) {
  for (const l of labels) l.remove();
  labels = Object.entries(points).map(([id, p]) => {
    const el = document.createElement("span");
    el.className = "label";
    el.textContent = LABELS[id] ?? id;
    el.style.left = `${p.x}px`;
    el.style.top = `${p.y}px`;
    stage.appendChild(el);
    return el;
  });
}

const core = new NexusCore(canvas, { reducedMotion: still, onLayout: layout });
// For inspection from the developer tools only.
(window as unknown as { nexusCore: NexusCore }).nexusCore = core;
const fit = () => core.resize(stage.clientWidth, stage.clientHeight);
new ResizeObserver(fit).observe(stage);
fit();

stage.addEventListener("pointermove", (e) => {
  const box = stage.getBoundingClientRect();
  core.setPointer(((e.clientX - box.left) / box.width) * 2 - 1, -(((e.clientY - box.top) / box.height) * 2 - 1));
});
stage.addEventListener("pointerleave", () => core.setPointer(0, 0));

function show(state: CoreState) {
  const scene = SCENES[state];
  const visual: NexusCoreVisualState = {
    state,
    activeTool: scene.active,
    activeModel: scene.active === "model" ? "gemini (preview)" : null,
    activityIntensity: scene.intensity,
    hasPendingApproval: state === "WAITING_APPROVAL",
    lastResult: state === "SUCCESS" ? "success" : state === "ERROR" ? "error" : null,
  };
  core.setState(visual, scene.nodes);
  caption.textContent = `PREVIEW · ${state}`;
  for (const b of bar.querySelectorAll("button")) b.setAttribute("aria-pressed", String(b.textContent === state));
  history.replaceState(null, "", `?state=${state}${still ? "&still=1" : ""}`);
}

for (const state of STATES) {
  const b = document.createElement("button");
  b.type = "button";
  b.textContent = state;
  b.addEventListener("click", () => show(state));
  bar.appendChild(b);
}
show(STATES.includes(initial) ? initial : "IDLE");
