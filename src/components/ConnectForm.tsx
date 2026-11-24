import { useId, useState } from "react";
import { api, type ConnectResult, type LocalDetection, type ProviderInfo } from "../lib/ipc";
import { toShownError, type ShownError } from "../lib/ui";
import { ErrorBox } from "./ErrorBox";

interface Props {
  info: ProviderInfo;
  onConnected: (res: ConnectResult) => void;
  autoFocus?: boolean;
  initialEndpoint?: string | null;
  /** Detection result, for local providers. */
  local?: LocalDetection | null;
}

/**
 * Connects one provider. Cloud providers take a key, the custom provider a
 * server URL (and optional key), local providers just a click. A key lives in
 * component state only until Connect is clicked.
 */
export function ConnectForm({ info, onConnected, autoFocus, initialEndpoint, local }: Props) {
  const id = useId();
  const custom = info.kind === "custom";
  const isLocal = info.kind === "local";
  const [key, setKey] = useState("");
  const [endpoint, setEndpoint] = useState(initialEndpoint ?? "");
  const [model, setModel] = useState("");
  const [allowLoopback, setAllowLoopback] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<ShownError | null>(null);
  const [offerSession, setOfferSession] = useState(false);

  async function connect(sessionOnly: boolean) {
    if (info.keyRequired && !key.trim()) {
      setError({ title: null, message: `Paste your ${info.keyLabel ?? "key"} first.` });
      return;
    }
    if (custom && !endpoint.trim()) {
      setError({ title: null, message: "Enter the server URL first." });
      return;
    }
    const k = key.trim();
    setError(null);
    setBusy(true);
    const p = api.connectProvider({
      provider: info.id,
      key: k,
      sessionOnly,
      ...(custom ? { endpoint: endpoint.trim(), allowLoopback, ...(model.trim() ? { model: model.trim() } : {}) } : {}),
    });
    setKey(""); // never keep the secret around after sending it
    try {
      const res = await p;
      if (res.storeUnavailable && !sessionOnly) {
        setOfferSession(true);
        return;
      }
      setOfferSession(false);
      onConnected(res);
    } catch (e) {
      setError(toShownError(e));
    } finally {
      setBusy(false);
    }
  }

  if (isLocal) {
    const usable = local?.available ?? false;
    return (
      <div className="connect-form">
        {local?.detail && <p className="small selectable">Found: {local.detail}</p>}
        {!usable && local?.hint && <p className="small">{local.hint}</p>}
        <p className="muted small">{info.verifyNote}</p>
        <p className="small">{info.billingNote}</p>
        <div className="row">
          <button type="button" className="primary" disabled={busy || !usable} onClick={() => void connect(false)}>
            {busy ? "Checking…" : `Use ${info.name}`}
          </button>
        </div>
        {error && <ErrorBox error={error} />}
      </div>
    );
  }

  return (
    <form
      className="connect-form"
      onSubmit={(e) => {
        e.preventDefault();
        void connect(false);
      }}
    >
      {custom && (
        <>
          <label htmlFor={`${id}-ep`}>Server URL</label>
          <input
            id={`${id}-ep`}
            type="url"
            value={endpoint}
            placeholder="https://openrouter.ai/api/v1"
            onChange={(e) => setEndpoint(e.target.value)}
            spellCheck={false}
          />
          <label className="check">
            <input type="checkbox" checked={allowLoopback} onChange={(e) => setAllowLoopback(e.target.checked)} />
            Allow a server on this computer (http://localhost)
          </label>
          <label htmlFor={`${id}-model`}>Model (optional)</label>
          <input
            id={`${id}-model`}
            type="text"
            value={model}
            placeholder="Picked from the server's list if empty"
            onChange={(e) => setModel(e.target.value)}
            spellCheck={false}
          />
        </>
      )}
      <label htmlFor={`${id}-key`}>{info.keyLabel}</label>
      <input
        id={`${id}-key`}
        type="password"
        autoComplete="off"
        spellCheck={false}
        value={key}
        autoFocus={autoFocus}
        onChange={(e) => setKey(e.target.value)}
      />
      {info.keyUrl && (
        <p className="muted small">
          Get one at{" "}
          <a href={info.keyUrl} target="_blank" rel="noreferrer">
            {info.keyUrl}
          </a>
        </p>
      )}
      <p className="muted small">{info.verifyNote}</p>
      <p className="small">{info.billingNote}</p>
      <div className="row">
        <button type="submit" className="primary" disabled={busy}>
          {busy ? "Connecting…" : "Connect"}
        </button>
      </div>
      {offerSession && (
        <div className="notice" role="status">
          <p>
            Your computer's secure key storage isn't available, so the key was not saved. You can use it for this
            session only; it is forgotten when you quit. For safety the field was cleared: paste the key again, then
            choose “Use for this session only”.
          </p>
          <button type="button" disabled={busy} onClick={() => void connect(true)}>
            Use for this session only
          </button>
        </div>
      )}
      {error && <ErrorBox error={error} />}
    </form>
  );
}
