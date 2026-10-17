import { useEffect, useRef } from "react";

export type ShortcutGroup = "Playback" | "Navigation" | "Selection" | "Timeline" | "General";

type Match = (e: KeyboardEvent) => boolean;

const mods = (e: KeyboardEvent) => e.ctrlKey || e.altKey || e.metaKey;
/** A letter or symbol without modifiers. Matched by `e.key`, so other layouts (German QWERTZ) work. */
const plain = (key: string, shift = false): Match => (e) => !mods(e) && e.shiftKey === shift && e.key.toLowerCase() === key;
const ctrl = (key: string): Match => (e) =>
  (e.ctrlKey || e.metaKey) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === key;
/** Ignores Shift: `?` and `+` need it on some layouts. */
const symbol = (...keys: string[]): Match => (e) => !mods(e) && keys.includes(e.key);

const DEFS = [
  { id: "playPause", keys: "Space", action: "Play / pause", group: "Playback", match: plain(" ") },
  { id: "pause", keys: "K", action: "Pause", group: "Playback", match: plain("k") },
  { id: "playForward", keys: "L", action: "Play; again for faster (1×, 2×, 4×)", group: "Playback", match: plain("l") },
  { id: "playBackward", keys: "J", action: "Play backwards; again for faster", group: "Playback", match: plain("j") },
  { id: "back1", keys: "←", action: "Back 1 s", group: "Navigation", match: plain("arrowleft") },
  { id: "forward1", keys: "→", action: "Forward 1 s", group: "Navigation", match: plain("arrowright") },
  { id: "back5", keys: "Shift+←", action: "Back 5 s", group: "Navigation", match: plain("arrowleft", true) },
  { id: "forward5", keys: "Shift+→", action: "Forward 5 s", group: "Navigation", match: plain("arrowright", true) },
  { id: "prevFrame", keys: ",", action: "Previous frame", group: "Navigation", match: symbol(",") },
  { id: "nextFrame", keys: ".", action: "Next frame", group: "Navigation", match: symbol(".") },
  { id: "nextSpike", keys: "N", action: "Next loud moment by rank", group: "Navigation", match: plain("n") },
  { id: "prevSpike", keys: "P", action: "Previous loud moment by rank", group: "Navigation", match: plain("p") },
  { id: "nextSpikeTime", keys: "Shift+N", action: "Next loud moment in time", group: "Navigation", match: plain("n", true) },
  { id: "prevSpikeTime", keys: "Shift+P", action: "Previous loud moment in time", group: "Navigation", match: plain("p", true) },
  { id: "setIn", keys: "I", action: "Set the in point at the playhead", group: "Selection", match: plain("i") },
  { id: "setOut", keys: "O", action: "Set the out point at the playhead", group: "Selection", match: plain("o") },
  { id: "selectSpike", keys: "S", action: "Select the current loud moment", group: "Selection", match: plain("s") },
  { id: "clearSelection", keys: "Esc", action: "Clear the selection", group: "Selection", match: plain("escape") },
  { id: "loop", keys: "Ctrl+L", action: "Loop playback inside the selection", group: "Selection", match: ctrl("l") },
  { id: "export", keys: "Ctrl+E", action: "Export the selection", group: "Selection", match: ctrl("e") },
  { id: "zoomIn", keys: "+", action: "Zoom in", group: "Timeline", match: symbol("+", "=") },
  { id: "zoomOut", keys: "−", action: "Zoom out", group: "Timeline", match: symbol("-") },
  { id: "zoomFit", keys: "0", action: "Show the whole recording", group: "Timeline", match: symbol("0") },
  { id: "zoomSelection", keys: "Z", action: "Zoom to the selection", group: "Timeline", match: plain("z") },
  { id: "follow", keys: "F", action: "Follow the playhead on / off", group: "Timeline", match: plain("f") },
  { id: "library", keys: "Ctrl+B", action: "Show / hide the library", group: "General", match: ctrl("b") },
  { id: "settings", keys: "Ctrl+,", action: "Settings", group: "General", match: ctrl(",") },
  { id: "shortcuts", keys: "?", action: "Keyboard shortcuts", group: "General", match: symbol("?") },
] as const satisfies readonly { id: string; keys: string; action: string; group: ShortcutGroup; match: Match }[];

export type ShortcutId = (typeof DEFS)[number]["id"];

/** Every shortcut, for the `?` overlay and the tooltips. The handlers register with `useShortcut`. */
export const SHORTCUTS: readonly { id: ShortcutId; keys: string; action: string; group: ShortcutGroup }[] = DEFS;

export const SHORTCUT_GROUPS: ShortcutGroup[] = ["Playback", "Navigation", "Selection", "Timeline", "General"];

export function shortcutKeys(id: ShortcutId): string {
  return DEFS.find((d) => d.id === id)?.keys ?? "";
}

/** True while the user types into a form field, so shortcuts stay out of the way. */
export function isTyping(target: EventTarget | null): boolean {
  return target instanceof HTMLElement && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
}

/** True while a modal (settings, shortcuts, a popover) is open; the page's shortcuts pause. */
export function dialogOpen(): boolean {
  return document.querySelector('dialog[open], [role="dialog"][data-state="open"], [role="alertdialog"][data-state="open"]') !== null;
}

const ACTIVATE = [" ", "Enter"];
const ARROWS = ["ArrowLeft", "ArrowRight", "ArrowUp", "ArrowDown", "Home", "End", "PageUp", "PageDown"];

/**
 * Keys a focused widget handles itself: a focused slider owns the arrows, a button focused with
 * the keyboard owns Space. A tab or row that was just clicked does not, so Space still plays.
 */
function widgetOwns(target: EventTarget | null, key: string): boolean {
  if (!(target instanceof HTMLElement)) return false;
  const role = target.getAttribute("role");
  if (role === "slider" || role === "separator") return ARROWS.includes(key);
  if (!target.matches(":focus-visible")) return false;
  if (role === "tab" || role === "radio" || role === "option" || role === "menuitem" || role === "menuitemradio") {
    return ACTIVATE.includes(key) || ARROWS.includes(key);
  }
  if (target.matches("button, a[href], summary, [role=switch], [role=checkbox], [role=button]")) {
    return ACTIVATE.includes(key);
  }
  return false;
}

/** `false`: the handler did not act, the key does its usual thing. */
type Handler = (e: KeyboardEvent) => boolean | void;

const handlers = new Map<ShortcutId, Handler[]>();

function onKeyDown(e: KeyboardEvent) {
  if (e.defaultPrevented || e.isComposing || isTyping(e.target) || dialogOpen()) return;
  const def = DEFS.find((d) => d.match(e));
  if (!def || widgetOwns(e.target, e.key)) return;
  const list = handlers.get(def.id);
  const handler = list?.[list.length - 1];
  if (!handler || handler(e) === false) return;
  e.preventDefault();
  spaceHandled = e.key === " ";
}

/** A mouse-focused button would also "click" on the keyup of a Space that played the video. */
let spaceHandled = false;
function onKeyUp(e: KeyboardEvent) {
  if (e.key !== " " || !spaceHandled) return;
  spaceHandled = false;
  e.preventDefault();
}

if (typeof window !== "undefined") {
  window.addEventListener("keydown", onKeyDown);
  window.addEventListener("keyup", onKeyUp);
}

/** Runs `handler` for the shortcut while the component is mounted. The last one registered wins. */
export function useShortcut(id: ShortcutId, handler: Handler, enabled = true) {
  const ref = useRef(handler);
  useEffect(() => {
    ref.current = handler;
  });
  useEffect(() => {
    if (!enabled) return;
    const h: Handler = (e) => ref.current(e);
    handlers.set(id, [...(handlers.get(id) ?? []), h]);
    return () => {
      handlers.set(id, (handlers.get(id) ?? []).filter((x) => x !== h));
    };
  }, [id, enabled]);
}
