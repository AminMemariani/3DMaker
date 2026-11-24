import { useEffect, useState } from "react";
import type { ProjectState, ProviderStatus, UserError } from "./ipc";

export interface ShownError {
  title: string | null;
  message: string;
}

/** Normalise a rejected IPC promise (UserError-shaped object or string). */
export function toShownError(e: unknown): ShownError {
  if (typeof e === "string") return { title: null, message: e };
  if (e && typeof e === "object") {
    const o = e as Record<string, unknown>;
    if (typeof o.message === "string")
      return { title: typeof o.title === "string" ? o.title : null, message: o.message };
  }
  return { title: null, message: "Something went wrong. Please try again." };
}

export function asUserError(e: unknown): UserError | null {
  if (e && typeof e === "object" && "kind" in e && "action" in e && "message" in e) return e as UserError;
  return null;
}

export function isConnected(s: ProviderStatus | undefined | null): boolean {
  return !!s?.connected;
}

export const ACTIVE_STATES: ProjectState[] = ["queued", "designing", "building", "saving"];

export function isActive(state: ProjectState | undefined | null): boolean {
  return !!state && ACTIVE_STATES.includes(state);
}

export const STATE_LABEL: Record<ProjectState, string> = {
  queued: "Waiting",
  designing: "Designing",
  building: "Building 3D",
  saving: "Saving",
  ready: "Ready",
  failed: "Didn't finish",
  cancelled: "Cancelled",
  needsAttention: "Needs attention",
};

export function usePrefersReducedMotion(): boolean {
  const query = "(prefers-reduced-motion: reduce)";
  const get = () => typeof window !== "undefined" && !!window.matchMedia?.(query).matches;
  const [reduced, setReduced] = useState(get);
  useEffect(() => {
    const mq = window.matchMedia?.(query);
    if (!mq) return;
    const fn = () => setReduced(mq.matches);
    mq.addEventListener?.("change", fn);
    return () => mq.removeEventListener?.("change", fn);
  }, []);
  return reduced;
}
