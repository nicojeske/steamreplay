import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { commands, type Backend, type DiscoveryInfo, type FfmpegPaths, type PreviewSettings } from "../bindings";
import { formatBytes } from "../lib/format";
import { errorMessage, unwrap } from "../lib/ipc";

const BACKEND_NAMES: Record<Backend, string> = {
  cuda: "NVIDIA (CUDA / NVENC)",
  d3d11vaAmf: "AMD (D3D11VA / AMF)",
  d3d11vaQsv: "Intel (D3D11VA / Quick Sync)",
  vaapi: "VAAPI (AMD / Intel on Linux)",
  qsv: "Intel Quick Sync",
  software: "Software (CPU)",
};

type Props = {
  info: DiscoveryInfo | null;
  onClose: () => void;
  onChanged: () => void;
  onError: (e: unknown) => void;
};

export function SettingsDialog({ info, onClose, onChanged, onError }: Props) {
  const ref = useRef<HTMLDialogElement>(null);
  const [copied, setCopied] = useState(false);
  const [ffmpeg, setFfmpeg] = useState<FfmpegPaths | string | null>(null);

  useEffect(() => {
    ref.current?.showModal();
    unwrap(commands.getFfmpegInfo()).then(setFfmpeg, (e: unknown) => setFfmpeg(errorMessage(e)));
  }, []);

  const setFfmpegDir = async (path: string | null) => {
    try {
      setFfmpeg(await unwrap(commands.setFfmpegDir(path)));
    } catch (e) {
      onError(e);
    }
  };

  const pickFfmpeg = async () => {
    const dir = await open({ directory: true, title: "Choose the folder with ffmpeg and ffprobe" });
    if (typeof dir === "string") await setFfmpegDir(dir);
  };

  const setOverride = async (path: string | null) => {
    try {
      await unwrap(commands.setRecordingsRootOverride(path));
      onChanged();
    } catch (e) {
      onError(e);
    }
  };

  const pick = async () => {
    const dir = await open({ directory: true, title: "Choose the gamerecordings folder" });
    if (typeof dir === "string") await setOverride(dir);
  };

  const copyDiagnostics = async () => {
    try {
      await writeText(await unwrap(commands.getDiagnostics()));
      setCopied(true);
    } catch (e) {
      onError(e);
    }
  };

  return (
    <dialog ref={ref} className="settings" onClose={onClose}>
      <h2>Settings</h2>

      <h3>Recordings folder override</h3>
      <p className="mono">{info?.recordingsRootOverride ?? "none (automatic)"}</p>
      <div className="buttons">
        <button type="button" onClick={pick}>
          Choose folder…
        </button>
        {info?.recordingsRootOverride && (
          <button type="button" onClick={() => setOverride(null)}>
            Clear override
          </button>
        )}
      </div>

      <h3>ffmpeg</h3>
      {ffmpeg === null ? (
        <p className="muted">Checking…</p>
      ) : typeof ffmpeg === "string" ? (
        <p className="warn">{ffmpeg}</p>
      ) : (
        <p>
          <span className="mono">{ffmpeg.ffmpeg}</span> ({ffmpeg.source})
          <br />
          <span className="muted">{ffmpeg.version}</span>
        </p>
      )}
      <div className="buttons">
        <button type="button" onClick={pickFfmpeg}>
          Choose ffmpeg folder…
        </button>
        {typeof ffmpeg === "object" && ffmpeg?.source === "settings" && (
          <button type="button" onClick={() => setFfmpegDir(null)}>
            Use bundled / PATH
          </button>
        )}
      </div>

      <h3>Video preview</h3>
      <PreviewSection onError={onError} />

      <h3>Detected Steam folders</h3>
      <ul className="checks">
        {info?.checkedRoots.map((c) => (
          <li key={c.path} className={c.exists ? "ok" : "muted"}>
            {c.exists ? "✓" : "✗"} <span className="mono">{c.path}</span>
            {c.canonical && c.canonical !== c.path && <span className="mono muted"> → {c.canonical}</span>}
          </li>
        ))}
      </ul>

      <h3>Accounts</h3>
      {info?.accounts.length ? (
        <ul className="checks">
          {info.accounts.map((a) => (
            <li key={a.accountId + a.recordingsRoot}>
              {a.accountId}: <span className="mono">{a.recordingsRoot}</span>
            </li>
          ))}
        </ul>
      ) : (
        <p className="muted">No account with recordings found.</p>
      )}

      <div className="buttons">
        <button type="button" onClick={copyDiagnostics}>
          {copied ? "Copied ✓" : "Copy diagnostics"}
        </button>
        <span className="spacer" />
        <button type="button" onClick={() => ref.current?.close()}>
          Close
        </button>
      </div>
    </dialog>
  );
}
/** Preview backend and cache size. */
function PreviewSection({ onError }: { onError: (e: unknown) => void }) {
  const [settings, setSettings] = useState<PreviewSettings | null>(null);
  const [cacheMb, setCacheMb] = useState("");

  const reload = useCallback(
    () =>
      unwrap(commands.getPreviewSettings())
        .then((s) => {
          setSettings(s);
          setCacheMb(String(s.cacheMb));
        })
        .catch(onError),
    [onError],
  );

  useEffect(() => {
    void reload();
  }, [reload]);

  if (!settings) return <p className="muted">Loading…</p>;

  const save = async (backend: Backend | null, mb: number) => {
    try {
      await unwrap(commands.setPreviewSettings(backend, mb));
      await reload();
    } catch (e) {
      onError(e);
    }
  };

  const clear = async () => {
    try {
      await unwrap(commands.clearPreviewCache());
      await reload();
    } catch (e) {
      onError(e);
    }
  };

  const mb = Number.parseInt(cacheMb, 10);
  return (
    <>
      <div className="buttons">
        <label>
          Backend{" "}
          <select
            value={settings.backend ?? "auto"}
            onChange={(e) => void save(e.target.value === "auto" ? null : (e.target.value as Backend), settings.cacheMb)}
          >
            <option value="auto">Automatic{settings.detected ? ` (${BACKEND_NAMES[settings.detected]})` : ""}</option>
            {(Object.keys(BACKEND_NAMES) as Backend[]).map((b) => (
              <option key={b} value={b}>
                {BACKEND_NAMES[b]}
              </option>
            ))}
          </select>
        </label>
      </div>
      <p className="muted">Automatic tries the GPU first. Choosing Automatic again re-detects on the next preview.</p>
      <div className="buttons">
        <label>
          Cache limit{" "}
          <input type="number" min={100} step={100} value={cacheMb} onChange={(e) => setCacheMb(e.target.value)} /> MB
        </label>
        <button
          type="button"
          disabled={!Number.isFinite(mb) || mb === settings.cacheMb}
          onClick={() => void save(settings.backend, mb)}
        >
          Apply
        </button>
        <span className="muted">{formatBytes(settings.cacheBytes)} used</span>
        <button type="button" onClick={clear}>
          Clear cache
        </button>
      </div>
    </>
  );
}
