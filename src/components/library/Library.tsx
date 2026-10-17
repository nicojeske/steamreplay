import { useMemo, useRef, useState } from "react";
import { ChevronDown, Search, X } from "lucide-react";
import type { SessionSummary } from "../../bindings";
import { formatBytes, formatDuration, formatRelativeDate } from "../../lib/format";
import { useUi, type KindFilter } from "../../state/ui";
import { Segmented } from "../ui/Segmented";
import { Badge } from "../ui/misc";
import { Thumbnail } from "./Thumbnail";

type Props = {
  /** `null` while the first list loads. */
  sessions: SessionSummary[] | null;
  selectedId: string | null;
  onSelect: (id: string) => void;
};

type Group = { appId: number; game: string; sessions: SessionSummary[]; bytes: number };

/** Groups by game. Sessions arrive newest first, so groups are ordered by their newest session. */
function groupByGame(sessions: SessionSummary[]): Group[] {
  const groups = new Map<number, Group>();
  for (const s of sessions) {
    let g = groups.get(s.appId);
    if (!g) {
      g = { appId: s.appId, game: s.gameName, sessions: [], bytes: 0 };
      groups.set(s.appId, g);
    }
    g.sessions.push(s);
    g.bytes += s.sizeBytes;
  }
  return [...groups.values()];
}

const KINDS: { value: KindFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "background", label: "Recordings" },
  { value: "clip", label: "Clips" },
];

/** The recordings and clips of all accounts, grouped by game, with search and a kind filter. */
export function Library({ sessions, selectedId, onSelect }: Props) {
  const [query, setQuery] = useState("");
  const kind = useUi((s) => s.kindFilter);
  const collapsed = useUi((s) => s.collapsedGames);
  const toggleGame = useUi((s) => s.toggleGame);
  const listRef = useRef<HTMLDivElement>(null);

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const filtered = (sessions ?? []).filter(
      (s) => (kind === "all" || s.kind === kind) && (!q || s.gameName.toLowerCase().includes(q) || String(s.appId) === q),
    );
    return groupByGame(filtered);
  }, [sessions, query, kind]);

  // Searching opens every group, so matches are not hidden.
  const isOpen = (g: Group) => query.trim() !== "" || !collapsed.includes(g.appId);
  const visible = groups.flatMap((g) => (isOpen(g) ? g.sessions : []));
  const focusId = visible.some((s) => s.id === selectedId) ? selectedId : visible[0]?.id;

  /** ↑/↓/Home/End move through the visible cards and open the recording. */
  const onKeyDown = (e: React.KeyboardEvent) => {
    const keys = ["ArrowDown", "ArrowUp", "Home", "End"];
    const target = e.target;
    if (!keys.includes(e.key) || !(target instanceof HTMLElement) || !target.matches(".session-card")) return;
    e.preventDefault();
    const i = visible.findIndex((s) => s.id === target.dataset.id);
    const next =
      e.key === "Home" ? 0 : e.key === "End" ? visible.length - 1 : Math.min(Math.max(i + (e.key === "ArrowDown" ? 1 : -1), 0), visible.length - 1);
    const s = visible[next];
    if (!s) return;
    onSelect(s.id);
    listRef.current?.querySelector<HTMLElement>(`[data-id="${CSS.escape(s.id)}"]`)?.focus();
  };

  return (
    <aside className="library" aria-label="Library">
      <div className="library-controls">
        <div className="search">
          <Search size={15} aria-hidden />
          <input
            type="search"
            placeholder="Search games"
            value={query}
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => e.key === "Escape" && setQuery("")}
            aria-label="Search games"
          />
          {query && (
            <button type="button" className="search-clear" aria-label="Clear search" onClick={() => setQuery("")}>
              <X size={14} />
            </button>
          )}
        </div>
        <Segmented label="Kind" size="sm" value={kind} options={KINDS} onChange={(k) => useUi.getState().set({ kindFilter: k })} />
      </div>

      <div className="library-list" ref={listRef} role="listbox" aria-label="Recordings" onKeyDown={onKeyDown}>
        {sessions === null &&
          Array.from({ length: 6 }, (_, i) => (
            <div key={i} className="session-card skeleton" aria-hidden>
              <span className="thumb" />
              <span className="skeleton-lines">
                <span />
                <span />
              </span>
            </div>
          ))}
        {sessions !== null && groups.length === 0 && <p className="library-empty muted">No recordings match.</p>}
        {groups.map((g) => {
          const open = isOpen(g);
          return (
            <section key={g.appId} className="game-group" role="presentation">
              <button type="button" className="game-header" aria-expanded={open} onClick={() => toggleGame(g.appId)}>
                <ChevronDown size={14} className="chevron" aria-hidden />
                <span className="game-name truncate">{g.game}</span>
                <span className="game-meta">
                  {g.sessions.length} · {formatBytes(g.bytes)}
                </span>
              </button>
              {open &&
                g.sessions.map((s) => (
                  <SessionCard key={s.id} s={s} selected={s.id === selectedId} tabbable={s.id === focusId} onSelect={onSelect} />
                ))}
            </section>
          );
        })}
      </div>
    </aside>
  );
}

function SessionCard({
  s,
  selected,
  tabbable,
  onSelect,
}: {
  s: SessionSummary;
  selected: boolean;
  tabbable: boolean;
  onSelect: (id: string) => void;
}) {
  const empty = s.durationS === 0;
  return (
    <div
      className={`session-card ${selected ? "selected" : ""}`}
      role="option"
      aria-selected={selected}
      tabIndex={tabbable ? 0 : -1}
      data-id={s.id}
      onClick={() => onSelect(s.id)}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect(s.id);
        }
      }}
    >
      <Thumbnail id={s.id} empty={empty} />
      <span className="card-text">
        <span className="card-title">
          <span className="truncate">{formatRelativeDate(s.startedAt)}</span>
          {s.kind === "clip" && <Badge tone="clip">Clip</Badge>}
          {empty && <Badge tone="warn">Empty</Badge>}
        </span>
        <span className="card-meta">
          <span title="Length">{formatDuration(s.durationS, true)}</span>
          {s.markerCount > 0 && <span title="Markers">◆ {s.markerCount}</span>}
          <span title="Size on disk">{formatBytes(s.sizeBytes)}</span>
        </span>
      </span>
    </div>
  );
}
