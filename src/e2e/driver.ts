// Scripted native E2E driver. Compiled in ONLY with `vite build --mode e2e`
// (main.tsx guards the dynamic import with __E2E__). It drives the real UI via
// the DOM and reports to Rust through the e2e-only commands.
import { invoke } from "@tauri-apps/api/core";
import type { ExportResult, ProjectDetail } from "../lib/ipc";
import type { ViewerAutomation } from "../viewer/viewer";

interface E2EConfig {
  phase: "first" | "relaunch" | "perf";
  testKey: string;
  wrongKey: string;
  teapotPrompt: string;
  secondPrompt: string;
  perfPlyPath: string | null;
}

interface Step {
  name: string;
  ok: boolean;
  detail: unknown;
  ms: number;
}

const OVERALL_TIMEOUT_MS = 180_000;
const steps: Step[] = [];
const data: Record<string, unknown> = {};

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const viewer = () => (window as unknown as { __viewer: ViewerAutomation }).__viewer;

async function waitFor<T>(what: string, fn: () => T | null | undefined | false, timeoutMs = 30_000, onPoll?: () => void): Promise<T> {
  const end = performance.now() + timeoutMs;
  let lastErr: unknown = null;
  while (performance.now() < end) {
    onPoll?.();
    try {
      const v = fn();
      if (v) return v;
    } catch (e) {
      lastErr = e;
    }
    await sleep(100);
  }
  throw new Error(`timed out after ${timeoutMs} ms waiting for ${what}${lastErr ? ` (last error: ${String(lastErr)})` : ""}`);
}

/** Saves progress so a stalled run still shows how far it got. */
function checkpoint(current: string) {
  void invoke("e2e_report", { report: { partial: true, current, steps, ...data }, quit: false }).catch(() => {});
}

async function step<T>(name: string, fn: () => Promise<T> | T): Promise<T> {
  const t = performance.now();
  checkpoint(name);
  try {
    const v = await fn();
    steps.push({ name, ok: true, detail: v ?? null, ms: Math.round(performance.now() - t) });
    return v;
  } catch (e) {
    steps.push({ name, ok: false, detail: e instanceof Error ? e.message : String(e), ms: Math.round(performance.now() - t) });
    throw e;
  }
}

const norm = (s: string | null | undefined) => (s ?? "").replace(/\s+/g, " ").trim();

/**
 * True when the OS has paused rendering for this window (locked screen,
 * display asleep, fully occluded). Pixel checks are then recorded as skipped
 * rather than passed; the orchestrator reports them as unverified.
 */
function renderPaused(): boolean {
  const paused = document.visibilityState === "hidden" && (viewer()?.frames ?? 0) === 0;
  if (paused) data.renderPaused = "document hidden and no frames rendered: the OS paused rendering (screen locked or display asleep)";
  return paused;
}

function assertVisible(nonBackgroundFraction: number) {
  if (renderPaused()) return;
  if (!(nonBackgroundFraction > 0.01)) throw new Error(`blank viewport (${nonBackgroundFraction})`);
}

function accessibleName(el: Element): string {
  return norm(el.getAttribute("aria-label") ?? el.textContent);
}

function button(name: string | RegExp): HTMLButtonElement | null {
  const all = Array.from(document.querySelectorAll<HTMLButtonElement>("button"));
  return (
    all.find((b) => (typeof name === "string" ? accessibleName(b) === name : name.test(accessibleName(b)))) ?? null
  );
}

function mustButton(name: string | RegExp): HTMLButtonElement {
  const b = button(name);
  if (!b) throw new Error(`button ${String(name)} not found`);
  return b;
}

function byLabel<T extends HTMLElement>(text: string): T | null {
  const label = Array.from(document.querySelectorAll("label")).find((l) => norm(l.textContent) === text);
  return (label?.control as T | null | undefined) ?? null;
}

function setValue(el: HTMLInputElement | HTMLTextAreaElement, value: string) {
  const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  Object.getOwnPropertyDescriptor(proto, "value")!.set!.call(el, value);
  el.dispatchEvent(new Event("input", { bubbles: true }));
}

function alertText(): string | null {
  const a = Array.from(document.querySelectorAll('[role="alert"]')).map((e) => norm(e.textContent)).filter(Boolean);
  return a.length ? a.join(" | ") : null;
}

function prompt(): HTMLTextAreaElement | null {
  return byLabel<HTMLTextAreaElement>("Describe one object");
}

function canvas(): HTMLCanvasElement {
  const c = document.querySelector<HTMLCanvasElement>('canvas[role="img"]');
  if (!c) throw new Error("viewer canvas not found");
  return c;
}

function pressCtrlEnter(el: HTMLElement) {
  const mac = /Mac/i.test(navigator.platform);
  el.dispatchEvent(
    new KeyboardEvent("keydown", { key: "Enter", code: "Enter", ctrlKey: !mac, metaKey: mac, bubbles: true, cancelable: true }),
  );
}

function key(el: HTMLElement, k: string) {
  el.dispatchEvent(new KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true }));
}

async function waitViewerLoad(afterLoads: number, timeoutMs = 120_000, onPoll?: () => void) {
  return waitFor(
    "viewer ready",
    () => {
      const v = viewer();
      if (v.error) throw new Error(`viewer error: ${v.error}`);
      return v.ready && v.loads > afterLoads ? v : null;
    },
    timeoutMs,
    onPoll,
  );
}

function pixelDiffFraction(a: Uint8ClampedArray, b: Uint8ClampedArray): number {
  if (a.length !== b.length) return 1;
  let bad = 0;
  for (let i = 0; i < a.length; i += 4) {
    if (Math.abs(a[i] - b[i]) + Math.abs(a[i + 1] - b[i + 1]) + Math.abs(a[i + 2] - b[i + 2]) > 30) bad++;
  }
  return bad / (a.length / 4);
}

const sameCam = (a: ReturnType<ViewerAutomation["getCamera"]>, b: ReturnType<ViewerAutomation["getCamera"]>) =>
  Math.abs(a.yaw - b.yaw) < 1e-3 &&
  Math.abs(a.pitch - b.pitch) < 1e-3 &&
  Math.abs(a.distance - b.distance) < 1e-3 * Math.max(1, a.distance);

async function phaseFirst(cfg: E2EConfig) {
  await step("connect screen shown", () =>
    waitFor("Connect heading", () =>
      Array.from(document.querySelectorAll("h1")).some((h) => norm(h.textContent) === "Choose your AI"),
    ),
  );
  await step("local AI detection shown", () =>
    waitFor("Claude Code and Ollama cards", () => {
      const names = Array.from(document.querySelectorAll('section[aria-label="Found on this computer"] h3')).map((h) =>
        norm(h.textContent),
      );
      return names.some((n) => n.startsWith("Claude Code")) && names.some((n) => n.startsWith("Ollama")) ? names : null;
    }),
  );
  await step("choose Anthropic", () => {
    const radio = Array.from(document.querySelectorAll<HTMLInputElement>('input[type="radio"]')).find(
      (r) => r.value === "anthropic",
    );
    if (!radio) throw new Error("Anthropic option not found");
    radio.click();
    return true;
  });
  const SECTION = 'section[aria-label="Connect Anthropic"]';
  const keyInput = () => {
    const el = document.querySelector<HTMLInputElement>(`${SECTION} input[type="password"]`);
    if (!el) throw new Error("key input not found");
    return el;
  };
  const connectBtn = () => {
    const b = Array.from(document.querySelectorAll<HTMLButtonElement>(`${SECTION} button[type="submit"]`))[0];
    if (!b) throw new Error("Connect button not found");
    return b;
  };
  await step("wrong key rejected", async () => {
    setValue(keyInput(), cfg.wrongKey);
    connectBtn().click();
    // React commits the cleared field on the next render, not synchronously.
    const cleared = await waitFor("key input cleared after Connect", () => keyInput().value === "", 2_000);
    const msg = await waitFor("invalid-key error", () => alertText(), 30_000);
    return { cleared, error: msg };
  });
  await step("valid key connects", async () => {
    setValue(keyInput(), cfg.testKey);
    connectBtn().click();
    const cleared = await waitFor(
      "key input cleared after Connect",
      () => !document.querySelector(`${SECTION} input[type="password"]`) || keyInput().value === "",
      2_000,
    );
    await waitFor("Create screen", () => prompt(), 30_000);
    return { cleared, keyInputGone: !document.querySelector('input[type="password"]') };
  });

  const labels = new Set<string>();
  const collect = () => {
    const l = norm(document.querySelector('[data-testid="job-label"]')?.textContent);
    if (l) labels.add(l);
  };
  const loadsBefore = viewer().loads;
  await step("generate teapot", async () => {
    const p = prompt()!;
    setValue(p, cfg.teapotPrompt);
    p.focus();
    pressCtrlEnter(p);
    await waitFor("Generate disabled (job active)", () => mustButton("Generate").disabled, 10_000, collect);
    return true;
  });
  await step("model picker shows the chosen model", () =>
    waitFor("model select", () => {
      const sel = document.querySelector<HTMLSelectElement>(".create .model-picker select");
      return sel?.value ? sel.value : null;
    }),
  );
  await step("viewer ready", async () => {
    const v = await waitViewerLoad(loadsBefore, 120_000, collect);
    const s = v.stats();
    data.labelsSeen = [...labels];
    data.viewer = { firstFrameMs: v.firstFrameMs, splatCount: v.splatCount, nonBackgroundFraction: s.nonBackgroundFraction, frames: v.frames };
    assertVisible(s.nonBackgroundFraction);
    return data.viewer;
  });
  const home = viewer().getCamera();
  await step("orbit by pointer drag", async () => {
    const c = canvas();
    const before = viewer().getCamera();
    const px0 = await viewer().readPixels();
    const r = c.getBoundingClientRect();
    const x = r.left + r.width / 2;
    const y = r.top + r.height / 2;
    const ev = (type: string, dx: number) =>
      c.dispatchEvent(
        new PointerEvent(type, {
          pointerId: 7,
          pointerType: "mouse",
          button: 0,
          buttons: type === "pointerup" ? 0 : 1,
          clientX: x + dx,
          clientY: y,
          bubbles: true,
          cancelable: true,
        }),
      );
    ev("pointerdown", 0);
    for (let i = 1; i <= 10; i++) {
      ev("pointermove", i * 15);
      await sleep(16);
    }
    ev("pointerup", 150);
    await sleep(400);
    const after = viewer().getCamera();
    const px1 = await viewer().readPixels();
    const diff = pixelDiffFraction(px0.data, px1.data);
    if (Math.abs(after.yaw - before.yaw) < 1) throw new Error(`yaw unchanged (${before.yaw} → ${after.yaw})`);
    if (renderPaused()) return { yawBefore: before.yaw, yawAfter: after.yaw, pixelDiff: "skipped: rendering paused by OS" };
    if (diff < 0.002) throw new Error(`pixels unchanged after orbit (${diff})`);
    return { yawBefore: before.yaw, yawAfter: after.yaw, pixelDiff: diff };
  });
  await step("keyboard orbit and reset", async () => {
    const c = canvas();
    c.focus();
    if (document.activeElement !== c) throw new Error("canvas not focusable");
    const before = viewer().getCamera();
    key(c, "ArrowLeft");
    const after = viewer().getCamera();
    if (Math.abs(after.yaw - before.yaw) < 1) throw new Error("ArrowLeft did not orbit");
    key(c, "r");
    const reset = viewer().getCamera();
    if (!sameCam(reset, home)) throw new Error(`R did not reset (${JSON.stringify(reset)} vs ${JSON.stringify(home)})`);
    return { yawBefore: before.yaw, yawAfter: after.yaw, reset: true };
  });
  await step("export splat", async () => {
    (window as unknown as { __lastExport?: ExportResult }).__lastExport = undefined;
    mustButton("Export splat (.ply)").click();
    const res = await waitFor(
      "export result",
      () => {
        const err = alertText();
        if (err) throw new Error(err);
        return (window as unknown as { __lastExport?: ExportResult }).__lastExport;
      },
      30_000,
    );
    if (!res.saved) throw new Error("export not saved");
    data.export = res;
    return res;
  });
  const loads2 = viewer().loads;
  await step("second object's design is unusable", async () => {
    const p = prompt()!;
    setValue(p, cfg.secondPrompt);
    await waitFor("Generate enabled", () => !mustButton("Generate").disabled, 10_000);
    mustButton("Generate").click();
    await waitFor("Ask again button", () => button("Ask again…"), 120_000);
    return { error: alertText() };
  });
  await step("ask again (confirmed) succeeds", async () => {
    mustButton("Ask again…").click();
    await waitFor("billing confirmation", () => button("Send new request"), 5_000);
    mustButton("Send new request").click();
    const v = await waitViewerLoad(loads2, 120_000);
    const s = v.stats();
    assertVisible(s.nonBackgroundFraction);
    data.retryViewer = { firstFrameMs: v.firstFrameMs, splatCount: v.splatCount, nonBackgroundFraction: s.nonBackgroundFraction };
    return data.retryViewer;
  });
}

async function phaseRelaunch(cfg: E2EConfig) {
  await step("create screen shown directly", () =>
    waitFor("Create screen", () => {
      if (Array.from(document.querySelectorAll("h1")).some((h) => norm(h.textContent) === "Choose your AI"))
        throw new Error("Connect screen shown for returning user");
      return prompt();
    }).then(() => true),
  );
  const btn = await step("recent teapot with thumbnail", async () => {
    const b = await waitFor(
      "teapot in Recent objects",
      () => {
        const items = Array.from(document.querySelectorAll<HTMLButtonElement>('[aria-label="Recent objects"] li button'));
        const found = items.find((i) => norm(i.textContent).includes(norm(cfg.teapotPrompt)));
        return found?.querySelector("img") ? found : null;
      },
      30_000,
    );
    const img = b.querySelector<HTMLImageElement>("img")!;
    if (img.complete && img.naturalWidth > 0) return b;
    // A hidden (locked-screen) webview may never decode images, so prove the
    // thumbnail through its bytes instead of its on-screen decode.
    const bytes = new Uint8Array(await (await fetch(img.src)).arrayBuffer());
    const png = [0x89, 0x50, 0x4e, 0x47].every((v, i) => bytes[i] === v);
    const jpeg = bytes[0] === 0xff && bytes[1] === 0xd8;
    if (!(png || jpeg)) throw new Error(`thumbnail is not an image (${bytes.length} bytes)`);
    data.thumbnail = { verifiedBy: "bytes", bytes: bytes.length };
    return b;
  });
  const id = btn.dataset.projectId;
  if (!id) throw new Error("recent item has no project id");
  const loads = viewer().loads;
  await step("open teapot", async () => {
    btn.click();
    const v = await waitViewerLoad(loads, 60_000);
    const s = v.stats();
    assertVisible(s.nonBackgroundFraction);
    data.viewer = { firstFrameMs: v.firstFrameMs, splatCount: v.splatCount, nonBackgroundFraction: s.nonBackgroundFraction };
    return data.viewer;
  });
  await step("project sha256", async () => {
    const d = await invoke<ProjectDetail>("get_project", { projectId: id });
    data.project = { id, splatSha256: d.splatSha256, splatBytes: d.splatBytes };
    return data.project;
  });
}

async function phasePerf() {
  const loads = viewer()?.loads ?? 0;
  await step("open perf ply", async () => {
    // Fresh profile without a key: go to the viewer without connecting.
    await waitFor("viewer or connect screen", () => button("Open 3D file…") || button("Open a .ply file without connecting"), 30_000);
    button("Open a .ply file without connecting")?.click();
    await waitFor("Open 3D file button", () => button("Open 3D file…"), 30_000);
    mustButton("Open 3D file…").click();
    const v = await waitViewerLoad(loads, 120_000);
    const c = canvas();
    const r = c.getBoundingClientRect();
    data.viewer = {
      firstFrameMs: v.firstFrameMs,
      splatCount: v.splatCount,
      nonBackgroundFraction: v.stats().nonBackgroundFraction,
      cssSize: [Math.round(r.width), Math.round(r.height)],
      bufferSize: [c.width, c.height],
      devicePixelRatio: window.devicePixelRatio,
    };
    return data.viewer;
  });
  await step("measure orbit", async () => {
    if (renderPaused()) return { skipped: "rendering paused by OS; no frame timing possible" };
    const m = await viewer().measureOrbit(10);
    data.orbit = m;
    return { frames: m.frames, medianFps: m.medianFps, p5Fps: m.p5Fps };
  });
}

function captureConsole() {
  const logs: string[] = [];
  data.console = logs;
  const push = (kind: string, args: unknown[]) => {
    if (logs.length < 80) logs.push(`${kind}: ${args.map((a) => (a instanceof Error ? `${a.message} ${a.stack ?? ""}` : String(a))).join(" ").slice(0, 600)}`);
  };
  for (const k of ["error", "warn"] as const) {
    const orig = console[k].bind(console);
    console[k] = (...a: unknown[]) => {
      push(k, a);
      orig(...a);
    };
  }
  window.addEventListener("error", (e) => push("onerror", [e.message, e.filename, e.lineno]));
  window.addEventListener("unhandledrejection", (e) => push("rejection", [e.reason]));
}

export async function run(): Promise<void> {
  captureConsole();
  let cfg: E2EConfig | null = null;
  let error: string | null = null;
  const t0 = performance.now();
  try {
    cfg = await invoke<E2EConfig>("e2e_config");
    const c = cfg;
    const phase = c.phase === "first" ? phaseFirst(c) : c.phase === "relaunch" ? phaseRelaunch(c) : phasePerf();
    await Promise.race([
      phase,
      sleep(OVERALL_TIMEOUT_MS).then(() => {
        throw new Error(`overall timeout ${OVERALL_TIMEOUT_MS} ms`);
      }),
    ]);
  } catch (e) {
    error = e instanceof Error ? e.message : String(e);
    try {
      const v = viewer();
      const c = document.querySelector<HTMLCanvasElement>('canvas[role="img"]');
      data.viewerState = v
        ? {
            ready: v.ready,
            error: v.error,
            loads: v.loads,
            created: v.created,
            splatCount: v.splatCount,
            frames: (v as unknown as { frames?: number }).frames,
            canvas: c ? [c.clientWidth, c.clientHeight, c.width, c.height] : null,
            visibility: document.visibilityState,
          }
        : null;
    } catch {}
  }
  const report = {
    phase: cfg?.phase ?? null,
    ok: error === null && steps.every((s) => s.ok),
    error,
    totalMs: Math.round(performance.now() - t0),
    steps,
    ...data,
  };
  try {
    await invoke("e2e_report", { report, quit: true });
  } catch (e) {
    console.error("e2e_report failed", e, report);
  }
}
