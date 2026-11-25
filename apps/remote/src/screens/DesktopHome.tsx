import { AppMark } from "@/components/Icon";
import { Button } from "@/components/ui";
import { plural } from "@/lib/format";
import { navigate } from "@/lib/router";
import { useTriage } from "@/lib/selectors";
import { useStore } from "@/lib/store";

/** The wide layout's right side before an agent is picked. */
export function DesktopHome() {
  const owner = useStore((s) => s.session?.scope === "owner");
  const offline = useStore((s) => s.connection === "offline");
  const count = useStore((s) => Object.values(s.agents).filter((a) => !a.archived).length);
  const triage = useTriage();
  return (
    <main className="flex min-h-dvh items-center justify-center px-8">
      <div className="animate-enter flex max-w-[380px] flex-col items-center text-center">
        <AppMark size={56} />
        <h1 className="mt-6 text-[22px] leading-tight font-semibold tracking-[-0.01em] text-fg">
          {count > 0 ? "Pick an agent" : "No agents yet"}
        </h1>
        <p className="mt-2 text-[14.5px] leading-relaxed text-muted">
          {count > 0
            ? `Choose one of your ${plural(count, "agent")} on the left to see its conversation, or start a new one.`
            : "Start your first agent and its conversation shows up here."}
        </p>
        <div className="mt-7 flex flex-wrap items-center justify-center gap-2.5">
          {owner && (
            <Button variant="primary" icon="plus" disabled={offline} onClick={() => navigate({ name: "new" })}>
              New agent
            </Button>
          )}
          {triage.count > 0 && (
            <Button variant="secondary" icon="inbox" onClick={() => navigate({ name: "inbox" })}>
              {`${triage.count} need${triage.count === 1 ? "s" : ""} you`}
            </Button>
          )}
        </div>
      </div>
    </main>
  );
}
