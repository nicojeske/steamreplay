import { create } from "zustand";

export type Theme = "system" | "dark" | "light";
export type InspectorTab = "moments" | "export" | "info";
export type KindFilter = "all" | "background" | "clip";
export type MomentsFilter = "all" | "loud" | "markers";
export type MomentsSort = "rank" | "time";

/** Layout and view preferences of this machine. Kept in `localStorage`; the defaults must work without it. */
type Prefs = {
  theme: Theme;
  libraryWidth: number;
  libraryCollapsed: boolean;
  inspectorWidth: number;
  timelineHeight: number;
  inspectorTab: InspectorTab;
  lastSessionId: string | null;
  /** App ids of the collapsed game groups in the library. */
  collapsedGames: number[];
  kindFilter: KindFilter;
  detectionOpen: boolean;
  loudnessLane: boolean;
  momentsFilter: MomentsFilter;
  momentsSort: MomentsSort;
  /** Preview volume, 0–1 on the slider (the video gets its square). */
  volume: number;
  muted: boolean;
};

type UiState = Prefs & {
  set: (patch: Partial<Prefs>) => void;
  toggleGame: (appId: number) => void;
};

export const LIMITS = {
  libraryWidth: [220, 480],
  inspectorWidth: [300, 560],
  timelineHeight: [170, 640],
} as const;

const KEY = "ui";

const DEFAULTS: Prefs = {
  theme: "system",
  libraryWidth: 300,
  libraryCollapsed: false,
  inspectorWidth: 360,
  timelineHeight: 270,
  inspectorTab: "moments",
  lastSessionId: null,
  collapsedGames: [],
  kindFilter: "all",
  detectionOpen: false,
  loudnessLane: false,
  momentsFilter: "all",
  momentsSort: "rank",
  volume: 1,
  muted: false,
};

function load(): Prefs {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) return { ...DEFAULTS, ...(JSON.parse(raw) as Partial<Prefs>) };
    // The flags of M6, so an update keeps them.
    return {
      ...DEFAULTS,
      detectionOpen: localStorage.getItem("spikeTuningOpen") === "1",
      loudnessLane: localStorage.getItem("loudnessLane") === "1",
    };
  } catch {
    return DEFAULTS;
  }
}

export const useUi = create<UiState>((set) => ({
  ...load(),
  set: (patch) => set(patch),
  toggleGame: (appId) =>
    set((s) => ({
      collapsedGames: s.collapsedGames.includes(appId)
        ? s.collapsedGames.filter((a) => a !== appId)
        : [...s.collapsedGames, appId],
    })),
}));

useUi.subscribe((s) => {
  const prefs = Object.fromEntries(Object.keys(DEFAULTS).map((k) => [k, s[k as keyof Prefs]]));
  try {
    localStorage.setItem(KEY, JSON.stringify(prefs));
  } catch {
    // Not important: the defaults come back next time.
  }
});

/** Sets `data-theme` on <html>; "system" leaves it to the media query. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  if (theme === "system") delete root.dataset.theme;
  else root.dataset.theme = theme;
}

/** Calls `redraw` when the colours may have changed (system scheme or the chosen theme). For canvases. */
export function onThemeChange(redraw: () => void): () => void {
  const media = window.matchMedia("(prefers-color-scheme: dark)");
  media.addEventListener("change", redraw);
  const unsubscribe = useUi.subscribe((s, prev) => {
    // `applyTheme` runs in its own subscriber; wait a frame so the new colours are computed.
    if (s.theme !== prev.theme) requestAnimationFrame(redraw);
  });
  return () => {
    media.removeEventListener("change", redraw);
    unsubscribe();
  };
}
