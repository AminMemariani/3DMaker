import type { ShownError } from "../lib/ui";

export function ErrorBox({ error }: { error: ShownError }) {
  return (
    <div className="error" role="alert">
      {error.title && <strong>{error.title}</strong>}
      <p>{error.message}</p>
    </div>
  );
}
