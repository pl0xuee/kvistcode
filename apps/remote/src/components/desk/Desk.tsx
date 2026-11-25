// The headless server's browser view on a computer screen, laid out like the Kvistcode desktop app:
// a title bar with search, the agent list on the left (Activity / Projects), and the open screen on a
// framed sheet. Only used when kvistcode-server serves the page (see `useDesk`); phones and the
// desktop app's own phone view never see it.
import { memo, useMemo, useRef, useState, useSyncExternalStore, type ReactNode } from "react";
import { AccountDot, ProviderBadge, StatusDot, StatusText } from "@/components/Identity";
import { AppMark, Icon, type IconName } from "@/components/Icon";
import { ConnectionPill } from "@/components/TopBar";
import { ConversationSearch } from "@/components/ConversationSearch";
import { backgroundActive, backgroundOf, statusWith } from "@/lib/background";
import { holdLabel } from "@/lib/budgetHold";
import { cn } from "@/lib/cn";
import { relativeTime, relativeTimeLong } from "@/lib/format";
import { useNow } from "@/lib/hooks";
import { safeColor } from "@/lib/meta";
import { resumesLabel } from "@/lib/resumes";
import { navigate, useRoute } from "@/lib/router";
import { accountOf, filterAgents, isUsageLimited, useAgentPending, useBackground, useTriage, USAGE_LIMIT_LABEL } from "@/lib/selectors";
import { storage } from "@/lib/storage";
import { groupByProject, NEED_RANK, type NeedsReason } from "@/lib/triage";
import { useStore } from "@/lib/store";
import type { Agent } from "@/lib/types";
import { SettingsSheet } from "@/screens/Fleet";

/* ---- The search text, shared by the title bar's box and the list's filter. ---- */
let query = "";
const listeners = new Set<() => void>();
function setQuery(q: string) {
  query = q;
  listeners.forEach((l) => l());
}
function useQuery(): string {
  return useSyncExternalStore(
    (l) => (listeners.add(l), () => listeners.delete(l)),
    () => query,
  );
}

const projectName = (path: string | undefined) => (path ?? "").replace(/[\\/]+$/, "").split(/[\\/]/).pop() || "No folder";

/** The 40px title bar across the top, as on the desktop app. */
export function DeskTitleBar() {
  const route = useRoute();
  const triage = useTriage();
  const q = useQuery();
  const owner = useStore((s) => s.session?.scope === "owner");
  const [settings, setSettings] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const crumb =
    route.name === "inbox" ? "Needs you" : route.name === "usage" ? "Usage" : route.name === "new" ? "New agent" : route.name === "review" ? "Review" : "Agents";
  return (
    <header className="desk-titlebar fixed inset-x-0 top-0 z-[45] flex h-10 items-center gap-2 bg-panel pr-2 pl-3">
      <button type="button" onClick={() => navigate({ name: "fleet" })} className="flex shrink-0 items-center gap-2 rounded-md">
        <AppMark size={18} />
        <span className="text-[13px] font-semibold text-fg">Kvistcode</span>
      </button>
      <span className="text-[13px] text-steel">/</span>
      <span className="min-w-0 truncate text-[13px] text-muted">{crumb}</span>
      <div className="flex flex-1 items-center justify-center gap-1">
        <DeskIcon icon="chevronLeft" label="Back" onClick={() => history.back()} />
        <DeskIcon icon="chevronRight" label="Forward" onClick={() => history.forward()} />
        <div className="relative w-full max-w-[340px]">
          <Icon name="search" size={14} className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-steel" />
          <label htmlFor="desk-search" className="sr-only">
            {owner ? "Search agents and conversations" : "Search agents"}
          </label>
          <input
            id="desk-search"
            ref={input}
            type="search"
            value={q}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setQuery("");
                input.current?.blur();
              }
            }}
            placeholder={owner ? "Search agents, history…" : "Search agents…"}
            autoComplete="off"
            className="block h-7 w-full rounded-md border border-line bg-base/60 pr-2 pl-8 text-[12.5px] text-fg outline-none placeholder:text-steel focus:border-accent/50 [&::-webkit-search-cancel-button]:appearance-none"
          />
        </div>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <ConnectionPill className="mr-1 h-6" />
        {triage.count > 0 && (
          <button
            type="button"
            onClick={() => navigate({ name: "inbox" })}
            className="inline-flex h-6 items-center gap-1.5 rounded-full border border-warn/40 bg-warn/10 px-2.5 text-[12px] font-medium text-warn hover:bg-warn/15"
          >
            <span className="h-1.5 w-1.5 rounded-full bg-warn" aria-hidden />
            {triage.count} {triage.count === 1 ? "needs" : "need"} you
          </button>
        )}
        <DeskIcon icon="sliders" label="This device" onClick={() => setSettings(true)} />
      </div>
      <SettingsSheet open={settings} onClose={() => setSettings(false)} />
    </header>
  );
}

function DeskIcon({ icon, label, onClick, active, badge }: { icon: IconName; label: string; onClick: () => void; active?: boolean; badge?: number }) {
  return (
    <button
      type="button"
      onClick={onClick}
      aria-label={label}
      title={label}
      aria-current={active ? "page" : undefined}
      className={cn(
        "relative inline-flex h-8 w-8 shrink-0 items-center justify-center rounded-lg transition-colors",
        active ? "desk-raised text-fg" : "text-steel hover:bg-raised hover:text-fg",
      )}
    >
      <Icon name={icon} size={16} />
      {!!badge && (
        <span className="absolute -top-1 -right-1 inline-flex h-4 min-w-4 items-center justify-center rounded-full bg-warn px-1 text-[10px] font-bold text-warn-fg tabular">
          {badge}
        </span>
      )}
    </button>
  );
}

type Tab = "activity" | "projects";
const TAB_KEY = "kvistcode.web.listTab";
const FOLD_KEY = "kvistcode.web.foldedProjects";

/** The agent list on the left, as on the desktop: header, Projects / Activity (projects first), filter, rows, and the view icons. */
export function DeskRail() {
  const agents = useStore((s) => s.agents);
  const stateStatus = useStore((s) => s.stateStatus);
  const owner = useStore((s) => s.session?.scope === "owner");
  const offline = useStore((s) => s.connection === "offline");
  const background = useStore((s) => s.background);
  const triage = useTriage();
  const route = useRoute();
  const now = useNow();
  const q = useQuery();
  const [tab, setTabState] = useState<Tab>(() => (storage.get(TAB_KEY) === "activity" ? "activity" : "projects"));
  const setTab = (t: Tab) => {
    setTabState(t);
    storage.set(TAB_KEY, t);
  };
  const [showArchived, setShowArchived] = useState(false);
  const [folded, setFolded] = useState<Set<string>>(() => {
    try {
      const raw: unknown = JSON.parse(storage.get(FOLD_KEY) ?? "[]");
      return new Set(Array.isArray(raw) ? raw.filter((x): x is string => typeof x === "string") : []);
    } catch {
      return new Set();
    }
  });
  const toggleFold = (path: string) =>
    setFolded((cur) => {
      const next = new Set(cur);
      if (!next.delete(path)) next.add(path);
      storage.set(FOLD_KEY, JSON.stringify([...next]));
      return next;
    });

  const list = useMemo(() => Object.values(agents), [agents]);
  const filtered = useMemo(() => filterAgents(list, q), [list, q]);
  const live = filtered.filter((a) => !a.archived);
  const archived = filtered.filter((a) => a.archived);
  // Activity: what needs you first (most urgent first), then the most recently active.
  const activity = useMemo(() => {
    const rank = (a: Agent) => {
      const r = triage.reasons.get(a.id);
      return r ? NEED_RANK[r] : 9;
    };
    return [...live].sort((a, b) => rank(a) - rank(b) || b.lastActivityAt - a.lastActivityAt);
  }, [live, triage.reasons]);
  const { projects } = useMemo(
    () => groupByProject(live, "", (a) => triage.reasons.get(a.id) ?? null, (a) => backgroundActive(backgroundOf(a, background[a.id]))),
    [live, triage.reasons, background],
  );

  const row = (a: Agent, showProject: boolean) => (
    <li key={a.id}>
      <DeskAgentRow agent={a} now={now} reason={triage.reasons.get(a.id) ?? null} showProject={showProject} />
    </li>
  );

  return (
    <div className="flex h-full flex-col">
      <div className="flex h-11 shrink-0 items-center gap-2 pr-2 pl-4">
        <span className="text-[13px] font-semibold text-fg">{tab === "projects" ? "Projects" : "Agents"}</span>
        <span className="rounded-md bg-raised px-1.5 text-[11px] leading-[18px] font-medium text-muted tabular">{tab === "projects" ? projects.length : live.length}</span>
        <span className="flex-1" />
        {owner && (
          <button
            type="button"
            disabled={offline}
            onClick={() => navigate({ name: "new" })}
            title={offline ? "Offline. Reconnect to start an agent." : "New agent"}
            className="inline-flex h-7 items-center gap-1 rounded-md bg-accent px-2.5 text-[12.5px] font-semibold text-accent-fg hover:brightness-110 disabled:opacity-50"
          >
            <Icon name="plus" size={14} strokeWidth={2.4} />
            New
          </button>
        )}
      </div>
      <div className="px-3">
        <div role="tablist" aria-label="Group agents by" className="grid grid-cols-2 gap-1 rounded-lg border border-line p-0.5">
          {(["projects", "activity"] as const).map((t) => (
            <button
              key={t}
              type="button"
              role="tab"
              aria-selected={tab === t}
              onClick={() => setTab(t)}
              className={cn("h-7 rounded-md text-[12.5px] font-medium capitalize", tab === t ? "desk-raised text-fg" : "text-muted hover:text-fg")}
            >
              {t}
            </button>
          ))}
        </div>
        <div className="relative mt-2.5">
          <Icon name="search" size={14} className="pointer-events-none absolute top-1/2 left-2.5 -translate-y-1/2 text-steel" />
          <label htmlFor="desk-filter" className="sr-only">
            Filter agents
          </label>
          <input
            id="desk-filter"
            type="search"
            value={q}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
            placeholder={tab === "projects" ? "Filter projects" : "Filter agents"}
            autoComplete="off"
            className="block h-8 w-full rounded-lg border border-line bg-base/50 pr-2 pl-8 text-[12.5px] text-fg outline-none placeholder:text-steel focus:border-accent/50 [&::-webkit-search-cancel-button]:appearance-none"
          />
        </div>
      </div>
      <nav aria-label="Agents" className="scroll-thin mt-2 min-h-0 flex-1 overflow-y-auto overscroll-contain px-2 pb-3">
        {stateStatus !== "ready" && list.length === 0 ? (
          <p className="px-2 py-6 text-center text-[12.5px] text-steel">Loading agents…</p>
        ) : list.length === 0 ? (
          <p className="px-2 py-6 text-center text-[12.5px] text-steel">No agents yet.</p>
        ) : (
          <>
            {q && !filtered.length && <p className="px-2 py-3 text-[12.5px] text-muted">No agents match "{q}".</p>}
            {tab === "activity" ? (
              <ul className="space-y-0.5">{activity.map((a) => row(a, true))}</ul>
            ) : (
              projects.map((p) => {
                const open = !folded.has(p.path) || !!q;
                return (
                  <section key={p.path || "none"} aria-label={p.name} className="mb-1">
                    <h2>
                      <button
                        type="button"
                        onClick={() => toggleFold(p.path)}
                        aria-expanded={open}
                        className="flex h-8 w-full items-center gap-1.5 rounded-md px-1 text-left hover:bg-card"
                      >
                        <Icon name="chevronDown" size={13} className={cn("shrink-0 text-steel transition-transform", !open && "-rotate-90")} />
                        <Icon name="folder" size={14} className="shrink-0 text-muted" />
                        <span className="min-w-0 truncate text-[13px] font-semibold text-fg">{p.name}</span>
                        <span className="shrink-0 text-[12px] text-steel tabular">{p.agents.length}</span>
                        {p.needs > 0 && (
                          <span className="inline-flex h-[18px] shrink-0 items-center gap-1 rounded-full border border-warn/40 bg-warn/10 px-1.5 text-[11px] font-semibold text-warn tabular">
                            <span className="h-1.5 w-1.5 rounded-full bg-warn" aria-hidden />
                            {p.needs}
                          </span>
                        )}
                      </button>
                    </h2>
                    {open && <ul className="mb-1 ml-[11px] space-y-0.5 border-l border-line/80 pl-1.5">{p.agents.map((a) => row(a, false))}</ul>}
                  </section>
                );
              })
            )}
            {archived.length > 0 && (
              <div className="mt-3">
                <button
                  type="button"
                  onClick={() => setShowArchived((v) => !v)}
                  aria-expanded={showArchived}
                  className="flex h-7 items-center gap-1 px-2 text-[11px] font-semibold tracking-[0.08em] text-steel uppercase hover:text-muted"
                >
                  Archived {archived.length}
                  <Icon name="chevronDown" size={13} className={cn("transition-transform", !showArchived && "-rotate-90")} />
                </button>
                {showArchived && <ul className="space-y-0.5">{archived.map((a) => row(a, true))}</ul>}
              </div>
            )}
            {owner && q && (
              <div className="mt-3 px-1">
                <ConversationSearch query={q} />
              </div>
            )}
          </>
        )}
      </nav>
      <div className="flex shrink-0 items-center gap-1 border-t border-line/70 px-3 py-2">
        <DeskIcon icon="bot" label="Agents" active={route.name === "fleet" || route.name === "agent" || route.name === "review"} onClick={() => navigate({ name: "fleet" })} />
        <DeskIcon icon="inbox" label="Needs you" active={route.name === "inbox"} badge={triage.count} onClick={() => navigate({ name: "inbox" })} />
        {owner && <DeskIcon icon="gauge" label="Usage" active={route.name === "usage"} onClick={() => navigate({ name: "usage" })} />}
        {owner && <DeskIcon icon="plus" label="New agent" active={route.name === "new"} onClick={() => navigate({ name: "new" })} />}
      </div>
    </div>
  );
}

/** One agent in the list, in the desktop's three lines: project and status, title, provider and branch. */
const DeskAgentRow = memo(function DeskAgentRow({
  agent,
  now,
  reason,
  showProject,
}: {
  agent: Agent;
  now: number;
  reason: NeedsReason | null;
  showProject: boolean;
}) {
  const account = useStore((s) => accountOf(s.accounts, agent));
  const pending = useAgentPending(agent);
  const limited = isUsageLimited(agent);
  const shown = statusWith(agent, useBackground(agent));
  const route = useRoute();
  const selected = (route.name === "agent" || route.name === "review") && route.agentId === agent.id;
  const needs = (reason !== null && reason !== "done") || pending > 0;
  const status = limited ? "needs_permission" : agent.budgetHold ? "needs_permission" : shown.status;
  return (
    <button
      type="button"
      onClick={() => navigate({ name: "agent", agentId: agent.id })}
      aria-current={selected ? "page" : undefined}
      className={cn("relative block w-full rounded-lg px-2.5 py-2 text-left transition-colors", selected ? "desk-raised" : "hover:bg-card")}
    >
      {selected && <span className="absolute inset-y-2 -left-1 w-[3px] rounded-full bg-accent" aria-hidden />}
      <span className="flex items-center gap-1.5 text-[11.5px] leading-4">
        {showProject && <span className="max-w-[45%] shrink-0 truncate text-steel">{projectName(agent.projectPath)}</span>}
        <StatusDot status={status} size={6} />
        {limited ? (
          <span className="min-w-0 truncate font-medium text-warn">{resumesLabel(agent, now) ?? USAGE_LIMIT_LABEL}</span>
        ) : agent.budgetHold ? (
          <span className="min-w-0 truncate font-medium text-warn">{holdLabel(agent.budgetHold)}</span>
        ) : (
          <StatusText status={shown.status} label={reason === "done" ? "Ready to review" : (shown.label ?? undefined)} className="min-w-0 truncate" />
        )}
        <span className="ml-auto shrink-0 text-steel tabular" title={relativeTimeLong(agent.lastActivityAt, now)}>
          {relativeTime(agent.lastActivityAt, now)}
        </span>
      </span>
      <span className="mt-0.5 flex items-center gap-2">
        <span className={cn("min-w-0 flex-1 truncate text-[13px] leading-5", agent.unread || needs ? "font-semibold text-fg" : "font-medium text-fg/90")}>
          {agent.title}
        </span>
        {pending > 0 ? (
          <span className="inline-flex h-4 min-w-4 shrink-0 items-center justify-center rounded-full bg-warn px-1 text-[10px] font-bold text-warn-fg tabular">
            {pending}
          </span>
        ) : (
          agent.unread && <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-accent" aria-label="Unread" />
        )}
      </span>
      <span className="mt-0.5 flex items-center gap-1.5 text-[11px] leading-4 text-steel">
        <ProviderBadge agent={agent} size={16} />
        {agent.branch && (
          <>
            <Icon name="branch" size={11} className="shrink-0" />
            <span className="min-w-0 truncate font-mono">{agent.branch}</span>
          </>
        )}
        {account && (
          <span className="ml-auto flex max-w-[40%] shrink-0 items-center gap-1">
            <AccountDot color={safeColor(account.color)} size={6} />
            <span className="truncate">{account.name}</span>
          </span>
        )}
      </span>
    </button>
  );
});

/**
 * The sheet the open screen sits on: a fixed, click-through frame whose rounded edge and the panel
 * colour around it cover whatever scrolls past (the page itself keeps the window's scroll).
 */
export function DeskFrame() {
  return <div className="desk-frame pointer-events-none fixed z-[35]" aria-hidden />;
}

export function DeskLayout({ children }: { children: ReactNode }) {
  return (
    <>
      <DeskTitleBar />
      <aside aria-label="Agents" className="fixed top-10 bottom-0 left-0 z-40 w-[var(--rail)] bg-panel">
        <DeskRail />
      </aside>
      <DeskFrame />
      <div data-pane className="desk-pane">
        {children}
      </div>
    </>
  );
}
