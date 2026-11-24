# Developing 3D Maker

Technical guide for building, testing and contributing. If you only want to use the app, see the
[README](../README.md).

Describe one object, let **your own** AI (Anthropic, OpenAI, any OpenAI-compatible server, or
Claude Code / Codex / Ollama already on this computer) design it, turn it around in a live 3D viewer,
and export a standard **3D Gaussian splat (.ply)**. You can also open your own `.ply`, `.obj` or `.stl`
files and change them with a prompt.

* Desktop app for macOS, Windows and Linux: stable Rust + Tauri 2, small React/TypeScript UI.
* Rust owns networking, credentials (OS credential store), local-provider detection, jobs,
  validation, persistence and export.
* The model writes a **shape design** (a JSON list of coloured boxes, ellipsoids, cylinders, cones and
  tori); 3D Maker samples those surfaces into Gaussian splats **on this computer**. No image or 3D
  generation service is involved, so any capable text model works.
* Rendering uses the locally bundled PlayCanvas Engine. Fonts and renderer are bundled; saved
  objects open fully offline, and with Ollama generation is offline too.

## Build from source

Prerequisites: Rust stable (≥ 1.85), Node.js ≥ 20, and on Linux
`libwebkit2gtk-4.1-dev librsvg2-dev patchelf libssl-dev libdbus-1-dev`.

```sh
npm ci --include=dev        # --include=dev matters if NODE_ENV=production in your shell
npm run fixtures            # synthetic test fixtures (fixtures/*.ply)
npm run tauri dev           # run the app in development
npm run bundle:mac          # → target/release/bundle/dmg/*.dmg   (macOS)
npm run bundle:windows      # → target/release/bundle/nsis/*.exe  (Windows)
npm run bundle:linux        # → target/release/bundle/{appimage,deb}/
```

Universal macOS build (Apple silicon + Intel), ad-hoc signed:

```sh
rustup target add aarch64-apple-darwin x86_64-apple-darwin
APPLE_SIGNING_IDENTITY="-" npx tauri build --target universal-apple-darwin --bundles app,dmg
```

The DMG step scripts Finder to lay out the installer window. On a headless or locked Mac it times
out (`AppleEvent timed out`); build with `CI=true npm run bundle:mac` to skip that cosmetic step.

## Releasing

Push a version tag and GitHub Actions builds the installers and attaches them to a **draft** release:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

Check the draft on the repository's *Releases* page, then click **Publish release**. Bump `version`
in `src-tauri/tauri.conf.json`, `Cargo.toml` and `package.json` before tagging a new version.

Installers are unsigned unless signing secrets are configured (users then see the operating-system
warnings described in the README). Optional signing inputs:
* **macOS**: `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`, and for
  notarization `APPLE_ID`, `APPLE_PASSWORD` (app-specific), `APPLE_TEAM_ID`.
* **Windows**: an Authenticode certificate configured via `bundle.windows.certificateThumbprint`
  (or a signing command) in `src-tauri/tauri.conf.json`.
* **Linux**: AppImage/deb need no signing; optionally sign the AppImage with GPG.

## How it works

1. **Choose your AI** — the first screen lists what was found **on this computer** (Claude Code,
   Codex and Ollama, detected automatically; **Look again** re-checks) and the **cloud** providers.
   Local providers connect with one click and no key. For Anthropic or OpenAI the key is checked with
   a free model-list call, saved in the OS credential store (Keychain / Credential Manager / Secret
   Service), and the field is cleared. If the store is unavailable the key can be used for this session
   only. A **custom provider** is any OpenAI-compatible server (OpenRouter, LM Studio, vLLM, …).
2. **Create** — type a description, pick the **Model** if you like, press **Generate** or ⌘/Ctrl+Enter.
3. **Watch** — “… is designing the object” → “Building the 3D splat” → “Saving object”. **Cancel**
   stays available; for cloud providers it says the provider may still bill for what it already wrote.
4. **Inspect** — drag to orbit, right/middle-drag or Shift-drag to pan, scroll/pinch to zoom;
   with the viewport focused: arrows orbit, Shift+arrows pan, +/− zoom, R resets.
5. **Export splat (.ply)** writes a byte-for-byte copy of the validated file. **Open 3D file…**
   imports Gaussian `.ply` files and `.obj` / `.stl` meshes (sampled into splats; up to 512 MB and
   5 million triangles).
6. **Adjust with a prompt** — always creates a new object. Objects made by 3D Maker have their shape
   design edited by the model; imported files get a list of operations (colour, brightness,
   saturation, opacity, scale, rotate, translate, mirror, delete, keep — each optionally limited to a
   region by bounding-box fraction and/or colour) applied locally.

Every successful generation is auto-saved in **Recent objects** (thumbnail + prompt) and reopens offline.

### Providers

| Choice | How it's reached | Key | Connection check (free) | Cost per object |
|---|---|---|---|---|
| Claude Code (local) | `claude -p --output-format json --tools "" --no-session-persistence --strict-mcp-config`, run in an empty temp folder; found on `PATH` or in the usual install folders | none (your Claude login) | `claude --version` | counts against your Claude plan |
| Codex (local) | `codex exec --ephemeral --skip-git-repo-check --sandbox read-only --ignore-user-config -` with the prompt on stdin, run in an empty temp folder; found on `PATH`, in the usual install folders, or inside `Codex.app` | none (your Codex login) | `codex --version` and `codex login status` | counts against your ChatGPT plan (or OpenAI account) |
| Ollama (local) | `POST http://127.0.0.1:11434/v1/chat/completions` | none | `GET /api/tags` (lists installed models) | free |
| Anthropic | `POST /v1/messages` | `x-api-key` | `GET /v1/models` | one billed request |
| OpenAI | `POST /v1/chat/completions` with `response_format: json_object` | Bearer | `GET /v1/models` (chat models listed first) | one billed request |
| Custom provider | `POST {url}/chat/completions` on any OpenAI-compatible server | optional Bearer | `GET {url}/models` | set by the server's operator |

The model list from the connection check fills the **Model** picker; the default is a mid-sized
model where the provider identifies one (newest Sonnet on Anthropic, newest GPT chat model on
OpenAI). No price estimate is shown because no current machine-readable price data is used.

The instructions ask for one JSON object with 6–60 parts (`shape`, `center`, `size`, `rotation`,
`color`, `label`), Y up, roughly 2 × 2 × 2 units. Your description is sent verbatim. Parsing tolerates
code fences, prose around the JSON, shape aliases (sphere → ellipsoid, ring → torus, …) and colour
notations, and rejects non-finite numbers, out-of-range sizes and more than 160 parts with a plain
reason. Each part's surface is sampled in proportion to its area (≈ 90,000 splats per object) as
flat, normal-aligned Gaussians with gentle baked lighting so flat colours still read as 3D.
Rebuilding a design is deterministic.

## Export format

Uncompressed `binary_little_endian` PLY with one `vertex` element in the standard 3DGS layout,
read by property **name and type**: `x y z`, `scale_0..2` (natural-log), `rot_0..3` (quaternion,
`rot_0` = w), `opacity` (logit), `f_dc_0..2`, optional `f_rest_*` (9/24/45 → SH degree 1–3), plus any
extra attributes (kept). Validation streams the file in bounded chunks with checked sizes and rejects
ASCII/big-endian/compressed files, plain XYZ/RGB clouds, preview videos, truncated or trailing data,
empty or oversized counts, non-finite values and zero quaternions — each with a plain-language
message. The export is the saved file, byte for byte (SHA-256 verified). Splats built by 3D Maker are
y-up; imported third-party files are assumed to use the common 3DGS y-down convention. Orientation
is corrected **in the viewer only**.

## How jobs stay safe

* Each design request is written to SQLite *before* it is sent. Nothing billable is ever repeated
  automatically: only a refused request (connection refused, or 429 with `Retry-After`) is retried; a
  timeout or server error after sending is reported, and asking again needs an explicit click (with a
  billing confirmation for paid providers).
* Double clicks and concurrent submissions are refused (one job at a time, idempotent click ids).
* The model's answer is saved as `design.json` before the splat is built, so an interrupted or failed
  build is redone locally with **Rebuild (free)**. On restart, interrupted objects are marked with the
  right choice (rebuild, or ask again); nothing is sent on its own.
* The splat is written to a temp file, validated off the UI thread, atomically renamed and only then
  marked ready. A full disk or partial write never marks an object ready.
* Cancel abandons the request (the HTTP connection is closed; a CLI process is ended).
* Network errors include the underlying cause (`Details: …`: DNS, refused connection, TLS, …).
* Keys never leave Rust, never reach logs, metadata, exports or crash output (redaction + masked status),
  are sent only to the provider's own origin (custom keys are stored per server origin), and redirects
  are never followed. Plain HTTP is allowed only to Ollama and to a loopback custom server you
  explicitly allow. Claude Code runs with every tool disabled; Codex runs in a read-only sandbox. The
  webview has no filesystem, shell, HTTP or dialog permissions — only the app's own commands (see
  `src-tauri/capabilities/default.json`) and an asset-protocol scope limited to the object library.

## Tests

Default suites need **no keys, no paid calls and no remote GPU**:

| Command | What it proves |
|---|---|
| `cargo test --workspace` | PLY schemas, truncation, NaN/Inf, mesh import (OBJ/STL), prompt adjustments, byte-identical export, migrations, redaction; design parsing and surface sampling; **provider contract tests** driving the real adapters + engine against a local fake of the Anthropic, OpenAI and Ollama APIs (bad keys, no credit, 429 + Retry-After, server error after sending never retried, cancel, double click, crash/restart with free rebuild, keys never on disk), with request counters proving no paid request repeats. The fake binary also acts as the Claude Code and Codex CLIs, so both CLI adapters (tools disabled / read-only, sign-in errors, cancel kills the process) and detection are tested without a login |
| `npm run typecheck && npm test` | UI flows: first-run connect, local-AI detection and one-click Claude Code / Codex, Look again, custom server, session-only fallback, wrong-key recovery, model picker, single submission on double click, cancel, paid vs local ask-again, free rebuild, keyboard-only navigation, Settings modal focus |
| `npm run test:render` | Renderer correctness against reference images from two fixed views, plus an independent viewer (GaussianSplats3D/three.js) opening the PLY |
| `npm run e2e:native` | The real desktop app (debug build with the test-only `e2e` feature) against the local fake provider from an empty profile: connect, generate, orbit, export, ask again, quit, relaunch offline, reopen. Linux: `xvfb-run -a npm run e2e:native` |

Run the native E2E on a visible, unlocked display: when macOS locks the screen the webview pauses,
and the offline-relaunch phase stalls on the watchdog. Results go to `tests/output/`.

### Live (opt-in) smoke test

```sh
npm run live:smoke -- --provider claude-code                     # your Claude plan
npm run live:smoke -- --provider codex                           # your ChatGPT plan
npm run live:smoke -- --provider ollama --model llama3.2         # free, local
ANTHROPIC_API_KEY=… npm run live:smoke -- --provider anthropic --yes-spend
OPENAI_API_KEY=…    npm run live:smoke -- --provider openai --yes-spend
```

At most two generations (teapot + “a yellow toy forklift with black wheels”; `--only-teapot` for one).
Outputs and a JSON record go to `tests/output/live-*`.

## Layout

```
crates/maker-core/     providers, local detection, shape designs → splats, mesh import, prompt
                       adjustments, job engine, PLY validation, SQLite store, credentials (no UI deps)
crates/fake-provider/  local Anthropic / OpenAI / Ollama / Claude Code / Codex stand-in (tests only)
src-tauri/             Tauri shell: commands, capabilities, bundling; `e2e` feature = test hooks only
src/                   React UI and PlayCanvas viewer
tests/render/          Playwright renderer + independent-viewer tests
tests/e2e-native/      native end-to-end orchestrator
```
