import type { Page } from "@playwright/test";

export type V3 = [number, number, number];
export interface Img {
  width: number;
  height: number;
  data: Uint8Array;
}

export const FIX = {
  bar: [0, 0, 0] as V3,
  green: [0, 0.55, 0.45] as V3,
  magenta: [0, 0.55, -0.45] as V3,
  red: [0.5, -0.45, 0.3] as V3,
  blue: [0.5, -0.45, -0.3] as V3,
  white: [-0.6, -0.45, 0] as V3,
};

const sub = (a: V3, b: V3): V3 => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const dot = (a: V3, b: V3) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
const cross = (a: V3, b: V3): V3 => [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
const norm = (a: V3): V3 => {
  const l = Math.hypot(...a);
  return [a[0] / l, a[1] / l, a[2] / l];
};

/** Pinhole projection for a camera at `cam` looking at the origin, up = +Y (independent of PlayCanvas). */
export function project(cam: V3, p: V3, w: number, h: number, fovDeg = 45): [number, number] {
  const f = norm(sub([0, 0, 0], cam));
  const r = norm(cross(f, [0, 1, 0]));
  const u = cross(r, f);
  const d = sub(p, cam);
  const x = dot(d, r);
  const y = dot(d, u);
  const z = dot(d, f);
  const fp = h / 2 / Math.tan((fovDeg * Math.PI) / 360);
  return [w / 2 + (fp * x) / z, h / 2 - (fp * y) / z];
}

export type ColorName = "green" | "magenta" | "red" | "blue" | "yellow" | "white";

export function classify(r: number, g: number, b: number): ColorName | null {
  if (r > 140 && g > 120 && b < 110 && Math.abs(r - g) < 90 && r - b > 80) return "yellow";
  if (g > 110 && g > r + 50 && g > b + 50) return "green";
  if (r > 110 && b > 110 && g < Math.min(r, b) - 50) return "magenta";
  if (r > 110 && r > g + 60 && r > b + 60) return "red";
  if (b > 110 && b > r + 50 && b > g + 40) return "blue";
  if (r > 170 && g > 170 && b > 170) return "white";
  return null;
}

export function countIn(img: Img, cx: number, cy: number, half: number, color: ColorName): number {
  let n = 0;
  for (let y = Math.round(cy - half); y <= Math.round(cy + half); y++) {
    for (let x = Math.round(cx - half); x <= Math.round(cx + half); x++) {
      if (x < 0 || y < 0 || x >= img.width || y >= img.height) continue;
      const i = (y * img.width + x) * 4;
      if (classify(img.data[i], img.data[i + 1], img.data[i + 2]) === color) n++;
    }
  }
  return n;
}

/** Yellow pixel counts on the "/" diagonal (upper-right + lower-left) vs "\" diagonal, within `radius` of centre. */
export function barDiagonals(img: Img, cx: number, cy: number, radius: number): { slash: number; backslash: number } {
  let slash = 0;
  let backslash = 0;
  for (let y = Math.round(cy - radius); y <= Math.round(cy + radius); y++) {
    for (let x = Math.round(cx - radius); x <= Math.round(cx + radius); x++) {
      const dx = x - cx;
      const dy = y - cy;
      if (Math.hypot(dx, dy) > radius || Math.abs(dx) < 6 || Math.abs(dy) < 6) continue;
      const i = (y * img.width + x) * 4;
      if (classify(img.data[i], img.data[i + 1], img.data[i + 2]) !== "yellow") continue;
      // Screen y grows downward: upper-right is dx>0, dy<0.
      if (dx * dy < 0) slash++;
      else backslash++;
    }
  }
  return { slash, backslash };
}

export function meanAbsDiff(a: Img, b: Img): number {
  let s = 0;
  for (let i = 0; i < a.data.length; i += 4) {
    s += Math.abs(a.data[i] - b.data[i]) + Math.abs(a.data[i + 1] - b.data[i + 1]) + Math.abs(a.data[i + 2] - b.data[i + 2]);
  }
  return s / ((a.data.length / 4) * 3);
}

export function mismatchFraction(a: Img, b: Img, tol: number): number {
  if (a.width !== b.width || a.height !== b.height) return 1;
  let bad = 0;
  for (let i = 0; i < a.data.length; i += 4) {
    if (
      Math.abs(a.data[i] - b.data[i]) > tol ||
      Math.abs(a.data[i + 1] - b.data[i + 1]) > tol ||
      Math.abs(a.data[i + 2] - b.data[i + 2]) > tol
    )
      bad++;
  }
  return bad / (a.data.length / 4);
}

export function fromB64(width: number, height: number, b64: string): Img {
  return { width, height, data: new Uint8Array(Buffer.from(b64, "base64")) };
}

/** Encode RGBA → PNG (in the browser; avoids a node PNG dependency). */
export async function encodePng(page: Page, img: Img): Promise<Buffer> {
  const b64 = await page.evaluate(
    ({ w, h, d }) => {
      const bytes = Uint8ClampedArray.from(atob(d), (ch) => ch.charCodeAt(0));
      const c = document.createElement("canvas");
      c.width = w;
      c.height = h;
      c.getContext("2d")!.putImageData(new ImageData(bytes, w, h), 0, 0);
      return c.toDataURL("image/png").split(",")[1];
    },
    { w: img.width, h: img.height, d: Buffer.from(img.data).toString("base64") },
  );
  return Buffer.from(b64, "base64");
}

export async function decodePng(page: Page, png: Buffer): Promise<Img> {
  const r = await page.evaluate(async (b64) => {
    const im = new Image();
    im.src = `data:image/png;base64,${b64}`;
    await im.decode();
    const c = document.createElement("canvas");
    c.width = im.naturalWidth;
    c.height = im.naturalHeight;
    const ctx = c.getContext("2d", { colorSpace: "srgb" })!;
    ctx.drawImage(im, 0, 0);
    const d = ctx.getImageData(0, 0, c.width, c.height).data;
    let s = "";
    for (let i = 0; i < d.length; i += 0x8000) s += String.fromCharCode(...d.subarray(i, i + 0x8000));
    return { w: c.width, h: c.height, b64: btoa(s) };
  }, png.toString("base64"));
  return fromB64(r.w, r.h, r.b64);
}
