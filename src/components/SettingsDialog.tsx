import { useState } from "react";
import {
  api,
  type ConnectResult,
  type LocalDetection,
  type ProviderId,
  type ProviderInfo,
  type ProviderStatus,
} from "../lib/ipc";
import { isConnected, toShownError, type ShownError } from "../lib/ui";
import { ConnectForm } from "./ConnectForm";
import { ErrorBox } from "./ErrorBox";
import { ModelPicker } from "./ModelPicker";
import { Modal } from "./Modal";

interface Props {
  providers: ProviderInfo[];
  statuses: ProviderStatus[];
  local: LocalDetection[];
  selected: ProviderId | null;
  initialReplace: ProviderId | null;
  onClose: () => void;
  onStatus: (s: ProviderStatus) => void;
  onConnected: (res: ConnectResult) => void;
  onSelect: (id: ProviderId) => void;
  onRefreshLocal: () => Promise<void>;
}

function connectionText(p: ProviderInfo, s: ProviderStatus | undefined): string {
  if (!isConnected(s)) return "Not connected";
  if (p.kind === "local") return "Connected";
  switch (s?.credential) {
    case "stored":
      return "Connected; key saved in your system's secure storage";
    case "session":
      return "Connected for this session only";
    default:
      return "Connected; no key needed";
  }
}

export function SettingsDialog(props: Props) {
  const { providers, statuses, local, selected, onClose } = props;
  const [replacing, setReplacing] = useState<ProviderId | null>(props.initialReplace);
  const [error, setError] = useState<ShownError | null>(null);

  async function disconnect(id: ProviderId) {
    setError(null);
    try {
      props.onStatus(await api.disconnectProvider(id));
    } catch (e) {
      setError(toShownError(e));
    }
  }

  function renderProvider(p: ProviderInfo) {
    const s = statuses.find((x) => x.id === p.id);
    const d = local.find((x) => x.id === p.id) ?? null;
    const connected = isConnected(s);
    return (
      <section key={p.id} className="provider-card" aria-label={p.name}>
        <h3>
          {p.name}
          {selected === p.id && connected && <span className="badge">In use</span>}
          {p.kind === "local" && !connected && d?.available && <span className="badge">Found</span>}
        </h3>
        <p className="small">{connectionText(p, s)}</p>
        {s?.maskedKey && (
          <p className="small">
            Key: <code>{s.maskedKey}</code>
          </p>
        )}
        {connected && s?.accountLabel && <p className="small">{s.accountLabel}</p>}
        {s?.endpoint && <p className="small selectable">Server: {s.endpoint}</p>}
        {connected && s && <ModelPicker status={s} onStatus={props.onStatus} />}
        <div className="row">
          {connected && selected !== p.id && (
            <button type="button" onClick={() => props.onSelect(p.id)}>
              Use {p.name}
            </button>
          )}
          <button type="button" onClick={() => setReplacing(replacing === p.id ? null : p.id)}>
            {connected ? (p.kind === "local" ? "Check again" : p.kind === "custom" ? "Change server" : "Replace key") : "Connect"}
          </button>
          {connected && (
            <button type="button" onClick={() => void disconnect(p.id)}>
              Disconnect
            </button>
          )}
        </div>
        {replacing === p.id && (
          <ConnectForm
            info={p}
            local={d}
            autoFocus
            initialEndpoint={s?.endpoint}
            onConnected={(res) => {
              setReplacing(null);
              props.onConnected(res);
            }}
          />
        )}
      </section>
    );
  }

  const byKind = (k: ProviderInfo["kind"]) => providers.filter((p) => p.kind === k);
  return (
    <Modal title="Settings" onClose={onClose}>
      <p className="muted small">Your saved objects stay available even without a connected provider.</p>
      <h2>On this computer</h2>
      {byKind("local").map(renderProvider)}
      <button type="button" className="link" onClick={() => void props.onRefreshLocal()}>
        Look again for local AI
      </button>
      <h2>Cloud</h2>
      {byKind("cloud").map(renderProvider)}
      <details className="advanced" open={byKind("custom").some((p) => p.id === replacing || isConnected(statuses.find((s) => s.id === p.id))) || undefined}>
        <summary>Custom provider (OpenAI-compatible server)</summary>
        {byKind("custom").map(renderProvider)}
      </details>
      {error && <ErrorBox error={error} />}
    </Modal>
  );
}
