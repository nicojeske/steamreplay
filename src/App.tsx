import { useCallback, useEffect, useRef, useState } from "react";
import { Tooltip } from "radix-ui";
import { Check, FolderSearch, MousePointerClick, X } from "lucide-react";
import { commands, type DiscoveryInfo, type SessionDetail, type SessionSummary } from "./bindings";
import { unwrap } from "./lib/ipc";
import { SHORTCUTS, useShortcut } from "./lib/shortcuts";
import { checkForUpdate, type Update } from "./lib/updater";
import { toast } from "./state/toasts";
import { LIMITS, useUi } from "./state/ui";
import { useView } from "./state/view";
import { Editor } from "./components/editor/Editor";
import { Library } from "./components/library/Library";
import { SettingsDialog } from "./components/settings/SettingsDialog";
import { ShortcutsOverlay } from "./components/ShortcutsOverlay";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { UpdatePrompt } from "./components/UpdatePrompt";
import { Button } from "./components/ui/Button";
import { EmptyState, Kbd, Spinner } from "./components/ui/misc";
import { Splitter } from "./components/ui/Splitter";

/** Shown on the empty stage. */
const HINTS = ["playPause", "nextSpike", "setIn", "export", "shortcuts"] as const;

export default function App() {
  const [sessions, setSessions] = useState<SessionSummary[] | null>(null);
  const [info, setInfo] = useState<DiscoveryInfo | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [detail, setDetail] = useState<SessionDetail | null>(null);
  const [loading, setLoading] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  const [update, setUpdate] = useState<Update | null>(null);
  const libraryCollapsed = useUi((s) => s.libraryCollapsed);
  const libraryWidth = useUi((s) => s.libraryWidth);
  const inspectorWidth = useUi((s) => s.inspectorWidth);
  const timelineHeight = useUi((s) => s.timelineHeight);

  const load = useCallback(
    () =>
      Promise.all([unwrap(commands.listSessions()), unwrap(commands.getDiscoveryInfo())])
        .then(([list, discovery]) => {
          setSessions(list);
          setInfo(discovery);
          // Reopen the recording of the last run.
          setSelectedId((cur) => {
            if (cur) return cur;
            const last = useUi.getState().lastSessionId;
            return last && list.some((s) => s.id === last) ? last : null;
          });
        })
        .catch(toast.error),
    [],
  );

  const refresh = useCallback(() => {
    setLoading(true);
    void load().finally(() => setLoading(false));
  }, [load]);

  useEffect(() => {
    void load();
  }, [load]);

  // One quiet update check shortly after start; offline or rate-limited is not worth a toast.
  useEffect(() => {
    const timer = window.setTimeout(() => {
      checkForUpdate().then(
        (u) => u && setUpdate(u),
        (e: unknown) => console.warn("update check failed:", e),
      );
    }, 5000);
    return () => window.clearTimeout(timer);
  }, []);

  const select = (id: string) => {
    setSelectedId(id);
    useUi.getState().set({ lastSessionId: id });
  };

  // The view is reset before the editor of a new session mounts, so the player can seek at once.
  const shownId = useRef<string | null>(null);
  useEffect(() => {
    if (!selectedId) return;
    let cancelled = false;
    unwrap(commands.getSession(selectedId))
      .then((d) => {
        if (cancelled) return;
        if (shownId.current !== d.summary.id) {
          shownId.current = d.summary.id;
          useView.getState().reset(d.summary.durationS);
        }
        setDetail(d);
      })
      .catch((e: unknown) => !cancelled && toast.error(e));
    return () => {
      cancelled = true;
    };
  }, [selectedId, sessions]);

  const closeSettings = () => {
    setSettingsOpen(false);
    useView.getState().bumpSettings();
  };

  useShortcut("shortcuts", () => setShortcutsOpen(true));
  useShortcut("settings", () => setSettingsOpen(true));
  useShortcut("library", () => useUi.getState().set({ libraryCollapsed: !useUi.getState().libraryCollapsed }));

  const visibleDetail = detail && detail.summary.id === selectedId ? detail : null;
  const noRecordings = sessions !== null && sessions.length === 0;
  const style = {
    "--library-w": `${libraryWidth}px`,
    "--inspector-w": `${inspectorWidth}px`,
    "--timeline-h": `${timelineHeight}px`,
  } as React.CSSProperties;

  return (
    <Tooltip.Provider delayDuration={450} skipDelayDuration={200}>
      <div className="app" style={style}>
        <TopBar
          session={visibleDetail?.summary ?? null}
          loading={loading || sessions === null}
          onRefresh={refresh}
          onSettings={() => setSettingsOpen(true)}
          onShortcuts={() => setShortcutsOpen(true)}
        />

        <div className={`workspace ${libraryCollapsed || noRecordings ? "no-library" : ""}`}>
          {!libraryCollapsed && !noRecordings && (
            <>
              <Library sessions={sessions} selectedId={selectedId} onSelect={select} />
              <Splitter
                axis="x"
                label="Resize the library"
                value={libraryWidth}
                min={LIMITS.libraryWidth[0]}
                max={LIMITS.libraryWidth[1]}
                onChange={(v) => useUi.getState().set({ libraryWidth: v })}
              />
            </>
          )}
          <main className="main">
            {noRecordings ? (
              <Onboarding info={info} onChoose={() => setSettingsOpen(true)} />
            ) : visibleDetail ? (
              <Editor key={visibleDetail.summary.id} detail={visibleDetail} />
            ) : selectedId ? (
              <div className="center-fill">
                <Spinner size={22} />
              </div>
            ) : (
              <EmptyState icon={<MousePointerClick size={30} />} title="Pick a recording" className="stage-empty">
                <p className="muted">Choose a recording or clip in the library to watch it, find the loud moments and cut a clip.</p>
                <dl className="hint-keys">
                  {HINTS.map((id) => {
                    const s = SHORTCUTS.find((x) => x.id === id);
                    return (
                      s && (
                        <div key={id}>
                          <dt>
                            <Kbd>{s.keys}</Kbd>
                          </dt>
                          <dd>{s.action}</dd>
                        </div>
                      )
                    );
                  })}
                </dl>
              </EmptyState>
            )}
          </main>
        </div>

        <SettingsDialog
          open={settingsOpen}
          info={info}
          onClose={closeSettings}
          onChanged={refresh}
          onShowShortcuts={() => {
            closeSettings();
            setShortcutsOpen(true);
          }}
          onUpdate={setUpdate}
        />
        <ShortcutsOverlay open={shortcutsOpen} onClose={() => setShortcutsOpen(false)} />
        <Toasts>{update && <UpdatePrompt update={update} onLater={() => setUpdate(null)} />}</Toasts>
      </div>
    </Tooltip.Provider>
  );
}

/** No recordings found: where the app looked, and how to point it elsewhere. */
function Onboarding({ info, onChoose }: { info: DiscoveryInfo | null; onChoose: () => void }) {
  return (
    <EmptyState icon={<FolderSearch size={30} />} title="No recordings found" className="onboarding">
      <p className="muted">
        SteamReplay lists Steam's background recordings and saved clips. Turn on background recording in Steam (Settings → Game Recording), play
        a bit, then refresh.
      </p>
      <h4 className="subheading">Checked these Steam folders</h4>
      <ul className="checks">
        {info?.checkedRoots.map((c) => (
          <li key={c.path} className={c.exists ? "" : "faint"}>
            {c.exists ? <Check size={14} className="ok" /> : <X size={14} />}
            <span className="mono break">{c.path}</span>
          </li>
        ))}
        {info?.recordingsRootOverride && (
          <li>
            <span>Override:</span> <span className="mono break">{info.recordingsRootOverride}</span>
          </li>
        )}
      </ul>
      <Button variant="primary" onClick={onChoose}>
        Choose recordings folder…
      </Button>
    </EmptyState>
  );
}
