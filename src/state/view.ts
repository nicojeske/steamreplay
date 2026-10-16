import { create } from "zustand";
import type { ExportModeKind } from "../bindings";

/** Smallest visible span when zooming in. */
export const MIN_SPAN_S = 2;

type ViewState = {
  durationS: number;
  playheadS: number;
  viewStartS: number;
  viewEndS: number;
  /** Index into the spike list (rank − 1). */
  selectedSpike: number | null;
  /** The preview video is playing. */
  playing: boolean;
  /** While playing, the timeline pages along with the playhead. Panning turns it off. */
  follow: boolean;
  /** Bumped by `seek`; the player jumps to `seekTarget` when it changes. */
  seekSeq: number;
  seekTarget: number;
  /** Bumped by `requestPlay`; the player starts playing when it changes. */
  playSeq: number;
  /** In/out selection for export, in video time. Both set or both null. */
  inS: number | null;
  outS: number | null;
  /** While playing, wrap from the out point back to the in point. */
  loop: boolean;
  /** Fast cuts at chunk boundaries (the timeline draws them), Precise on the exact frames. Kept across sessions. */
  exportMode: ExportModeKind;
  /** Shows the whole session and puts the playhead at 0. */
  reset: (durationS: number) => void;
  /** Moves the playhead without moving the video (used by the video itself). */
  setPlayhead: (t: number) => void;
  /** The user wants to go to `t`: moves the playhead and the video. */
  seek: (t: number) => void;
  /** Sets the visible range, clamped to the session and to at least `MIN_SPAN_S`. */
  setView: (startS: number, endS: number) => void;
  selectSpike: (index: number | null) => void;
  setPlaying: (playing: boolean) => void;
  setFollow: (follow: boolean) => void;
  requestPlay: () => void;
  /** Sets the in point; the out point defaults to the end of the session. Swaps if needed. */
  setIn: (t: number) => void;
  /** Sets the out point; the in point defaults to 0. Swaps if needed. */
  setOut: (t: number) => void;
  setSelection: (a: number, b: number) => void;
  clearSelection: () => void;
  setLoop: (loop: boolean) => void;
  setExportMode: (mode: ExportModeKind) => void;
};

function clampView(start: number, end: number, duration: number): [number, number] {
  const span = Math.min(Math.max(end - start, MIN_SPAN_S), Math.max(duration, MIN_SPAN_S));
  let s = start;
  if (s + span > duration) s = duration - span;
  if (s < 0) s = 0;
  return [s, s + span];
}

const clampTime = (t: number, duration: number) => Math.min(Math.max(t, 0), duration);

export const useView = create<ViewState>((set) => ({
  durationS: 0,
  playheadS: 0,
  viewStartS: 0,
  viewEndS: 0,
  selectedSpike: null,
  playing: false,
  follow: true,
  seekSeq: 0,
  seekTarget: 0,
  playSeq: 0,
  inS: null,
  outS: null,
  loop: false,
  exportMode: "fast",
  reset: (durationS) =>
    set({
      durationS,
      playheadS: 0,
      viewStartS: 0,
      viewEndS: durationS,
      selectedSpike: null,
      playing: false,
      follow: true,
      inS: null,
      outS: null,
      loop: false,
    }),
  setPlayhead: (t) => set((st) => ({ playheadS: clampTime(t, st.durationS) })),
  seek: (t) =>
    set((st) => {
      const target = clampTime(t, st.durationS);
      return { playheadS: target, seekTarget: target, seekSeq: st.seekSeq + 1, follow: true };
    }),
  setView: (startS, endS) =>
    set((st) => {
      const [viewStartS, viewEndS] = clampView(startS, endS, st.durationS);
      return { viewStartS, viewEndS };
    }),
  selectSpike: (selectedSpike) => set({ selectedSpike }),
  setPlaying: (playing) => set(playing ? { playing, follow: true } : { playing }),
  setFollow: (follow) => set({ follow }),
  requestPlay: () => set((st) => ({ playSeq: st.playSeq + 1 })),
  setIn: (t) => set((st) => selection(t, st.outS ?? st.durationS, st.durationS)),
  setOut: (t) => set((st) => selection(st.inS ?? 0, t, st.durationS)),
  setSelection: (a, b) => set((st) => selection(a, b, st.durationS)),
  clearSelection: () => set({ inS: null, outS: null }),
  setLoop: (loop) => set({ loop }),
  setExportMode: (exportMode) => set({ exportMode }),
}));

function selection(a: number, b: number, duration: number) {
  const [x, y] = [clampTime(a, duration), clampTime(b, duration)];
  return { inS: Math.min(x, y), outS: Math.max(x, y) };
}

/** Shows `[start, end]` with a margin of 15 % on each side. */
export function zoomToRange(startS: number, endS: number) {
  const margin = (endS - startS) * 0.15;
  useView.getState().setView(startS - margin, endS + margin);
}

/** In follow mode, pages the view so the playhead stays visible (it moves to 10 % from the left). */
export function followPlayhead() {
  const { follow, playheadS, viewStartS, viewEndS, setView } = useView.getState();
  if (!follow) return;
  const span = viewEndS - viewStartS;
  if (playheadS < viewStartS || playheadS > viewStartS + span * 0.9) {
    const start = playheadS - span * 0.1;
    setView(start, start + span);
  }
}
