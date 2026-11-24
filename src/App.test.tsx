import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import type { AppState, JobView, LocalDetection, ProjectDetail, ProviderInfo, ProviderStatus } from "./lib/ipc";
import App from "./App";
import { EXAMPLE_PROMPT } from "./components/CreatePanel";

const h = vi.hoisted(() => ({
  handlers: {} as Record<string, (e: { payload: unknown }) => void>,
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => ({
  invoke: h.invoke,
  convertFileSrc: (p: string) => `asset://localhost/${p}`,
}));
vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (name: string, cb: (e: { payload: unknown }) => void) => {
    h.handlers[name] = cb;
    return () => {};
  }),
}));
// No WebGL in jsdom: replace the viewport with a focusable stand-in.
vi.mock("./viewer/Viewport", async () => {
  const React = await import("react");
  return {
    Viewport: React.forwardRef(function Viewport(
      props: { url: string | null; label: string; onFirstFrame?: (i: unknown) => void },
      ref: React.Ref<unknown>,
    ) {
      React.useImperativeHandle(ref, () => ({ resetView: () => {}, captureThumbnail: async () => new Uint8Array([1]) }));
      React.useEffect(() => {
        if (props.url) props.onFirstFrame?.({ firstFrameMs: 1, splatCount: 6 });
        // eslint-disable-next-line react-hooks/exhaustive-deps
      }, [props.url]);
      return React.createElement("canvas", { role: "img", tabIndex: 0, "aria-label": `3D preview of ${props.label}` });
    }),
  };
});

const base = {
  verifyNote: "Connect checks the key with a free model-list request.",
  flowSummary: "The model designs your object as a set of coloured shapes.",
};
const anthropic: ProviderInfo = {
  ...base,
  id: "anthropic",
  name: "Anthropic",
  kind: "cloud",
  keyUrl: "https://console.anthropic.com/settings/keys",
  keyLabel: "API key",
  keyRequired: true,
  billingNote: "Anthropic bills your account for one model request per object.",
};
const openai: ProviderInfo = { ...anthropic, id: "openai", name: "OpenAI", keyUrl: "https://platform.openai.com/api-keys", billingNote: "OpenAI bills your account for one model request per object." };
const custom: ProviderInfo = { ...anthropic, id: "custom", name: "Custom provider", kind: "custom", keyUrl: null, keyLabel: "API key (optional)", keyRequired: false, billingNote: "Any charges are set by whoever runs the server." };
const claude: ProviderInfo = { ...base, id: "claude-code", name: "Claude Code", kind: "local", keyUrl: null, keyLabel: null, keyRequired: false, billingNote: "Requests count against your Claude plan's usage limits." };
const codex: ProviderInfo = { ...claude, id: "codex", name: "Codex", billingNote: "Requests count against your ChatGPT plan's Codex limits, or your OpenAI account if Codex signs in with a key." };
const ollama: ProviderInfo = { ...claude, id: "ollama", name: "Ollama", billingNote: "Free: everything runs on your own hardware." };

const missing = (id: ProviderStatus["id"]): ProviderStatus => ({
  id,
  connected: false,
  credential: "missing",
  maskedKey: null,
  accountLabel: null,
  endpoint: null,
  model: null,
  models: [],
  verifiedAt: null,
});
const stored: ProviderStatus = {
  ...missing("anthropic"),
  connected: true,
  credential: "stored",
  maskedKey: "sk-a…a1F4",
  model: "claude-test-sonnet",
  models: ["claude-test-sonnet", "claude-test-opus"],
  verifiedAt: 1,
};
const claudeConnected: ProviderStatus = {
  ...missing("claude-code"),
  connected: true,
  credential: "notRequired",
  accountLabel: "Claude Code 2.1.0",
  model: "default",
  models: ["default", "sonnet", "opus"],
  verifiedAt: 1,
};
const found: LocalDetection[] = [
  { id: "claude-code", available: true, installed: true, detail: "2.1.0 at /usr/local/bin/claude", models: ["default"], hint: null },
  { id: "codex", available: true, installed: true, detail: "0.98.0 at /opt/homebrew/bin/codex", models: ["default"], hint: null },
  { id: "ollama", available: false, installed: true, detail: null, models: [], hint: "Ollama is installed but not running." },
];
const codexConnected: ProviderStatus = { ...claudeConnected, id: "codex", accountLabel: "Codex 0.98.0", models: ["default"] };
const allMissing = () => ["anthropic", "openai", "custom", "claude-code", "codex", "ollama"].map((id) => missing(id as ProviderStatus["id"]));

function appState(over: Partial<AppState> = {}): AppState {
  return {
    providers: [anthropic, openai, custom, claude, codex, ollama],
    statuses: allMissing(),
    local: found,
    selectedProvider: null,
    credentialStoreAvailable: true,
    activeJob: null,
    recent: [],
    dataDir: "/data",
    ...over,
  };
}
const returning = () =>
  appState({ statuses: [stored, ...allMissing().filter((s) => s.id !== "anthropic")], selectedProvider: "anthropic" });

function detail(over: Partial<ProjectDetail> = {}): ProjectDetail {
  return {
    id: "p1",
    prompt: "teapot",
    provider: "anthropic",
    state: "ready",
    createdAt: 1,
    thumbPath: "/data/p1/thumb.png",
    splatCount: 6,
    splatPath: "/data/p1/splat.ply",
    splatBytes: 100,
    splatSha256: "abc",
    shDegree: 0,
    bounds: null,
    viewRotation: [0, 0, 0],
    model: "claude-test-sonnet",
    partCount: 6,
    error: null,
    retry: { rebuild: false, full: true },
    ...over,
  };
}

function job(over: Partial<JobView> = {}): JobView {
  return { projectId: "p1", state: "designing", label: "Anthropic is designing the object", error: null, cancel: null, ...over };
}

type Handler = (args: Record<string, unknown>) => unknown;
function backend(state: AppState, handlers: Record<string, Handler> = {}) {
  h.invoke.mockImplementation((cmd: string, args: Record<string, unknown>) => {
    if (handlers[cmd]) return Promise.resolve().then(() => handlers[cmd](args));
    if (cmd === "get_app_state") return Promise.resolve(state);
    if (cmd === "get_project") return Promise.resolve(detail());
    if (cmd === "detect_providers") return Promise.resolve(state.local);
    return Promise.resolve(null);
  });
}
const connectButton = () =>
  within(screen.getByRole("region", { name: "Connect Anthropic" })).getByRole("button", { name: "Connect" });
const keyInput = () =>
  within(screen.getByRole("region", { name: "Connect Anthropic" })).getByLabelText("API key") as HTMLInputElement;
const calls = (cmd: string) => h.invoke.mock.calls.filter((c) => c[0] === cmd);
const emitJob = (j: JobView) => act(() => h.handlers["job-update"]({ payload: j }));

function deferred<T>() {
  let resolve!: (v: T) => void;
  let reject!: (e: unknown) => void;
  const promise = new Promise<T>((a, b) => {
    resolve = a;
    reject = b;
  });
  return { promise, resolve, reject };
}

beforeEach(() => {
  h.invoke.mockReset();
  h.handlers = {};
});

describe("connect", () => {
  it("first run: connects a cloud provider, clears the key immediately, shows masked status and model", async () => {
    const d = deferred<unknown>();
    backend(appState(), { connect_provider: () => d.promise });
    render(<App />);
    expect(await screen.findByRole("heading", { name: "Choose your AI" })).toBeInTheDocument();
    await userEvent.click(screen.getByRole("radio", { name: "Anthropic" }));
    const input = keyInput();
    expect(input).toHaveAttribute("type", "password");
    await userEvent.type(input, "sk-ant-secret");
    await userEvent.click(connectButton());
    expect(input.value).toBe(""); // cleared before the result arrives
    expect(calls("connect_provider")[0][1]).toEqual({
      req: { provider: "anthropic", key: "sk-ant-secret", sessionOnly: false },
    });
    await act(async () => d.resolve({ status: stored, storeUnavailable: false }));
    expect(await screen.findByLabelText("Describe one object")).toBeInTheDocument();
    expect(screen.getByText(/sk-a…a1F4/)).toBeInTheDocument();
    expect(screen.getByText(anthropic.billingNote)).toBeInTheDocument();
    expect(screen.getByLabelText("Model")).toHaveValue("claude-test-sonnet");
  });

  it("shows detected local AI and connects Claude Code without a key", async () => {
    backend(appState(), { connect_provider: () => ({ status: claudeConnected, storeUnavailable: false }) });
    render(<App />);
    const card = await screen.findByRole("region", { name: "Connect Claude Code" });
    expect(within(card).getByText("Found")).toBeInTheDocument();
    expect(within(card).getByText(/2\.1\.0 at \/usr\/local\/bin\/claude/)).toBeInTheDocument();
    expect(within(card).queryByLabelText(/key/i)).toBeNull();
    // Ollama is installed but not running: its button is disabled with a hint.
    const o = screen.getByRole("region", { name: "Connect Ollama" });
    expect(within(o).getByRole("button", { name: "Use Ollama" })).toBeDisabled();
    expect(within(o).getByText(/not running/)).toBeInTheDocument();

    await userEvent.click(within(card).getByRole("button", { name: "Use Claude Code" }));
    expect(calls("connect_provider")[0][1]).toEqual({ req: { provider: "claude-code", key: "", sessionOnly: false } });
    expect(await screen.findByLabelText("Describe one object")).toBeInTheDocument();
    expect(screen.getByText(/Using Claude Code/)).toBeInTheDocument();
  });

  it("shows detected Codex and connects it without a key", async () => {
    backend(appState(), { connect_provider: () => ({ status: codexConnected, storeUnavailable: false }) });
    render(<App />);
    const card = await screen.findByRole("region", { name: "Connect Codex" });
    expect(within(card).getByText("Found")).toBeInTheDocument();
    expect(within(card).getByText(/0\.98\.0 at \/opt\/homebrew\/bin\/codex/)).toBeInTheDocument();
    expect(within(card).queryByLabelText(/key/i)).toBeNull();
    await userEvent.click(within(card).getByRole("button", { name: "Use Codex" }));
    expect(calls("connect_provider")[0][1]).toEqual({ req: { provider: "codex", key: "", sessionOnly: false } });
    expect(await screen.findByLabelText("Describe one object")).toBeInTheDocument();
    expect(screen.getByText(/Using Codex/)).toBeInTheDocument();
  });

  it("Look again re-runs detection", async () => {
    const later: LocalDetection[] = found.map((d) =>
      d.id === "ollama" ? { ...d, available: true, models: ["llama3.2:latest"], hint: null, detail: "running, 1 model(s)" } : d,
    );
    backend(appState(), { detect_providers: () => later });
    render(<App />);
    const o = await screen.findByRole("region", { name: "Connect Ollama" });
    expect(within(o).getByRole("button", { name: "Use Ollama" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Look again" }));
    await waitFor(() => expect(within(o).getByRole("button", { name: "Use Ollama" })).toBeEnabled());
  });

  it("offers session-only use when the OS credential store is unavailable", async () => {
    let n = 0;
    backend(appState(), {
      connect_provider: () =>
        ++n === 1
          ? { status: missing("anthropic"), storeUnavailable: true }
          : { status: { ...stored, credential: "session" }, storeUnavailable: false },
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Choose your AI" });
    const input = keyInput();
    await userEvent.type(input, "sk-ant-secret");
    await userEvent.click(connectButton());
    const offer = await screen.findByRole("button", { name: "Use for this session only" });
    expect(screen.getByText(/paste the key again/i)).toBeInTheDocument();
    expect(input.value).toBe("");
    await userEvent.type(input, "sk-ant-secret");
    await userEvent.click(offer);
    expect(calls("connect_provider")[1][1]).toEqual({
      req: { provider: "anthropic", key: "sk-ant-secret", sessionOnly: true },
    });
    expect(await screen.findByLabelText("Describe one object")).toBeInTheDocument();
  });

  it("shows a wrong-key error, keeps the form usable, then connects with a replacement", async () => {
    let n = 0;
    backend(appState(), {
      connect_provider: () => {
        if (++n === 1)
          throw { kind: "invalidKey", title: "Anthropic rejected your key", message: "The key may be mistyped.", action: "replaceKey", actionUrl: null };
        return { status: stored, storeUnavailable: false };
      },
    });
    render(<App />);
    await screen.findByRole("heading", { name: "Choose your AI" });
    await userEvent.type(keyInput(), "bad");
    await userEvent.click(connectButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("Anthropic rejected your key");
    expect(keyInput()).toBeEnabled();
    await userEvent.type(keyInput(), "sk-ant-good");
    await userEvent.click(connectButton());
    expect(await screen.findByLabelText("Describe one object")).toBeInTheDocument();
  });

  it("custom provider sends the server URL", async () => {
    backend(appState(), { connect_provider: () => ({ status: { ...missing("custom"), connected: true, endpoint: "https://openrouter.ai/api/v1" }, storeUnavailable: false }) });
    render(<App />);
    await userEvent.click(await screen.findByText(/Custom provider/));
    await userEvent.type(screen.getByLabelText("Server URL"), "https://openrouter.ai/api/v1");
    await userEvent.type(screen.getByLabelText("API key (optional)"), "sk-or-x");
    await userEvent.click(screen.getAllByRole("button", { name: "Connect" }).at(-1)!);
    expect(calls("connect_provider")[0][1]).toEqual({
      req: { provider: "custom", key: "sk-or-x", sessionOnly: false, endpoint: "https://openrouter.ai/api/v1", allowLoopback: false },
    });
  });

  it("returning user lands on Create with the prompt focused", async () => {
    backend(returning());
    render(<App />);
    const prompt = await screen.findByLabelText("Describe one object");
    expect(screen.queryByRole("heading", { name: "Choose your AI" })).toBeNull();
    expect(prompt).toHaveFocus();
  });
});

describe("create", () => {
  it("example chip fills the exact teapot prompt", async () => {
    backend(returning());
    render(<App />);
    await userEvent.click(await screen.findByRole("button", { name: /Try an example/ }));
    expect(screen.getByLabelText("Describe one object")).toHaveValue(EXAMPLE_PROMPT);
    expect(EXAMPLE_PROMPT).toBe(
      "A cobalt-blue ceramic teapot with one curved spout, a round brass lid, and a large loop handle.",
    );
  });

  it("changing the model saves it", async () => {
    backend(returning(), { set_model: (a) => ({ ...stored, model: a.model as string }) });
    render(<App />);
    await userEvent.selectOptions(await screen.findByLabelText("Model"), "claude-test-opus");
    expect(calls("set_model")[0][1]).toEqual({ provider: "anthropic", model: "claude-test-opus" });
    await waitFor(() => expect(screen.getByLabelText("Model")).toHaveValue("claude-test-opus"));
  });

  it("Ctrl/Cmd+Enter submits and the prompt is kept after an error", async () => {
    backend(returning(), {
      start_generation: () => {
        throw { kind: "network", title: "No connection", message: "Couldn't reach Anthropic.", action: "checkConnection", actionUrl: null };
      },
    });
    render(<App />);
    const prompt = await screen.findByLabelText("Describe one object");
    await userEvent.type(prompt, "a red toy car");
    fireEvent.keyDown(prompt, { key: "Enter", ctrlKey: true });
    await waitFor(() => expect(calls("start_generation")).toHaveLength(1));
    const req = calls("start_generation")[0][1].req;
    expect(req).toMatchObject({ prompt: "a red toy car", provider: "anthropic" });
    expect(typeof req.clientRequestId).toBe("string");
    expect(await screen.findByRole("alert")).toHaveTextContent("Couldn't reach Anthropic.");
    expect(prompt).toHaveValue("a red toy car");
    fireEvent.keyDown(prompt, { key: "Enter", metaKey: true });
    await waitFor(() => expect(calls("start_generation")).toHaveLength(2));
  });

  it("double-click sends one startGeneration and Generate stays disabled while active", async () => {
    const d = deferred<string>();
    backend(returning(), { start_generation: () => d.promise });
    render(<App />);
    await userEvent.type(await screen.findByLabelText("Describe one object"), "a mug");
    const gen = screen.getByRole("button", { name: "Generate" });
    fireEvent.click(gen);
    fireEvent.click(gen);
    await waitFor(() => expect(gen).toBeDisabled());
    expect(calls("start_generation")).toHaveLength(1);
    emitJob(job());
    await act(async () => d.resolve("p1"));
    expect(gen).toBeDisabled();
  });
});

describe("job progress and errors", () => {
  it("shows stage labels and the cancel report", async () => {
    backend(returning(), {
      cancel_generation: () => ({ outcome: "localOnly", message: "Stopped. Anthropic may still bill for the part of the answer it had already written." }),
    });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    emitJob(job());
    expect(screen.getByText("Anthropic is designing the object")).toBeInTheDocument();
    expect(screen.getByRole("progressbar")).not.toHaveAttribute("aria-valuenow");
    emitJob(job({ state: "building", label: "Building the 3D splat" }));
    expect(screen.getByText("Building the 3D splat")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Generate" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(calls("cancel_generation")[0][1]).toEqual({ projectId: "p1" });
    expect(await screen.findByText(/may still bill/)).toBeInTheDocument();
  });

  const failed = (action: string) =>
    job({
      state: "failed",
      error: { kind: "invalidDesign", title: "The model's design couldn't be used", message: "Anthropic answered, but the answer contained no JSON object.", action: action as never, actionUrl: null },
    });

  it("asking a paid provider again requires confirmation", async () => {
    backend(returning(), { get_project: () => detail({ state: "failed" }) });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    emitJob(failed("retryAll"));
    await userEvent.click(await screen.findByRole("button", { name: "Ask again…" }));
    expect(calls("retry_project")).toHaveLength(0);
    expect(screen.getByText("This sends a new request that your provider bills.")).toBeInTheDocument();
    await userEvent.click(screen.getByRole("button", { name: "Send new request" }));
    expect(calls("retry_project")[0][1]).toEqual({ projectId: "p1", action: "full" });
  });

  it("asking a local provider again needs no confirmation", async () => {
    backend(returning(), { get_project: () => detail({ state: "failed", provider: "ollama" }) });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    emitJob(failed("retryAll"));
    await userEvent.click(await screen.findByRole("button", { name: "Ask again" }));
    expect(calls("retry_project")[0][1]).toEqual({ projectId: "p1", action: "full" });
  });

  it("rebuild reuses the saved design for free", async () => {
    backend(returning(), { get_project: () => detail({ state: "needsAttention", retry: { rebuild: true, full: true } }) });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    emitJob(job({ state: "needsAttention", error: { kind: "internal", title: "Building was interrupted", message: "The design is saved.", action: "rebuild", actionUrl: null } }));
    await userEvent.click(await screen.findByRole("button", { name: "Rebuild (free)" }));
    expect(calls("retry_project")[0][1]).toEqual({ projectId: "p1", action: "rebuild" });
  });

  it("replaceKey opens Settings with the key form", async () => {
    backend(returning(), { get_project: () => detail({ state: "failed" }) });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    emitJob(failed("replaceKey"));
    await userEvent.click(await screen.findByRole("button", { name: "Open settings" }));
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(within(dialog).getByRole("region", { name: "Anthropic" })).toContainElement(within(dialog).getByLabelText("API key"));
  });
});

describe("keyboard and settings", () => {
  it("Tab order reaches prompt → Generate → Reset view → Export after ready", async () => {
    backend(returning(), { get_project: () => detail() });
    render(<App />);
    const prompt = await screen.findByLabelText("Describe one object");
    await userEvent.type(prompt, "a mug");
    emitJob(job({ state: "ready", label: "Ready" }));
    const reset = await screen.findByRole("button", { name: "Reset view" });
    const exp = screen.getByRole("button", { name: "Export splat (.ply)" });
    const gen = screen.getByRole("button", { name: "Generate" });
    prompt.focus();
    const order: Element[] = [document.activeElement!];
    for (let i = 0; i < 30 && document.activeElement !== exp; i++) {
      await userEvent.tab();
      order.push(document.activeElement!);
    }
    const idx = (el: Element) => order.indexOf(el);
    expect(idx(gen)).toBeGreaterThan(idx(prompt));
    expect(idx(reset)).toBeGreaterThan(idx(gen));
    expect(idx(exp)).toBeGreaterThan(idx(reset));
    expect(screen.getByRole("img", { name: /3D preview of teapot/ })).toHaveAttribute("tabindex", "0");
    expect(calls("get_project")[0][1]).toEqual({ projectId: "p1" });
  });

  it("Settings modal closes on Escape and restores focus", async () => {
    backend(returning());
    render(<App />);
    await screen.findByLabelText("Describe one object");
    const gear = screen.getByRole("button", { name: "Settings" });
    gear.focus();
    await userEvent.click(gear);
    const dialog = await screen.findByRole("dialog", { name: "Settings" });
    expect(dialog).toContainElement(document.activeElement as HTMLElement);
    expect(within(dialog).getByText("sk-a…a1F4")).toBeInTheDocument();
    // Claude Code and Codex are both detected but not connected.
    expect(within(dialog).getAllByText("Found")).toHaveLength(2);
    await userEvent.keyboard("{Escape}");
    expect(screen.queryByRole("dialog")).toBeNull();
    expect(gear).toHaveFocus();
  });

  it("disconnect updates the provider status", async () => {
    backend(returning(), { disconnect_provider: () => missing("anthropic") });
    render(<App />);
    await screen.findByLabelText("Describe one object");
    await userEvent.click(screen.getByRole("button", { name: "Settings" }));
    await userEvent.click(screen.getByRole("button", { name: "Disconnect" }));
    expect(calls("disconnect_provider")[0][1]).toEqual({ provider: "anthropic" });
    await waitFor(() =>
      expect(within(screen.getByRole("region", { name: "Anthropic" })).getByText("Not connected")).toBeInTheDocument(),
    );
  });

  it("adjusting the open object sends the prompt and tracks the new object", async () => {
    backend(
      appState({
        statuses: returning().statuses,
        selectedProvider: "anthropic",
        recent: [{ id: "p1", prompt: "teapot", provider: "anthropic", state: "ready", createdAt: 1, thumbPath: null, splatCount: 6 }],
      }),
      { adjust_object: () => "p2" },
    );
    render(<App />);
    await userEvent.click(within(await screen.findByRole("region", { name: "Recent objects" })).getByRole("button", { name: /teapot/ }));
    const field = await screen.findByLabelText("Adjust with a prompt");
    await userEvent.type(field, "make the handle gold");
    await userEvent.click(screen.getByRole("button", { name: "Adjust" }));
    const req = calls("adjust_object")[0][1].req;
    expect(req).toMatchObject({ projectId: "p1", instruction: "make the handle gold", provider: "anthropic" });
    expect(field).toHaveValue("");
    expect(await screen.findByText("Starting")).toBeInTheDocument();
  });

  it("opening a recent object and exporting shows the saved path", async () => {
    backend(
      appState({
        statuses: returning().statuses,
        recent: [{ id: "p1", prompt: "teapot", provider: "anthropic", state: "ready", createdAt: 1, thumbPath: "/t.png", splatCount: 6 }],
      }),
      { export_splat: () => ({ saved: true, path: "/Users/me/teapot.ply", bytes: 100, sha256: "abc" }) },
    );
    render(<App />);
    await userEvent.click(within(await screen.findByRole("region", { name: "Recent objects" })).getByRole("button", { name: /teapot/ }));
    await userEvent.click(await screen.findByRole("button", { name: "Export splat (.ply)" }));
    expect(await screen.findByText("Saved to /Users/me/teapot.ply")).toBeInTheDocument();
  });
});
