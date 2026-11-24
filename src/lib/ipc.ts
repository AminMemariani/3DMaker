// IPC contract between the React UI and the Rust core.
//
// Every type here mirrors a serde type in `src-tauri/src/commands.rs` /
// `crates/maker-core` (camelCase on the wire). Binary data never crosses this
// boundary as JSON: splats are loaded through the scoped asset protocol
// (`convertFileSrc`) and thumbnails are sent as a raw byte body.
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ProviderId = "anthropic" | "openai" | "custom" | "claude-code" | "codex" | "ollama";

/** Cloud and custom providers need a key; local ones are found on this computer. */
export type ProviderKind = "cloud" | "custom" | "local";

export interface ProviderInfo {
  id: ProviderId;
  name: string;
  kind: ProviderKind;
  keyUrl: string | null;
  /** Null when the provider needs no key. */
  keyLabel: string | null;
  keyRequired: boolean;
  verifyNote: string;
  flowSummary: string;
  billingNote: string;
}

/** What auto-detection found for a local provider. */
export interface LocalDetection {
  id: ProviderId;
  available: boolean;
  installed: boolean;
  detail: string | null;
  models: string[];
  hint: string | null;
}

export type CredentialState = "missing" | "stored" | "session" | "notRequired";

export interface ProviderStatus {
  id: ProviderId;
  connected: boolean;
  credential: CredentialState;
  /** e.g. "sk-a…a1F4" — never the secret itself. */
  maskedKey: string | null;
  accountLabel: string | null;
  /** Custom provider only. */
  endpoint: string | null;
  model: string | null;
  models: string[];
  verifiedAt: number | null;
}

export interface AppState {
  providers: ProviderInfo[];
  statuses: ProviderStatus[];
  local: LocalDetection[];
  selectedProvider: ProviderId | null;
  credentialStoreAvailable: boolean;
  activeJob: JobView | null;
  recent: ProjectSummary[];
  dataDir: string;
}

export interface ConnectRequest {
  provider: ProviderId;
  /** Sent once; the UI clears its input right after invoking. Empty for local providers. */
  key: string;
  sessionOnly: boolean;
  /** Custom provider only: https://… or an explicitly allowed loopback http URL. */
  endpoint?: string;
  allowLoopback?: boolean;
  model?: string;
}

export interface ConnectResult {
  status: ProviderStatus;
  /** True when the OS credential store failed and the key was NOT saved. */
  storeUnavailable: boolean;
}

export interface GenerateRequest {
  prompt: string;
  provider: ProviderId;
  /** Unique per click; Rust rejects a second concurrent job regardless. */
  clientRequestId: string;
}

export type ProjectState =
  | "queued"
  | "designing"
  | "building"
  | "saving"
  | "ready"
  | "failed"
  | "cancelled"
  | "needsAttention";

export type ErrorKind =
  | "invalidKey"
  | "noCredit"
  | "rateLimited"
  | "network"
  | "unavailable"
  | "providerFailed"
  | "invalidDesign"
  | "invalidAsset"
  | "diskFull"
  | "unknownOutcome"
  | "cancelled"
  | "busy"
  | "internal";

/** Actions the UI can offer; each maps to a command. */
export type NextAction =
  | "replaceKey" // open Settings for this provider
  | "addCredit" // open the billing page (UserError.actionUrl)
  | "retryLater"
  | "retryAll" // ask the model again — may be billed
  | "rebuild" // rebuild the saved design locally — free
  | "checkConnection"
  | "startLocal" // install / start / sign in to a local provider
  | "freeSpace"
  | "none";

export interface UserError {
  kind: ErrorKind;
  title: string;
  message: string;
  action: NextAction;
  actionUrl: string | null;
}

export interface JobView {
  projectId: string;
  state: ProjectState;
  /** Plain-language label: "Anthropic is designing the object", "Building the 3D splat". */
  label: string;
  error: UserError | null;
  cancel: CancelReport | null;
}

export type CancelOutcome =
  | "localOnly" // stopped here; a cloud provider may still bill for work already done
  | "confirmed" // stopped, nothing keeps running
  | "alreadyFinished";

export interface CancelReport {
  outcome: CancelOutcome;
  message: string;
}

export interface Bounds {
  center: [number, number, number];
  /** Robust radius (ignores the outermost ~2% of splats). */
  radius: number;
  min: [number, number, number];
  max: [number, number, number];
}

export interface ProjectSummary {
  id: string;
  prompt: string;
  provider: ProviderId | "import";
  state: ProjectState;
  createdAt: number;
  thumbPath: string | null;
  splatCount: number | null;
}

export interface ProjectDetail extends ProjectSummary {
  splatPath: string | null;
  splatBytes: number | null;
  splatSha256: string | null;
  shDegree: number | null;
  bounds: Bounds | null;
  /** Viewer-only orientation correction (degrees, XYZ Euler). Never applied to the exported file. */
  viewRotation: [number, number, number];
  model: string | null;
  partCount: number | null;
  error: UserError | null;
  /** Which retries are possible without guessing. */
  retry: { rebuild: boolean; full: boolean };
}

export type RetryAction = "rebuild" | "full";

export interface ExportResult {
  /** False if the user dismissed the save dialog. */
  saved: boolean;
  path: string | null;
  bytes: number | null;
  sha256: string | null;
}

/** Providers that charge per request (Retry asks for confirmation). */
export function billsPerCall(id: ProviderId | "import" | null | undefined): boolean {
  return id === "anthropic" || id === "openai" || id === "custom";
}

export const api = {
  getAppState: () => invoke<AppState>("get_app_state"),
  detectProviders: () => invoke<LocalDetection[]>("detect_providers"),
  connectProvider: (req: ConnectRequest) => invoke<ConnectResult>("connect_provider", { req }),
  disconnectProvider: (provider: ProviderId) => invoke<ProviderStatus>("disconnect_provider", { provider }),
  selectProvider: (provider: ProviderId) => invoke<void>("select_provider", { provider }),
  setModel: (provider: ProviderId, model: string) => invoke<ProviderStatus>("set_model", { provider, model }),
  startGeneration: (req: GenerateRequest) => invoke<string>("start_generation", { req }),
  /** Changes a finished object as described; the result is a new object. */
  adjustObject: (req: { projectId: string; instruction: string; provider: ProviderId; clientRequestId: string }) =>
    invoke<string>("adjust_object", { req }),
  cancelGeneration: (projectId: string) => invoke<CancelReport>("cancel_generation", { projectId }),
  retryProject: (projectId: string, action: RetryAction) => invoke<void>("retry_project", { projectId, action }),
  listProjects: () => invoke<ProjectSummary[]>("list_projects"),
  getProject: (projectId: string) => invoke<ProjectDetail>("get_project", { projectId }),
  deleteProject: (projectId: string) => invoke<void>("delete_project", { projectId }),
  /** Opens the native save dialog in Rust and writes a byte-for-byte copy of the validated PLY. */
  exportSplat: (projectId: string) => invoke<ExportResult>("export_splat", { projectId }),
  /** Opens the native open dialog in Rust: a .ply is validated and copied, an .obj/.stl is converted. Null if dismissed. */
  importSplat: () => invoke<string | null>("import_splat"),
  /** Raw PNG bytes (not JSON) — saved as the object's thumbnail when it has none. */
  saveThumbnail: (projectId: string, png: Uint8Array) =>
    invoke<void>("save_thumbnail", png, { headers: { "x-project-id": projectId } }),
  /** Report that the viewer could not start GPU rendering (logged without PII). */
  reportViewerIssue: (detail: string) => invoke<void>("report_viewer_issue", { detail }),
};

/** Event emitted whenever the active job changes. Payload: JobView. */
export const JOB_EVENT = "job-update";
/** Event emitted when the recent-objects list changes. Payload: ProjectSummary[]. */
export const PROJECTS_EVENT = "projects-update";

export function onJobUpdate(cb: (job: JobView) => void): Promise<UnlistenFn> {
  return listen<JobView>(JOB_EVENT, (e) => cb(e.payload));
}

export function onProjectsUpdate(cb: (list: ProjectSummary[]) => void): Promise<UnlistenFn> {
  return listen<ProjectSummary[]>(PROJECTS_EVENT, (e) => cb(e.payload));
}

/** Local file → URL the webview may load (asset protocol, scoped to the app's object library). */
export function assetUrl(path: string): string {
  return convertFileSrc(path);
}
