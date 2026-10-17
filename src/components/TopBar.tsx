import { Keyboard, PanelLeftClose, PanelLeftOpen, RefreshCw, Settings } from "lucide-react";
import type { SessionSummary } from "../bindings";
import { formatDuration, formatRelativeDate } from "../lib/format";
import { useUi } from "../state/ui";
import { ExportsMenu } from "./exports/ExportsMenu";
import { IconButton } from "./ui/Button";

type Props = {
  session: SessionSummary | null;
  loading: boolean;
  onRefresh: () => void;
  onSettings: () => void;
  onShortcuts: () => void;
};

export function TopBar({ session, loading, onRefresh, onSettings, onShortcuts }: Props) {
  const collapsed = useUi((s) => s.libraryCollapsed);
  return (
    <header className="topbar">
      <IconButton
        label={collapsed ? "Show the library" : "Hide the library"}
        shortcut="library"
        tooltipSide="bottom"
        icon={collapsed ? <PanelLeftOpen size={18} /> : <PanelLeftClose size={18} />}
        onClick={() => useUi.getState().set({ libraryCollapsed: !collapsed })}
      />
      <span className="brand">
        <img src="/icon.svg" alt="" width={20} height={20} />
        SteamReplay
      </span>
      {session && (
        <span className="breadcrumb truncate">
          <span className="crumb-sep">/</span>
          <strong className="truncate">{session.gameName}</strong>
          <span className="muted">{formatRelativeDate(session.startedAt)}</span>
          <span className="faint num">{formatDuration(session.durationS, true)}</span>
        </span>
      )}
      <span className="spacer" />
      <ExportsMenu />
      <IconButton
        label="Refresh the recordings"
        tooltipSide="bottom"
        icon={<RefreshCw size={17} className={loading ? "spin" : undefined} />}
        disabled={loading}
        onClick={onRefresh}
      />
      <IconButton label="Keyboard shortcuts" shortcut="shortcuts" tooltipSide="bottom" icon={<Keyboard size={18} />} onClick={onShortcuts} />
      <IconButton label="Settings" shortcut="settings" tooltipSide="bottom" icon={<Settings size={18} />} onClick={onSettings} />
    </header>
  );
}
