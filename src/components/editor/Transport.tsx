import {
  ArrowLeftToLine,
  ArrowRightToLine,
  ChevronLeft,
  ChevronRight,
  Pause,
  Play,
  Repeat,
  Rewind,
  FastForward,
  SquarePlay,
  X,
} from "lucide-react";
import { formatTimecode } from "../../lib/format";
import { useShortcut } from "../../lib/shortcuts";
import { playSelection, useView } from "../../state/view";
import { IconButton } from "../ui/Button";
import { Tip } from "../ui/Tooltip";

type Props = {
  ready: boolean;
  playing: boolean;
  rateLabel: string;
  onTogglePlay: () => void;
  onJump: (dt: number) => void;
  onFrame: (dir: 1 | -1) => void;
  onRate: () => void;
};

/** Play controls, timecode, and the in/out selection under the video. Owns the selection keys. */
export function Transport({ ready, playing, rateLabel, onTogglePlay, onJump, onFrame, onRate }: Props) {
  const playhead = useView((s) => s.playheadS);
  const duration = useView((s) => s.durationS);
  const inS = useView((s) => s.inS);
  const outS = useView((s) => s.outS);
  const loop = useView((s) => s.loop);
  const hasSelection = inS !== null && outS !== null;

  const setIn = () => useView.getState().setIn(useView.getState().playheadS);
  const setOut = () => useView.getState().setOut(useView.getState().playheadS);
  const toggleLoop = () => useView.getState().setLoop(!useView.getState().loop);
  useShortcut("setIn", setIn);
  useShortcut("setOut", setOut);
  useShortcut("loop", toggleLoop);
  useShortcut("clearSelection", () => {
    if (useView.getState().inS === null) return false;
    useView.getState().clearSelection();
  });

  return (
    <div className="transport">
      <div className="transport-group">
        <IconButton label="Previous frame" shortcut="prevFrame" icon={<ChevronLeft size={18} />} disabled={!ready} onClick={() => onFrame(-1)} />
        <IconButton label="Back 5 s" shortcut="back5" icon={<Rewind size={17} />} disabled={!ready} onClick={() => onJump(-5)} />
        <IconButton
          label={playing ? "Pause" : "Play"}
          shortcut="playPause"
          variant="primary"
          size="lg"
          className="play-button"
          icon={playing ? <Pause size={20} fill="currentColor" /> : <Play size={20} fill="currentColor" />}
          disabled={!ready}
          onClick={onTogglePlay}
        />
        <IconButton label="Forward 5 s" shortcut="forward5" icon={<FastForward size={17} />} disabled={!ready} onClick={() => onJump(5)} />
        <IconButton label="Next frame" shortcut="nextFrame" icon={<ChevronRight size={18} />} disabled={!ready} onClick={() => onFrame(1)} />
        <span className="timecode">
          <span className="mono">{formatTimecode(playhead, 2)}</span>
          <span className="faint mono"> / {formatTimecode(duration)}</span>
        </span>
        <Tip label="Playback speed" shortcut="playForward">
          <button type="button" className="rate-pill mono" onMouseDown={(e) => e.preventDefault()} onClick={onRate} disabled={!ready}>
            {rateLabel}
          </button>
        </Tip>
      </div>

      <div className="transport-group selection-group">
        <IconButton label="Set in point" shortcut="setIn" icon={<ArrowRightToLine size={17} />} onClick={setIn} />
        <div className={`selection-readout ${hasSelection ? "" : "empty"}`}>
          {hasSelection ? (
            <>
              <span className="mono">{formatTimecode(inS, 1)}</span>
              <span className="faint">→</span>
              <span className="mono">{formatTimecode(outS, 1)}</span>
              <span className="selection-length num">{(outS - inS).toFixed(1)} s</span>
            </>
          ) : (
            <span className="faint">No selection</span>
          )}
        </div>
        <IconButton label="Set out point" shortcut="setOut" icon={<ArrowLeftToLine size={17} />} onClick={setOut} />
        <IconButton label="Loop the selection" shortcut="loop" icon={<Repeat size={16} />} active={loop} onClick={toggleLoop} />
        <IconButton label="Play the selection" icon={<SquarePlay size={17} />} disabled={!hasSelection || !ready} onClick={playSelection} />
        <IconButton
          label="Clear the selection"
          shortcut="clearSelection"
          icon={<X size={17} />}
          disabled={!hasSelection}
          onClick={() => useView.getState().clearSelection()}
        />
      </div>
    </div>
  );
}
