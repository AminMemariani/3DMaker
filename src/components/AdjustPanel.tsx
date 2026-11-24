import { useId, useRef, useState } from "react";
import { api, type ProjectDetail, type ProviderInfo } from "../lib/ipc";
import { toShownError, type ShownError } from "../lib/ui";
import { ErrorBox } from "./ErrorBox";

interface Props {
  project: ProjectDetail;
  provider: ProviderInfo | null;
  jobActive: boolean;
  onStarted: (projectId: string) => void;
}

/** Describe a change to the open object; the result is saved as a new object. */
export function AdjustPanel({ project, provider, jobActive, onStarted }: Props) {
  const id = useId();
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const inFlight = useRef(false);
  const [error, setError] = useState<ShownError | null>(null);

  async function submit() {
    if (inFlight.current || !provider || jobActive || !text.trim()) return;
    inFlight.current = true;
    setBusy(true);
    setError(null);
    try {
      const newId = await api.adjustObject({
        projectId: project.id,
        instruction: text.trim(),
        provider: provider.id,
        clientRequestId: crypto.randomUUID(),
      });
      setText("");
      onStarted(newId);
    } catch (e) {
      setError(toShownError(e));
    } finally {
      inFlight.current = false;
      setBusy(false);
    }
  }

  return (
    <form
      className="adjust"
      aria-label="Adjust this object"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <label htmlFor={`${id}-a`}>Adjust with a prompt</label>
      <div className="row">
        <input
          id={`${id}-a`}
          type="text"
          value={text}
          placeholder="e.g. make the handle gold, remove the lid, twice as tall"
          onChange={(e) => setText(e.target.value)}
        />
        <button type="submit" className="primary" disabled={busy || jobActive || !provider || !text.trim()}>
          Adjust
        </button>
      </div>
      <p className="muted small">
        {provider
          ? `Saved as a new object; the original is kept. Uses ${provider.name}.`
          : "Connect an AI provider in Settings to adjust objects."}
      </p>
      {error && <ErrorBox error={error} />}
    </form>
  );
}
