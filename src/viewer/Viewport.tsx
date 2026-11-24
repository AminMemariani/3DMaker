import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";
import type { Bounds } from "../lib/ipc";
import { createViewer, type FirstFrameInfo, type Viewer } from "./viewer";
import type { Vec3T } from "./orbit";

export interface ViewportHandle {
  resetView(): void;
  captureThumbnail(): Promise<Uint8Array | null>;
}

export interface ViewportProps {
  /** Asset URL of the PLY to show; null shows an empty scene. */
  url: string | null;
  viewRotation: Vec3T;
  bounds: Bounds | null;
  /** Plain description of the object, used in the canvas label. */
  label: string;
  onFirstFrame?: (info: FirstFrameInfo) => void;
  onLoadError?: (message: string) => void;
  /** GPU could not start. */
  onInitError?: (detail: string) => void;
}

const KEY_HELP =
  "Drag to turn it, right-drag or Shift-drag to move, scroll or pinch to zoom. " +
  "Keyboard: arrow keys turn, Shift plus arrows move, plus and minus zoom, R resets the view.";

export const Viewport = forwardRef<ViewportHandle, ViewportProps>(function Viewport(props, ref) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const viewerRef = useRef<Viewer | null>(null);
  const propsRef = useRef(props);
  propsRef.current = props;
  const loadedUrl = useRef<string | null>(null);

  useImperativeHandle(ref, () => ({
    resetView: () => viewerRef.current?.resetView(),
    captureThumbnail: async () => (viewerRef.current ? viewerRef.current.captureThumbnail() : null),
  }));

  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) return;
    let viewer: Viewer;
    try {
      viewer = createViewer(canvas);
    } catch (e) {
      propsRef.current.onInitError?.(e instanceof Error ? e.message : String(e));
      return;
    }
    viewerRef.current = viewer;
    return () => {
      viewerRef.current = null;
      loadedUrl.current = null;
      viewer.destroy();
    };
  }, []);

  const rotKey = props.viewRotation.join(",");
  useEffect(() => {
    const v = viewerRef.current;
    if (!v || !props.url) return;
    if (loadedUrl.current === props.url) {
      v.setRotation(props.viewRotation);
      return;
    }
    loadedUrl.current = props.url;
    v.loadUrl(props.url, { viewRotation: props.viewRotation, bounds: props.bounds }).then(
      (info) => propsRef.current.onFirstFrame?.(info),
      (e: unknown) => {
        const msg = e instanceof Error ? e.message : String(e);
        if (msg !== "superseded") propsRef.current.onLoadError?.(msg);
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.url, rotKey]);

  return (
    <canvas
      ref={canvasRef}
      className="viewport-canvas"
      role="img"
      tabIndex={0}
      aria-label={`3D preview of ${props.label}. ${KEY_HELP}`}
    />
  );
});
