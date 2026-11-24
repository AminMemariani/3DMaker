// Renderer harness: the SAME viewer module the app uses, on a fixed 512×512 canvas.
import { createViewer } from "../../src/viewer/viewer";

const canvas = document.getElementById("c") as HTMLCanvasElement;
try {
  createViewer(canvas);
} catch (e) {
  console.error("viewer init failed", e);
}
(window as unknown as { harnessReady: boolean }).harnessReady = true;
