/**
 * The selection snapped outwards to chunk boundaries, as the export cuts it (at least one chunk).
 * Mirrors `chunk_range` in `media/export.rs`; used for drawing only, the export reports the real cut.
 */
export function snapSelection(inS: number, outS: number, segS: number, durationS: number): [number, number] {
  const first = Math.floor(Math.max(0, inS) / segS);
  const end = Math.max(Math.ceil(outS / segS), first + 1);
  const lastEnd = Math.max(Math.ceil(durationS / segS - 1e-6), 1);
  const e = Math.min(end, lastEnd);
  const f = Math.min(first, e - 1);
  return [f * segS, e * segS];
}
