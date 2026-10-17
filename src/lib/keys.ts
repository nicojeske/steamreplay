/** True while the user types into a form field, so global shortcuts stay out of the way. */
export function isTyping(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** True while a modal dialog (settings, shortcuts) is open; the page's shortcuts pause. */
export function dialogOpen(): boolean {
  return document.querySelector("dialog[open]") !== null;
}

export type ShortcutGroup = "Playback" | "Navigation" | "Selection" | "General";

/**
 * Every keyboard shortcut, for the `?` overlay. Documentation only: the handlers stay in
 * `Player.tsx`, `SpikeList.tsx`, `ExportPanel.tsx` and `App.tsx`; keep this list in sync.
 */
export const SHORTCUTS: { keys: string; action: string; group: ShortcutGroup }[] = [
  { keys: "Space", action: "Play / pause", group: "Playback" },
  { keys: "K", action: "Pause", group: "Playback" },
  { keys: "L", action: "Play; again for faster (1×, 2×, 4×)", group: "Playback" },
  { keys: "J", action: "Play backwards; again for faster", group: "Playback" },
  { keys: "← / →", action: "Back / forward 1 s", group: "Navigation" },
  { keys: "Shift+← / Shift+→", action: "Back / forward 5 s", group: "Navigation" },
  { keys: ", / .", action: "Previous / next frame", group: "Navigation" },
  { keys: "N / P", action: "Next / previous loud moment by rank", group: "Navigation" },
  { keys: "Shift+N / Shift+P", action: "Next / previous loud moment in time", group: "Navigation" },
  { keys: "I / O", action: "Set in / out point at the playhead", group: "Selection" },
  { keys: "S", action: "Select the current loud moment for export", group: "Selection" },
  { keys: "Esc", action: "Clear the selection", group: "Selection" },
  { keys: "Ctrl+L", action: "Loop playback inside the selection", group: "Selection" },
  { keys: "?", action: "Show this list", group: "General" },
];
