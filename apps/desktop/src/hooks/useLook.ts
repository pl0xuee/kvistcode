import { sidebarGroupOf, type Look, type SidebarGroup } from "@/lib/look";
import { useUi } from "@/store/ui";

export function useLook(): Look {
  return useUi((s) => s.look);
}

/** True on the one-sidebar layout (both Clean and Terminal; false only for the old Classic layout). */
export function useMinimal(): boolean {
  return useUi((s) => s.look !== "classic");
}

/** True in the Terminal look (the conversation drawn like Claude Code in a terminal). */
export function useTerminal(): boolean {
  return useUi((s) => s.look === "terminal");
}

/**
 * The sidebar lists agents by activity (Minimal, "Activity"): ⌘1–9 and the like follow that order.
 * `sections`: how many sections the user made (no longer changes the default, which is projects).
 */
export const byActivity = (s: { look: Look; sidebarGroup: SidebarGroup | null }, sections: number): boolean =>
  s.look !== "classic" && sidebarGroupOf(s.sidebarGroup, sections) === "activity";
