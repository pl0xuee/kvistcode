import { useEffect } from "react";
import { Button } from "@/components/ui";
import { plural, relativeTimeLong } from "@/lib/format";
import { checkServerUpdate, installServerUpdate, isReconnecting, refreshServerUpdate, useServerUpdate } from "@/lib/serverUpdate";

/**
 * "Updates" in the headless server's settings: the version, whether a newer one is out, and one button to
 * install it. The server restarts into it once no agent is working; the page reconnects by itself. Renders
 * nothing anywhere else (the desktop app updates itself).
 */
export function ServerUpdateRow() {
  const u = useServerUpdate();
  useEffect(() => {
    void refreshServerUpdate();
  }, []);
  if (!u) return null;

  const reconnecting = isReconnecting();
  const latest = u.latest?.version;
  let line: string;
  if (reconnecting || u.phase === "restarting") line = `Restarting into ${u.installed ?? latest ?? "the new version"}… this page reconnects by itself.`;
  else if (u.phase === "waiting") line = `${u.installed} is installed. Restarts when ${plural(u.waitingOn, "agent")} ${u.waitingOn === 1 ? "finishes" : "finish"}.`;
  else if (u.phase === "downloading") line = `Downloading ${latest ?? "the update"}…`;
  else if (u.phase === "checking") line = "Looking for updates…";
  else if (latest) line = `Kvistcode ${latest} is out.`;
  else line = u.checkedAt ? `Up to date. Checked ${relativeTimeLong(u.checkedAt)}.` : "Up to date.";

  const busy = u.phase !== "idle" || reconnecting;
  return (
    <section aria-label="Updates" className="py-3">
      <div className="flex items-baseline justify-between gap-4">
        <p className="text-[14px] text-muted">Kvistcode server</p>
        <p className="font-mono text-[13px] text-fg">{u.current}</p>
      </div>
      <p role="status" className="mt-1 text-[13.5px] leading-snug text-fg">
        {line}
      </p>
      {u.error && !busy && <p className="mt-1 text-[12.5px] leading-snug text-err">{u.error}</p>}
      {latest && !busy && !u.canInstall && u.reason && <p className="mt-1 text-[12.5px] leading-snug text-warn">{u.reason}</p>}
      <div className="mt-2.5 flex flex-wrap gap-2">
        {latest && u.phase === "idle" && !reconnecting && u.canInstall && (
          <Button variant="primary" size="sm" onClick={() => void installServerUpdate()}>
            Update to {latest}
          </Button>
        )}
        {u.phase === "waiting" && u.waitingOn > 0 && (
          <Button variant="secondary" size="sm" onClick={() => void installServerUpdate(true)}>
            Restart now (stops the agents)
          </Button>
        )}
        {!latest && !busy && (
          <Button variant="secondary" size="sm" onClick={() => void checkServerUpdate()}>
            Check now
          </Button>
        )}
        {latest && u.latest?.url && !busy && (
          <a href={u.latest.url} target="_blank" rel="noreferrer" className="inline-flex h-9 items-center px-1 text-[13px] text-muted underline underline-offset-2">
            What's new
          </a>
        )}
      </div>
      <p className="mt-2 text-[12.5px] leading-snug text-steel">
        {u.auto ? "Updates install by themselves when no agent is working." : "Automatic updates are off (--no-auto-update)."}
      </p>
    </section>
  );
}
