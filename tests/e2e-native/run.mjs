#!/usr/bin/env node
// Native end-to-end run of the real desktop app (Tauri webview + Rust core)
// against the local fake provider. No keys, no paid calls, no network.
//
//   node tests/e2e-native/run.mjs            # build, first run, offline relaunch
//   node tests/e2e-native/run.mjs --perf     # + 100k-splat performance phase
//   node tests/e2e-native/run.mjs --no-build # reuse existing e2e build
//
// Linux CI: wrap in `xvfb-run -a`. Writes tests/output/e2e-native-report.json.
import { spawn, spawnSync, execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "../..");
const args = new Set(process.argv.slice(2));
const exe = process.platform === "win32" ? ".exe" : "";
const appBin = path.join(root, "target", "debug", `threedmaker${exe}`);
const fakeBin = path.join(root, "target", "debug", `fake-provider${exe}`);
const out = path.join(root, "tests", "output", `e2e-${Date.now()}`);
mkdirSync(out, { recursive: true });
const results = { startedAt: new Date().toISOString(), environment: environment(), phases: {}, checks: [] };
let failed = false;

function check(name, ok, detail) {
  results.checks.push({ name, ok: !!ok, detail });
  console.log(`${ok ? "PASS" : "FAIL"}  ${name}${detail !== undefined ? `  ${JSON.stringify(detail)}` : ""}`);
  if (!ok) failed = true;
}

function sh(cmd, opts = {}) {
  console.log(`$ ${cmd}`);
  execSync(cmd, { cwd: root, stdio: "inherit", ...opts });
}

function sha256(file) {
  return createHash("sha256").update(readFileSync(file)).digest("hex");
}

function environment() {
  const env = { os: `${os.type()} ${os.release()}`, platform: process.platform, arch: os.arch(), cpu: os.cpus()[0]?.model, memGB: Math.round(os.totalmem() / 2 ** 30) };
  try {
    if (process.platform === "darwin") {
      const gpu = spawnSync("system_profiler", ["SPDisplaysDataType"], { encoding: "utf8" }).stdout;
      env.gpu = (gpu.match(/Chipset Model: (.*)/) || [])[1];
      env.metal = (gpu.match(/Metal Support: (.*)/) || [])[1];
      env.osVersion = spawnSync("sw_vers", ["-productVersion"], { encoding: "utf8" }).stdout.trim();
    } else if (process.platform === "linux") {
      env.gpu = spawnSync("sh", ["-c", "lspci 2>/dev/null | grep -i -E 'vga|3d' | head -1"], { encoding: "utf8" }).stdout.trim() || "unknown";
    } else if (process.platform === "win32") {
      env.gpu = spawnSync("powershell", ["-NoProfile", "-Command", "(Get-CimInstance Win32_VideoController | Select-Object -First 1).Name"], { encoding: "utf8" }).stdout.trim();
    }
  } catch {}
  return env;
}

// ---- build ----
if (!args.has("--no-build")) {
  sh("cargo run -q -p maker-core --example make_fixtures -- fixtures");
  sh("npm run build:e2e");
  const tauriConfig = readFileSync(path.join(root, "src-tauri", "tauri.e2e.conf.json"), "utf8");
  // dist/ is embedded at compile time; make Cargo notice the new frontend build.
  const now = new Date();
  utimesSync(path.join(root, "src-tauri", "src", "lib.rs"), now, now);
  // custom-protocol embeds dist/ (otherwise a debug build would load the dev server).
  sh('cargo build -p threedmaker --features "e2e tauri/custom-protocol" -p fake-provider', { env: { ...process.env, TAURI_CONFIG: tauriConfig } });
}

// ---- fake provider ----
function startFake() {
  return new Promise((resolve, reject) => {
    const p = spawn(fakeBin, ["--bad-design-once-for", "forklift", "--delay-ms", "1500"], { stdio: ["ignore", "pipe", "inherit"] });
    p.stdout.on("data", (d) => {
      const m = String(d).match(/LISTENING (\S+)/);
      if (m) resolve({ proc: p, base: m[1] });
    });
    p.on("exit", (c) => reject(new Error(`fake provider exited ${c}`)));
  });
}

// ---- process memory sampling (app + its webview processes) ----
function psTable() {
  if (process.platform === "win32") {
    const r = spawnSync("powershell", ["-NoProfile", "-Command", "Get-CimInstance Win32_Process | ForEach-Object { \"$($_.ProcessId) $($_.ParentProcessId) $([int]($_.WorkingSetSize/1024)) $($_.Name)\" }"], { encoding: "utf8" });
    return r.stdout.split(/\r?\n/).filter(Boolean).map((l) => { const [pid, ppid, rss, ...n] = l.trim().split(/\s+/); return { pid: +pid, ppid: +ppid, rss: +rss, name: n.join(" ") }; });
  }
  const r = spawnSync("ps", ["-A", "-o", "pid=,ppid=,rss=,comm="], { encoding: "utf8" });
  return r.stdout.split("\n").filter(Boolean).map((l) => { const [pid, ppid, rss, ...n] = l.trim().split(/\s+/); return { pid: +pid, ppid: +ppid, rss: +rss, name: n.join(" ") }; });
}

function sampler(appPid, preexisting) {
  let peakKB = 0;
  const timer = setInterval(() => {
    const t = psTable();
    const mine = new Set([appPid]);
    let grew = true;
    while (grew) { grew = false; for (const p of t) if (!mine.has(p.pid) && mine.has(p.ppid)) { mine.add(p.pid); grew = true; } }
    // macOS WebKit content/GPU processes are spawned by launchd (XPC), not the app.
    for (const p of t) if (!preexisting.has(p.pid) && /WebKit|WebView2|msedgewebview2/i.test(p.name)) mine.add(p.pid);
    const kb = t.filter((p) => mine.has(p.pid)).reduce((a, p) => a + p.rss, 0);
    peakKB = Math.max(peakKB, kb);
  }, 250);
  return { stop: () => { clearInterval(timer); return peakKB; } };
}

// ---- app runs ----
function runApp(phase, extraEnv, timeoutMs = 200_000) {
  const report = path.join(out, `report-${phase}.json`);
  rmSync(report, { force: true });
  const pre = new Set(psTable().map((p) => p.pid));
  return new Promise((resolve) => {
    const t0 = Date.now();
    const p = spawn(appBin, [], { env: { ...process.env, MAKER_E2E_PHASE: phase, MAKER_E2E_REPORT: report, ...extraEnv }, stdio: "inherit" });
    const mem = sampler(p.pid, pre);
    const kill = setTimeout(() => { console.error(`${phase}: timeout, killing app`); p.kill(); }, timeoutMs);
    p.on("exit", (code) => {
      clearTimeout(kill);
      const peakKB = mem.stop();
      let r = null;
      try { r = JSON.parse(readFileSync(report, "utf8")); } catch {}
      resolve({ code, report: r, wallMs: Date.now() - t0, peakMB: Math.round(peakKB / 1024) });
    });
  });
}

const profile = path.join(out, "profile");
const exportPath = path.join(out, "export", "teapot.ply");
mkdirSync(path.dirname(exportPath), { recursive: true });

const fake = await startFake();
let stats;
try {
  const first = await runApp("first", {
    MAKER_DATA_DIR: profile,
    MAKER_TEST_PROVIDER_BASE: fake.base,
    // The fake binary doubles as a Claude Code CLI, so detection doesn't depend on this machine.
    MAKER_TEST_CLAUDE_CLI: fakeBin,
    MAKER_E2E_EXPORT_PATH: exportPath,
  });
  results.phases.first = first;
  check("first run: driver finished without failures", first.report?.ok, first.report?.error ?? first.report?.steps?.filter((s) => !s.ok));
  for (const s of first.report?.steps ?? []) check(`first: ${s.name}`, s.ok, s.ok ? undefined : s.detail);
  stats = await (await fetch(`${fake.base}/__admin/stats`)).json();
} finally {
  fake.proc.kill();
}
results.providerCounters = stats;
check("design requests: teapot 1, forklift 1 unusable + 1 explicit retry", stats?.["anthropic.messages"] === 3, stats);
check("key checks used the free model-list endpoint", (stats?.["anthropic.models"] ?? 0) >= 2, stats);

check("export exists", existsSync(exportPath));
if (existsSync(exportPath)) {
  check("export sha matches what the app reported", results.phases.first.report?.export?.sha256 === sha256(exportPath), { export: sha256(exportPath) });
}

if (results.phases.first.report?.renderPaused) {
  results.renderingUnverified = results.phases.first.report.renderPaused;
  console.log(`SKIP  native pixel checks: ${results.renderingUnverified}`);
}

if (args.has("--first-only")) {
  writeFileSync(path.join(root, "tests", "output", "e2e-native-report.json"), JSON.stringify(results, null, 2));
  process.exit(failed ? 1 : 0);
}

// Network "disabled": the fake server is stopped and the provider base points at a closed port.
const relaunch = await runApp("relaunch", { MAKER_DATA_DIR: profile, MAKER_TEST_PROVIDER_BASE: "http://127.0.0.1:9" });
results.phases.relaunch = relaunch;
check("offline relaunch: driver finished without failures", relaunch.report?.ok, relaunch.report?.error ?? relaunch.report?.steps?.filter((s) => !s.ok));
for (const s of relaunch.report?.steps ?? []) check(`relaunch: ${s.name}`, s.ok, s.ok ? undefined : s.detail);
if (existsSync(exportPath)) {
  check("reopened asset is the same file", relaunch.report?.project?.splatSha256 === sha256(exportPath), relaunch.report?.project);
}

if (existsSync(exportPath) && !args.has("--skip-independent")) {
  const r = spawnSync("npx", ["playwright", "test", "tests/render/independent.spec.ts", "--reporter=line"], { cwd: root, env: { ...process.env, INDEPENDENT_PLY: exportPath }, stdio: "inherit", shell: process.platform === "win32" });
  check("exported PLY opens in an independent viewer (GaussianSplats3D)", r.status === 0);
}

if (args.has("--perf")) {
  const perfProfile = path.join(out, "perf-profile");
  // Window sized so the viewport (window minus the 340px sidebar and header) is about 1280x720.
  const perf = await runApp("perf", { MAKER_DATA_DIR: perfProfile, MAKER_TEST_PROVIDER_BASE: "http://127.0.0.1:9", MAKER_E2E_PERF_PLY: path.join(root, "fixtures", "perf-100k.ply"), MAKER_E2E_WINDOW: "1620x790" });
  results.phases.perf = perf;
  const o = perf.report?.orbit;
  check("perf: driver finished", perf.report?.ok, perf.report?.error);
  results.performance = {
    fixture: "fixtures/perf-100k.ply (100,000 splats)",
    timeToFirstFrameMs: perf.report?.viewer?.firstFrameMs,
    canvasCss: perf.report?.viewer?.cssSize,
    drawingBuffer: perf.report?.viewer?.bufferSize,
    devicePixelRatio: perf.report?.viewer?.devicePixelRatio,
    orbitSeconds: 10,
    medianFps: o?.medianFps,
    p5Fps: o?.p5Fps,
    frames: o?.frames,
    peakMemoryMB: perf.peakMB,
    note: "Peak memory sums the app process and its webview processes (RSS). Provider latency is not included.",
  };
  if (perf.report?.renderPaused) {
    results.performance.unverified = perf.report.renderPaused;
    console.log(`SKIP  perf frame timing: ${perf.report.renderPaused}`);
  } else {
    check("perf: median FPS >= 30 during a 10 s orbit", (o?.medianFps ?? 0) >= 30, results.performance);
  }
}

results.finishedAt = new Date().toISOString();
results.ok = !failed;
writeFileSync(path.join(root, "tests", "output", "e2e-native-report.json"), JSON.stringify(results, null, 2));
console.log(`\n${failed ? "E2E FAILED" : "E2E PASSED"} — report: tests/output/e2e-native-report.json (artifacts in ${path.relative(root, out)})`);
process.exit(failed ? 1 : 0);
