import type { ReactNode } from "react";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, FolderOpen } from "lucide-react";
import type { PreviewInfo, SessionDetail, StreamInfo } from "../../bindings";
import { BACKEND_NAMES } from "../../lib/backends";
import { formatBytes, formatDateTime, formatDuration, formatResolution } from "../../lib/format";
import { toast } from "../../state/toasts";
import { Button, IconButton } from "../ui/Button";
import { Callout } from "../ui/misc";

function videoLine(v: StreamInfo): string {
  const mbit = v.bandwidth ? `, ${(v.bandwidth / 1e6).toFixed(1)} Mbit/s` : "";
  return `${formatResolution(v.width, v.height)}, ${v.codecs}${mbit}`;
}

function audioLine(a: StreamInfo): string {
  const rate = a.sampleRate ? `${a.sampleRate / 1000} kHz` : "? kHz";
  const ch = a.channels ? `${a.channels} ch` : "? ch";
  return `${a.codecs}, ${rate}, ${ch}`;
}

/** Shows ranges of consecutive numbers: `[5,6,7,9]` → `5–7, 9`. */
function ranges(nums: number[]): string {
  const out: string[] = [];
  let i = 0;
  while (i < nums.length) {
    let j = i;
    while (j + 1 < nums.length && nums[j + 1] === nums[j] + 1) j++;
    out.push(i === j ? `${nums[i]}` : `${nums[i]}–${nums[j]}`);
    i = j + 1;
  }
  return out.join(", ");
}

function Fact({ label, children }: { label: string; children: ReactNode }) {
  return (
    <>
      <dt>{label}</dt>
      <dd>{children}</dd>
    </>
  );
}

/** Everything known about the recording on disk. */
export function InfoTab({ detail, preview }: { detail: SessionDetail; preview: PreviewInfo | null }) {
  const { summary: s, mpd, chunks } = detail;
  const copy = (text: string) =>
    writeText(text).then(
      () => toast.info("Copied"),
      (e: unknown) => toast.error(e),
    );

  return (
    <div className="info-tab">
      {chunks.missing.length > 0 && (
        <Callout tone="warn">
          {chunks.missing.length} chunk{chunks.missing.length === 1 ? " is" : "s are"} missing ({ranges(chunks.missing)}). Those parts show as hatched on
          the timeline.
        </Callout>
      )}
      {!mpd && <Callout tone="danger">Manifest: {detail.mpdError ?? "missing"}</Callout>}

      <section className="card">
        <div className="card-label">Recording</div>
        <dl className="facts">
          <Fact label="Game">{s.gameName}</Fact>
          <Fact label="Kind">{s.kind === "clip" ? "Saved clip" : "Background recording"}</Fact>
          <Fact label="Started">{formatDateTime(s.startedAt)}</Fact>
          <Fact label="Length">{formatDuration(s.durationS)}</Fact>
          <Fact label="Size">{formatBytes(s.sizeBytes)}</Fact>
          <Fact label="Markers">
            {detail.markers.length}
            {detail.markersOutside > 0 && <span className="faint"> (+{detail.markersOutside} outside the video)</span>}
          </Fact>
          <Fact label="Account">{s.accountId}</Fact>
          <Fact label="App ID">{s.appId}</Fact>
        </dl>
        <div className="card-actions">
          <Button size="sm" icon={<FolderOpen size={14} />} onClick={() => revealItemInDir(s.dir).catch(toast.error)}>
            Open folder
          </Button>
          <IconButton label="Copy folder path" size="sm" icon={<Copy size={14} />} onClick={() => void copy(s.dir)} />
        </div>
      </section>

      <section className="card">
        <div className="card-label">Streams</div>
        <dl className="facts">
          {mpd && (
            <>
              <Fact label="Video">{videoLine(mpd.video)}</Fact>
              <Fact label="Audio">{audioLine(mpd.audio)}</Fact>
              <Fact label="Segments">
                {mpd.segmentDurationS} s, startNumber {mpd.startNumber}, period start {formatDuration(mpd.periodStartS)}
              </Fact>
            </>
          )}
          <Fact label="Chunks">{chunks.count === 0 ? "none" : `${chunks.first}–${chunks.last} (${chunks.count} files)`}</Fact>
          <Fact label="Preview">
            {preview ? (
              preview.backend ? `720p preview, ${BACKEND_NAMES[preview.backend]}` : "Original video"
            ) : (
              <span className="faint">not open</span>
            )}
          </Fact>
        </dl>
      </section>

      <section className="card">
        <div className="card-label">Timeline</div>
        <dl className="facts">
          <Fact label="Video starts">{formatDuration(detail.offsetS)} after the folder timestamp</Fact>
          <Fact label="File">
            <span className="break">{detail.timelineFile ?? "none found"}</span>
          </Fact>
          {detail.timelineLeadS !== null && <Fact label="Lead">{detail.timelineLeadS} s before the folder</Fact>}
          {detail.timelineEndS !== null && <Fact label="Ends at">{formatDuration(detail.timelineEndS)}</Fact>}
        </dl>
      </section>

      <p className="faint folder-path break">{s.dir}</p>
    </div>
  );
}
