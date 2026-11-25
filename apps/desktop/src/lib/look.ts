// Kvistcode has two looks over the same features and themes, both on one layout (one sidebar, a single
// raised sheet, details on request):
//   clean:    the conversation reads like a chat app: your messages in bubbles, replies as plain text.
//   terminal: the conversation reads like Claude Code in a terminal: monospace, ● and ⎿ markers.
// "classic" (the icon rail, project tree and side inspector) is the old layout. It is no longer offered;
// a saved Classic or Minimal opens as Clean. Only a dev link (?look=classic) still shows it.
// The choice lives in localStorage (it is a view preference of this window, like the open panels) and is
// mirrored on <html>: data-look is the layout ("minimal" for both looks, which the `minimal:` Tailwind
// variant in globals.css reads) and data-chat is how the conversation is drawn ("clean" | "terminal").
// Keep LOOK_KEY in step with public/theme-init.js, which paints both before the app loads.
// Components read it with useLook / useMinimal / useTerminal (hooks/useLook.ts).

export type Look = "clean" | "terminal" | "classic";

export const LOOK_KEY = "kvistcode.look";
export const DEFAULT_LOOK: Look = "clean";

/** The looks offered in Settings. */
export const LOOKS: { id: Look; name: string; description: string }[] = [
  { id: "clean", name: "Clean", description: "Reads like a chat app: your messages in bubbles, replies as plain text." },
  {
    id: "terminal",
    name: "Terminal",
    description: "Reads like Claude Code in a terminal: monospace, with each step and its result on its own line.",
  },
];

export function isLook(v: unknown): v is Look {
  return v === "clean" || v === "terminal" || v === "classic";
}

/** The saved look: Terminal, else Clean (a saved Classic or Minimal, from before there were these two, too). */
export function lookOf(saved: unknown): Look {
  return saved === "terminal" ? "terminal" : DEFAULT_LOOK;
}

export function readLook(): Look {
  try {
    return lookOf(localStorage.getItem(LOOK_KEY));
  } catch {
    return DEFAULT_LOOK;
  }
}

export function saveLook(look: Look) {
  try {
    localStorage.setItem(LOOK_KEY, look);
  } catch {
    /* ignore */
  }
}

/** Put the look on <html> (what CSS reads): the layout in data-look, the conversation style in data-chat. */
export function applyLook(look: Look) {
  if (typeof document === "undefined") return;
  const html = document.documentElement;
  html.dataset.look = look === "classic" ? "classic" : "minimal";
  html.dataset.chat = look === "terminal" ? "terminal" : "clean";
}

/** How the sidebar lists agents: by what is happening, or under their projects (as Classic does). */
export type SidebarGroup = "activity" | "project";
const SIDEBAR_GROUP_KEY = "kvistcode.sidebarGroup";

/** The saved choice; null until one is made (see sidebarGroupOf). */
export function readSidebarGroup(): SidebarGroup | null {
  try {
    const v = localStorage.getItem(SIDEBAR_GROUP_KEY);
    return v === "project" || v === "activity" ? v : null;
  } catch {
    return null;
  }
}

/** The grouping in effect: the saved choice, else by project. (`_sections` is kept for callers; it no longer matters.) */
export function sidebarGroupOf(saved: SidebarGroup | null, _sections?: number): SidebarGroup {
  return saved ?? "project";
}

export function saveSidebarGroup(group: SidebarGroup) {
  try {
    localStorage.setItem(SIDEBAR_GROUP_KEY, group);
  } catch {
    /* ignore */
  }
}
