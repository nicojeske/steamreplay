import { create } from "zustand";

/** Smallest visible span when zooming in. */
export const MIN_SPAN_S = 2;

type ViewState = {
  durationS: number;
  playheadS: number;
  viewStartS: number;
  viewEndS: number;
  /** Index into the spike list (rank − 1). */
  selectedSpike: number | null;
  /** Shows the whole session and puts the playhead at 0. */
  reset: (durationS: number) => void;
  setPlayhead: (t: number) => void;
  /** Sets the visible range, clamped to the session and to at least `MIN_SPAN_S`. */
  setView: (startS: number, endS: number) => void;
  selectSpike: (index: number | null) => void;
};

function clampView(start: number, end: number, duration: number): [number, number] {
  const span = Math.min(Math.max(end - start, MIN_SPAN_S), Math.max(duration, MIN_SPAN_S));
  let s = start;
  if (s + span > duration) s = duration - span;
  if (s < 0) s = 0;
  return [s, s + span];
}

export const useView = create<ViewState>((set) => ({
  durationS: 0,
  playheadS: 0,
  viewStartS: 0,
  viewEndS: 0,
  selectedSpike: null,
  reset: (durationS) => set({ durationS, playheadS: 0, viewStartS: 0, viewEndS: durationS, selectedSpike: null }),
  setPlayhead: (t) => set((st) => ({ playheadS: Math.min(Math.max(t, 0), st.durationS) })),
  setView: (startS, endS) =>
    set((st) => {
      const [viewStartS, viewEndS] = clampView(startS, endS, st.durationS);
      return { viewStartS, viewEndS };
    }),
  selectSpike: (selectedSpike) => set({ selectedSpike }),
}));

/** Shows `[start, end]` with a margin of 15 % on each side. */
export function zoomToRange(startS: number, endS: number) {
  const margin = (endS - startS) * 0.15;
  useView.getState().setView(startS - margin, endS + margin);
}
