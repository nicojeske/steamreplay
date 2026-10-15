import { revealItemInDir } from "@tauri-apps/plugin-opener";
import type { SessionDetail, StreamInfo } from "../bindings";
import { formatBytes, formatDateTime, formatDuration, formatResolution } from "../lib/format";
import { WaveformPanel } from "./WaveformPanel";

type Props = { detail: SessionDetail; onError: (e: unknown) => void };

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

export function SessionDetails({ detail, onError }: Props) {
  const { summary: s, mpd, chunks } = detail;
  const reveal = () => revealItemInDir(s.dir).catch(onError);

  return (
    <div className="details">
      <h1>{s.gameName}</h1>
      <p className="subtitle">
        <span className={`badge ${s.kind}`}>{s.kind === "clip" ? "Clip" : "Background"}</span>
        {s.durationS === 0 && <span className="badge empty">empty</span>}
        {formatDateTime(s.startedAt)} · {formatDuration(s.durationS)} · {formatBytes(s.sizeBytes)}
      </p>

      {s.durationS > 0 && <WaveformPanel key={s.id} detail={detail} onError={onError} />}

      <details className="recording-facts">
        <summary>Recording details</summary>
      <dl className="facts">
        <dt>Folder</dt>
        <dd>
          <button type="button" className="link" onClick={reveal} title="Show in file manager">
            {s.dir}
          </button>
        </dd>
        <dt>Account</dt>
        <dd>{s.accountId}</dd>
        <dt>App ID</dt>
        <dd>{s.appId}</dd>
        {mpd ? (
          <>
            <dt>Video</dt>
            <dd>{videoLine(mpd.video)}</dd>
            <dt>Audio</dt>
            <dd>{audioLine(mpd.audio)}</dd>
            <dt>Segments</dt>
            <dd>
              {mpd.segmentDurationS} s, manifest startNumber {mpd.startNumber}, period start{" "}
              {formatDuration(mpd.periodStartS)}
            </dd>
          </>
        ) : (
          <>
            <dt>Manifest</dt>
            <dd className="warn">{detail.mpdError ?? "missing"}</dd>
          </>
        )}
        <dt>Chunks</dt>
        <dd>
          {chunks.count === 0 ? "none" : `${chunks.first}–${chunks.last} (${chunks.count} files)`}
        </dd>
        <dt>Gaps</dt>
        <dd className={chunks.missing.length ? "warn" : undefined}>
          {chunks.missing.length ? ranges(chunks.missing) : "none"}
        </dd>
        <dt>Video starts</dt>
        <dd>{formatDuration(detail.offsetS)} after the folder timestamp</dd>
        <dt>Timeline</dt>
        <dd>
          {detail.timelineFile ?? "none found"}
          {detail.timelineLeadS !== null && ` (starts ${detail.timelineLeadS} s before the folder`}
          {detail.timelineEndS !== null && `, ends at ${formatDuration(detail.timelineEndS)}`}
          {detail.timelineLeadS !== null && ")"}
        </dd>
      </dl>
      </details>

      <h2>
        Markers <span className="count">{detail.markers.length}</span>
      </h2>
      {detail.markersOutside > 0 && (
        <p className="muted">{detail.markersOutside} markers outside the buffer (before or after the video on disk)</p>
      )}
      {detail.markers.length === 0 ? (
        <p className="muted">No markers in this recording.</p>
      ) : (
        <table className="markers">
          <thead>
            <tr>
              <th>Video time</th>
              <th>Type</th>
              <th>Title</th>
              <th>Timeline time</th>
            </tr>
          </thead>
          <tbody>
            {detail.markers.map((m, i) => (
              <tr key={i}>
                <td className="mono">{formatDuration(m.videoTimeS)}</td>
                <td>{m.kind}</td>
                <td>{m.title}</td>
                <td className="mono muted">{formatDuration(m.timelineTimeS)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
}
