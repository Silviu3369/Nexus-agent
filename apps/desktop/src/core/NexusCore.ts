// The Nexus Core: a small Three.js scene whose motion encodes the state the
// Host publishes. It owns no runtime state and grants nothing; it only draws.
//
// Four layers, inside out: an energy core (one camera-facing shader quad:
// nucleus, breathing envelope, falloff), active paths (signal packets that
// travel node to node, often through the core between clusters), a neural
// lattice (clustered nodes with local links, a few long-range ones and
// spokes to the core, faded with depth) and an outer shell (a faint rim).
// Context nodes outside the shell (model, Web read, Memory, Notes, You) are
// joined to it by beams; a tool in use receives a stream of packets from the
// core that continues along its beam, while a tool that is only proposed
// gets a dashed beam and signals that stop at the shell (intent is not
// execution). The lattice also changes shape by state (it holds in when
// waiting, opens and trembles while thinking, a cluster scatters on an
// error), computed in the vertex shaders.
//
// Cost control: the camera never moves, geometry is fixed (160 nodes, ~300
// links, 180 particles, 24 packets), each layer is one draw call, nothing is
// allocated per frame, and frames are rendered on demand at a per-state rate
// (8 fps idle, 15 while an idle signal travels, 24-30 working, 5 unfocused,
// 3 after 20 s idle, none while hidden); reduced motion renders one still
// frame per change.

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
  Vector2,
  Vector3,
  WebGLRenderer,
} from "three";
import type { CoreState, NexusCoreVisualState } from "../model/execution.ts";

export type CoreNodeStatus = "active" | "waiting" | "failed" | "done";
export interface CoreNodeInput {
  id: string;
  status: CoreNodeStatus;
  /** Only proposed so far: nothing has been executed on it yet. */
  proposed?: boolean;
}
export type ScreenPoints = Record<string, { x: number; y: number }>;

interface Tuning {
  /** Lattice rotation, rad/s. */
  spin: number;
  /** Random packets per second. */
  packets: number;
  /** Cluster activations per second. */
  clusters: number;
  /** Lattice brightness, 0..1. */
  intensity: number;
  /** Energy of the core, 0..1. */
  core: number;
  breathe: number;
  breatheHz: number;
  /** Speed of the bands in the core's envelope. */
  bands: number;
  /** Seconds between packets sent to the active tool; 0 sends none. */
  flowEvery: number;
  /** Size of the lattice (1 at rest): held in, or opened up. */
  shape: number;
  /** Per-node tremble, in world units. */
  tremble: number;
  fps: number;
}

const TUNING: Record<CoreState, Tuning> = {
  IDLE: { spin: 0.04, packets: 0.12, clusters: 0, intensity: 0.5, core: 0.55, breathe: 0.07, breatheHz: 0.16, bands: 0.18, flowEvery: 0, shape: 1, tremble: 0.004, fps: 8 },
  LISTENING: { spin: 0.05, packets: 0.35, clusters: 0, intensity: 0.62, core: 0.72, breathe: 0.08, breatheHz: 0.4, bands: 0.35, flowEvery: 0, shape: 1, tremble: 0.004, fps: 30 },
  THINKING: { spin: 0.075, packets: 3.6, clusters: 1.3, intensity: 0.78, core: 0.9, breathe: 0.09, breatheHz: 0.95, bands: 0.9, flowEvery: 1.2, shape: 1.035, tremble: 0.02, fps: 30 },
  PLANNING: { spin: 0.055, packets: 0.25, clusters: 0, intensity: 0.72, core: 0.85, breathe: 0.07, breatheHz: 0.6, bands: 0.55, flowEvery: 0, shape: 1.02, tremble: 0.01, fps: 30 },
  USING_TOOL: { spin: 0.05, packets: 0.5, clusters: 0, intensity: 0.7, core: 0.88, breathe: 0.06, breatheHz: 0.55, bands: 0.6, flowEvery: 0.32, shape: 1, tremble: 0.008, fps: 30 },
  EXECUTING: { spin: 0.045, packets: 0, clusters: 0, intensity: 0.82, core: 1, breathe: 0.035, breatheHz: 0.5, bands: 0.7, flowEvery: 0.3, shape: 0.985, tremble: 0, fps: 30 },
  WAITING_APPROVAL: { spin: 0.012, packets: 0.05, clusters: 0, intensity: 0.3, core: 0.82, breathe: 0.06, breatheHz: 0.12, bands: 0.08, flowEvery: 0, shape: 0.93, tremble: 0, fps: 8 },
  SUCCESS: { spin: 0.05, packets: 0.3, clusters: 0, intensity: 0.68, core: 0.85, breathe: 0.06, breatheHz: 0.3, bands: 0.3, flowEvery: 0, shape: 1, tremble: 0.004, fps: 30 },
  ERROR: { spin: 0.025, packets: 0.08, clusters: 0, intensity: 0.42, core: 0.65, breathe: 0.04, breatheHz: 0.2, bands: 0.15, flowEvery: 0, shape: 1, tremble: 0.004, fps: 24 },
};

const NODE_COUNT = 160;
const CLUSTER_COUNT = 10;
const PER_CLUSTER = 12;
/** Graph index of the energy core (not drawn as a node). */
const CORE = NODE_COUNT;
const DUST_COUNT = 180;
const PACKET_COUNT = 24;
const TAIL = 4;
const SHELL_RADIUS = 1.28;
const LATTICE_RADIUS = 1.12;
/** The lattice is seen slightly from above, so its rotation shows depth. */
const BASE_TILT = 0.3;

const UNFOCUSED_FPS = 5;
/** While a packet, a wave or a beam is moving; calm states move slowly. */
const MOTION_FPS = 24;
const CALM_MOTION_FPS = 15;
const POINTER_FPS = 30;
/** After 20 s in IDLE the core mostly breathes, a few frames a second, with
 *  a rare signal (measured: each WebGL frame costs far more than its pixels,
 *  so the frame count is what the idle cost follows). */
const REST_AFTER_MS = 20_000;
const REST_FPS = 3;
const REST_PACKETS = 0.03;

// Colours are display (sRGB) values; the shaders output them as is.
const ICE = new Vector3(0.49, 0.83, 0.99);
const ICE_DIM = new Vector3(0.2, 0.33, 0.46);
const NODE_DIM = new Vector3(0.3, 0.5, 0.66);
const ENVELOPE = new Vector3(0.38, 0.78, 0.98);
const AMBER = new Vector3(0.98, 0.75, 0.32);
const RED = new Vector3(0.97, 0.45, 0.45);
const GREEN = new Vector3(0.45, 0.93, 0.72);
const DONE = new Vector3(0.42, 0.55, 0.66);
const PACKET = new Vector3(0.82, 0.95, 1);
const PACKET_DIM = new Vector3(0.36, 0.6, 0.78);

/** Fixed slots, so a node never jumps when others appear. */
const SLOT: Record<string, [number, number]> = {
  model: [-1, 0.42],
  web: [1, 0.42],
  files: [1, -0.55],
  approval: [-1, -0.55],
  memory: [-1.05, -0.06],
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

// Depth cue shared by the lattice layers: the camera sits at z = 6.2 and the
// shell spans about 4.9..7.5 in view depth; far nodes are smaller and dimmer.
const DEPTH = /* glsl */ `
  float depthOf(vec4 mv) { return clamp((-mv.z - 4.95) / 2.5, 0.0, 1.0); }
`;

// The lattice's shape by state: held in or opened up (uShape), a fine
// tremble (uTremble), and the scatter of one cluster on an error. The core
// itself (cluster -2) never moves.
const SHAPE = /* glsl */ `
  uniform float uShape;
  uniform float uTremble;
  uniform float uHitCluster;
  uniform float uScatter;
  vec3 shaped(vec3 p, float seed, float cluster, float time) {
    if (cluster < -1.5) return p;
    vec3 q = p * uShape;
    q += uTremble * vec3(sin(time * 1.7 + seed * 20.0), sin(time * 1.3 + seed * 13.0), sin(time * 2.1 + seed * 7.0));
    if (abs(cluster - uHitCluster) < 0.5) q += normalize(p) * uScatter * (0.08 + 0.07 * fract(seed * 7.0));
    return q;
  }
`;

// A wave front moving through the lattice (inward while listening, outward
// on success): a radial band around uWave.
const WAVE = /* glsl */ `
  uniform float uWave;
  uniform float uWaveAmp;
  float waveAt(vec3 p) {
    float d = (length(p) - uWave) / 0.17;
    return uWaveAmp * exp(-d * d);
  }
`;

const NODE_VERTEX = /* glsl */ `
  attribute float aAct;
  attribute float aSize;
  attribute float aSeed;
  attribute float aCluster;
  uniform float uPixelRatio;
  uniform float uTime;
  uniform float uTwinkle;
  varying float vAct;
  varying float vDepth;
  varying float vWave;
  ${DEPTH}
  ${WAVE}
  ${SHAPE}
  void main() {
    vec3 p = shaped(position, aSeed, aCluster, uTime);
    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    float depth = depthOf(mv);
    vDepth = 1.0 - depth * 0.72;
    vWave = waveAt(p);
    vAct = aAct;
    float twinkle = 1.0 + uTwinkle * sin(uTime * 0.7 + aSeed * 6.2831);
    float size = aSize * twinkle + (aAct + vWave) * 9.0;
    gl_PointSize = size * uPixelRatio * (6.0 / -mv.z) * (0.7 + 0.3 * vDepth);
    gl_Position = projectionMatrix * mv;
  }
`;

const NODE_FRAGMENT = /* glsl */ `
  uniform vec3 uDim;
  uniform vec3 uHot;
  uniform vec3 uWaveColor;
  uniform float uIntensity;
  varying float vAct;
  varying float vDepth;
  varying float vWave;
  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    float act = clamp(vAct + vWave, 0.0, 1.0);
    float dotA = smoothstep(0.55, 0.25, r);
    float halo = pow(max(0.0, 1.0 - r), 2.0) * (0.2 + 0.7 * act);
    vec3 c = mix(uDim, uHot, act);
    c = mix(c, uWaveColor, clamp(vWave, 0.0, 1.0) * 0.6);
    float light = (0.6 + uIntensity * 0.7 + act * 1.1) * vDepth;
    gl_FragColor = vec4(c * (dotA + halo) * light, 1.0);
  }
`;

const LINK_VERTEX = /* glsl */ `
  attribute float aAct;
  attribute float aSeed;
  attribute float aCluster;
  uniform float uTime;
  varying float vAct;
  varying float vDepth;
  varying float vWave;
  ${DEPTH}
  ${WAVE}
  ${SHAPE}
  void main() {
    // Each end follows its node exactly.
    vec3 p = shaped(position, aSeed, aCluster, uTime);
    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    vDepth = 1.0 - depthOf(mv) * 0.82;
    vWave = waveAt(p);
    vAct = aAct;
    gl_Position = projectionMatrix * mv;
  }
`;

const LINK_FRAGMENT = /* glsl */ `
  uniform vec3 uDim;
  uniform vec3 uHot;
  uniform vec3 uWaveColor;
  uniform float uIntensity;
  varying float vAct;
  varying float vDepth;
  varying float vWave;
  void main() {
    float act = clamp(vAct + vWave, 0.0, 1.0);
    vec3 c = mix(uDim, uHot, act);
    c = mix(c, uWaveColor, clamp(vWave, 0.0, 1.0) * 0.6);
    // A negative activation is a broken link: darker than at rest.
    float a = max(0.0, 0.12 + uIntensity * 0.18 + vAct * 0.95 + vWave * 0.6) * vDepth;
    gl_FragColor = vec4(c * a, 1.0);
  }
`;

const PACKET_VERTEX = /* glsl */ `
  attribute float aAct;
  attribute float aSize;
  attribute vec3 aColor;
  uniform float uPixelRatio;
  uniform float uShape;
  varying float vAct;
  varying float vDepth;
  varying vec3 vColor;
  ${DEPTH}
  void main() {
    // Signals ride the lattice as it holds in or opens up.
    vec4 mv = modelViewMatrix * vec4(position * uShape, 1.0);
    vDepth = 1.0 - depthOf(mv) * 0.6;
    vAct = aAct;
    vColor = aColor;
    gl_PointSize = aSize * uPixelRatio * (6.0 / -mv.z);
    gl_Position = projectionMatrix * mv;
  }
`;

const PACKET_FRAGMENT = /* glsl */ `
  varying float vAct;
  varying float vDepth;
  varying vec3 vColor;
  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    float a = smoothstep(1.0, 0.0, r);
    a *= a;
    gl_FragColor = vec4(vColor * a * vAct * 2.0 * vDepth, 1.0);
  }
`;

const DUST_VERTEX = /* glsl */ `
  attribute float aSize;
  attribute float aSeed;
  uniform float uPixelRatio;
  uniform float uTime;
  uniform float uPull;
  varying float vDepth;
  varying float vSeed;
  ${DEPTH}
  void main() {
    vec3 p = position;
    p += 0.04 * vec3(sin(uTime * 0.13 + aSeed * 12.0), sin(uTime * 0.11 + aSeed * 7.0), sin(uTime * 0.17 + aSeed * 3.0));
    // Listening: a slow breath inward.
    p *= 1.0 - uPull * 0.1 * (0.5 + 0.5 * sin(uTime * 2.6 - length(position) * 6.0));
    vec4 mv = modelViewMatrix * vec4(p, 1.0);
    vDepth = 1.0 - depthOf(mv) * 0.75;
    vSeed = aSeed;
    gl_PointSize = aSize * uPixelRatio * (6.0 / -mv.z);
    gl_Position = projectionMatrix * mv;
  }
`;

const DUST_FRAGMENT = /* glsl */ `
  uniform vec3 uColor;
  uniform float uIntensity;
  varying float vDepth;
  varying float vSeed;
  void main() {
    float r = length(gl_PointCoord - 0.5) * 2.0;
    float a = smoothstep(1.0, 0.0, r);
    gl_FragColor = vec4(uColor * a * (0.2 + uIntensity * 0.3) * (0.5 + 0.5 * vSeed) * vDepth, 1.0);
  }
`;

const UV_VERTEX = /* glsl */ `
  varying vec2 vUv;
  void main() {
    vUv = uv;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

// The energy core, in quad coordinates (-1..1 across the quad):
// a small near-white nucleus, a translucent envelope with slow bands and a
// brighter rim (so it reads as a volume), and a falloff into the lattice.
const CORE_FRAGMENT = /* glsl */ `
  uniform float uTime;
  uniform float uEnergy;
  uniform float uBreath;
  uniform float uBands;
  uniform vec3 uEnv;
  uniform vec3 uAccent;
  uniform float uAccentAmt;
  varying vec2 vUv;
  void main() {
    vec2 p = (vUv - 0.5) * 2.0;
    float r = length(p);
    float nucleus = exp(-r * r * 1100.0) * 1.15 + exp(-r * r * 170.0) * 0.5;
    float R = 0.27 * uBreath;
    float inside = 1.0 - smoothstep(R * 0.82, R, r);
    float rim = smoothstep(R * 0.5, R * 0.97, r) * inside;
    float ang = atan(p.y, p.x);
    float bands = 0.5 + 0.5 * sin(ang * 3.0 + uTime * uBands + sin(r * 26.0 - uTime * uBands * 1.7) * 0.9);
    float swirl = 0.5 + 0.5 * sin(r * 38.0 - uTime * (0.4 + uBands * 1.4));
    float env = inside * (0.035 + 0.17 * bands * swirl) + rim * rim * 0.42;
    float fall = exp(-r * 6.0) * 0.2;
    vec3 tint = mix(uEnv, uAccent, uAccentAmt);
    vec3 c = vec3(0.9, 0.97, 1.0) * nucleus * (0.55 + 0.45 * uEnergy)
      + tint * env * (0.45 + 0.6 * uEnergy)
      + tint * fall * (0.35 + 0.65 * uEnergy);
    gl_FragColor = vec4(c, 1.0);
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
    float a = (pow(vRim, 5.0) + pow(vRim, 1.5) * 0.04) * uStrength;
    gl_FragColor = vec4(uColor * a, 1.0);
  }
`;

// Beams to the context nodes keep their original look.
const BEAM_POINT_VERTEX = /* glsl */ `
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

const BEAM_POINT_FRAGMENT = /* glsl */ `
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

const BEAM_LINE_VERTEX = /* glsl */ `
  attribute float aAct;
  attribute float aDist;
  varying float vAct;
  varying float vDist;
  void main() {
    vAct = aAct;
    vDist = aDist;
    gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
  }
`;

const BEAM_LINE_FRAGMENT = /* glsl */ `
  uniform vec3 uDim;
  uniform vec3 uHot;
  uniform float uIntensity;
  uniform float uAlpha;
  uniform float uDash;
  varying float vAct;
  varying float vDist;
  void main() {
    // A proposal is drawn dashed: an intent, not an executed effect.
    if (uDash > 0.5 && fract(vDist * 7.0) > 0.5) discard;
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

const FLAT_FRAGMENT = /* glsl */ `
  uniform vec3 uColor;
  uniform float uStrength;
  varying vec2 vUv;
  void main() { gl_FragColor = vec4(uColor * uStrength, uStrength); }
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

interface Beam {
  id: string;
  target: Vector3;
  /** Where the beam leaves the shell, towards its node. */
  from: Vector3;
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
  proposed: boolean;
  /** 0..1 while appearing, so a node fades in instead of popping. */
  presence: number;
  removing: boolean;
}

/** A signal travelling along a route of lattice nodes (CORE included). */
interface Packet {
  alive: boolean;
  route: number[];
  seg: number;
  /** Progress along the current segment, 0..1. */
  t: number;
  /** World units per second. */
  speed: number;
  strength: number;
  color: Vector3;
}

const IDLE_VISUAL: NexusCoreVisualState = {
  state: "IDLE",
  activeTool: null,
  activeModel: null,
  activityIntensity: 0,
  hasPendingApproval: false,
  lastResult: null,
};

export class NexusCore {
  private readonly renderer: WebGLRenderer;
  private readonly scene = new Scene();
  private readonly camera = new PerspectiveCamera(38, 1, 0.1, 50);
  /** Base tilt plus the pointer's parallax. */
  private readonly orient = new Group();
  private readonly lattice = new Group();
  private readonly dust = new Group();
  private readonly beams = new Map<string, Beam>();
  private readonly disposables: { dispose(): void }[] = [];

  // Lattice graph. Index CORE is the energy core at the origin.
  private readonly nodes: Vector3[] = [];
  private readonly cluster = new Int16Array(NODE_COUNT + 1).fill(-1);
  private readonly hubs: number[] = [];
  private readonly members: number[][] = [];
  private readonly adj: number[][] = [];
  private readonly edgeIndex = new Map<number, number>();
  private edges = new Uint16Array(0);
  private edgeCount = 0;

  private readonly act = new Float32Array(NODE_COUNT);
  private edgeAct = new Float32Array(0);
  private edgeBreak = new Float32Array(0);
  private lineAct = new Float32Array(0);
  private coreKick = 0;
  // Breadth-first search buffers, reused for every route.
  private readonly bfsPrev = new Int16Array(NODE_COUNT + 1);
  private readonly bfsQueue = new Int16Array(NODE_COUNT + 1);

  private readonly packets: Packet[] = [];
  private alivePackets = 0;
  private readonly packetPos = new Float32Array(PACKET_COUNT * TAIL * 3);
  private readonly packetAct = new Float32Array(PACKET_COUNT * TAIL);
  private readonly packetSize = new Float32Array(PACKET_COUNT * TAIL);
  private readonly packetColor = new Float32Array(PACKET_COUNT * TAIL * 3);

  private readonly nodeGeo: BufferGeometry;
  private readonly linkGeo: BufferGeometry;
  private readonly packetGeo: BufferGeometry;
  private readonly nodeMat: ShaderMaterial;
  private readonly linkMat: ShaderMaterial;
  private readonly packetMat: ShaderMaterial;
  private readonly dustMat: ShaderMaterial;
  private readonly coreMat: ShaderMaterial;
  private readonly shellMat: ShaderMaterial;
  private readonly pixelRatio: { value: number };
  /** Shared by the node and link materials. */
  private readonly wave = { r: { value: 0 }, amp: { value: 0 }, color: { value: ICE.clone() } };
  /** Shared by the lattice materials. */
  private readonly shape = { size: { value: 1 }, tremble: { value: 0 }, hit: { value: -10 }, scatter: { value: 0 } };
  /** Signals towards proposed tools, which stop at the shell. */
  private proposalTimer = 0;

  private visual: NexusCoreVisualState = IDLE_VISUAL;
  private state: CoreState = "IDLE";
  private stateSince = performance.now();
  private tune: Tuning = { ...TUNING.IDLE };
  private target: Tuning = TUNING.IDLE;
  private activeId: string | null = null;
  private reduced: boolean;
  private focused = true;
  private raf = 0;
  private sleep = 0;
  private wakeAt = 0;
  private lastTick = 0;
  private time = 0;
  private packetDebt = 0;
  private clusterDebt = 0;
  private flowTimer = 0;
  private execIndex = 0;
  // Waves: listening (inward, repeating) and success (outward, once).
  private listenPhase = 0;
  private successAge = -1;
  // Error: links break, then mend.
  private errorAge = -1;
  private jitter = 0;
  // Planning: try three routes, then follow one.
  private planAge = -1;
  private readonly planRoutes: number[][] = [];
  private planChosen = -1;
  // Accent colour in the core's envelope only (amber, red, green).
  private readonly accentColor = AMBER.clone();
  private accent = 0;
  private accentGoal = 0;
  // Pointer parallax.
  private readonly pointer = new Vector2();
  private readonly pointerGoal = new Vector2();
  private pointerUntil = 0;

  private width = 1;
  private height = 1;
  private readonly rand = mulberry32(0x6e78);
  private readonly tmp = new Vector3();
  private readonly tmp2 = new Vector3();
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
    this.pixelRatio = { value: this.renderer.getPixelRatio() };

    this.buildLattice();
    const time = { value: 0 };

    // Nodes.
    const nodePositions = new Float32Array(NODE_COUNT * 3);
    const nodeSizes = new Float32Array(NODE_COUNT);
    const nodeSeeds = new Float32Array(NODE_COUNT);
    for (let i = 0; i < NODE_COUNT; i++) {
      this.nodes[i].toArray(nodePositions, i * 3);
      const hub = this.hubs.includes(i);
      nodeSizes[i] = hub ? 8.5 + this.rand() * 2 : this.cluster[i] >= 0 ? 3.6 + this.rand() * 3 : 2.6 + this.rand() * 2.4;
      nodeSeeds[i] = this.rand();
    }
    this.nodeGeo = new BufferGeometry();
    this.nodeGeo.setAttribute("position", new BufferAttribute(nodePositions, 3));
    this.nodeGeo.setAttribute("aAct", new BufferAttribute(this.act, 1));
    this.nodeGeo.setAttribute("aSize", new BufferAttribute(nodeSizes, 1));
    this.nodeGeo.setAttribute("aSeed", new BufferAttribute(nodeSeeds, 1));
    const nodeClusters = new Float32Array(NODE_COUNT);
    for (let i = 0; i < NODE_COUNT; i++) nodeClusters[i] = this.cluster[i];
    this.nodeGeo.setAttribute("aCluster", new BufferAttribute(nodeClusters, 1));
    this.nodeMat = additive(NODE_VERTEX, NODE_FRAGMENT, {
      uPixelRatio: this.pixelRatio,
      uTime: time,
      uTwinkle: { value: 0.15 },
      uDim: { value: NODE_DIM.clone() },
      uHot: { value: ICE.clone() },
      uIntensity: { value: 0.5 },
      uWave: this.wave.r,
      uWaveAmp: this.wave.amp,
      uWaveColor: this.wave.color,
      ...this.shapeUniforms(),
    });

    // Links.
    const linkPositions = new Float32Array(this.edgeCount * 6);
    const linkSeeds = new Float32Array(this.edgeCount * 2);
    const linkClusters = new Float32Array(this.edgeCount * 2);
    for (let e = 0; e < this.edgeCount; e++) {
      for (let end = 0; end < 2; end++) {
        const n = this.edges[e * 2 + end];
        this.nodes[n].toArray(linkPositions, e * 6 + end * 3);
        linkSeeds[e * 2 + end] = n === CORE ? 0 : nodeSeeds[n];
        linkClusters[e * 2 + end] = n === CORE ? -2 : this.cluster[n];
      }
    }
    this.linkGeo = new BufferGeometry();
    this.linkGeo.setAttribute("position", new BufferAttribute(linkPositions, 3));
    this.linkGeo.setAttribute("aAct", new BufferAttribute(this.lineAct, 1));
    this.linkGeo.setAttribute("aSeed", new BufferAttribute(linkSeeds, 1));
    this.linkGeo.setAttribute("aCluster", new BufferAttribute(linkClusters, 1));
    this.linkMat = additive(LINK_VERTEX, LINK_FRAGMENT, {
      uTime: time,
      uDim: { value: ICE_DIM.clone() },
      uHot: { value: ICE.clone() },
      uIntensity: { value: 0.5 },
      uWave: this.wave.r,
      uWaveAmp: this.wave.amp,
      uWaveColor: this.wave.color,
      ...this.shapeUniforms(),
    });

    // Packets: a short tail of points each; unused points have no size.
    for (let i = 0; i < PACKET_COUNT; i++) {
      this.packets.push({ alive: false, route: [], seg: 0, t: 0, speed: 1, strength: 1, color: PACKET });
    }
    this.packetGeo = new BufferGeometry();
    this.packetGeo.setAttribute("position", new BufferAttribute(this.packetPos, 3));
    this.packetGeo.setAttribute("aAct", new BufferAttribute(this.packetAct, 1));
    this.packetGeo.setAttribute("aSize", new BufferAttribute(this.packetSize, 1));
    this.packetGeo.setAttribute("aColor", new BufferAttribute(this.packetColor, 3));
    this.packetMat = additive(PACKET_VERTEX, PACKET_FRAGMENT, { uPixelRatio: this.pixelRatio, uShape: this.shape.size });
    this.packetAttributes = ["position", "aAct", "aSize", "aColor"].map((n) => this.packetGeo.getAttribute(n) as BufferAttribute);

    this.lattice.add(new LineSegments(this.linkGeo, this.linkMat));
    this.lattice.add(new Points(this.nodeGeo, this.nodeMat));
    const packets = new Points(this.packetGeo, this.packetMat);
    packets.frustumCulled = false;
    this.lattice.add(packets);
    this.orient.add(this.lattice);

    // Micro particles between the lattice and the shell, drifting on the GPU.
    const dustPos = new Float32Array(DUST_COUNT * 3);
    const dustSize = new Float32Array(DUST_COUNT);
    const dustSeed = new Float32Array(DUST_COUNT);
    for (let i = 0; i < DUST_COUNT; i++) {
      this.tmp.set(this.rand() * 2 - 1, this.rand() * 2 - 1, this.rand() * 2 - 1).normalize();
      this.tmp.multiplyScalar(0.3 + Math.cbrt(this.rand()) * 0.92).toArray(dustPos, i * 3);
      dustSize[i] = 1.6 + this.rand() * 2.2;
      dustSeed[i] = this.rand();
    }
    const dustGeo = new BufferGeometry();
    dustGeo.setAttribute("position", new BufferAttribute(dustPos, 3));
    dustGeo.setAttribute("aSize", new BufferAttribute(dustSize, 1));
    dustGeo.setAttribute("aSeed", new BufferAttribute(dustSeed, 1));
    this.dustMat = additive(DUST_VERTEX, DUST_FRAGMENT, {
      uPixelRatio: this.pixelRatio,
      uTime: time,
      uPull: { value: 0 },
      uColor: { value: ICE.clone() },
      uIntensity: { value: 0.5 },
    });
    this.dust.add(new Points(dustGeo, this.dustMat));
    this.orient.add(this.dust);
    this.orient.rotation.x = BASE_TILT;
    this.scene.add(this.orient);

    // Energy core: one camera-facing quad (the camera never moves).
    const coreGeo = new PlaneGeometry(1, 1);
    this.coreMat = additive(UV_VERTEX, CORE_FRAGMENT, {
      uTime: time,
      uEnergy: { value: 0.55 },
      uBreath: { value: 1 },
      uBands: { value: 0.2 },
      uEnv: { value: ENVELOPE.clone() },
      uAccent: { value: AMBER.clone() },
      uAccentAmt: { value: 0 },
    });
    const core = new Mesh(coreGeo, this.coreMat);
    core.scale.setScalar(1.7);
    this.scene.add(core);

    // Outer shell: a faint rim, so the volume reads without a glass ball.
    const shellGeo = new SphereGeometry(SHELL_RADIUS, 48, 32);
    this.shellMat = additive(SHELL_VERTEX, SHELL_FRAGMENT, { uColor: { value: ICE.clone() }, uStrength: { value: 0.3 } });
    this.scene.add(new Mesh(shellGeo, this.shellMat));

    this.disposables.push(
      this.nodeGeo,
      this.linkGeo,
      this.packetGeo,
      dustGeo,
      coreGeo,
      shellGeo,
      this.nodeMat,
      this.linkMat,
      this.packetMat,
      this.dustMat,
      this.coreMat,
      this.shellMat,
    );
    this.timeUniform = time;

    canvas.addEventListener("webglcontextlost", this.handleContextLost);
    window.addEventListener("focus", this.handleFocus);
    window.addEventListener("blur", this.handleFocus);
    document.addEventListener("visibilitychange", this.handleVisibility);
    this.focused = document.hasFocus();
  }

  private readonly timeUniform: { value: number };

  private shapeUniforms() {
    return {
      uShape: this.shape.size,
      uTremble: this.shape.tremble,
      uHitCluster: this.shape.hit,
      uScatter: this.shape.scatter,
    };
  }
  private readonly packetAttributes: BufferAttribute[];

  /** Called when the GPU context is lost; the stage switches to its still fallback. */
  onContextLost: (() => void) | null = null;

  setState(visual: NexusCoreVisualState, nodes: CoreNodeInput[]) {
    const state = visual.state;
    const changed = state !== this.state;
    if (changed) {
      this.enter(state);
      this.stateSince = performance.now();
    }
    this.visual = visual;
    this.state = state;
    this.target = TUNING[state];
    this.activeId = visual.activeTool;
    this.accentGoal = this.restingAccent();
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

  /** Pointer over the stage, -1..1 on both axes; (0, 0) when it leaves. */
  setPointer(x: number, y: number) {
    if (this.reduced) return;
    this.pointerGoal.set(Math.max(-1, Math.min(1, x)), Math.max(-1, Math.min(1, y)));
    const now = performance.now();
    this.pointerUntil = now + 900;
    // Wake a long rest sleep, but never render faster than the pointer rate.
    if (this.sleep && this.wakeAt - now > 1000 / POINTER_FPS) this.halt();
    this.kick();
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

  // ---------- lattice ----------

  /** Clusters of nodes around hubs, some scattered nodes, local links, a
   *  backbone between hubs, a few long links and spokes to the core. */
  private buildLattice() {
    const rand = this.rand;
    const gauss = () => {
      let u = 0;
      while (u === 0) u = rand();
      return Math.sqrt(-2 * Math.log(u)) * Math.cos(2 * Math.PI * rand());
    };
    const fit = (p: Vector3) => {
      const len = p.length();
      if (len > LATTICE_RADIUS) p.multiplyScalar(LATTICE_RADIUS / len);
      if (len < 0.32) p.multiplyScalar(0.32 / Math.max(len, 1e-3));
      return p;
    };

    let i = 0;
    for (let c = 0; c < CLUSTER_COUNT; c++) {
      const y = 1 - (2 * (c + 0.5)) / CLUSTER_COUNT;
      const ring = Math.sqrt(1 - y * y);
      const theta = c * 2.399963 + rand() * 0.6;
      const centre = new Vector3(Math.cos(theta) * ring, y * 0.85, Math.sin(theta) * ring)
        .normalize()
        .multiplyScalar(0.5 + rand() * 0.45);
      this.members.push([]);
      for (let k = 0; k < PER_CLUSTER; k++, i++) {
        const p = k === 0 ? centre.clone() : fit(centre.clone().add(this.tmp.set(gauss(), gauss(), gauss()).multiplyScalar(0.12)));
        this.nodes.push(p);
        this.cluster[i] = c;
        this.members[c].push(i);
        if (k === 0) this.hubs.push(i);
      }
    }
    for (; i < NODE_COUNT; i++) {
      const dir = new Vector3(rand() * 2 - 1, rand() * 2 - 1, rand() * 2 - 1).normalize();
      this.nodes.push(dir.multiplyScalar(0.36 + Math.cbrt(rand()) * (LATTICE_RADIUS - 0.36)));
    }
    this.nodes.push(new Vector3(0, 0, 0)); // CORE
    for (let n = 0; n <= NODE_COUNT; n++) this.adj.push([]);

    const pairs: number[] = [];
    const link = (a: number, b: number) => {
      if (a === b) return;
      const lo = Math.min(a, b);
      const hi = Math.max(a, b);
      const key = lo * 1024 + hi;
      if (this.edgeIndex.has(key)) return;
      this.edgeIndex.set(key, pairs.length / 2);
      pairs.push(lo, hi);
      this.adj[lo].push(hi);
      this.adj[hi].push(lo);
    };
    const nearest = (n: number, among: number[], count: number) =>
      among
        .filter((m) => m !== n)
        .map((m) => ({ m, d: this.nodes[m].distanceToSquared(this.nodes[n]) }))
        .sort((a, b) => a.d - b.d)
        .slice(0, count);
    const everyone = Array.from({ length: NODE_COUNT }, (_, n) => n);

    for (let n = 0; n < NODE_COUNT; n++) {
      const c = this.cluster[n];
      if (c >= 0) for (const { m } of nearest(n, this.members[c], 2)) link(n, m);
      const [close] = nearest(n, everyone, 1);
      if (close && close.d < 0.36 * 0.36) link(n, close.m);
    }
    for (const h of this.hubs) for (const { m } of nearest(h, this.hubs, 2)) link(h, m);
    for (let tries = 0, long = 0; long < 5 && tries < 60; tries++) {
      const a = this.hubs[Math.floor(rand() * this.hubs.length)];
      const b = this.hubs[Math.floor(rand() * this.hubs.length)];
      const key = Math.min(a, b) * 1024 + Math.max(a, b);
      if (a !== b && !this.edgeIndex.has(key) && this.nodes[a].distanceTo(this.nodes[b]) > 1) {
        link(a, b);
        long++;
      }
    }
    const inner = [...this.hubs].sort((a, b) => this.nodes[a].lengthSq() - this.nodes[b].lengthSq());
    for (const h of inner.slice(0, 5)) link(h, CORE);
    for (const { m } of nearest(CORE, everyone, 2)) link(m, CORE);

    // Every node must be reachable from the core, so routes always exist.
    for (;;) {
      const reached = this.reachable();
      const lost = everyone.filter((n) => !reached[n]);
      if (lost.length === 0) break;
      const found = everyone.filter((n) => reached[n]);
      const [to] = nearest(lost[0], found, 1);
      link(lost[0], to.m);
    }

    this.edges = Uint16Array.from(pairs);
    this.edgeCount = pairs.length / 2;
    this.edgeAct = new Float32Array(this.edgeCount);
    this.edgeBreak = new Float32Array(this.edgeCount);
    this.lineAct = new Float32Array(this.edgeCount * 2);
  }

  private reachable(): boolean[] {
    const seen = new Array<boolean>(NODE_COUNT + 1).fill(false);
    const stack = [CORE];
    seen[CORE] = true;
    while (stack.length > 0) {
      const n = stack.pop()!;
      for (const m of this.adj[n]) {
        if (!seen[m]) {
          seen[m] = true;
          stack.push(m);
        }
      }
    }
    return seen;
  }

  /** Shortest route between two nodes (CORE included), or null. */
  private route(from: number, to: number): number[] | null {
    if (from === to) return null;
    const prev = this.bfsPrev.fill(-1);
    const queue = this.bfsQueue;
    let head = 0;
    let tail = 0;
    queue[tail++] = from;
    prev[from] = from;
    while (head < tail) {
      const n = queue[head++];
      if (n === to) break;
      for (const m of this.adj[n]) {
        if (prev[m] === -1) {
          prev[m] = n;
          queue[tail++] = m;
        }
      }
    }
    if (prev[to] === -1) return null;
    const out: number[] = [];
    for (let n = to; n !== from; n = prev[n]) out.push(n);
    out.push(from);
    return out.reverse();
  }

  private randomNode(): number {
    return Math.floor(this.rand() * NODE_COUNT);
  }

  private randomMember(c: number): number {
    const list = this.members[c];
    return list[Math.floor(this.rand() * list.length)];
  }

  /** Thinking in the background: a short local walk, a hop to another
   *  cluster, or a passage through the core between two clusters. */
  private randomRoute(local: boolean): number[] | null {
    const pick = local ? 0 : this.rand();
    const start = this.randomNode();
    if (pick < 0.4) {
      const walk = [start];
      let prev = -1;
      for (let hop = 0; hop < 3 + Math.floor(this.rand() * 2); hop++) {
        const here = walk[walk.length - 1];
        const options = this.adj[here].filter((m) => m !== prev && m !== CORE);
        if (options.length === 0) break;
        prev = here;
        walk.push(options[Math.floor(this.rand() * options.length)]);
      }
      return walk.length > 1 ? walk : null;
    }
    const otherCluster = Math.floor(this.rand() * CLUSTER_COUNT);
    const end = this.randomMember(otherCluster);
    if (pick < 0.7) return this.route(start, end);
    const toCore = this.route(start, CORE);
    const fromCore = this.route(CORE, end);
    if (!toCore || !fromCore) return null;
    return toCore.concat(fromCore.slice(1));
  }

  /** The node nearest the shell in the direction of a context node, given
   *  the lattice's current rotation. */
  private exitNode(direction: Vector3): number {
    this.lattice.updateMatrixWorld();
    let best = 0;
    let bestScore = -Infinity;
    for (let n = 0; n < NODE_COUNT; n++) {
      const score = this.tmp.copy(this.nodes[n]).applyMatrix4(this.lattice.matrixWorld).dot(direction);
      if (score > bestScore) {
        bestScore = score;
        best = n;
      }
    }
    return best;
  }

  private spawn(route: number[] | null, strength: number, speed: number, color: Vector3) {
    if (!route || route.length < 2) return;
    const p = this.packets.find((x) => !x.alive);
    if (!p) return;
    p.alive = true;
    p.route = route;
    p.seg = 0;
    p.t = 0;
    p.speed = speed;
    p.strength = strength;
    p.color = color;
    this.alivePackets++;
    this.touchEdge(route[0], route[1], strength * 0.9);
    this.light(route[0], strength * 0.8);
  }

  private light(node: number, strength: number) {
    if (node === CORE) this.coreKick = Math.max(this.coreKick, strength * 0.7);
    else this.act[node] = Math.max(this.act[node], strength);
  }

  private touchEdge(a: number, b: number, strength: number) {
    const e = this.edgeIndex.get(Math.min(a, b) * 1024 + Math.max(a, b));
    if (e !== undefined) this.edgeAct[e] = Math.max(this.edgeAct[e], strength);
  }

  // ---------- states ----------

  /** One-off effects when a state begins. */
  private enter(state: CoreState) {
    if (state === "SUCCESS") {
      // One wave from the centre outward; the nodes it crosses light together.
      this.successAge = 0;
      this.wave.color.value.copy(GREEN);
      this.accentColor.copy(GREEN);
      this.accent = Math.max(this.accent, 0.55);
    }
    if (state === "ERROR") {
      // A short disruption: links around two clusters break, packets in
      // flight are dropped, a red signal in the core; then it settles.
      this.errorAge = 0;
      this.jitter = 0.012;
      this.accentColor.copy(RED);
      this.accent = Math.max(this.accent, 0.85);
      const hit = Math.floor(this.rand() * CLUSTER_COUNT);
      // The hit cluster scatters briefly, then returns.
      this.shape.hit.value = hit;
      this.shape.scatter.value = 1;
      for (let e = 0; e < this.edgeCount; e++) {
        const near = this.cluster[this.edges[e * 2]] === hit || this.cluster[this.edges[e * 2 + 1]] === hit;
        if (near || this.rand() < 0.04) this.edgeBreak[e] = 1;
      }
      for (const p of this.packets) p.alive = false;
      this.alivePackets = 0;
    }
    if (state === "PLANNING") {
      this.planAge = 0;
      this.startPlan();
    } else {
      this.planAge = -1;
    }
    if (state === "LISTENING") this.listenPhase = 0;
  }

  /** The accent the envelope settles on in this state. */
  private restingAccent(): number {
    switch (this.state) {
      case "WAITING_APPROVAL":
        this.accentColor.copy(AMBER);
        return 0.8;
      case "ERROR":
        return 0.25;
      case "SUCCESS":
        return 0;
      default:
        if (this.visual.hasPendingApproval) {
          this.accentColor.copy(AMBER);
          return 0.15;
        }
        return 0;
    }
  }

  /** Planning: three candidate routes leave the core at once, dim. */
  private startPlan() {
    this.planRoutes.length = 0;
    this.planChosen = -1;
    const used = new Set<number>();
    while (this.planRoutes.length < 3 && used.size < CLUSTER_COUNT) {
      const c = Math.floor(this.rand() * CLUSTER_COUNT);
      if (used.has(c)) continue;
      used.add(c);
      const r = this.route(CORE, this.randomMember(c));
      if (r) this.planRoutes.push(r);
    }
    for (const r of this.planRoutes) this.spawn(r, 0.45, 0.8, PACKET_DIM);
  }

  private stepPlan(dt: number) {
    if (this.planAge < 0) return;
    this.planAge += dt;
    if (this.planChosen < 0 && this.planAge > 1.2 && this.planRoutes.length > 0) {
      // One route becomes dominant: followed again, bright, three times.
      this.planChosen = Math.floor(this.rand() * this.planRoutes.length);
      const r = this.planRoutes[this.planChosen];
      this.spawn(r, 1, 1.15, PACKET);
      this.planFollowUps = 2;
      this.planNext = this.planAge + 0.28;
    }
    if (this.planChosen >= 0 && this.planFollowUps > 0 && this.planAge >= this.planNext) {
      this.spawn(this.planRoutes[this.planChosen], 0.9, 1.15, PACKET);
      this.planFollowUps--;
      this.planNext = this.planAge + 0.28;
    }
    if (this.planChosen >= 0) {
      // The chosen route stays lit while it is followed.
      const r = this.planRoutes[this.planChosen];
      for (let k = 0; k + 1 < r.length; k++) this.touchEdge(r[k], r[k + 1], 0.45);
    }
    if (this.planAge > 3.4) {
      this.planAge = 0;
      this.startPlan();
    }
  }

  private planFollowUps = 0;
  private planNext = 0;

  /** Packets from the core to the node of the tool in use, continuing
   *  along its beam. */
  private flowToActive(strength: number, speed: number) {
    const beam = this.activeId ? this.beams.get(this.activeId) : undefined;
    if (!beam || beam.removing) return false;
    const dir = this.tmp2.copy(beam.target).normalize();
    this.spawn(this.route(CORE, this.exitNode(dir)), strength, speed, PACKET);
    return true;
  }

  // ---------- context nodes ----------

  private slotPosition(id: string, out: Vector3): Vector3 {
    const [sx, sy] = SLOT[id] ?? [0, 1];
    const halfH = Math.tan((this.camera.fov * Math.PI) / 360) * this.camera.position.z;
    const halfW = halfH * this.camera.aspect;
    return out.set(sx * Math.min(2.65, halfW * 0.8), sy * Math.min(1.9, halfH * 1.25), 0);
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
      beam.proposed = n.proposed === true;
      beam.active = n.id === this.activeId && !beam.proposed;
    }
    this.publishLayout();
  }

  private createBeam(id: string): Beam {
    const lineGeo = new BufferGeometry();
    lineGeo.setAttribute("position", new BufferAttribute(new Float32Array(6), 3));
    lineGeo.setAttribute("aAct", new BufferAttribute(new Float32Array([0.2, 1]), 1));
    lineGeo.setAttribute("aDist", new BufferAttribute(new Float32Array(2), 1));
    const lineMat = additive(BEAM_LINE_VERTEX, BEAM_LINE_FRAGMENT, {
      uDim: { value: DONE.clone() },
      uHot: { value: ICE.clone() },
      uIntensity: { value: 0 },
      uAlpha: { value: 0 },
      uDash: { value: 0 },
    });
    const line = new LineSegments(lineGeo, lineMat);

    const headGeo = new BufferGeometry();
    headGeo.setAttribute("position", new BufferAttribute(new Float32Array(9), 3));
    headGeo.setAttribute("aAct", new BufferAttribute(new Float32Array([1, 0.7, 0.45]), 1));
    headGeo.setAttribute("aSize", new BufferAttribute(new Float32Array([7, 5, 4]), 1));
    const headMat = additive(BEAM_POINT_VERTEX, BEAM_POINT_FRAGMENT, {
      uPixelRatio: this.pixelRatio,
      uBase: { value: 4 },
      uDim: { value: ICE.clone() },
      uHot: { value: ICE.clone() },
      uIntensity: { value: 1 },
      uAlpha: { value: 0 },
    });
    const head = new Points(headGeo, headMat);
    head.frustumCulled = false;

    const nodeGeo = new PlaneGeometry(1, 1);
    const nodeMat = additive(UV_VERTEX, GLOW_FRAGMENT, { uColor: { value: ICE.clone() }, uStrength: { value: 0 }, uFalloff: { value: 3 } });
    const node = new Mesh(nodeGeo, nodeMat);
    node.scale.setScalar(0.55);
    const ringGeo = new RingGeometry(0.15, 0.162, 48);
    const ringMat = additive(UV_VERTEX, FLAT_FRAGMENT, { uColor: { value: ICE.clone() }, uStrength: { value: 0 } });
    const ring = new Mesh(ringGeo, ringMat);

    this.scene.add(line, head, node, ring);
    const beam: Beam = {
      id,
      target: new Vector3(),
      from: new Vector3(),
      line,
      lineMat,
      head,
      headMat,
      node,
      nodeMat,
      ring,
      ringMat,
      status: "done",
      active: false,
      proposed: false,
      presence: 0,
      removing: false,
    };
    this.placeBeam(beam);
    return beam;
  }

  private placeBeam(beam: Beam) {
    this.slotPosition(beam.id, beam.target);
    beam.from.copy(beam.target).normalize().multiplyScalar(SHELL_RADIUS * 0.98);
    const pos = beam.line.geometry.getAttribute("position") as BufferAttribute;
    pos.setXYZ(0, beam.from.x, beam.from.y, beam.from.z);
    pos.setXYZ(1, beam.target.x, beam.target.y, beam.target.z);
    pos.needsUpdate = true;
    const dist = beam.line.geometry.getAttribute("aDist") as BufferAttribute;
    dist.setX(1, beam.from.distanceTo(beam.target));
    dist.needsUpdate = true;
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
      const p = this.tmp.copy(beam.target).project(this.camera);
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
    // Slow frame rates (unfocused, resting) still advance in real time.
    const dt = Math.min(0.25, (now - this.lastTick) / 1000);
    this.lastTick = now;
    this.step(dt, now);
    this.renderer.render(this.scene, this.camera);
    // Sleep until the next frame this state needs instead of waking at
    // the display's refresh rate.
    const wait = 1000 / this.frameRate(now) - (performance.now() - now);
    if (wait > 12) {
      this.wakeAt = performance.now() + wait - 6;
      this.sleep = window.setTimeout(() => {
        this.sleep = 0;
        if (!this.disposed && !this.reduced && !document.hidden) this.raf = requestAnimationFrame(this.frame);
      }, wait - 6);
    } else {
      this.raf = requestAnimationFrame(this.frame);
    }
  };

  private resting(now: number): boolean {
    return this.state === "IDLE" && now - this.stateSince > REST_AFTER_MS;
  }

  private frameRate(now: number): number {
    let settled = true;
    for (const b of this.beams.values()) {
      if (b.removing ? b.presence >= 0.02 : b.presence <= 0.98) {
        settled = false;
        break;
      }
    }
    const moving = this.alivePackets > 0 || this.wave.amp.value > 0.01 || this.jitter > 0.0005 || !settled;
    let fps = this.resting(now) ? REST_FPS : this.target.fps;
    const calm = this.state === "IDLE" || this.state === "WAITING_APPROVAL";
    if (moving) fps = Math.max(fps, calm ? CALM_MOTION_FPS : MOTION_FPS);
    if (this.focused && now < this.pointerUntil) fps = Math.max(fps, POINTER_FPS);
    if (!this.focused) fps = Math.min(fps, UNFOCUSED_FPS);
    return fps;
  }

  private stillFrame() {
    if (this.disposed) return;
    // A representative, motionless picture of the state.
    this.tune = { ...this.target };
    const lit = this.target.packets > 1.5 ? 0.35 : this.target.flowEvery > 0 ? 0.2 : 0.08;
    for (let i = 0; i < NODE_COUNT; i++) this.act[i] = (i * 7) % 20 < lit * 20 ? 0.7 : 0;
    this.edgeAct.fill(0);
    this.edgeBreak.fill(0);
    for (const p of this.packets) p.alive = false;
    this.alivePackets = 0;
    this.successAge = -1;
    this.errorAge = -1;
    this.planAge = -1;
    this.jitter = 0;
    this.wave.amp.value = 0;
    this.shape.scatter.value = 0;
    this.accent = this.state === "ERROR" ? 0.45 : this.state === "SUCCESS" ? 0.4 : this.accentGoal;
    if (this.state === "SUCCESS") this.accentColor.copy(GREEN);
    if (this.state === "ERROR") this.accentColor.copy(RED);
    this.pointer.set(0, 0);
    this.orient.position.set(0, 0, 0);
    this.orient.rotation.set(BASE_TILT, 0, 0);
    this.syncLinks();
    this.writePackets();
    this.applyUniforms();
    for (const beam of this.beams.values()) {
      beam.presence = beam.removing ? 0 : 1;
      this.styleBeam(beam, 0, true);
    }
    this.pruneBeams();
    this.renderer.render(this.scene, this.camera);
  }

  private step(dt: number, now: number) {
    this.time += dt;
    const k = 1 - Math.exp(-dt * 3.5);
    const t = this.tune;
    const g = this.target;
    t.spin += (g.spin - t.spin) * k;
    t.packets += (g.packets - t.packets) * k;
    t.clusters += (g.clusters - t.clusters) * k;
    t.intensity += (g.intensity - t.intensity) * k;
    t.core += (g.core - t.core) * k;
    t.breathe += (g.breathe - t.breathe) * k;
    t.breatheHz += (g.breatheHz - t.breatheHz) * k;
    t.bands += (g.bands - t.bands) * k;
    t.flowEvery = g.flowEvery;
    // Shape changes ease from wherever they are, so an interrupted change
    // never jumps.
    const ks = 1 - Math.exp(-dt * 2.2);
    t.shape += (g.shape - t.shape) * ks;
    t.tremble += (g.tremble - t.tremble) * ks;

    // Rotation and the pointer's parallax (the dust moves a little more,
    // which reads as depth).
    this.lattice.rotation.y += t.spin * dt;
    this.dust.rotation.y -= t.spin * 0.5 * dt;
    this.pointer.lerp(this.pointerGoal, 1 - Math.exp(-dt * 4));
    if (now > this.pointerUntil) this.pointerGoal.set(0, 0);
    this.orient.rotation.x = BASE_TILT + this.pointer.y * 0.12;
    this.orient.rotation.y = this.pointer.x * 0.16;
    this.dust.rotation.x = this.pointer.y * 0.05;
    this.jitter *= Math.exp(-dt / 0.25);
    if (this.jitter > 0.0005) {
      this.orient.position.set(Math.sin(this.time * 37) * this.jitter, Math.cos(this.time * 29) * this.jitter, 0);
    } else {
      this.orient.position.set(0, 0, 0);
    }

    // Activity by state.
    const intensity = 0.7 + 0.6 * this.visual.activityIntensity;
    const rate = this.resting(now) ? REST_PACKETS : t.packets * intensity;
    const slow = this.state === "WAITING_APPROVAL" ? 0.45 : 1;
    this.packetDebt += rate * dt;
    while (this.packetDebt >= 1) {
      this.packetDebt -= 1;
      const calm = this.state === "IDLE" || this.state === "WAITING_APPROVAL";
      this.spawn(this.randomRoute(calm), 0.55 + this.rand() * 0.4, (calm ? 1.1 : 0.85) * slow, PACKET);
    }
    this.clusterDebt += t.clusters * dt;
    while (this.clusterDebt >= 1) {
      this.clusterDebt -= 1;
      for (const n of this.members[Math.floor(this.rand() * CLUSTER_COUNT)]) {
        this.act[n] = Math.max(this.act[n], 0.45 + this.rand() * 0.3);
      }
    }
    if (t.flowEvery > 0) {
      this.flowTimer -= dt;
      if (this.flowTimer <= 0) {
        this.flowTimer = t.flowEvery;
        if (this.state === "EXECUTING") {
          // Deterministic: the core's routes to each hub in turn, plus the
          // stream to the node being written.
          const hub = this.hubs[this.execIndex++ % this.hubs.length];
          this.spawn(this.route(CORE, hub), 0.85, 1.25, PACKET);
          this.flowToActive(1, 1.3);
        } else {
          this.flowToActive(this.state === "THINKING" ? 0.7 : 1, 1.3);
        }
      }
    }
    // Signals towards proposed tools stop at the shell.
    this.proposalTimer -= dt;
    if (this.proposalTimer <= 0) {
      this.proposalTimer = 3.5;
      for (const beam of this.beams.values()) {
        if (!beam.proposed || beam.removing) continue;
        this.spawn(this.route(CORE, this.exitNode(this.tmp2.copy(beam.target).normalize())), 0.5, 0.9, PACKET_DIM);
      }
    }
    this.stepPlan(dt);
    this.stepPackets(dt);

    // Waves.
    if (this.state === "LISTENING") {
      this.listenPhase = (this.listenPhase + dt / 2.4) % 1;
      this.wave.color.value.copy(ICE);
      this.wave.r.value = 1.25 - this.listenPhase * 1.05;
      this.wave.amp.value = 0.4 * Math.sin(Math.PI * this.listenPhase);
    } else if (this.successAge >= 0) {
      this.successAge += dt;
      const p = this.successAge / 1.4;
      if (p >= 1) {
        this.successAge = -1;
        this.wave.amp.value = 0;
      } else {
        this.wave.r.value = 0.12 + p * 1.3;
        this.wave.amp.value = 0.95 * (1 - p * 0.7);
      }
    } else {
      this.wave.amp.value *= Math.exp(-dt / 0.2);
    }

    // Error: links stay broken briefly, then mend; the scatter returns
    // within about a second and a half.
    if (this.errorAge >= 0) {
      this.errorAge += dt;
      if (this.errorAge > 0.25) this.shape.scatter.value *= Math.exp(-dt / 0.4);
      if (this.errorAge > 0.8) {
        const mend = Math.exp(-dt / 0.9);
        for (let e = 0; e < this.edgeCount; e++) this.edgeBreak[e] *= mend;
      }
      if (this.errorAge > 6) {
        this.edgeBreak.fill(0);
        this.shape.scatter.value = 0;
        this.errorAge = -1;
      }
    }

    // Decay.
    const nodeDecay = Math.exp(-dt / 0.6);
    for (let i = 0; i < NODE_COUNT; i++) this.act[i] *= nodeDecay;
    const edgeDecay = Math.exp(-dt / 0.5);
    for (let e = 0; e < this.edgeCount; e++) this.edgeAct[e] *= edgeDecay;
    this.coreKick *= Math.exp(-dt / 0.4);
    this.accent += (this.accentGoal - this.accent) * (1 - Math.exp(-dt / 1.2));

    this.syncLinks();
    this.writePackets();
    this.applyUniforms();

    for (const beam of this.beams.values()) {
      beam.presence += ((beam.removing ? 0 : 1) - beam.presence) * (1 - Math.exp(-dt * 5));
      this.styleBeam(beam, this.time, false);
    }
    this.pruneBeams();
  }

  private stepPackets(dt: number) {
    for (const p of this.packets) {
      if (!p.alive) continue;
      const a = this.nodes[p.route[p.seg]];
      const b = this.nodes[p.route[p.seg + 1]];
      p.t += (p.speed * dt) / Math.max(0.05, a.distanceTo(b));
      while (p.t >= 1) {
        p.t -= 1;
        p.seg++;
        const arrived = p.route[p.seg];
        this.light(arrived, p.strength);
        if (p.seg >= p.route.length - 1) {
          p.alive = false;
          this.alivePackets--;
          break;
        }
        // A broken link stops the signal.
        const e = this.edgeIndex.get(Math.min(arrived, p.route[p.seg + 1]) * 1024 + Math.max(arrived, p.route[p.seg + 1]));
        if (e !== undefined && this.edgeBreak[e] > 0.5) {
          p.alive = false;
          this.alivePackets--;
          break;
        }
        this.touchEdge(arrived, p.route[p.seg + 1], p.strength * 0.9);
      }
    }
  }

  /** Each link shows the brighter of its ends and the signals on it; a
   *  broken link goes darker than at rest. */
  private syncLinks() {
    const coreAct = 0.2 + this.tune.core * 0.35 + this.coreKick;
    for (let e = 0; e < this.edgeCount; e++) {
      const a = this.edges[e * 2];
      const b = this.edges[e * 2 + 1];
      const ea = this.edgeAct[e];
      const broken = this.edgeBreak[e];
      const actA = a === CORE ? coreAct : this.act[a] * 0.85;
      const actB = b === CORE ? coreAct : this.act[b] * 0.85;
      this.lineAct[e * 2] = Math.max(actA, ea) * (1 - broken) - broken * 1.5;
      this.lineAct[e * 2 + 1] = Math.max(actB, ea) * (1 - broken) - broken * 1.5;
    }
    (this.nodeGeo.getAttribute("aAct") as BufferAttribute).needsUpdate = true;
    (this.linkGeo.getAttribute("aAct") as BufferAttribute).needsUpdate = true;
  }

  private writePackets() {
    for (let i = 0; i < PACKET_COUNT; i++) {
      const p = this.packets[i];
      for (let k = 0; k < TAIL; k++) {
        const v = i * TAIL + k;
        if (!p.alive) {
          this.packetAct[v] = 0;
          this.packetSize[v] = 0;
          continue;
        }
        const a = this.nodes[p.route[p.seg]];
        const b = this.nodes[p.route[p.seg + 1]];
        const f = Math.max(0, p.t - k * 0.09);
        this.tmp.copy(a).lerp(b, f).toArray(this.packetPos, v * 3);
        p.color.toArray(this.packetColor, v * 3);
        this.packetAct[v] = p.strength * (1 - k / TAIL);
        this.packetSize[v] = 12 - k * 2.5;
      }
    }
    for (const attribute of this.packetAttributes) attribute.needsUpdate = true;
  }

  private applyUniforms() {
    const t = this.tune;
    this.timeUniform.value = this.time;
    const breath = 1 + Math.sin(this.time * Math.PI * 2 * t.breatheHz) * t.breathe;
    const core = this.coreMat.uniforms;
    core.uEnergy.value = Math.min(1.4, t.core + this.coreKick * 0.5);
    core.uBreath.value = breath;
    core.uBands.value = t.bands;
    (core.uAccent.value as Vector3).copy(this.accentColor);
    core.uAccentAmt.value = this.accent;
    for (const m of [this.nodeMat, this.linkMat]) m.uniforms.uIntensity.value = t.intensity;
    this.nodeMat.uniforms.uTwinkle.value = this.state === "WAITING_APPROVAL" ? 0.05 : 0.15;
    this.dustMat.uniforms.uIntensity.value = t.intensity;
    this.dustMat.uniforms.uPull.value = this.state === "LISTENING" ? 1 : 0;
    // On success every node pulses once, in step with the wave.
    const pulse = this.successAge >= 0 ? 0.045 * Math.sin(Math.PI * Math.min(1, this.successAge / 1.4)) : 0;
    this.shape.size.value = t.shape * (1 + pulse);
    this.shape.tremble.value = t.tremble;
    // The success wave brightens the rim as it reaches it.
    const atRim = this.successAge >= 0 ? this.wave.amp.value * Math.max(0, this.wave.r.value - 1) * 1.2 : 0;
    this.shellMat.uniforms.uStrength.value = 0.13 + t.intensity * 0.08 + atRim;
  }

  private styleBeam(beam: Beam, time: number, still: boolean) {
    const color = beam.status === "waiting" ? AMBER : beam.status === "failed" ? RED : beam.status === "active" ? ICE : DONE;
    const strong = beam.active || beam.status === "waiting";
    const p = beam.presence;
    const line = beam.lineMat.uniforms;
    (line.uHot.value as Vector3).copy(color);
    (line.uDim.value as Vector3).copy(color).multiplyScalar(0.35);
    line.uIntensity.value = strong ? 0.6 : 0.05;
    line.uAlpha.value = p * (strong ? 1 : 0.5);
    line.uDash.value = beam.proposed ? 1 : 0;
    const throb = still ? 1 : 0.75 + 0.25 * Math.sin(time * (beam.status === "waiting" ? 1.6 : 4));
    (beam.nodeMat.uniforms.uColor.value as Vector3).copy(color);
    beam.nodeMat.uniforms.uStrength.value = (strong ? 0.9 * throb : 0.45) * p;
    (beam.ringMat.uniforms.uColor.value as Vector3).copy(color);
    beam.ringMat.uniforms.uStrength.value = (strong ? 0.7 : 0.3) * p;
    beam.ring.scale.setScalar(strong && !still ? 1 + 0.12 * Math.sin(time * 2.2) : 1);

    // Travelling impulse: outward to an active tool, inward from a waiting
    // approval (the core depends on it). A proposal's impulse waits at the
    // shell, where the signals from the core stop: nothing has gone out.
    const parked = beam.proposed;
    const moving = !still && !parked && (beam.active || beam.status === "waiting");
    beam.headMat.uniforms.uAlpha.value = moving ? p : parked ? p * (still ? 0.8 : 0.55 + 0.35 * Math.sin(time * 2.4)) : 0;
    (beam.headMat.uniforms.uHot.value as Vector3).copy(color);
    (beam.headMat.uniforms.uDim.value as Vector3).copy(color);
    if (parked) {
      const pos = beam.head.geometry.getAttribute("position") as BufferAttribute;
      for (let i = 0; i < 3; i++) {
        this.tmp.copy(beam.from).lerp(beam.target, 0.02 + i * 0.012);
        pos.setXYZ(i, this.tmp.x, this.tmp.y, this.tmp.z);
      }
      pos.needsUpdate = true;
    } else if (moving) {
      const speed = beam.status === "waiting" ? 0.35 : 0.9;
      const pos = beam.head.geometry.getAttribute("position") as BufferAttribute;
      for (let i = 0; i < 3; i++) {
        let f = (time * speed - i * 0.045) % 1;
        if (f < 0) f += 1;
        if (beam.status === "waiting") f = 1 - f;
        this.tmp.copy(beam.from).lerp(beam.target, f);
        pos.setXYZ(i, this.tmp.x, this.tmp.y, this.tmp.z);
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
