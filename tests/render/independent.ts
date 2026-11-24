// Independent check: load the PLY with a different 3DGS implementation
// (@mkkellogg/gaussian-splats-3d on three.js). Never imported by src/.
// @ts-expect-error -- package ships without type declarations
import * as GS from "@mkkellogg/gaussian-splats-3d";

type W = typeof window & {
  indep: { ready: boolean; error: string | null; capture(): { width: number; height: number; b64: string } };
};
const w = window as W;
w.indep = { ready: false, error: null, capture: () => ({ width: 0, height: 0, b64: "" }) };

const ply = new URLSearchParams(location.search).get("ply") ?? "/fixtures/renderer-asym.ply";
const root = document.getElementById("root")!;

const viewer = new GS.Viewer({
  rootElement: root,
  selfDrivenMode: false,
  useBuiltInControls: false,
  cameraUp: [0, 1, 0],
  initialCameraPosition: [0, 0, 3],
  initialCameraLookAt: [0, 0, 0],
  sharedMemoryForWorkers: false,
  gpuAcceleratedSort: false,
  integerBasedSort: false,
  dynamicScene: false,
  sphericalHarmonicsDegree: 0,
  logLevel: 0,
});

function toB64(bytes: Uint8ClampedArray): string {
  let s = "";
  for (let i = 0; i < bytes.length; i += 0x8000) {
    s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  }
  return btoa(s);
}

w.indep.capture = () => {
  viewer.update();
  viewer.render();
  // Same task as render(): the drawing buffer is still valid.
  const src = viewer.renderer.domElement as HTMLCanvasElement;
  const c = document.createElement("canvas");
  c.width = src.width;
  c.height = src.height;
  const ctx = c.getContext("2d")!;
  ctx.drawImage(src, 0, 0);
  const d = ctx.getImageData(0, 0, c.width, c.height).data;
  return { width: c.width, height: c.height, b64: toB64(d) };
};

viewer
  .addSplatScene(ply, {
    format: GS.SceneFormat.Ply,
    showLoadingUI: false,
    progressiveLoad: false,
    splatAlphaRemovalThreshold: 1,
  })
  .then(() => {
    const loop = () => {
      viewer.update();
      viewer.render();
      requestAnimationFrame(loop);
    };
    loop();
    setTimeout(() => (w.indep.ready = true), 1500);
  })
  .catch((e: unknown) => {
    w.indep.error = String(e instanceof Error ? e.message : e);
  });
