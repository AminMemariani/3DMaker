import { useEffect, useId, useRef, type ReactNode } from "react";

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), summary, [tabindex]:not([tabindex="-1"])';

/** Accessible modal: traps focus, closes on Escape, restores focus to the opener. */
export function Modal({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  const id = useId();
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const opener = document.activeElement as HTMLElement | null;
    const first = ref.current?.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? ref.current)?.focus();
    return () => opener?.focus?.();
  }, []);

  return (
    <div className="modal-backdrop" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div
        ref={ref}
        className="modal"
        role="dialog"
        aria-modal="true"
        aria-labelledby={`${id}-t`}
        tabIndex={-1}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            e.stopPropagation();
            onClose();
            return;
          }
          if (e.key !== "Tab" || !ref.current) return;
          const items = Array.from(ref.current.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
            (el) => !el.closest("details:not([open])") || el.tagName === "SUMMARY",
          );
          if (!items.length) return;
          const first = items[0];
          const last = items[items.length - 1];
          if (e.shiftKey && document.activeElement === first) {
            e.preventDefault();
            last.focus();
          } else if (!e.shiftKey && document.activeElement === last) {
            e.preventDefault();
            first.focus();
          }
        }}
      >
        <div className="modal-head">
          <h2 id={`${id}-t`}>{title}</h2>
          <button type="button" onClick={onClose} aria-label="Close settings">
            ✕
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}
