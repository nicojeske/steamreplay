import { useCallback, useEffect, useRef, useState } from "react";
import { commands, type DiscoveryInfo, type SessionDetail, type SessionSummary } from "./bindings";
import { errorMessage, unwrap } from "./lib/ipc";
import { SessionList } from "./components/SessionList";
import { SessionDetails } from "./components/SessionDetails";
import { SettingsDialog } from "./components/SettingsDialog";
import { ShortcutsOverlay } from "./components/ShortcutsOverlay";
import { dialogOpen, isTyping } from "./lib/keys";
import { useView } from "./state/view";
import { Toasts, type Toast } from "./components/Toasts";
import "./App.css";

export default function App() {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [info, setInfo] = useState<DiscoveryInfo | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [toasts, setToasts] = useState<Toast[]>([]);
  const nextToast = useRef(0);

  const showError = useCallback((e: unknown) => {
    const id = nextToast.current++;
    setToasts((t) => [...t, { id, text: errorMessage(e) }]);
    setTimeout(() => setToasts((t) => t.filter((x) => x.id !== id)), 8000);
  }, []);

  const load = useCallback(
    () =>
      Promise.all([unwrap(commands.listSessions()), unwrap(commands.getDiscoveryInfo())])
        .then(([list, discovery]) => {
          setSessions(list);
          setInfo(discovery);
        })
        .catch(showError),
    [showError],
  );

  const refresh = useCallback(() => {
    setLoading(true);
    void load().finally(() => setLoading(false));
  }, [load]);

  useEffect(() => {
    void load();
  }, [load]);

  // `?` shows the shortcuts. Matched by `e.key`, so Shift+ß on a German layout works too.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "?" || e.ctrlKey || e.altKey || e.metaKey || isTyping(e.target) || dialogOpen()) return;
      e.preventDefault();
      setShortcutsOpen(true);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    unwrap(commands.getSession(selectedId))
      .then((d) => !cancelled && setDetail(d))
      .catch((e: unknown) => !cancelled && showError(e));
    return () => {
      cancelled = true;
    };
  }, [selectedId, sessions, showError]);

  const visibleDetail = detail && detail.summary.id === selectedId ? detail : null;

  return (
    <div className="app">
      <header className="toolbar">
        <span className="title">SteamReplay</span>
        <span className="muted">{sessions ? `${sessions.length} recordings` : ""}</span>
        <span className="spacer" />
        <button type="button" onClick={refresh} disabled={loading || sessions === null}>
          {loading || sessions === null ? "Loading…" : "Refresh"}
        </button>
        <button type="button" onClick={() => setSettingsOpen(true)}>
          Settings
        </button>
      </header>

      {sessions && sessions.length === 0 ? (
        <main className="empty-state">
          <h1>No recordings found</h1>
          <p>Checked these Steam folders:</p>
          <ul className="checks">
            {info?.checkedRoots.map((c) => (
              <li key={c.path} className={c.exists ? "ok" : "muted"}>
                {c.exists ? "✓" : "✗"} <span className="mono">{c.path}</span>
              </li>
            ))}
            {info?.recordingsRootOverride && (
              <li>
                Override: <span className="mono">{info.recordingsRootOverride}</span>
              </li>
            )}
          </ul>
          <p className="muted">
            If Steam records to a different folder, choose it in Settings.
          </p>
          <button type="button" onClick={() => setSettingsOpen(true)}>
            Choose recordings folder…
          </button>
        </main>
      ) : (
        <main className="panes">
          <aside className="left">
            {sessions && <SessionList sessions={sessions} selectedId={selectedId} onSelect={setSelectedId} />}
          </aside>
          <section className="right">
            {visibleDetail ? (
              <SessionDetails detail={visibleDetail} onError={showError} />
            ) : (
              <p className="muted placeholder">{selectedId ? "Loading…" : "Select a recording."}</p>
            )}
          </section>
        </main>
      )}

      {settingsOpen && (
        <SettingsDialog
          info={info}
          onClose={() => {
            setSettingsOpen(false);
            useView.getState().bumpSettings();
          }}
          onChanged={refresh}
          onShowShortcuts={() => {
            setSettingsOpen(false);
            useView.getState().bumpSettings();
            setShortcutsOpen(true);
          }}
          onError={showError}
        />
      )}
      {shortcutsOpen && <ShortcutsOverlay onClose={() => setShortcutsOpen(false)} />}
      <Toasts toasts={toasts} onDismiss={(id) => setToasts((t) => t.filter((x) => x.id !== id))} />
    </div>
  );
}
