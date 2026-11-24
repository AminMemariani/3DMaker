import { useId, useRef, useState } from "react";
import { api, type ProviderInfo, type ProviderStatus } from "../lib/ipc";
import { toShownError, type ShownError } from "../lib/ui";
import { ErrorBox } from "./ErrorBox";
import { ModelPicker } from "./ModelPicker";

export const EXAMPLE_PROMPT =
  "A cobalt-blue ceramic teapot with one curved spout, a round brass lid, and a large loop handle.";

interface Props {
  provider: ProviderInfo | null;
  status: ProviderStatus | null;
  jobActive: boolean;
  onStarted: (projectId: string) => void;
  onConnectClick: () => void;
  onStatus: (s: ProviderStatus) => void;
}

export function CreatePanel({ provider, status, jobActive, onStarted, onConnectClick, onStatus }: Props) {
  const id = useId();
  const [prompt, setPrompt] = useState("");
  const [submitting, setSubmitting] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<ShownError | null>(null);

  const canGenerate = !!provider && !jobActive && !submitting && prompt.trim().length > 0;

  async function submit() {
    if (inFlight.current || !provider || jobActive || !prompt.trim()) return;
    inFlight.current = true; // synchronous double-click guard
    setSubmitting(true);
    setError(null);
    try {
      const projectId = await api.startGeneration({
        prompt: prompt.trim(),
        provider: provider.id,
        clientRequestId: crypto.randomUUID(),
      });
      onStarted(projectId);
    } catch (e) {
      setError(toShownError(e));
    } finally {
      inFlight.current = false;
      setSubmitting(false);
    }
  }

  return (
    <section className="create" aria-labelledby={`${id}-h`}>
      <h2 id={`${id}-h`}>Create an object</h2>
      <label htmlFor={`${id}-prompt`}>Describe one object</label>
      <textarea
        id={`${id}-prompt`}
        rows={4}
        autoFocus
        value={prompt}
        placeholder="For example: a red wooden toy car with four black wheels"
        onChange={(e) => setPrompt(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
            e.preventDefault();
            void submit();
          }
        }}
      />
      <button type="button" className="chip" onClick={() => setPrompt(EXAMPLE_PROMPT)}>
        Try an example: a cobalt-blue teapot
      </button>
      {provider ? (
        <>
          <p className="muted small">
            Using {provider.name}
            {status?.maskedKey ? ` (${status.maskedKey})` : ""}
            <br />
            <span>{provider.billingNote}</span>
          </p>
          {status && <ModelPicker status={status} onStatus={onStatus} disabled={jobActive} />}
        </>
      ) : (
        <p className="small">
          Connect an AI provider to create new objects.{" "}
          <button type="button" className="link" onClick={onConnectClick}>
            Connect
          </button>
        </p>
      )}
      <button type="button" className="primary" disabled={!canGenerate} onClick={() => void submit()}>
        Generate
      </button>
      <p className="muted small">Tip: press Ctrl+Enter (⌘+Enter on Mac) to generate.</p>
      {error && <ErrorBox error={error} />}
    </section>
  );
}
