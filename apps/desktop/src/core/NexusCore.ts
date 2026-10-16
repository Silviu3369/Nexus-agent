// The Nexus Core: a small Three.js scene whose motion encodes the state the
// Host publishes. It owns no runtime state and grants nothing; it only draws.
//
// Cost control: the camera never moves, geometry is fixed (64 neural nodes,
// ~130 paths, 220 particles), frames are rendered on demand at a per-state
// rate (12 fps idle, 30 fps working, 5 fps unfocused, none while hidden), and
// reduced motion renders one still frame per change.

import {
  AdditiveBlending,
  BufferAttribute,
  BufferGeometry,
  Group,
  LineSegments,
  Mesh,
  PerspectiveCamera,
  PlaneGeometry,
  Points,
  RingGeometry,
  Scene,
  ShaderMaterial,
  SphereGeometry,
  Vector3,
  WebGLRenderer,
} from "three";
import type { CoreState } from "../model/execution.ts";

export type CoreNodeStatus = "active" | "waiting" | "failed" | "done";
export interface CoreNodeInput {
  id: string;
  status: CoreNodeStatus;
}
export type ScreenPoints = Record<string, { x: number; y: number }>;

interface Tuning {
  spin: number;
  fire: number;
  intensity: number;
  breathe: number;
  breatheHz: number;
  amber: number;
  red: number;
  jitter: number;
  fps: number;
  branches: boolean;
}

const TUNING: Record<CoreState, Tuning> = {
  IDLE: { spin: 0.035, fire: 0.25, intensity: 0.42, breathe: 0.045, breatheHz: 0.2, amber: 0, red: 0, jitter: 0, fps: 8, branches: false },
  LISTENING: { spin: 0.05, fire: 0.9, intensity: 0.6, breathe: 0.06, breatheHz: 0.45, amber: 0, red: 0, jitter: 0, fps: 30, branches: false },
  THINKING: { spin: 0.09, fire: 7, intensity: 0.85, breathe: 0.025, breatheHz: 0.8, amber: 0, red: 0, jitter: 0, fps: 30, branches: false },
  PLANNING: { spin: 0.07, fire: 2.5, intensity: 0.8, breathe: 0.03, breatheHz: 0.6, amber: 0, red: 0, jitter: 0, fps: 30, branches: true },
  USING_TOOL: { spin: 0.06, fire: 2, intensity: 0.72, breathe: 0.03, breatheHz: 0.5, amber: 0, red: 0, jitter: 0, fps: 30, branches: false },
  EXECUTING: { spin: 0.08, fire: 4, intensity: 0.95, breathe: 0.03, breatheHz: 0.7, amber: 0, red: 0, jitter: 0, fps: 30, branches: false },
  WAITING_APPROVAL: { spin: 0.012, fire: 0.12, intensity: 0.38, breathe: 0.035, breatheHz: 0.12, amber: 0.35, red: 0, jitter: 0, fps: 8, branches: false },
  SUCCESS: { spin: 0.05, fire: 1.2, intensity: 0.8, breathe: 0.04, breatheHz: 0.3, amber: 0, red: 0, jitter: 0, fps: 30, branches: false },
  ERROR: { spin: 0.02, fire: 0.25, intensity: 0.5, breathe: 0.03, breatheHz: 0.2, amber: 0, red: 0.45, jitter: 0.003, fps: 20, branches: false },
};

const UNFOCUSED_FPS = 5;
/** After this long in IDLE the core only breathes, at a couple of frames a
 *  second (measured: each WebGL frame costs far more than its pixels). */
const REST_AFTER_MS = 15_000;
const REST_FPS = 2;
const NEURAL_COUNT = 64;
const PARTICLE_COUNT = 220;
const SHELL_RADIUS = 1.28;

// Colours are given in display (sRGB) values; the shaders output them as is.
const ICE: [number, number, number] = [0.49, 0.83, 0.99];
const ICE_DIM: [number, number, number] = [0.26, 0.42, 0.55];
const AMBER: [number, number, number] = [0.98, 0.75, 0.32];
const RED: [number, number, number] = [0.97, 0.45, 0.45];
const DONE: [number, number, number] = [0.42, 0.55, 0.66];

/** Fixed slots, so a node never jumps when others appear. */
const SLOT: Record<string, [number, number]> = {
  model: [-1, 0.42],
  web: [1, 0.42],
  files: [1, -0.55],
  approval: [-1, -0.55],
};

function mulberry32(seed: number) {
  let a = seed;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const POINT_VERTEX = /* glsl */ `
  attribute float aAct;
  attribute float aSize;
  uniform float uPixelRatio;
  uniform float uBase;
  varying float vAct;
  void main() {
    vAct = aAct;
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    gl_PointSize = (aSize + aAct * uBase) * uPixelRatio * (6.0 / -mv.z);
    gl_Position = projectionMatrix * mv;
  }
`;

const POINT_FRAGMENT = /* glsl */ `
  uniform vec3 uDim;
  uniform vec3 uHot;
  uniform float uIntensity;
  uniform float uAlpha;
  varying float vAct;
  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    float a = smoothstep(1.0, 0.0, r);
    a *= a;
    vec3 c = mix(uDim, uHot, clamp(vAct, 0.0, 1.0));
    gl_FragColor = vec4(c * a * (0.35 + uIntensity * 0.65 + vAct * 0.6), a * uAlpha);
  }
`;

const LINE_VERTEX = /* glsl */ `
  attribute float aAct;
  varying float vAct;
  void main() {
    vAct = aAct;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

const LINE_FRAGMENT = /* glsl */ `
  uniform vec3 uDim;
  uniform vec3 uHot;
  uniform float uIntensity;
  uniform float uAlpha;
  varying float vAct;
  void main() {
    vec3 c = mix(uDim, uHot, clamp(vAct, 0.0, 1.0));
    float a = max(0.0, 0.10 + uIntensity * 0.12 + vAct * 0.75) * uAlpha;
    gl_FragColor = vec4(c * a, a);
  }
`;

const GLOW_FRAGMENT = /* glsl */ `
  uniform vec3 uColor;
  uniform float uStrength;
  uniform float uFalloff;
  varying vec2 vUv;
  void main() {
    float r = length(vUv - 0.5) * 2.0;
    float a = pow(max(0.0, 1.0 - r), uFalloff) * uStrength;
    gl_FragColor = vec4(uColor * a, a);
  }
`;

const UV_VERTEX = /* glsl */ `
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

const SHELL_VERTEX = /* glsl */ `
  varying float vRim;
  void main() {
    vec3 n = normalize(normalMatrix * normal);
    vec4 mv = modelViewMatrix * vec4(position, 1.0);
    vRim = 1.0 - abs(dot(n, normalize(-mv.xyz)));
    gl_Position = projectionMatrix * mv;
  }
`;

const SHELL_FRAGMENT = /* glsl */ `
  uniform vec3 uColor;
  uniform float uStrength;
  varying float vRim;
  void main() {
    float a = pow(vRim, 3.0) * uStrength;
    gl_FragColor = vec4(uColor * a, a);
  }
`;

function additive(vertexShader: string, fragmentShader: string, uniforms: Record<string, { value: unknown }>) {
  return new ShaderMaterial({
    vertexShader,
    fragmentShader,
    uniforms,
    transparent: true,
    depthWrite: false,
    depthTest: false,
    blending: AdditiveBlending,
  });
}

const v3 = (c: [number, number, number]) => new Vector3(c[0], c[1], c[2]);

interface Beam {
  id: string;
  target: Vector3;
  line: LineSegments;
  lineMat: ShaderMaterial;
  head: Points;
  headMat: ShaderMaterial;
  node: Mesh;
  nodeMat: ShaderMaterial;
  ring: Mesh;
  ringMat: ShaderMaterial;
  status: CoreNodeStatus;
  active: boolean;
  /** 0..1 while appearing, so a node fades in instead of popping. */
  presence: number;
  removing: boolean;
}

export class NexusCore {
  private readonly renderer: WebGLRenderer;
  private readonly scene = new Scene();
  private readonly camera = new PerspectiveCamera(38, 1, 0.1, 50);
  private readonly brain = new Group();
  private readonly dust = new Group();
  private readonly beams = new Map<string, Beam>();
  private readonly disposables: { dispose(): void }[] = [];

  private readonly neuralPos: Vector3[] = [];
  private readonly neighbours: number[][] = [];
  private readonly act: Float32Array;
  private readonly lineAct: Float32Array;
  private readonly edges: [number, number][] = [];
  private readonly neuralGeo: BufferGeometry;
  private readonly lineGeo: BufferGeometry;
  private readonly pending: { node: number; at: number; strength: number; depth: number; outward: boolean }[] = [];

  private readonly neuralMat: ShaderMaterial;
  private readonly lineMat: ShaderMaterial;
  private readonly dustMat: ShaderMaterial;
  private readonly glowMat: ShaderMaterial;
  private readonly coreMat: ShaderMaterial;
  private readonly shellMat: ShaderMaterial;
  private readonly gridMat: ShaderMaterial;
  private readonly pulseMat: ShaderMaterial;
  private readonly pulse: Mesh;
  private readonly glow: Mesh;
  private readonly shell: Group;

  private state: CoreState = "IDLE";
  private stateSince = performance.now();
  private tune: Tuning = { ...TUNING.IDLE };
  private target: Tuning = TUNING.IDLE;
  private activeId: string | null = null;
  private reduced: boolean;
  private focused = true;
  private raf = 0;
  private sleep = 0;
  private lastTick = 0;
  private time = 0;
  private fireDebt = 0;
  private pulseAge = -1;
  private width = 1;
  private height = 1;
  private readonly rand = mulberry32(0x6e78);
  private disposed = false;
  private readonly onLayout: (points: ScreenPoints) => void;

  constructor(canvas: HTMLCanvasElement, options: { reducedMotion: boolean; onLayout: (points: ScreenPoints) => void }) {
    this.reduced = options.reducedMotion;
    this.onLayout = options.onLayout;
    this.renderer = new WebGLRenderer({ canvas, antialias: true, alpha: false, powerPreference: "low-power" });
    this.renderer.setClearColor(0x06080c, 1);
    this.renderer.setPixelRatio(Math.min(window.devicePixelRatio || 1, 1.5));
    this.camera.position.set(0, 0, 6.2);
    this.camera.lookAt(0, 0, 0);

    const pixelRatio = { value: this.renderer.getPixelRatio() };

    // Neural nodes on jittered shells, joined to their nearest neighbours.
    for (let i = 0; i < NEURAL_COUNT; i++) {
      const y = 1 - (2 * (i + 0.5)) / NEURAL_COUNT;
      const ring = Math.sqrt(1 - y * y);
      const theta = i * 2.399963;
      const radius = 0.38 + 0.72 * Math.cbrt(this.rand());
      this.neuralPos.push(
        new Vector3(Math.cos(theta) * ring, y, Math.sin(theta) * ring).multiplyScalar(radius).addScalar((this.rand() - 0.5) * 0.08),
      );
    }
    const seen = new Set<string>();
    for (let i = 0; i < NEURAL_COUNT; i++) {
      const near = this.neuralPos
        .map((p, j) => ({ j, d: p.distanceToSquared(this.neuralPos[i]) }))
        .filter((x) => x.j !== i)
        .sort((a, b) => a.d - b.d)
        .slice(0, 3);
      this.neighbours.push(near.map((x) => x.j));
      for (const { j } of near) {
        const key = i < j ? `${i}-${j}` : `${j}-${i}`;
        if (!seen.has(key)) {
          seen.add(key);
          this.edges.push([i, j]);
        }
      }
    }
    for (let i = 0; i < NEURAL_COUNT; i++) {
      for (const [a, b] of this.edges) {
        if (a === i && !this.neighbours[i].includes(b)) this.neighbours[i].push(b);
        if (b === i && !this.neighbours[i].includes(a)) this.neighbours[i].push(a);
      }
    }

    this.act = new Float32Array(NEURAL_COUNT);
    const neuralPositions = new Float32Array(NEURAL_COUNT * 3);
    const neuralSizes = new Float32Array(NEURAL_COUNT);
    this.neuralPos.forEach((p, i) => {
      p.toArray(neuralPositions, i * 3);
      neuralSizes[i] = 5 + this.rand() * 4;
    });
    this.neuralGeo = new BufferGeometry();
    this.neuralGeo.setAttribute("position", new BufferAttribute(neuralPositions, 3));
    this.neuralGeo.setAttribute("aAct", new BufferAttribute(this.act, 1));
    this.neuralGeo.setAttribute("aSize", new BufferAttribute(neuralSizes, 1));
    this.neuralMat = additive(POINT_VERTEX, POINT_FRAGMENT, {
      uPixelRatio: pixelRatio,
      uBase: { value: 14 },
      uDim: { value: v3(ICE_DIM) },
      uHot: { value: v3(ICE) },
      uIntensity: { value: 0.4 },
      uAlpha: { value: 1 },
    });
    this.brain.add(new Points(this.neuralGeo, this.neuralMat));

    const linePositions = new Float32Array(this.edges.length * 6);
    this.lineAct = new Float32Array(this.edges.length * 2);
    this.edges.forEach(([a, b], k) => {
      this.neuralPos[a].toArray(linePositions, k * 6);
      this.neuralPos[b].toArray(linePositions, k * 6 + 3);
    });
    this.lineGeo = new BufferGeometry();
    this.lineGeo.setAttribute("position", new BufferAttribute(linePositions, 3));
    this.lineGeo.setAttribute("aAct", new BufferAttribute(this.lineAct, 1));
    this.lineMat = additive(LINE_VERTEX, LINE_FRAGMENT, {
      uDim: { value: v3(ICE_DIM) },
      uHot: { value: v3(ICE) },
      uIntensity: { value: 0.4 },
      uAlpha: { value: 1 },
    });
    this.brain.add(new LineSegments(this.lineGeo, this.lineMat));
    this.scene.add(this.brain);

    // Dust: slow particles between the brain and the shell.
    const dustPos = new Float32Array(PARTICLE_COUNT * 3);
    const dustSize = new Float32Array(PARTICLE_COUNT);
    const dustAct = new Float32Array(PARTICLE_COUNT);
    for (let i = 0; i < PARTICLE_COUNT; i++) {
      const dir = new Vector3(this.rand() * 2 - 1, this.rand() * 2 - 1, this.rand() * 2 - 1).normalize();
      dir.multiplyScalar(0.6 + this.rand() * 0.62).toArray(dustPos, i * 3);
      dustSize[i] = 1.2 + this.rand() * 1.8;
      dustAct[i] = this.rand() * 0.25;
    }
    const dustGeo = new BufferGeometry();
    dustGeo.setAttribute("position", new BufferAttribute(dustPos, 3));
    dustGeo.setAttribute("aSize", new BufferAttribute(dustSize, 1));
    dustGeo.setAttribute("aAct", new BufferAttribute(dustAct, 1));
    this.dustMat = additive(POINT_VERTEX, POINT_FRAGMENT, {
      uPixelRatio: pixelRatio,
      uBase: { value: 2 },
      uDim: { value: v3(ICE_DIM) },
      uHot: { value: v3(ICE) },
      uIntensity: { value: 0.3 },
      uAlpha: { value: 0.55 },
    });
    this.dust.add(new Points(dustGeo, this.dustMat));
    this.scene.add(this.dust);
    this.disposables.push(dustGeo);

    // Energy core: two camera-facing glows (the camera never moves).
    const glowGeo = new PlaneGeometry(1, 1);
    this.glowMat = additive(UV_VERTEX, GLOW_FRAGMENT, {
      uColor: { value: v3(ICE) },
      uStrength: { value: 0.35 },
      uFalloff: { value: 2.6 },
    });
    this.glow = new Mesh(glowGeo, this.glowMat);
    this.glow.scale.setScalar(2.4);
    this.scene.add(this.glow);
    this.coreMat = additive(UV_VERTEX, GLOW_FRAGMENT, {
      uColor: { value: v3([0.85, 0.96, 1]) },
      uStrength: { value: 0.9 },
      uFalloff: { value: 5 },
    });
    const coreGlow = new Mesh(glowGeo, this.coreMat);
    coreGlow.scale.setScalar(0.85);
    this.scene.add(coreGlow);
    this.disposables.push(glowGeo);

    // Transparent shell: a rim glow plus a sparse latitude/longitude grid.
    this.shell = new Group();
    const shellGeo = new SphereGeometry(SHELL_RADIUS, 48, 32);
    this.shellMat = additive(SHELL_VERTEX, SHELL_FRAGMENT, { uColor: { value: v3(ICE) }, uStrength: { value: 0.32 } });
    this.shell.add(new Mesh(shellGeo, this.shellMat));
    const grid: number[] = [];
    const seg = 64;
    for (const lat of [-0.66, -0.33, 0, 0.33, 0.66]) {
      const r = Math.sqrt(1 - lat * lat) * SHELL_RADIUS;
      const y = lat * SHELL_RADIUS;
      for (let k = 0; k < seg; k++) {
        const a0 = (k / seg) * Math.PI * 2;
        const a1 = ((k + 1) / seg) * Math.PI * 2;
        grid.push(Math.cos(a0) * r, y, Math.sin(a0) * r, Math.cos(a1) * r, y, Math.sin(a1) * r);
      }
    }
    for (let m = 0; m < 6; m++) {
      const phi = (m / 6) * Math.PI;
      for (let k = 0; k < seg; k++) {
        const t0 = (k / seg) * Math.PI * 2;
        const t1 = ((k + 1) / seg) * Math.PI * 2;
        const p = (t: number) => [Math.cos(t) * Math.cos(phi) * SHELL_RADIUS, Math.sin(t) * SHELL_RADIUS, Math.cos(t) * Math.sin(phi) * SHELL_RADIUS];
        grid.push(...p(t0), ...p(t1));
      }
    }
    const gridGeo = new BufferGeometry();
    gridGeo.setAttribute("position", new BufferAttribute(new Float32Array(grid), 3));
    gridGeo.setAttribute("aAct", new BufferAttribute(new Float32Array(grid.length / 3), 1));
    this.gridMat = additive(LINE_VERTEX, LINE_FRAGMENT, { uDim: { value: v3(ICE_DIM) }, uHot: { value: v3(ICE) }, uIntensity: { value: -0.55 }, uAlpha: { value: 1 } });
    this.shell.add(new LineSegments(gridGeo, this.gridMat));
    this.shell.rotation.x = 0.35;
    this.scene.add(this.shell);
    this.disposables.push(shellGeo, gridGeo);

    // Success: one ring that widens and fades.
    const pulseGeo = new RingGeometry(0.96, 1, 96);
    this.pulseMat = additive(UV_VERTEX, /* glsl */ `
      uniform vec3 uColor;
      uniform float uStrength;
      varying vec2 vUv;
      void main() { gl_FragColor = vec4(uColor * uStrength, uStrength); }
    `, { uColor: { value: v3(ICE) }, uStrength: { value: 0 } });
    this.pulse = new Mesh(pulseGeo, this.pulseMat);
    this.pulse.visible = false;
    this.scene.add(this.pulse);
    this.disposables.push(pulseGeo);

    this.disposables.push(
      this.neuralGeo,
      this.lineGeo,
      this.neuralMat,
      this.lineMat,
      this.dustMat,
      this.glowMat,
      this.coreMat,
      this.shellMat,
      this.gridMat,
      this.pulseMat,
    );

    canvas.addEventListener("webglcontextlost", this.handleContextLost);
    window.addEventListener("focus", this.handleFocus);
    window.addEventListener("blur", this.handleFocus);
    document.addEventListener("visibilitychange", this.handleVisibility);
    this.focused = document.hasFocus();
  }

  /** Called when the GPU context is lost; the stage switches to its still fallback. */
  onContextLost: (() => void) | null = null;

  setState(state: CoreState, nodes: CoreNodeInput[], active: string | null) {
    if (state === "SUCCESS" && this.state !== "SUCCESS") this.pulseAge = 0;
    // A short disruption that settles, never a flashing alarm.
    if (state === "ERROR" && this.state !== "ERROR") this.tune.jitter = 0.018;
    const changed = state !== this.state;
    if (changed) this.stateSince = performance.now();
    this.state = state;
    this.target = TUNING[state];
    this.activeId = active;
    this.syncBeams(nodes);
    if (this.reduced) {
      this.stillFrame();
      return;
    }
    // A new state is shown at once, not after the current (possibly long) rest sleep.
    if (changed) this.halt();
    this.kick();
  }

  setReducedMotion(on: boolean) {
    this.reduced = on;
    if (on) {
      this.halt();
      this.stillFrame();
    } else {
      this.kick();
    }
  }

  resize(width: number, height: number) {
    this.width = Math.max(1, width);
    this.height = Math.max(1, height);
    this.renderer.setSize(this.width, this.height, false);
    this.camera.aspect = this.width / this.height;
    // Keep the shell fully visible in tall, narrow stages.
    this.camera.fov = this.camera.aspect < 1.1 ? 38 / Math.max(0.55, this.camera.aspect / 1.1) : 38;
    this.camera.updateProjectionMatrix();
    for (const beam of this.beams.values()) this.placeBeam(beam);
    this.publishLayout();
    if (this.reduced) this.stillFrame();
    else this.kick();
  }

  dispose() {
    this.disposed = true;
    this.halt();
    this.renderer.domElement.removeEventListener("webglcontextlost", this.handleContextLost);
    window.removeEventListener("focus", this.handleFocus);
    window.removeEventListener("blur", this.handleFocus);
    document.removeEventListener("visibilitychange", this.handleVisibility);
    for (const beam of this.beams.values()) this.disposeBeam(beam);
    this.beams.clear();
    for (const d of this.disposables) d.dispose();
    this.renderer.dispose();
  }

  // ---------- context nodes ----------

  private slotPosition(id: string): Vector3 {
    const [sx, sy] = SLOT[id] ?? [0, 1];
    const halfH = Math.tan((this.camera.fov * Math.PI) / 360) * this.camera.position.z;
    const halfW = halfH * this.camera.aspect;
    return new Vector3(sx * Math.min(2.65, halfW * 0.8), sy * Math.min(1.9, halfH * 1.25), 0);
  }

  private syncBeams(nodes: CoreNodeInput[]) {
    const wanted = new Set(nodes.map((n) => n.id));
    for (const [id, beam] of this.beams) if (!wanted.has(id)) beam.removing = true;
    for (const n of nodes) {
      let beam = this.beams.get(n.id);
      if (!beam) {
        beam = this.createBeam(n.id);
        this.beams.set(n.id, beam);
      }
      beam.removing = false;
      beam.status = n.status;
      beam.active = n.id === this.activeId;
    }
    this.publishLayout();
  }

  private createBeam(id: string): Beam {
    const lineGeo = new BufferGeometry();
    lineGeo.setAttribute("position", new BufferAttribute(new Float32Array(6), 3));
    lineGeo.setAttribute("aAct", new BufferAttribute(new Float32Array([0.2, 1]), 1));
    const lineMat = additive(LINE_VERTEX, LINE_FRAGMENT, { uDim: { value: v3(DONE) }, uHot: { value: v3(ICE) }, uIntensity: { value: 0 }, uAlpha: { value: 0 } });
    const line = new LineSegments(lineGeo, lineMat);

    const headGeo = new BufferGeometry();
    headGeo.setAttribute("position", new BufferAttribute(new Float32Array(9), 3));
    headGeo.setAttribute("aAct", new BufferAttribute(new Float32Array([1, 0.7, 0.45]), 1));
    headGeo.setAttribute("aSize", new BufferAttribute(new Float32Array([7, 5, 4]), 1));
    const headMat = additive(POINT_VERTEX, POINT_FRAGMENT, {
      uPixelRatio: { value: this.renderer.getPixelRatio() },
      uBase: { value: 4 },
      uDim: { value: v3(ICE) },
      uHot: { value: v3(ICE) },
      uIntensity: { value: 1 },
      uAlpha: { value: 0 },
    });
    const head = new Points(headGeo, headMat);

    const nodeGeo = new PlaneGeometry(1, 1);
    const nodeMat = additive(UV_VERTEX, GLOW_FRAGMENT, { uColor: { value: v3(ICE) }, uStrength: { value: 0 }, uFalloff: { value: 3 } });
    const node = new Mesh(nodeGeo, nodeMat);
    node.scale.setScalar(0.55);
    const ringGeo = new RingGeometry(0.15, 0.162, 48);
    const ringMat = additive(UV_VERTEX, `
      uniform vec3 uColor;
      uniform float uStrength;
      varying vec2 vUv;
      void main() { gl_FragColor = vec4(uColor * uStrength, uStrength); }
    `, { uColor: { value: v3(ICE) }, uStrength: { value: 0 } });
    const ring = new Mesh(ringGeo, ringMat);

    this.scene.add(line, head, node, ring);
    const beam: Beam = { id, target: new Vector3(), line, lineMat, head, headMat, node, nodeMat, ring, ringMat, status: "done", active: false, presence: 0, removing: false };
    this.placeBeam(beam);
    return beam;
  }

  private placeBeam(beam: Beam) {
    beam.target.copy(this.slotPosition(beam.id));
    const from = beam.target.clone().normalize().multiplyScalar(SHELL_RADIUS * 0.98);
    const pos = beam.line.geometry.getAttribute("position") as BufferAttribute;
    pos.setXYZ(0, from.x, from.y, from.z);
    pos.setXYZ(1, beam.target.x, beam.target.y, beam.target.z);
    pos.needsUpdate = true;
    beam.node.position.copy(beam.target);
    beam.ring.position.copy(beam.target);
  }

  private disposeBeam(beam: Beam) {
    this.scene.remove(beam.line, beam.head, beam.node, beam.ring);
    for (const obj of [beam.line, beam.head, beam.node, beam.ring]) obj.geometry.dispose();
    for (const m of [beam.lineMat, beam.headMat, beam.nodeMat, beam.ringMat]) m.dispose();
  }

  private publishLayout() {
    const points: ScreenPoints = {};
    // Labels stay inside the stage even when it is narrow.
    const margin = Math.min(80, this.width / 4);
    for (const [id, beam] of this.beams) {
      if (beam.removing) continue;
      const p = beam.target.clone().project(this.camera);
      const x = ((p.x + 1) / 2) * this.width;
      points[id] = { x: Math.max(margin, Math.min(this.width - margin, x)), y: ((1 - p.y) / 2) * this.height };
    }
    this.onLayout(points);
  }

  // ---------- animation ----------

  private kick() {
    if (this.disposed || this.reduced || this.raf || this.sleep || document.hidden) return;
    this.lastTick = performance.now();
    this.raf = requestAnimationFrame(this.frame);
  }

  private halt() {
    cancelAnimationFrame(this.raf);
    window.clearTimeout(this.sleep);
    this.raf = 0;
    this.sleep = 0;
  }

  private readonly frame = (now: number) => {
    this.raf = 0;
    if (this.disposed || this.reduced || document.hidden) return;
    const dt = Math.min(0.1, (now - this.lastTick) / 1000);
    this.lastTick = now;
    this.step(dt);
    this.renderer.render(this.scene, this.camera);
    // Sleep until the next frame this state needs instead of waking at
    // the display's refresh rate.
    const settled = [...this.beams.values()].every((b) => (b.removing ? b.presence < 0.02 : b.presence > 0.98));
    const resting = this.state === "IDLE" && now - this.stateSince > REST_AFTER_MS && this.pulseAge < 0 && settled;
    let fps = this.focused ? this.target.fps : Math.min(UNFOCUSED_FPS, this.target.fps);
    if (resting) fps = Math.min(fps, REST_FPS);
    const wait = 1000 / fps - (performance.now() - now);
    if (wait > 12) {
      this.sleep = window.setTimeout(() => {
        this.sleep = 0;
        if (!this.disposed && !this.reduced && !document.hidden) this.raf = requestAnimationFrame(this.frame);
      }, wait - 6);
    } else {
      this.raf = requestAnimationFrame(this.frame);
    }
  };

  private stillFrame() {
    if (this.disposed) return;
    // A representative, motionless picture of the state.
    this.tune = { ...this.target };
    const lit = this.target.fire > 3 ? 0.45 : this.target.fire > 1 ? 0.25 : 0.1;
    for (let i = 0; i < NEURAL_COUNT; i++) this.act[i] = (i * 7) % 10 < lit * 10 ? 0.75 : 0;
    this.syncActivation();
    this.pending.length = 0;
    this.pulseAge = -1;
    this.pulse.visible = false;
    this.shell.position.set(0, 0, 0);
    this.applyUniforms(0);
    for (const beam of this.beams.values()) {
      beam.presence = beam.removing ? 0 : 1;
      this.styleBeam(beam, 0, true);
    }
    this.pruneBeams();
    this.renderer.render(this.scene, this.camera);
  }

  private step(dt: number) {
    this.time += dt;
    const k = 1 - Math.exp(-dt * 3.5);
    const t = this.tune;
    const g = this.target;
    t.spin += (g.spin - t.spin) * k;
    t.fire += (g.fire - t.fire) * k;
    t.intensity += (g.intensity - t.intensity) * k;
    t.breathe += (g.breathe - t.breathe) * k;
    t.breatheHz += (g.breatheHz - t.breatheHz) * k;
    t.amber += (g.amber - t.amber) * k;
    t.red += (g.red - t.red) * k;
    t.jitter += (g.jitter - t.jitter) * (1 - Math.exp(-dt * 1.5));
    t.branches = g.branches;

    this.brain.rotation.y += t.spin * dt;
    this.dust.rotation.y -= t.spin * 0.6 * dt;
    this.shell.rotation.y += t.spin * 0.35 * dt;
    if (t.jitter > 0.0005) {
      this.shell.position.set(Math.sin(this.time * 37) * t.jitter, Math.cos(this.time * 29) * t.jitter, 0);
    } else {
      this.shell.position.set(0, 0, 0);
    }

    // Firing: new impulses at the state's rate, spreading to neighbours.
    this.fireDebt += t.fire * dt;
    while (this.fireDebt >= 1) {
      this.fireDebt -= 1;
      this.ignite();
    }
    for (let i = this.pending.length - 1; i >= 0; i--) {
      const p = this.pending[i];
      if (p.at > this.time) continue;
      this.pending.splice(i, 1);
      this.act[p.node] = Math.min(1, Math.max(this.act[p.node], p.strength));
      if (p.depth > 0) {
        const next = this.neighbours[p.node].filter((n) => !p.outward || this.neuralPos[n].lengthSq() > this.neuralPos[p.node].lengthSq());
        const fan = p.outward ? next.slice(0, 2) : next.slice(0, 1 + Math.floor(this.rand() * 2));
        for (const n of fan) this.pending.push({ node: n, at: this.time + 0.09 + this.rand() * 0.08, strength: p.strength * 0.78, depth: p.depth - 1, outward: p.outward });
      }
    }
    const decay = Math.exp(-dt / 0.55);
    for (let i = 0; i < NEURAL_COUNT; i++) this.act[i] *= decay;
    this.syncActivation();

    this.applyUniforms(this.time);

    // Success pulse.
    if (this.pulseAge >= 0) {
      this.pulseAge += dt;
      const p = this.pulseAge / 1.2;
      if (p >= 1) {
        this.pulseAge = -1;
        this.pulse.visible = false;
      } else {
        this.pulse.visible = true;
        this.pulse.scale.setScalar(SHELL_RADIUS * (1 + p * 0.55));
        this.pulseMat.uniforms.uStrength.value = (1 - p) * 0.55;
      }
    }

    for (const beam of this.beams.values()) {
      beam.presence += ((beam.removing ? 0 : 1) - beam.presence) * (1 - Math.exp(-dt * 5));
      this.styleBeam(beam, this.time, false);
    }
    this.pruneBeams();
  }

  /** Paths take the activation of the nodes they join. */
  private syncActivation() {
    this.edges.forEach(([a, b], e) => {
      this.lineAct[e * 2] = this.act[a];
      this.lineAct[e * 2 + 1] = this.act[b];
    });
    (this.neuralGeo.getAttribute("aAct") as BufferAttribute).needsUpdate = true;
    (this.lineGeo.getAttribute("aAct") as BufferAttribute).needsUpdate = true;
  }

  private ignite() {
    if (this.tune.branches) {
      // Several branches leaving the centre at once.
      const inner = this.neuralPos.map((p, i) => ({ i, d: p.lengthSq() })).sort((a, b) => a.d - b.d).slice(0, 8);
      for (let b = 0; b < 3; b++) {
        const start = inner[Math.floor(this.rand() * inner.length)].i;
        this.pending.push({ node: start, at: this.time + b * 0.05, strength: 1, depth: 4, outward: true });
      }
      return;
    }
    const node = Math.floor(this.rand() * NEURAL_COUNT);
    this.pending.push({ node, at: this.time, strength: 0.6 + this.rand() * 0.4, depth: 2 + Math.floor(this.rand() * 2), outward: false });
  }

  private tint(base: [number, number, number]): Vector3 {
    const t = this.tune;
    const out = v3(base);
    out.lerp(v3(AMBER), t.amber * 0.6);
    out.lerp(v3(RED), t.red * 0.7);
    return out;
  }

  private applyUniforms(time: number) {
    const t = this.tune;
    const breath = 1 + Math.sin(time * Math.PI * 2 * t.breatheHz) * t.breathe;
    this.glow.scale.setScalar(2.4 * breath);
    this.glowMat.uniforms.uStrength.value = 0.18 + t.intensity * 0.32;
    this.glowMat.uniforms.uColor.value = this.tint(ICE);
    this.coreMat.uniforms.uStrength.value = 0.45 + t.intensity * 0.55 * breath;
    this.shellMat.uniforms.uStrength.value = 0.22 + t.intensity * 0.18;
    this.shellMat.uniforms.uColor.value = this.tint(ICE);
    this.gridMat.uniforms.uDim.value = this.tint(ICE_DIM);
    for (const m of [this.neuralMat, this.lineMat]) {
      m.uniforms.uIntensity.value = t.intensity;
      m.uniforms.uHot.value = this.tint(ICE);
      m.uniforms.uDim.value = this.tint(ICE_DIM);
    }
    this.dustMat.uniforms.uIntensity.value = t.intensity * 0.6;
  }

  private styleBeam(beam: Beam, time: number, still: boolean) {
    const color = beam.status === "waiting" ? AMBER : beam.status === "failed" ? RED : beam.status === "active" ? ICE : DONE;
    const strong = beam.active || beam.status === "waiting";
    const p = beam.presence;
    beam.lineMat.uniforms.uHot.value = v3(color);
    beam.lineMat.uniforms.uDim.value = v3(color).multiplyScalar(0.35);
    beam.lineMat.uniforms.uIntensity.value = strong ? 0.6 : 0.05;
    beam.lineMat.uniforms.uAlpha.value = p * (strong ? 1 : 0.5);
    const throb = still ? 1 : 0.75 + 0.25 * Math.sin(time * (beam.status === "waiting" ? 1.6 : 4));
    beam.nodeMat.uniforms.uColor.value = v3(color);
    beam.nodeMat.uniforms.uStrength.value = (strong ? 0.9 * throb : 0.45) * p;
    beam.ringMat.uniforms.uColor.value = v3(color);
    beam.ringMat.uniforms.uStrength.value = (strong ? 0.7 : 0.3) * p;
    beam.ring.scale.setScalar(strong && !still ? 1 + 0.12 * Math.sin(time * 2.2) : 1);

    // Travelling impulse: outward to an active tool, inward from a waiting
    // approval (the core depends on it).
    const moving = !still && (beam.active || beam.status === "waiting");
    beam.headMat.uniforms.uAlpha.value = moving ? p : 0;
    beam.headMat.uniforms.uHot.value = v3(color);
    beam.headMat.uniforms.uDim.value = v3(color);
    if (moving) {
      const from = beam.target.clone().normalize().multiplyScalar(SHELL_RADIUS);
      const speed = beam.status === "waiting" ? 0.35 : 0.9;
      const pos = beam.head.geometry.getAttribute("position") as BufferAttribute;
      for (let i = 0; i < 3; i++) {
        let f = (time * speed - i * 0.045) % 1;
        if (f < 0) f += 1;
        if (beam.status === "waiting") f = 1 - f;
        const at = from.clone().lerp(beam.target, f);
        pos.setXYZ(i, at.x, at.y, at.z);
      }
      pos.needsUpdate = true;
    }
  }

  private pruneBeams() {
    for (const [id, beam] of this.beams) {
      if (beam.removing && beam.presence < 0.02) {
        this.disposeBeam(beam);
        this.beams.delete(id);
      }
    }
  }

  private readonly handleFocus = () => {
    this.focused = document.hasFocus();
    // Regaining focus should not wait out an unfocused 200 ms sleep.
    if (this.focused && this.sleep) {
      this.halt();
    }
    this.kick();
  };

  private readonly handleVisibility = () => {
    if (document.hidden) {
      this.halt();
    } else if (this.reduced) {
      this.stillFrame();
    } else {
      this.kick();
    }
  };

  private readonly handleContextLost = (event: Event) => {
    event.preventDefault();
    this.halt();
    this.onContextLost?.();
  };
}
