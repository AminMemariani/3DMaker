// PlayCanvas-based Gaussian splat viewer with its own orbit controller.
// PlayCanvas handles the real 3DGS rendering (anisotropic Gaussians, alpha
// blending, sorting, SH). This module owns loading, framing, input and the
// `window.__viewer` automation API.
import { Application, Asset, Color, Entity, FILLMODE_NONE, RESOLUTION_AUTO } from "playcanvas";
import type { Bounds } from "../lib/ipc";
import {
  clampDistance,
  clampPitch,
  framePose,
  panPose,
  poseFromView,
  poseToPosition,
  rotateEuler,
  type OrbitPose,
  type Vec3T,
} from "./orbit";

export interface LoadOptions {
  viewRotation?: Vec3T;
  bounds?: Bounds | null;
}

export interface FirstFrameInfo {
  firstFrameMs: number;
  splatCount: number | null;
}

export interface PixelCapture {
  width: number;
  height: number;
  data: Uint8ClampedArray;
}

export interface OrbitMeasurement {
  frames: number;
  medianFps: number;
  p5Fps: number;
  frameTimesMs: number[];
}

export interface ViewerAutomation {
  ready: boolean;
  error: string | null;
  firstFrameMs: number | null;
  splatCount: number | null;
  /** Increments every time a load reaches its first visible frame. */
  loads: number;
  created: boolean;
  /** Frames rendered by the live viewer. */
  frames: number;
  getCamera(): { yaw: number; pitch: number; distance: number; target: Vec3T };
  setView(opts: { position: Vec3T; target: Vec3T; fov?: number }): void;
  orbitBy(dYawDeg: number, dPitchDeg: number): void;
  resetView(): void;
  readPixels(): Promise<PixelCapture>;
  stats(): { nonBackgroundFraction: number };
  measureOrbit(seconds: number): Promise<OrbitMeasurement>;
  loadUrl(url: string, opts?: LoadOptions): Promise<FirstFrameInfo>;
}

export class ViewerInitError extends Error {}

/** Background clear colour (sRGB 0..255). */
export const BACKGROUND: Vec3T = [20, 22, 27];
const AWAKE_MS = 1200;

let active: Viewer | null = null;
let globalError: string | null = null;

function notCreated(): never {
  throw new Error("viewer not created");
}

// Always present and harmless: delegates to the live viewer if one exists.
const automation: ViewerAutomation = {
  get ready() {
    return active?.ready ?? false;
  },
  get error() {
    return active?.error ?? globalError;
  },
  get firstFrameMs() {
    return active?.firstFrameMs ?? null;
  },
  get splatCount() {
    return active?.splatCount ?? null;
  },
  get loads() {
    return active?.loads ?? 0;
  },
  get created() {
    return active !== null;
  },
  get frames() {
    return active?.frames ?? 0;
  },
  getCamera: () => (active ?? notCreated()).getCamera(),
  setView: (o) => (active ?? notCreated()).setView(o),
  orbitBy: (a, b) => (active ?? notCreated()).orbitBy(a, b),
  resetView: () => (active ?? notCreated()).resetView(),
  readPixels: () => (active ?? notCreated()).readPixels(),
  stats: () => (active ?? notCreated()).stats(),
  measureOrbit: (s) => (active ?? notCreated()).measureOrbit(s),
  loadUrl: (u, o) => (active ?? notCreated()).loadUrl(u, o),
};
if (typeof window !== "undefined") {
  (window as unknown as { __viewer: ViewerAutomation }).__viewer = automation;
}

export function createViewer(canvas: HTMLCanvasElement): Viewer {
  try {
    const v = new Viewer(canvas);
    active = v;
    globalError = null;
    return v;
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    globalError = msg;
    throw new ViewerInitError(msg);
  }
}

const sleepFrame = () => new Promise<number>((r) => requestAnimationFrame(r));

export class Viewer {
  ready = false;
  error: string | null = null;
  firstFrameMs: number | null = null;
  splatCount: number | null = null;
  loads = 0;

  private app: Application;
  private camera: Entity;
  private splat: Entity | null = null;
  private asset: Asset | null = null;
  private pose: OrbitPose = { yaw: 35, pitch: -15, distance: 3, target: [0, 0, 0] };
  private home: OrbitPose = { ...this.pose };
  private radius = 1;
  private fileCenter: Vec3T = [0, 0, 0];
  private rotation: Vec3T = [0, 0, 0];
  private fov = 45;
  private loadToken = 0;
  private awakeUntil = 0;
  private forceRender = false;
  private pointers = new Map<number, { x: number; y: number }>();
  private pinchDist = 0;
  private resizeObs: ResizeObserver | null = null;
  private cleanup: (() => void)[] = [];

  constructor(private canvas: HTMLCanvasElement) {
    const app = new Application(canvas, {
      graphicsDeviceOptions: {
        antialias: false,
        alpha: false,
        preserveDrawingBuffer: true,
        powerPreference: "high-performance",
        deviceTypes: ["webgl2"],
      },
    });
    if (!app.graphicsDevice) throw new Error("No graphics device");
    this.app = app;
    app.graphicsDevice.maxPixelRatio = Math.min(window.devicePixelRatio || 1, 2);
    app.setCanvasFillMode(FILLMODE_NONE);
    app.setCanvasResolution(RESOLUTION_AUTO);
    this.syncSize();

    const camera = new Entity("camera");
    camera.addComponent("camera", {
      clearColor: new Color(BACKGROUND[0] / 255, BACKGROUND[1] / 255, BACKGROUND[2] / 255, 1),
      fov: this.fov,
      nearClip: 0.01,
      farClip: 1000,
    });
    app.root.addChild(camera);
    this.camera = camera;
    this.applyPose();

    app.on("update", () => {
      if (!this.forceRender && performance.now() > this.awakeUntil) app.autoRender = false;
    });
    app.on("postrender", () => {
      this.frames++;
    });
    app.start();
    this.wake();

    if (typeof ResizeObserver !== "undefined") {
      this.resizeObs = new ResizeObserver(() => {
        this.syncSize();
        this.wake();
      });
      this.resizeObs.observe(canvas);
    }
    this.attachInput();
  }

  destroy(): void {
    this.loadToken++;
    this.resizeObs?.disconnect();
    this.cleanup.forEach((f) => f());
    if (active === this) active = null;
    this.app.destroy();
  }

  private syncSize(): void {
    const w = Math.max(1, Math.round(this.canvas.clientWidth || this.canvas.width));
    const h = Math.max(1, Math.round(this.canvas.clientHeight || this.canvas.height));
    const r = this.app.graphicsDevice.maxPixelRatio;
    const bw = Math.max(1, Math.round(w * r));
    const bh = Math.max(1, Math.round(h * r));
    if (this.canvas.width !== bw || this.canvas.height !== bh) {
      this.app.graphicsDevice.setResolution(bw, bh);
    }
  }

  /** Render continuously for a short while (interaction, loading, sorting). */
  private wake(ms = AWAKE_MS): void {
    this.awakeUntil = Math.max(this.awakeUntil, performance.now() + ms);
    this.app.autoRender = true;
  }

  private applyPose(): void {
    this.pose.pitch = clampPitch(this.pose.pitch);
    this.pose.distance = clampDistance(this.pose.distance, this.radius);
    const pos = poseToPosition(this.pose);
    this.camera.setPosition(pos[0], pos[1], pos[2]);
    this.camera.lookAt(this.pose.target[0], this.pose.target[1], this.pose.target[2]);
    if (this.camera.camera) {
      this.camera.camera.fov = this.fov;
      this.camera.camera.nearClip = Math.max(0.001, this.pose.distance / 1000);
      this.camera.camera.farClip = Math.max(100, this.pose.distance * 100 + this.radius * 10);
    }
    this.wake();
  }

  private computeHome(): OrbitPose {
    const aspect = this.canvas.width / Math.max(1, this.canvas.height);
    const center = rotateEuler(this.fileCenter, this.rotation);
    return framePose(center, this.radius, this.fov, aspect);
  }

  getCamera() {
    return { ...this.pose, target: [...this.pose.target] as Vec3T };
  }

  setView(opts: { position: Vec3T; target: Vec3T; fov?: number }): void {
    if (opts.fov) this.fov = opts.fov;
    this.pose = poseFromView(opts.position, opts.target);
    // Explicit views bypass distance clamping so test viewpoints are exact.
    const pos = opts.position;
    this.camera.setPosition(pos[0], pos[1], pos[2]);
    this.camera.lookAt(opts.target[0], opts.target[1], opts.target[2]);
    if (this.camera.camera) this.camera.camera.fov = this.fov;
    this.wake();
  }

  orbitBy(dYaw: number, dPitch: number): void {
    this.pose.yaw += dYaw;
    this.pose.pitch += dPitch;
    this.applyPose();
  }

  panBy(dx: number, dy: number): void {
    this.pose = panPose(this.pose, dx, dy, this.fov, this.canvas.clientHeight || this.canvas.height);
    this.applyPose();
  }

  zoomBy(factor: number): void {
    this.pose.distance *= factor;
    this.applyPose();
  }

  resetView(): void {
    this.fov = 45;
    this.pose = { ...this.home, target: [...this.home.target] as Vec3T };
    this.applyPose();
  }

  /** Viewer-only orientation correction; re-frames the object. */
  setRotation(rot: Vec3T): void {
    this.rotation = [...rot] as Vec3T;
    this.splat?.setEulerAngles(rot[0], rot[1], rot[2]);
    this.home = this.computeHome();
    this.resetView();
  }

  async loadUrl(url: string, opts: LoadOptions = {}): Promise<FirstFrameInfo> {
    const token = ++this.loadToken;
    const t0 = performance.now();
    this.ready = false;
    this.error = null;
    this.firstFrameMs = null;
    this.splatCount = null;
    this.unload();

    const asset = new Asset("splat", "gsplat", { url, filename: "splat.ply" });
    this.asset = asset;
    this.app.assets.add(asset);
    try {
      await new Promise<void>((resolve, reject) => {
        asset.once("load", () => resolve());
        asset.once("error", (err: unknown) => reject(new Error(String(err))));
        this.app.assets.load(asset);
      });
    } catch (e) {
      if (token === this.loadToken) this.error = e instanceof Error ? e.message : String(e);
      throw e;
    }
    if (token !== this.loadToken) throw new Error("superseded");

    const rot = opts.viewRotation ?? [0, 0, 0];
    this.rotation = [...rot] as Vec3T;
    const entity = new Entity("splat");
    entity.addComponent("gsplat", { asset });
    entity.setEulerAngles(rot[0], rot[1], rot[2]);
    this.app.root.addChild(entity);
    this.splat = entity;

    const res = asset.resource as unknown as {
      numSplats?: number;
      aabb?: { center: { x: number; y: number; z: number }; halfExtents: { x: number; y: number; z: number } };
      gsplatData?: { numSplats?: number };
    } | null;
    this.splatCount = res?.numSplats ?? res?.gsplatData?.numSplats ?? null;

    const b = opts.bounds;
    if (b && Number.isFinite(b.radius) && b.radius > 0) {
      this.fileCenter = [...b.center] as Vec3T;
      this.radius = b.radius;
    } else if (res?.aabb) {
      const c = res.aabb.center;
      const h = res.aabb.halfExtents;
      this.fileCenter = [c.x, c.y, c.z];
      this.radius = Math.max(1e-3, Math.hypot(h.x, h.y, h.z) * 0.85);
    } else {
      this.fileCenter = [0, 0, 0];
      this.radius = 1;
    }
    this.syncSize();
    this.home = this.computeHome();
    this.resetView();

    // Wait until the splats are actually visible (sorting is asynchronous).
    this.wake(3000);
    const deadline = performance.now() + 8000;
    while (performance.now() < deadline) {
      await this.nextRender();
      if (token !== this.loadToken) throw new Error("superseded");
      if (this.stats(8).nonBackgroundFraction > 0) break;
    }
    this.firstFrameMs = performance.now() - t0;
    this.ready = true;
    this.loads++;
    return { firstFrameMs: this.firstFrameMs, splatCount: this.splatCount };
  }

  private unload(): void {
    if (this.splat) {
      this.splat.destroy();
      this.splat = null;
    }
    if (this.asset) {
      this.app.assets.remove(this.asset);
      this.asset.unload();
      this.asset = null;
    }
  }

  /** Number of frames actually rendered (diagnostics / automation). */
  frames = 0;

  /**
   * Resolves after the next rendered frame, or after a short timeout so a
   * stalled render loop (hidden window, lost context, teardown) can never
   * hang a caller forever.
   */
  private nextRender(timeoutMs = 500): Promise<void> {
    return new Promise((resolve) => {
      this.app.renderNextFrame = true;
      this.app.autoRender = true;
      const t = setTimeout(resolve, timeoutMs);
      this.app.once("postrender", () => {
        clearTimeout(t);
        resolve();
      });
    });
  }

  private grab(maxSide?: number): HTMLCanvasElement {
    const w = this.canvas.width;
    const h = this.canvas.height;
    const s = maxSide ? Math.min(1, maxSide / Math.max(w, h)) : 1;
    const out = document.createElement("canvas");
    out.width = Math.max(1, Math.round(w * s));
    out.height = Math.max(1, Math.round(h * s));
    out.getContext("2d")!.drawImage(this.canvas, 0, 0, out.width, out.height);
    return out;
  }

  async readPixels(): Promise<PixelCapture> {
    await this.nextRender();
    const c = this.grab();
    const img = c.getContext("2d")!.getImageData(0, 0, c.width, c.height);
    return { width: c.width, height: c.height, data: img.data };
  }

  stats(step = 2): { nonBackgroundFraction: number } {
    const c = this.grab();
    const d = c.getContext("2d")!.getImageData(0, 0, c.width, c.height).data;
    let n = 0;
    let hit = 0;
    for (let y = 0; y < c.height; y += step) {
      for (let x = 0; x < c.width; x += step) {
        const i = (y * c.width + x) * 4;
        n++;
        const diff =
          Math.abs(d[i] - BACKGROUND[0]) + Math.abs(d[i + 1] - BACKGROUND[1]) + Math.abs(d[i + 2] - BACKGROUND[2]);
        if (diff > 30) hit++;
      }
    }
    return { nonBackgroundFraction: n ? hit / n : 0 };
  }

  /** Square PNG (~256 px) of the current view, for the library thumbnail. */
  async captureThumbnail(size = 256): Promise<Uint8Array> {
    await this.nextRender();
    const w = this.canvas.width;
    const h = this.canvas.height;
    const side = Math.min(w, h);
    const out = document.createElement("canvas");
    out.width = size;
    out.height = size;
    out.getContext("2d")!.drawImage(this.canvas, (w - side) / 2, (h - side) / 2, side, side, 0, 0, size, size);
    const blob = await new Promise<Blob | null>((r) => out.toBlob(r, "image/png"));
    if (!blob) throw new Error("thumbnail encode failed");
    return new Uint8Array(await blob.arrayBuffer());
  }

  async measureOrbit(seconds: number): Promise<OrbitMeasurement> {
    this.forceRender = true;
    this.app.autoRender = true;
    const times: number[] = [];
    try {
      let last = await sleepFrame();
      const end = last + seconds * 1000;
      while (true) {
        const now = await sleepFrame();
        const dt = now - last;
        last = now;
        times.push(dt);
        this.pose.yaw += 36 * (dt / 1000);
        this.applyPose();
        if (now >= end) break;
      }
    } finally {
      this.forceRender = false;
      this.wake();
    }
    const sorted = [...times].sort((a, b) => a - b);
    const q = (p: number) => sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))] ?? 0;
    return {
      frames: times.length,
      medianFps: q(0.5) ? 1000 / q(0.5) : 0,
      p5Fps: q(0.95) ? 1000 / q(0.95) : 0,
      frameTimesMs: times.map((t) => Math.round(t * 100) / 100),
    };
  }

  private attachInput(): void {
    const c = this.canvas;
    const on = <K extends keyof HTMLElementEventMap>(
      type: K,
      fn: (e: HTMLElementEventMap[K]) => void,
      opts?: AddEventListenerOptions,
    ) => {
      c.addEventListener(type, fn as EventListener, opts);
      this.cleanup.push(() => c.removeEventListener(type, fn as EventListener, opts));
    };
    let mode: "orbit" | "pan" = "orbit";
    on("contextmenu", (e) => e.preventDefault());
    on("pointerdown", (e) => {
      try {
        c.setPointerCapture(e.pointerId);
      } catch {
        /* synthetic pointers may not be capturable */
      }
      this.pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      mode = e.button === 1 || e.button === 2 || e.shiftKey ? "pan" : "orbit";
      if (this.pointers.size === 2) {
        const [a, b] = [...this.pointers.values()];
        this.pinchDist = Math.hypot(a.x - b.x, a.y - b.y);
      }
      this.wake();
    });
    on("pointermove", (e) => {
      const prev = this.pointers.get(e.pointerId);
      if (!prev) return;
      const dx = e.clientX - prev.x;
      const dy = e.clientY - prev.y;
      this.pointers.set(e.pointerId, { x: e.clientX, y: e.clientY });
      if (this.pointers.size >= 2) {
        const [a, b] = [...this.pointers.values()];
        const d = Math.hypot(a.x - b.x, a.y - b.y);
        if (this.pinchDist > 0 && d > 0) this.zoomBy(this.pinchDist / d);
        this.pinchDist = d;
        this.panBy(dx / 2, dy / 2);
      } else if (mode === "pan") {
        this.panBy(dx, dy);
      } else {
        this.orbitBy(-dx * 0.3, -dy * 0.3);
      }
    });
    const end = (e: PointerEvent) => {
      this.pointers.delete(e.pointerId);
      this.pinchDist = 0;
    };
    on("pointerup", end);
    on("pointercancel", end);
    on(
      "wheel",
      (e) => {
        e.preventDefault();
        const k = e.ctrlKey ? 0.01 : 0.0015;
        const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
        this.zoomBy(Math.exp(Math.max(-1, Math.min(1, dy * k))));
      },
      { passive: false },
    );
    on("keydown", (e) => {
      const step = 10;
      const panStep = 30;
      let handled = true;
      switch (e.key) {
        case "ArrowLeft":
          if (e.shiftKey) this.panBy(panStep, 0);
          else this.orbitBy(step, 0);
          break;
        case "ArrowRight":
          if (e.shiftKey) this.panBy(-panStep, 0);
          else this.orbitBy(-step, 0);
          break;
        case "ArrowUp":
          if (e.shiftKey) this.panBy(0, panStep);
          else this.orbitBy(0, step);
          break;
        case "ArrowDown":
          if (e.shiftKey) this.panBy(0, -panStep);
          else this.orbitBy(0, -step);
          break;
        case "+":
        case "=":
          this.zoomBy(0.85);
          break;
        case "-":
        case "_":
          this.zoomBy(1 / 0.85);
          break;
        case "r":
        case "R":
        case "0":
          this.resetView();
          break;
        default:
          handled = false;
      }
      if (handled) e.preventDefault();
    });
  }
}
