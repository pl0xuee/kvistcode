import { useSyncExternalStore } from "react";

/**
 * A computer-sized window (a desktop browser on the headless server, a laptop, a large tablet in
 * landscape): the agent list stays on the left and the open screen fills the rest.
 */
const QUERY = "(min-width: 1024px)";

function media(): MediaQueryList | null {
  return typeof window !== "undefined" && typeof window.matchMedia === "function" ? window.matchMedia(QUERY) : null;
}

function subscribe(cb: () => void) {
  const m = media();
  m?.addEventListener("change", cb);
  return () => m?.removeEventListener("change", cb);
}

export function isWide(): boolean {
  return media()?.matches ?? false;
}

export function useWide(): boolean {
  return useSyncExternalStore(subscribe, isWide, () => false);
}
