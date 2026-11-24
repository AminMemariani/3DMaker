import { useState } from "react";
import type { ConnectResult, LocalDetection, ProviderId, ProviderInfo } from "../lib/ipc";
import { ConnectForm } from "./ConnectForm";

interface Props {
  providers: ProviderInfo[];
  local: LocalDetection[];
  onConnected: (res: ConnectResult) => void;
  onRefreshLocal: () => Promise<void>;
  onSkip?: () => void;
  /** Whether saved objects exist (changes the skip link's wording). */
  hasSaved?: boolean;
}

export function ConnectScreen({ providers, local, onConnected, onRefreshLocal, onSkip, hasSaved }: Props) {
  const localInfos = providers.filter((p) => p.kind === "local");
  const cloud = providers.filter((p) => p.kind === "cloud");
  const custom = providers.find((p) => p.kind === "custom");
  const [choice, setChoice] = useState<ProviderId>(cloud[0]?.id ?? "anthropic");
  const [refreshing, setRefreshing] = useState(false);
  const info = cloud.find((p) => p.id === choice);
  const found = localInfos.filter((p) => local.find((d) => d.id === p.id)?.available);

  return (
    <main className="connect-screen">
      <h1>Choose your AI</h1>
      <p className="muted">
        A language model designs each object as a set of coloured shapes; 3D Maker turns the design into a 3D splat on
        this computer. Keys stay on this computer.
      </p>

      <section aria-label="Found on this computer">
        <h2>On this computer</h2>
        {found.length === 0 && <p className="small muted">No local AI was found.</p>}
        {localInfos.map((p) => {
          const d = local.find((x) => x.id === p.id) ?? null;
          return (
            <section key={p.id} className="provider-card" aria-label={`Connect ${p.name}`}>
              <h3>
                {p.name}
                {d?.available && <span className="badge">Found</span>}
              </h3>
              <ConnectForm info={p} local={d} onConnected={onConnected} />
            </section>
          );
        })}
        <button
          type="button"
          className="link"
          disabled={refreshing}
          onClick={() => {
            setRefreshing(true);
            void onRefreshLocal().finally(() => setRefreshing(false));
          }}
        >
          {refreshing ? "Looking…" : "Look again"}
        </button>
      </section>

      <section aria-label="Cloud providers">
        <h2>Cloud</h2>
        <fieldset className="provider-choice">
          <legend>Provider</legend>
          {cloud.map((p) => (
            <label key={p.id} className="radio">
              <input
                type="radio"
                name="provider"
                value={p.id}
                checked={choice === p.id}
                onChange={() => setChoice(p.id)}
              />
              {p.name}
            </label>
          ))}
        </fieldset>
        {info && (
          <section aria-label={`Connect ${info.name}`}>
            <ConnectForm key={info.id} info={info} onConnected={onConnected} autoFocus={found.length === 0} />
          </section>
        )}
      </section>

      {custom && (
        <details className="advanced">
          <summary>Custom provider (OpenAI-compatible server)</summary>
          <ConnectForm info={custom} onConnected={onConnected} />
        </details>
      )}
      {onSkip && (
        <p>
          <button type="button" className="link" onClick={onSkip}>
            {hasSaved ? "View saved objects without connecting" : "Open a .ply file without connecting"}
          </button>
        </p>
      )}
    </main>
  );
}
