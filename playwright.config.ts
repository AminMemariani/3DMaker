import { defineConfig } from "@playwright/test";

// Renderer correctness tests. Chromium runs headless with SwiftShader (via ANGLE)
// so WebGL2 works identically on developer Macs and Linux CI without a GPU.
export default defineConfig({
  testDir: "tests/render",
  testMatch: /.*\.spec\.ts$/,
  timeout: 120_000,
  workers: 1,
  fullyParallel: false,
  reporter: [["list"]],
  use: {
    baseURL: "http://127.0.0.1:4179",
    viewport: { width: 640, height: 640 },
    deviceScaleFactor: 1,
    browserName: "chromium",
    launchOptions: {
      args: [
        "--use-angle=swiftshader",
        "--use-gl=angle",
        "--enable-unsafe-swiftshader",
        "--ignore-gpu-blocklist",
        "--enable-webgl",
      ],
    },
  },
  webServer: {
    command: "npx vite --config tests/render/vite.render.config.ts",
    url: "http://127.0.0.1:4179/tests/render/harness.html",
    reuseExistingServer: !process.env.CI,
    timeout: 120_000,
  },
});
