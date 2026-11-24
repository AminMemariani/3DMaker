// Vite dev server for the renderer harness pages (used by Playwright's webServer).
// Root is the repo root so the harness can import src/viewer and fetch fixtures/.
import { defineConfig } from "vite";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../..", import.meta.url));

export default defineConfig({
  root,
  clearScreen: false,
  define: { __E2E__: "false" },
  server: {
    port: 4179,
    strictPort: true,
    host: "127.0.0.1",
    // INDEPENDENT_PLY may point anywhere on disk (served via /@fs/).
    fs: { strict: false, allow: [root] },
  },
  optimizeDeps: {
    entries: ["tests/render/harness.html", "tests/render/independent.html"],
  },
});
