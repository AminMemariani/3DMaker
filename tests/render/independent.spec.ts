// Independent viewer check: a different 3DGS implementation (three.js-based
// @mkkellogg/gaussian-splats-3d) must render the PLY non-blank with the green
// FRONT marker visible from the front. INDEPENDENT_PLY=<abs path> checks any
// file (e.g. an exported splat).
import { test, expect } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { classify, countIn, fromB64, project, FIX } from "./util";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const envPly = process.env.INDEPENDENT_PLY;
const plyFile = envPly ? path.resolve(envPly) : path.join(__dirname, "../../fixtures/renderer-asym.ply");
const plyUrl = envPly ? `/@fs${plyFile.startsWith("/") ? "" : "/"}${plyFile.replace(/\\/g, "/")}` : "/fixtures/renderer-asym.ply";
const isFixture = !envPly;

test("independent 3DGS viewer renders the PLY", async ({ page }) => {
  test.skip(!fs.existsSync(plyFile), `${plyFile} missing — run \`npm run fixtures\``);
  const logs: string[] = [];
  page.on("console", (m) => logs.push(`${m.type()}: ${m.text()}`));
  page.on("pageerror", (e) => logs.push(`pageerror: ${e.message}`));

  await page.goto(`/tests/render/independent.html?ply=${encodeURIComponent(plyUrl)}`);
  await page.waitForFunction(() => (window as any).indep?.ready || (window as any).indep?.error, null, {
    timeout: 90_000,
  });
  const err = await page.evaluate(() => (window as any).indep.error);
  expect(err, logs.join("\n")).toBeNull();

  const r = await page.evaluate(() => (window as any).indep.capture());
  const img = fromB64(r.width, r.height, r.b64);
  expect(img.width).toBeGreaterThan(100);

  // Non-blank: count non-black pixels (three.js clears to black).
  let lit = 0;
  for (let i = 0; i < img.data.length; i += 4) if (img.data[i] + img.data[i + 1] + img.data[i + 2] > 40) lit++;
  const frac = lit / (img.data.length / 4);
  expect(frac, "non-blank fraction").toBeGreaterThan(0.01);

  if (isFixture) {
    // Green FRONT marker near top-centre; camera (0,0,3). The library's FOV may
    // differ from 45°, so search a generous column around the projected spot.
    const p = project([0, 0, 3], FIX.green, img.width, img.height, 50);
    const green = countIn(img, p[0], p[1], 40, "green");
    const magenta = countIn(img, p[0], p[1], 12, "magenta");
    expect(green, "green front marker visible").toBeGreaterThan(100);
    expect(magenta, "magenta hidden from the front").toBeLessThan(green / 4);
    // Sanity: some yellow (the bar) present too.
    let yellow = 0;
    for (let i = 0; i < img.data.length; i += 4) if (classify(img.data[i], img.data[i + 1], img.data[i + 2]) === "yellow") yellow++;
    expect(yellow).toBeGreaterThan(100);
  }
});
