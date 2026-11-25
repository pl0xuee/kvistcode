import { useEffect, useRef } from "react";
import { ConfirmHost } from "./components/ConfirmHost";
import { ErrorBoundary } from "./components/ErrorBoundary";
import { Toasts } from "./components/Toasts";
import { ensureTimeline } from "./lib/controller";
import { navigate, useRoute } from "./lib/router";
import { useNeedsYouCount } from "./lib/selectors";
import { useDesk } from "./lib/wide";
import { DesktopHome } from "./screens/DesktopHome";
import { DeskLayout } from "./components/desk/Desk";
import { useStore } from "./lib/store";
import { AgentDetail } from "./screens/AgentDetail";
import { Fleet } from "./screens/Fleet";
import { Inbox } from "./screens/Inbox";
import { NewAgent } from "./screens/NewAgent";
import { Review } from "./screens/Review";
import { Usage } from "./screens/Usage";
import { Blocked, Boot, Pairing } from "./screens/Pairing";

export function App() {
  const phase = useStore((s) => s.phase);
  return (
    <>
      <ErrorBoundary resetKey={phase}>
        {phase === "boot" && <Boot />}
        {phase === "unpaired" && <Pairing />}
        {phase === "blocked" && <Blocked />}
        {phase === "ready" && <Paired />}
      </ErrorBoundary>
      <ErrorBoundary>
        <Toasts />
        <ConfirmHost />
      </ErrorBoundary>
    </>
  );
}

function Paired() {
  const route = useRoute();
  const session = useStore((s) => s.session);
  useDocumentTitle();

  const viewerAgent = session?.scope === "viewer" ? session.agentId : null;

  // A read-only link only ever shows its one agent.
  useEffect(() => {
    if (viewerAgent && (route.name !== "agent" || route.agentId !== viewerAgent)) {
      navigate({ name: "agent", agentId: viewerAgent }, { replace: true });
    }
  }, [route, viewerAgent]);

  useEffect(() => {
    if (route.name === "agent") ensureTimeline(route.agentId);
  }, [route]);

  // Move focus to the new screen's heading so screen readers announce it
  // (not on the very first screen, where focus should stay at the top).
  const firstRoute = useRef(true);
  useEffect(() => {
    if (firstRoute.current) {
      firstRoute.current = false;
      return;
    }
    const raf = requestAnimationFrame(() => {
      // In the wide layout the list's header comes first; the new screen is the pane's.
      const h = document.querySelector<HTMLElement>("[data-pane] header h1") ?? document.querySelector<HTMLElement>("header h1");
      if (!h) return;
      h.tabIndex = -1;
      h.focus({ preventScroll: true });
    });
    return () => cancelAnimationFrame(raf);
  }, [route]);

  // Only on the headless server, where the browser is the only screen; the desktop app's phone
  // view stays as it was.
  const wide = useDesk();
  // Fixed overlays (toasts) center on the pane, not the whole window.
  useEffect(() => {
    document.documentElement.classList.toggle("has-rail", wide);
    return () => document.documentElement.classList.remove("has-rail");
  }, [wide]);

  if (wide) {
    return (
      <DeskLayout>
        <ErrorBoundary resetKey={routeKey(route)}>
          {route.name === "fleet" ? <DesktopHome /> : <Screen route={route} viewerAgent={null} />}
        </ErrorBoundary>
      </DeskLayout>
    );
  }

  return (
    <ErrorBoundary resetKey={viewerAgent ?? routeKey(route)}>
      <Screen route={route} viewerAgent={viewerAgent} />
    </ErrorBoundary>
  );
}

function routeKey(route: ReturnType<typeof useRoute>) {
  return route.name === "agent" || route.name === "review" ? `${route.name}:${route.agentId}` : route.name;
}

function Screen({ route, viewerAgent }: { route: ReturnType<typeof useRoute>; viewerAgent: string | null }) {
  if (viewerAgent) return <AgentDetail key={viewerAgent} agentId={viewerAgent} />;
  switch (route.name) {
    case "agent":
      return (
        <div key={route.agentId} className="animate-enter-right">
          <AgentDetail agentId={route.agentId} />
        </div>
      );
    case "review":
      return (
        <div key={`review-${route.agentId}`} className="animate-enter-right">
          <Review agentId={route.agentId} />
        </div>
      );
    case "new":
      return (
        <div key="new" className="animate-enter-right">
          <NewAgent />
        </div>
      );
    case "inbox":
      return (
        <div key="inbox" className="animate-enter-right">
          <Inbox />
        </div>
      );
    case "usage":
      return (
        <div key="usage" className="animate-enter-right">
          <Usage />
        </div>
      );
    default:
      return (
        <div key="fleet" className="animate-fade">
          <Fleet />
        </div>
      );
  }
}

function useDocumentTitle() {
  const count = useNeedsYouCount();
  useEffect(() => {
    document.title = count > 0 ? `(${count}) Kvistcode` : "Kvistcode";
  }, [count]);
}
