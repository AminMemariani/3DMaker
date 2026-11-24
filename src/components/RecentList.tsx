import { assetUrl, type ProjectSummary } from "../lib/ipc";
import { STATE_LABEL } from "../lib/ui";

interface Props {
  items: ProjectSummary[];
  currentId: string | null;
  onOpen: (id: string) => void;
}

export function RecentList({ items, currentId, onOpen }: Props) {
  return (
    <section className="recent" aria-label="Recent objects">
      <h2>Recent objects</h2>
      {items.length === 0 ? (
        <p className="muted small">Objects you create or open appear here.</p>
      ) : (
        <ul>
          {items.map((p) => (
            <li key={p.id}>
              <button
                type="button"
                data-project-id={p.id}
                aria-current={p.id === currentId ? "true" : undefined}
                onClick={() => onOpen(p.id)}
              >
                {p.thumbPath ? (
                  <img className="thumb" src={assetUrl(p.thumbPath)} alt="" />
                ) : (
                  <span className="thumb empty" aria-hidden="true" />
                )}
                <span className="recent-text">
                  <span className="recent-prompt">{p.prompt || "Imported object"}</span>
                  <span className="muted small">{STATE_LABEL[p.state]}</span>
                </span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
