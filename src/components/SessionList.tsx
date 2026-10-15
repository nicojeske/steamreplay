import type { SessionSummary } from "../bindings";
import { formatBytes, formatDateTime, formatDuration } from "../lib/format";

type Props = {
  sessions: SessionSummary[];
  selectedId: string | null;
  onSelect: (id: string) => void;
};

/** Groups by game. Sessions arrive newest first, so groups are ordered by their newest session. */
function groupByGame(sessions: SessionSummary[]): [string, SessionSummary[]][] {
  const groups = new Map<string, SessionSummary[]>();
  for (const s of sessions) {
    const key = `${s.appId}`;
    const list = groups.get(key);
    if (list) list.push(s);
    else groups.set(key, [s]);
  }
  return [...groups.values()].map((list) => [list[0].gameName, list]);
}

export function SessionList({ sessions, selectedId, onSelect }: Props) {
  return (
    <div className="session-list">
      {groupByGame(sessions).map(([game, list]) => (
        <section key={list[0].appId}>
          <h2 className="game-name">{game}</h2>
          <ul>
            {list.map((s) => (
              <li key={s.id}>
                <button
                  type="button"
                  className={`session-row${s.id === selectedId ? " selected" : ""}`}
                  onClick={() => onSelect(s.id)}
                >
                  <span className="row-top">
                    <span className={`badge ${s.kind}`}>{s.kind === "clip" ? "Clip" : "Background"}</span>
                    {s.durationS === 0 && <span className="badge empty">empty</span>}
                    <span className="row-date">{formatDateTime(s.startedAt)}</span>
                  </span>
                  <span className="row-meta">
                    <span title="Length">{formatDuration(s.durationS)}</span>
                    <span title="Markers">◆ {s.markerCount}</span>
                    <span title="Size on disk">{formatBytes(s.sizeBytes)}</span>
                  </span>
                </button>
              </li>
            ))}
          </ul>
        </section>
      ))}
    </div>
  );
}
