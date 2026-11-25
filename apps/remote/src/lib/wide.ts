import { useSyncExternalStore } from "react";
import { isHeadless } from "./host";
import { useStore } from "./store";

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

/**
 * The desktop-app layout (components/desk): a wide window on the headless server, where the browser
 * is the only screen. Not for a read-only link, and never on the desktop app's own phone view.
 */
export function useDesk(): boolean {
  const wide = useWide();
  const viewer = useStore((s) => s.session?.scope === "viewer");
  return wide && !viewer && isHeadless();
}
