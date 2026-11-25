/**
 * Updates of the headless server (`kvistcode-server`): it checks GitHub itself and, unless started with
 * --no-auto-update, installs updates when no agent is working. This module mirrors its status for the Settings
 * sheet and the title bar, and asks it to check or install. Only the server has `/api/server-update`; the
 * desktop app's phone view never asks.
 */
import { useSyncExternalStore } from "react";
import { api, ApiError } from "./api";
import { activeBackend, handleAuthError, toastError } from "./controller";
import { isHeadless } from "./host";
import { getState } from "./store";

export type UpdatePhase = "idle" | "checking" | "downloading" | "waiting" | "restarting";

export interface ServerUpdateStatus {
  current: string;
  latest: { version: string; notes: string; url: string } | null;
  installed: string | null;
  checkedAt: number | null;
  phase: UpdatePhase;
  /** Agents the restart waits for (phase "waiting"). */
  waitingOn: number;
  error: string | null;
  canInstall: boolean;
  /** Why it can't install (and what to do), when `canInstall` is false. */
  reason: string | null;
  auto: boolean;
}

let status: ServerUpdateStatus | null = null;
/** The connection dropped during a restart: the server is coming back with the new version. */
let reconnecting = false;
const listeners = new Set<() => void>();
let timer: ReturnType<typeof setTimeout> | null = null;

function set(next: ServerUpdateStatus | null) {
  status = next;
  listeners.forEach((l) => l());
}

export function useServerUpdate(): ServerUpdateStatus | null {
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => status,
  );
}

function applies() {
  return isHeadless() && getState().session?.scope === "owner";
}

const busy = (p: UpdatePhase | undefined) => p === "checking" || p === "downloading" || p === "waiting" || p === "restarting";

/** While something is happening, follow it (only while the page is visible). */
function follow() {
  if (timer) clearTimeout(timer);
  timer = null;
  if (!busy(status?.phase) && !reconnecting) return;
  timer = setTimeout(() => {
    timer = null;
    if (document.visibilityState === "visible") void refreshServerUpdate();
    else document.addEventListener("visibilitychange", () => void refreshServerUpdate(), { once: true });
  }, 2000);
}

/** Reads the server's update status. Quiet on failure: this is a side panel. */
export async function refreshServerUpdate(): Promise<void> {
  const b = activeBackend();
  if (!b || !applies()) return;
  try {
    const next = await api.serverUpdate(b);
    if (reconnecting && status && next.current !== status.current) {
      reconnecting = false;
    }
    if (reconnecting && next.phase === "idle") reconnecting = false;
    set(next);
  } catch (e) {
    if (handleAuthError(e)) return;
    // The server restarting into the new version drops the connection for a moment.
    if (status?.phase === "restarting" || status?.phase === "waiting" || reconnecting) reconnecting = true;
    else if (e instanceof ApiError && e.status === 404) set(null);
  }
  follow();
}

export async function checkServerUpdate(): Promise<void> {
  const b = activeBackend();
  if (!b || !applies()) return;
  try {
    set(await api.checkServerUpdate(b));
  } catch (e) {
    if (!handleAuthError(e)) toastError("Couldn't look for updates", e);
  }
  follow();
}

/** Installs the newest version; `now` restarts without waiting for working agents. */
export async function installServerUpdate(now = false): Promise<boolean> {
  const b = activeBackend();
  if (!b || !applies()) return false;
  try {
    set(await api.installServerUpdate(b, now));
    follow();
    return true;
  } catch (e) {
    if (!handleAuthError(e)) toastError("Couldn't update", e);
    void refreshServerUpdate();
    return false;
  }
}

export function isReconnecting() {
  return reconnecting;
}

/** For tests. */
export function resetServerUpdate() {
  if (timer) clearTimeout(timer);
  timer = null;
  reconnecting = false;
  set(null);
}
