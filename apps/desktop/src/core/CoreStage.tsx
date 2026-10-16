// The stage around the Nexus Core: the WebGL canvas (loaded on demand),
// labels for the context nodes, and a still fallback when WebGL is off or
// unavailable. The core reflects state; it never starts anything.

import { useEffect, useRef, useState } from "react";
import type { PointerEvent } from "react";
import type { CoreView, NodeKind } from "../model/execution.ts";
import { ago } from "../model/format.ts";
import { prefersReducedMotion, usePref } from "../state/prefs";
import type { NexusCore, ScreenPoints } from "./NexusCore";

const STATE_TEXT: Record<CoreView["state"], string> = {
  IDLE: "Idle",
  LISTENING: "Listening",
  THINKING: "Thinking",
  PLANNING: "Planning",
  USING_TOOL: "Using a tool",
  EXECUTING: "Executing",
  WAITING_APPROVAL: "Waiting for approval",
  SUCCESS: "Completed",
  ERROR: "Error",
};

/** Labels that only repeat the state; the caption leaves them out. */
const REPEATS = new Set(["Idle", "Listening", "Done", "Waiting for your approval"]);

export function CoreStage({ view, onNode, now }: { view: CoreView; onNode: (id: NodeKind) => void; now: number }) {
  const [mode] = usePref("core");
  const [failed, setFailed] = useState(false);
  const host = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const core = useRef<NexusCore | null>(null);
  const [points, setPoints] = useState<ScreenPoints>({});
  const latest = useRef(view);
  latest.current = view;

  const webgl = mode !== "off" && !failed;
  const reduced = mode === "still" || prefersReducedMotion();

  useEffect(() => {
    if (!webgl || !canvas.current || !host.current) return;
    let alive = true;
    let observer: ResizeObserver | null = null;
    const el = canvas.current;
    const box = host.current;
    void import("./NexusCore")
      .then(({ NexusCore }) => {
        if (!alive) return;
        const instance = new NexusCore(el, { reducedMotion: reduced, onLayout: (p) => alive && setPoints(p) });
        instance.onContextLost = () => alive && setFailed(true);
        core.current = instance;
        const fit = () => instance.resize(box.clientWidth, box.clientHeight);
        observer = new ResizeObserver(fit);
        observer.observe(box);
        fit();
        const v = latest.current;
        instance.setState(v.visual, v.nodes);
      })
      .catch(() => alive && setFailed(true));
    return () => {
      alive = false;
      observer?.disconnect();
      core.current?.dispose();
      core.current = null;
    };
    // Built once per WebGL session; `reduced` and the state are applied by
    // the effects below without rebuilding the scene.
  }, [webgl]);

  useEffect(() => {
    core.current?.setReducedMotion(reduced);
  }, [reduced]);

  const nodeKey = view.nodes.map((n) => `${n.id}:${n.status}:${n.proposed}`).join("|");
  const v = view.visual;
  const visualKey = [v.state, v.activeTool, v.activeModel, v.activityIntensity, v.hasPendingApproval, v.lastResult].join("|");
  useEffect(() => {
    core.current?.setState(view.visual, view.nodes);
    // The keys stand for the contents of the node list and the visual state.
  }, [visualKey, nodeKey]);

  // A restrained parallax: the core leans a few degrees towards the pointer.
  const onPointerMove = (e: PointerEvent<HTMLDivElement>) => {
    const box = e.currentTarget.getBoundingClientRect();
    core.current?.setPointer(((e.clientX - box.left) / box.width) * 2 - 1, -(((e.clientY - box.top) / box.height) * 2 - 1));
  };
  // The outcome of the last run stays in words after the picture settles.
  const outcome = view.outcome;
  const detail = !REPEATS.has(view.label) ? view.label : outcome ? outcome.text : null;
  const age = REPEATS.has(view.label) && outcome ? ago(outcome.at, now) : null;

  return (
    <div
      className={`core-stage core-${view.state.toLowerCase()}`}
      ref={host}
      onPointerMove={webgl && !reduced ? onPointerMove : undefined}
      onPointerLeave={webgl && !reduced ? () => core.current?.setPointer(0, 0) : undefined}
    >
      {webgl ? (
        <canvas ref={canvas} className="core-canvas" aria-hidden="true" />
      ) : (
        <StillCore state={view.state} />
      )}
      {view.nodes.map((n) => {
        const p = webgl ? points[n.id] : STILL_POINTS[n.id];
        if (!p) return null;
        const style = webgl ? { left: `${p.x}px`, top: `${p.y}px` } : { left: `${p.x}%`, top: `${p.y}%` };
        return (
          <button
            type="button"
            key={n.id}
            className={`core-node core-node-${n.status}${n.proposed ? " core-node-proposed" : ""}${view.active === n.id ? " core-node-current" : ""}`}
            style={style}
            onClick={() => onNode(n.id)}
            aria-label={`${n.label} node${n.sublabel ? `, ${n.sublabel}` : ""}`}
            title={`${n.label}${n.sublabel ? ` · ${n.sublabel}` : ""}`}
          >
            <span className="core-node-label">{n.label}</span>
            {n.sublabel && <span className="core-node-sub">{n.sublabel}</span>}
          </button>
        );
      })}
      <div className="core-caption" role="status" aria-live="polite">
        <span className="core-heading">
          <span className="core-title">Nexus Core</span>
          <span className="core-state">{STATE_TEXT[view.state]}</span>
        </span>
        {detail && (
          <span className={`core-label${outcome && detail === outcome.text ? ` core-outcome-${outcome.result}` : ""}`}>
            {detail}
            {age && <span aria-hidden="true"> · {age}</span>}
          </span>
        )}
      </div>
      {view.path.length > 0 && (
        <ol className="core-path" aria-label="Execution path">
          <li>Core</li>
          {view.path.map((id, i) => (
            <li key={`${id}-${i}`} className={i === view.path.length - 1 && view.active === id ? "current" : undefined}>
              {view.nodes.find((n) => n.id === id)?.label ?? id}
            </li>
          ))}
        </ol>
      )}
    </div>
  );
}

/** Node positions (percent of the stage) for the still fallback. */
const STILL_POINTS: Record<string, { x: number; y: number }> = {
  model: { x: 16, y: 26 },
  web: { x: 84, y: 26 },
  files: { x: 84, y: 70 },
  approval: { x: 16, y: 70 },
  memory: { x: 14, y: 48 },
};

function StillCore({ state }: { state: CoreView["state"] }) {
  return (
    <svg className="core-still" viewBox="-100 -100 200 200" aria-hidden="true">
      <defs>
        <radialGradient id="still-glow">
          <stop offset="0" stopColor="currentColor" stopOpacity="0.85" />
          <stop offset="0.35" stopColor="currentColor" stopOpacity="0.25" />
          <stop offset="1" stopColor="currentColor" stopOpacity="0" />
        </radialGradient>
      </defs>
      <circle r="62" className="still-shell" />
      <circle r={state === "IDLE" || state === "WAITING_APPROVAL" ? 24 : 30} fill="url(#still-glow)" />
      <circle r="12" className="still-envelope" />
      <circle r="3.5" className="still-core" />
    </svg>
  );
}
