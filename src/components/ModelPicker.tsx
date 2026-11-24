import { useId, useState } from "react";
import { api, type ProviderStatus } from "../lib/ipc";
import { toShownError, type ShownError } from "../lib/ui";
import { ErrorBox } from "./ErrorBox";

interface Props {
  status: ProviderStatus;
  onStatus: (s: ProviderStatus) => void;
  disabled?: boolean;
}

/** Chooses the model a connected provider uses. Falls back to free text when the server listed none. */
export function ModelPicker({ status, onStatus, disabled }: Props) {
  const id = useId();
  const [error, setError] = useState<ShownError | null>(null);
  const [draft, setDraft] = useState(status.model ?? "");
  const options = status.model && !status.models.includes(status.model) ? [status.model, ...status.models] : status.models;

  async function choose(model: string) {
    if (!model.trim() || model === status.model) return;
    setError(null);
    try {
      onStatus(await api.setModel(status.id, model));
    } catch (e) {
      setError(toShownError(e));
    }
  }

  return (
    <div className="field model-picker">
      <label htmlFor={`${id}-m`}>Model</label>
      {options.length > 0 ? (
        <select
          id={`${id}-m`}
          value={status.model ?? ""}
          disabled={disabled}
          onChange={(e) => void choose(e.target.value)}
        >
          {options.map((m) => (
            <option key={m} value={m}>
              {m === "default" ? "Default" : m}
            </option>
          ))}
        </select>
      ) : (
        <input
          id={`${id}-m`}
          type="text"
          value={draft}
          disabled={disabled}
          spellCheck={false}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => void choose(draft)}
        />
      )}
      {error && <ErrorBox error={error} />}
    </div>
  );
}
