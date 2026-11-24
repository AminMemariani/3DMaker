import { useCallback, useEffect, useRef, useState } from "react";
import {
  api,
  onJobUpdate,
  onProjectsUpdate,
  type AppState,
  type ConnectResult,
  type JobView,
  type ProjectDetail,
  type ProviderId,
  type ProviderStatus,
} from "./lib/ipc";
import { isActive, isConnected, toShownError, type ShownError } from "./lib/ui";
import { ConnectScreen } from "./components/ConnectScreen";
import { CreatePanel } from "./components/CreatePanel";
import { JobPanel } from "./components/JobPanel";
import { RecentList } from "./components/RecentList";
import { SettingsDialog } from "./components/SettingsDialog";
import { ViewerPane } from "./components/ViewerPane";
import { ErrorBox } from "./components/ErrorBox";

function detailAsJob(d: ProjectDetail): JobView {
  return { projectId: d.id, state: d.state, label: "", error: d.error, cancel: null };
}

export default function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [bootError, setBootError] = useState<ShownError | null>(null);
  const [skipConnect, setSkipConnect] = useState(false);
  const [job, setJob] = useState<JobView | null>(null);
  const [jobDetail, setJobDetail] = useState<ProjectDetail | null>(null);
  const [project, setProject] = useState<ProjectDetail | null>(null);
  const [openError, setOpenError] = useState<ShownError | null>(null);
  const [settings, setSettings] = useState<{ open: boolean; replace: ProviderId | null }>({
    open: false,
    replace: null,
  });
  const handled = useRef<string>("");

  useEffect(() => {
    let alive = true;
    api.getAppState().then(
      (s) => {
        if (!alive) return;
        setState(s);
        if (s.activeJob) setJob(s.activeJob);
      },
      (e) => alive && setBootError(toShownError(e)),
    );
    const unJob = onJobUpdate((j) => setJob(j));
    const unProj = onProjectsUpdate((list) => setState((s) => (s ? { ...s, recent: list } : s)));
    return () => {
      alive = false;
      unJob.then((f) => f()).catch(() => {});
      unProj.then((f) => f()).catch(() => {});
    };
  }, []);

  // React to terminal job states: load ready objects, fetch details for failures.
  useEffect(() => {
    if (!job) return;
    const key = `${job.projectId}:${job.state}`;
    if (handled.current === key) return;
    if (job.state === "ready" || job.state === "failed" || job.state === "needsAttention") {
      handled.current = key;
      api.getProject(job.projectId).then(
        (d) => {
          if (d.state === "ready" || job.state === "ready") {
            setJobDetail(null);
            setProject(d);
          } else setJobDetail(d);
        },
        () => {},
      );
    } else if (isActive(job.state)) {
      handled.current = key;
      setJobDetail(null);
    }
  }, [job]);

  // Once the user has reached Create, disconnecting must not hide saved objects.
  useEffect(() => {
    if (state?.statuses.some(isConnected)) setSkipConnect(true);
  }, [state?.statuses]);

  const openProject = useCallback(async (id: string) => {
    setOpenError(null);
    try {
      const d = await api.getProject(id);
      if (d.state === "ready") {
        setProject(d);
        setJob((j) => (j && j.projectId === id ? null : j));
      } else if (d.state === "failed" || d.state === "needsAttention" || d.state === "cancelled") {
        setJob(detailAsJob(d));
        setJobDetail(d);
        handled.current = `${d.id}:${d.state}`;
      }
    } catch (e) {
      setOpenError(toShownError(e));
    }
  }, []);

  const updateStatus = useCallback((st: ProviderStatus) => {
    setState((s) => (s ? { ...s, statuses: [...s.statuses.filter((x) => x.id !== st.id), st] } : s));
  }, []);

  const selectProvider = useCallback((id: ProviderId) => {
    setState((s) => (s ? { ...s, selectedProvider: id } : s));
    api.selectProvider(id).catch(() => {});
  }, []);

  const onConnected = useCallback(
    (res: ConnectResult) => {
      updateStatus(res.status);
      if (isConnected(res.status)) selectProvider(res.status.id);
    },
    [updateStatus, selectProvider],
  );

  const refreshLocal = useCallback(async () => {
    try {
      const local = await api.detectProviders();
      setState((s) => (s ? { ...s, local } : s));
    } catch {
      // Detection is best-effort; the previous result stays.
    }
  }, []);

  if (bootError) {
    return (
      <main className="connect-screen">
        <h1>3D Maker couldn't start</h1>
        <ErrorBox error={bootError} />
      </main>
    );
  }
  if (!state) return <main className="loading" aria-busy="true">Loading…</main>;

  const anyConnected = state.statuses.some(isConnected);
  const selectedId =
    (state.selectedProvider && isConnected(state.statuses.find((s) => s.id === state.selectedProvider))
      ? state.selectedProvider
      : state.statuses.find(isConnected)?.id) ?? null;
  const providerInfo = state.providers.find((p) => p.id === selectedId) ?? null;
  const providerStatus = state.statuses.find((s) => s.id === selectedId) ?? null;

  if (!anyConnected && !skipConnect) {
    return (
      <ConnectScreen
        providers={state.providers}
        local={state.local}
        onConnected={onConnected}
        onRefreshLocal={refreshLocal}
        onSkip={() => setSkipConnect(true)}
        hasSaved={state.recent.length > 0}
      />
    );
  }

  const jobActive = isActive(job?.state);

  return (
    <div className="app">
      <header className="topbar">
        <h1>3D Maker</h1>
        <button
          type="button"
          className="icon"
          aria-label="Settings"
          onClick={() => setSettings({ open: true, replace: null })}
        >
          <span aria-hidden="true">⚙</span>
        </button>
      </header>
      <div className="main">
        <aside className="sidebar">
          <CreatePanel
            provider={providerInfo}
            status={providerStatus}
            jobActive={jobActive}
            onStarted={(id) =>
              setJob((j) =>
                j && j.projectId === id
                  ? j
                  : { projectId: id, state: "queued", label: "Starting", error: null, cancel: null },
              )
            }
            onConnectClick={() => setSettings({ open: true, replace: null })}
            onStatus={updateStatus}
          />
          {job && job.state !== "ready" && (
            <JobPanel
              job={job}
              detail={jobDetail && jobDetail.id === job.projectId ? jobDetail : null}
              onReplaceKey={() => setSettings({ open: true, replace: selectedId })}
            />
          )}
          {openError && <ErrorBox error={openError} />}
          <RecentList items={state.recent} currentId={project?.id ?? null} onOpen={(id) => void openProject(id)} />
        </aside>
        <ViewerPane
          project={project}
          onOpenProject={(id) => void openProject(id)}
          provider={providerInfo}
          jobActive={jobActive}
          onAdjustStarted={(id) =>
            setJob((j) => (j && j.projectId === id ? j : { projectId: id, state: "queued", label: "Starting", error: null, cancel: null }))
          }
        />
      </div>
      {settings.open && (
        <SettingsDialog
          providers={state.providers}
          statuses={state.statuses}
          local={state.local}
          selected={selectedId}
          onRefreshLocal={refreshLocal}
          initialReplace={settings.replace}
          onClose={() => setSettings({ open: false, replace: null })}
          onStatus={updateStatus}
          onConnected={onConnected}
          onSelect={selectProvider}
        />
      )}
    </div>
  );
}
