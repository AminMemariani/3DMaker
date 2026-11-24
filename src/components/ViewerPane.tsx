import { useEffect, useRef, useState } from "react";
import { api, assetUrl, type ProjectDetail, type ProviderInfo } from "../lib/ipc";
import { toShownError, type ShownError } from "../lib/ui";
import { Viewport, type ViewportHandle } from "../viewer/Viewport";
import { AdjustPanel } from "./AdjustPanel";
import { ErrorBox } from "./ErrorBox";

interface Props {
  project: ProjectDetail | null;
  onOpenProject: (id: string) => void;
  /** Provider used for prompt adjustments; null when none is connected. */
  provider?: ProviderInfo | null;
  jobActive?: boolean;
  onAdjustStarted?: (projectId: string) => void;
}

export function ViewerPane({ project, onOpenProject, provider = null, jobActive = false, onAdjustStarted }: Props) {
  const ref = useRef<ViewportHandle>(null);
  const [gpuError, setGpuError] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [upright, setUpright] = useState(0);
  const [msg, setMsg] = useState<string | null>(null);
  const [err, setErr] = useState<ShownError | null>(null);
  const [busy, setBusy] = useState(false);
  const thumbsDone = useRef(new Set<string>());

  useEffect(() => {
    setUpright(0);
    setLoadError(null);
    setMsg(null);
    setErr(null);
  }, [project?.id]);

  const url = project?.splatPath ? assetUrl(project.splatPath) : null;
  const base = project?.viewRotation ?? [0, 0, 0];
  const rotation: [number, number, number] = [base[0] + upright, base[1], base[2]];

  async function exportSplat() {
    if (!project) return;
    setBusy(true);
    setErr(null);
    setMsg(null);
    try {
      const res = await api.exportSplat(project.id);
      if (__E2E__) (window as unknown as { __lastExport: unknown }).__lastExport = res;
      if (res.saved) setMsg(`Saved to ${res.path ?? "the chosen file"}`);
    } catch (e) {
      setErr(toShownError(e));
    } finally {
      setBusy(false);
    }
  }

  async function openPly() {
    setErr(null);
    setMsg(null);
    try {
      const id = await api.importSplat();
      if (id) onOpenProject(id);
    } catch (e) {
      setErr(toShownError(e));
    }
  }

  return (
    <section className="viewer-pane" aria-label="3D preview">
      {gpuError ? (
        <div className="gpu-error" role="alert">
          <p>
            Your computer's graphics driver couldn't start 3D preview. Your object is saved and you can still export
            it.
          </p>
          <p className="muted small">Details: {gpuError}</p>
        </div>
      ) : (
        <Viewport
          ref={ref}
          url={url}
          viewRotation={rotation}
          bounds={project?.bounds ?? null}
          label={project?.prompt || "your object"}
          onInitError={(detail) => {
            setGpuError(detail);
            api.reportViewerIssue(detail).catch(() => {});
          }}
          onLoadError={(m) => setLoadError(m)}
          onFirstFrame={() => {
            const p = project;
            if (!p || p.thumbPath || thumbsDone.current.has(p.id)) return;
            thumbsDone.current.add(p.id);
            ref.current
              ?.captureThumbnail()
              .then((png) => (png ? api.saveThumbnail(p.id, png) : undefined))
              .catch(() => {});
          }}
        />
      )}
      {!project && !gpuError && <p className="placeholder">Your object will appear here.</p>}
      <div className="toolbar" role="toolbar" aria-label="Object tools">
        {project && !gpuError && (
          <button type="button" onClick={() => ref.current?.resetView()}>
            Reset view
          </button>
        )}
        {project && (
          <button type="button" onClick={() => void exportSplat()} disabled={busy || !project.splatPath}>
            Export splat (.ply)
          </button>
        )}
        {project && !gpuError && (
          <button type="button" onClick={() => setUpright((u) => (u + 90) % 360)}>
            Turn upright
          </button>
        )}
        <button type="button" onClick={() => void openPly()}>
          Open 3D file…
        </button>
      </div>
      {project?.splatPath && onAdjustStarted && (
        <AdjustPanel project={project} provider={provider} jobActive={jobActive} onStarted={onAdjustStarted} />
      )}
      <div className="viewer-status">
        {msg && (
          <p className="notice selectable" role="status">
            {msg}
          </p>
        )}
        {loadError && <ErrorBox error={{ title: "Couldn't show this object", message: loadError }} />}
        {err && <ErrorBox error={err} />}
      </div>
    </section>
  );
}
