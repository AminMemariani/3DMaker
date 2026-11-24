// Renderer correctness: the app's viewer module renders fixtures/renderer-asym.ply
// from two fixed viewpoints; see docs/dev/renderer-fixture.md.
import { test, expect, type Page } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  FIX,
  barDiagonals,
  countIn,
  decodePng,
  encodePng,
  fromB64,
  meanAbsDiff,
  mismatchFraction,
  project,
  type Img,
  type V3,
} from "./util";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const REFS = path.join(__dirname, "refs");
const FIXTURE = path.join(__dirname, "../../fixtures/renderer-asym.ply");
const CAM_A: V3 = [0, 0, 3];
const CAM_B: V3 = [0, 0, -3];

async function capture(page: Page, cam: V3): Promise<Img> {
  const r = await page.evaluate(async (pos) => {
    const v = (window as any).__viewer;
    v.setView({ position: pos, target: [0, 0, 0], fov: 45 });
    // Let the asynchronous splat sort settle for the new viewpoint.
    for (let i = 0; i < 3; i++) {
      await new Promise((res) => setTimeout(res, 300));
      await v.readPixels();
    }
    const px = await v.readPixels();
    let s = "";
    for (let i = 0; i < px.data.length; i += 0x8000) s += String.fromCharCode(...px.data.subarray(i, i + 0x8000));
    return { width: px.width, height: px.height, b64: btoa(s) };
  }, cam);
  return fromB64(r.width, r.height, r.b64);
}

test("renders the asymmetric fixture correctly from front and back", async ({ page }) => {
  test.skip(!fs.existsSync(FIXTURE), "fixtures/renderer-asym.ply missing — run `npm run fixtures`");
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`${m.type()}: ${m.text()}`));
  page.on("pageerror", (e) => logs.push(`pageerror: ${e.message}`));

  await page.goto("/tests/render/harness.html");
  await page.waitForFunction(() => (window as any).harnessReady === true);
  const created = await page.evaluate(() => (window as any).__viewer.created);
  expect(created, `viewer did not start (WebGL2 unavailable?)\n${logs.join("\n")}`).toBe(true);

  const info = await page.evaluate(() =>
    (window as any).__viewer.loadUrl("/fixtures/renderer-asym.ply", { viewRotation: [0, 0, 0], bounds: null }),
  );
  expect(info.splatCount).toBe(6);

  const a = await capture(page, CAM_A);
  const b = await capture(page, CAM_B);
  expect(a.width).toBe(512);
  expect(a.height).toBe(512);
  const W = a.width;
  const H = a.height;

  const nonBg = await page.evaluate(() => (window as any).__viewer.stats().nonBackgroundFraction);
  expect(nonBg, "non-background fraction").toBeGreaterThan(0.03);

  // Front/back markers: same top-centre region shows green in A and magenta in B.
  const gA = project(CAM_A, FIX.green, W, H);
  const mB = project(CAM_B, FIX.magenta, W, H);
  const box = 6;
  const area = (2 * box + 1) ** 2;
  expect(countIn(a, gA[0], gA[1], box, "green") / area, "green at top-centre in A").toBeGreaterThan(0.6);
  expect(countIn(a, gA[0], gA[1], box, "magenta"), "magenta hidden in A").toBe(0);
  expect(countIn(b, mB[0], mB[1], box, "magenta") / area, "magenta at top-centre in B").toBeGreaterThan(0.6);
  expect(countIn(b, mB[0], mB[1], box, "green"), "green hidden in B").toBe(0);

  // Depth order: red near disk in A (blue behind), blue near disk in B at the mirrored location.
  const rA = project(CAM_A, FIX.red, W, H);
  const bB = project(CAM_B, FIX.blue, W, H);
  expect(rA[0]).toBeGreaterThan(W / 2); // lower-right in A
  expect(bB[0]).toBeLessThan(W / 2); // lower-left in B
  expect(countIn(a, rA[0], rA[1], box, "red") / area, "red visible in A").toBeGreaterThan(0.6);
  expect(countIn(a, rA[0], rA[1], box, "blue"), "blue not over red in A").toBe(0);
  expect(countIn(b, bB[0], bB[1], box, "blue") / area, "blue visible in B").toBeGreaterThan(0.6);
  expect(countIn(b, bB[0], bB[1], box, "red"), "red not over blue in B").toBe(0);

  // White anchor: lower-left in A, lower-right in B.
  const wA = project(CAM_A, FIX.white, W, H);
  const wB = project(CAM_B, FIX.white, W, H);
  expect(countIn(a, wA[0], wA[1], 4, "white"), "white anchor in A").toBeGreaterThan(20);
  expect(countIn(b, wB[0], wB[1], 4, "white"), "white anchor in B").toBeGreaterThan(20);

  // Yellow bar orientation: "/" in A, "\" in B.
  const c = project(CAM_A, FIX.bar, W, H);
  const dA = barDiagonals(a, c[0], c[1], 95);
  const dB = barDiagonals(b, c[0], c[1], 95);
  expect(dA.slash, `bar diagonal A ${JSON.stringify(dA)}`).toBeGreaterThan(3 * dA.backslash + 50);
  expect(dB.backslash, `bar diagonal B ${JSON.stringify(dB)}`).toBeGreaterThan(3 * dB.slash + 50);

  // The two views must differ substantially (not a flat billboard).
  expect(mismatchFraction(a, b, 40), "A vs B differ").toBeGreaterThan(0.03);
  expect(meanAbsDiff(a, b)).toBeGreaterThan(2);

  // Tolerant reference comparison.
  fs.mkdirSync(REFS, { recursive: true });
  for (const [name, img] of [
    ["view-a.png", a],
    ["view-b.png", b],
  ] as const) {
    const file = path.join(REFS, name);
    if (process.env.UPDATE_REFS === "1" || !fs.existsSync(file)) {
      fs.writeFileSync(file, await encodePng(page, img));
      if (process.env.UPDATE_REFS !== "1") console.warn(`wrote missing reference ${file}`);
      continue;
    }
    const ref = await decodePng(page, fs.readFileSync(file));
    const mm = mismatchFraction(img, ref, 32);
    if (mm > 0.03) {
      const out = test.info().outputPath(`actual-${name}`);
      fs.writeFileSync(out, await encodePng(page, img));
    }
    expect(mm, `${name} mismatched fraction vs reference`).toBeLessThanOrEqual(0.03);
  }
});
