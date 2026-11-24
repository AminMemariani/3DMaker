import { useEffect, useState } from "react";
import { api, billsPerCall, type CancelReport, type JobView, type ProjectDetail, type RetryAction } from "../lib/ipc";
import { isActive, toShownError, usePrefersReducedMotion, type ShownError } from "../lib/ui";
import { ErrorBox } from "./ErrorBox";

interface Props {
  job: JobView;
  /** Saved project detail (provider, retry flags) when known. */
  detail: ProjectDetail | null;
  onReplaceKey: () => void;
}

export function JobPanel({ job, detail, onReplaceKey }: Props) {
  const reduced = usePrefersReducedMotion();
  const active = isActive(job.state);
  const [cancelReport, setCancelReport] = useState<CancelReport | null>(null);
  const [cancelling, setCancelling] = useState(false);
  const [cancelError, setCancelError] = useState<ShownError | null>(null);

  useEffect(() => {
    setCancelReport(null);
    setCancelError(null);
  }, [job.projectId]);

  const report = cancelReport ?? job.cancel;

  async function cancel() {
    setCancelling(true);
    try {
      setCancelReport(await api.cancelGeneration(job.projectId));
    } catch (e) {
      setCancelError(toShownError(e));
    } finally {
      setCancelling(false);
    }
  }

  return (
    <section className="job" aria-label="Generation progress">
      {active && (
        <>
          <p className="job-label" data-testid="job-label" aria-live="polite">
            {job.label}
          </p>
          {reduced ? (
            <p className="small">Working…</p>
          ) : (
            <div className="bar indeterminate" role="progressbar" aria-label={job.label}>
              <div className="bar-fill" />
            </div>
          )}
          <button type="button" onClick={() => void cancel()} disabled={cancelling}>
            Cancel
          </button>
        </>
      )}
      {report && (
        <p className="notice" role="status">
          {report.message}
        </p>
      )}
      {cancelError && <ErrorBox error={cancelError} />}
      {(job.state === "failed" || job.state === "needsAttention" || job.state === "cancelled") && (
        <ErrorActions job={job} detail={detail} onReplaceKey={onReplaceKey} />
      )}
    </section>
  );
}

function ErrorActions({ job, detail, onReplaceKey }: Props) {
  const err = job.error ?? detail?.error ?? null;
  const paid = billsPerCall(detail?.provider ?? null) || !detail;
  const [confirmFull, setConfirmFull] = useState(false);
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<ShownError | null>(null);

  async function retry(action: RetryAction) {
    setPending(true);
    setFailure(null);
    try {
      await api.retryProject(job.projectId, action);
      setConfirmFull(false);
    } catch (e) {
      setFailure(toShownError(e));
    } finally {
      setPending(false);
    }
  }

  /** Paid providers confirm before asking again; local ones don't. */
  function askAgain() {
    if (paid) setConfirmFull(true);
    else void retry("full");
  }

  /** Best retry that doesn't guess: a free rebuild if the design is saved, else ask again. */
  function tryAgain() {
    if (detail?.retry.rebuild) return void retry("rebuild");
    askAgain();
  }

  const askButton = (
    <button type="button" disabled={pending} onClick={askAgain}>
      {paid ? "Ask again…" : "Ask again"}
    </button>
  );
  const tryAgainButton = (
    <button type="button" disabled={pending} onClick={tryAgain}>
      Try again
    </button>
  );

  let body: React.ReactNode = null;
  switch (err?.action) {
    case "replaceKey":
      body = (
        <button type="button" onClick={onReplaceKey}>
          Open settings
        </button>
      );
      break;
    case "addCredit":
      body = err.actionUrl ? (
        <p>
          <a href={err.actionUrl} target="_blank" rel="noreferrer">
            Add credit
          </a>{" "}
          <span className="muted small selectable">{err.actionUrl}</span>
        </p>
      ) : null;
      break;
    case "rebuild":
      body = (
        <button type="button" disabled={pending} onClick={() => void retry("rebuild")}>
          Rebuild (free)
        </button>
      );
      break;
    case "retryAll":
      body = askButton;
      break;
    case "startLocal":
    case "checkConnection":
    case "retryLater":
    case "freeSpace":
      body = tryAgainButton;
      break;
    default:
      if (detail && (detail.retry.rebuild || detail.retry.full)) {
        body = (
          <div className="row">
            {detail.retry.rebuild && (
              <button type="button" disabled={pending} onClick={() => void retry("rebuild")}>
                Rebuild (free)
              </button>
            )}
            {detail.retry.full && askButton}
          </div>
        );
      }
  }

  return (
    <div className="error-actions">
      {(err || job.state !== "cancelled") && (
        <ErrorBox
          error={
            err
              ? { title: err.title, message: err.message }
              : { title: "This object didn't finish", message: "Choose how to continue." }
          }
        />
      )}
      {body}
      {confirmFull && (
        <div className="notice" role="group" aria-label="Confirm asking again">
          <p>This sends a new request that your provider bills.</p>
          <div className="row">
            <button type="button" className="primary" disabled={pending} onClick={() => void retry("full")}>
              Send new request
            </button>
            <button type="button" onClick={() => setConfirmFull(false)}>
              Keep it
            </button>
          </div>
        </div>
      )}
      {failure && <ErrorBox error={failure} />}
    </div>
  );
}
