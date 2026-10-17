import { useEffect, useState, type FormEvent } from "react";
import { CloudUpload, RefreshCw } from "lucide-react";
import { commands, type FireshareOptions } from "../../bindings";
import { errorMessage, unwrap } from "../../lib/ipc";
import { guessGame, isConfigured, useFireshare, type UploadRequest } from "../../state/fireshare";
import { Button, IconButton } from "../ui/Button";
import { Callout, Spinner } from "../ui/misc";
import { Modal } from "../ui/Modal";

/** Title, game and tags of an upload to Fireshare. One dialog for the whole app. */
export function UploadDialog() {
  const request = useFireshare((s) => s.request);
  const close = useFireshare((s) => s.closeUpload);
  return (
    <Modal open={request !== null} onClose={close} title="Upload to Fireshare" className="upload-modal">
      {request && <UploadForm key={`${request.gameName}|${request.defaultTitle}`} request={request} onDone={close} />}
    </Modal>
  );
}

function UploadForm({ request, onDone }: { request: UploadRequest; onDone: () => void }) {
  const config = useFireshare((s) => s.config);
  const loadOptions = useFireshare((s) => s.loadOptions);
  const openSettings = useFireshare((s) => s.openSettings);
  const [options, setOptions] = useState<FireshareOptions | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [title, setTitle] = useState(request.defaultTitle);
  const [gameId, setGameId] = useState<number | null>(null);
  const [tagIds, setTagIds] = useState<number[]>([]);
  const [reload, setReload] = useState(0);
  const configured = isConfigured(config);

  useEffect(() => {
    if (!configured) return;
    let active = true;
    Promise.all([loadOptions(reload > 0), unwrap(commands.getFireshareGame(request.gameName)).catch(() => null)])
      .then(([o, remembered]) => {
        if (!active) return;
        setOptions(o);
        setGameId(guessGame(o.games, request.gameName, remembered));
        setTagIds((ids) => ids.filter((id) => o.tags.some((t) => t.id === id)));
      })
      .catch((e: unknown) => active && setError(errorMessage(e)));
    return () => {
      active = false;
    };
  }, [configured, loadOptions, reload, request.gameName]);

  if (!configured) {
    return (
      <div className="modal-body">
        <Callout>Set up the Fireshare server and an upload token first.</Callout>
        <div className="modal-footer">
          <Button variant="primary" onClick={openSettings}>
            Open settings
          </Button>
        </div>
      </div>
    );
  }

  const submit = (e: FormEvent) => {
    e.preventDefault();
    if (!options) return;
    const folder = gameId === null ? null : (options.folderRules.find((r) => r.gameId === gameId)?.folder ?? null);
    unwrap(commands.setFireshareGame(request.gameName, gameId)).catch((err: unknown) => console.warn("fireshare game", err));
    request.onSubmit({ title: title.trim() || null, gameId, tagIds, folder });
    onDone();
  };

  const toggleTag = (id: number) => setTagIds((ids) => (ids.includes(id) ? ids.filter((t) => t !== id) : [...ids, id]));

  return (
    <form className="modal-body upload-form" onSubmit={submit}>
      {error && (
        <Callout tone="danger">
          {error}{" "}
          <button type="button" className="link-button" onClick={openSettings}>
            Settings
          </button>
        </Callout>
      )}
      <label className="upload-field">
        <span>Title</span>
        <input className="input" value={title} onChange={(e) => setTitle(e.target.value)} placeholder="The file name" autoFocus />
      </label>
      <div className="upload-field">
        <span className="row">
          Game
          <span className="spacer" />
          <IconButton
            label="Reload games and tags"
            size="sm"
            icon={<RefreshCw size={14} />}
            onClick={() => {
              setOptions(null);
              setError(null);
              setReload((n) => n + 1);
            }}
          />
        </span>
        {options ? (
          <select
            className="select"
            value={gameId ?? ""}
            onChange={(e) => setGameId(e.target.value ? Number(e.target.value) : null)}
            aria-label="Game"
          >
            <option value="">No game</option>
            {options.games.map((g) => (
              <option key={g.id} value={g.id}>
                {g.name}
              </option>
            ))}
          </select>
        ) : (
          !error && <Spinner />
        )}
        {options && gameId === null && (
          <span className="muted hint-text">Missing a game? Add it in Fireshare, then reload.</span>
        )}
      </div>
      {options && options.tags.length > 0 && (
        <div className="upload-field">
          <span>Tags</span>
          <div className="tag-chips">
            {options.tags.map((t) => (
              <button
                key={t.id}
                type="button"
                className={`tag-chip ${tagIds.includes(t.id) ? "is-on" : ""}`}
                style={{ "--tag": t.color ?? "var(--accent)" } as React.CSSProperties}
                aria-pressed={tagIds.includes(t.id)}
                onClick={() => toggleTag(t.id)}
              >
                {t.name}
              </button>
            ))}
          </div>
        </div>
      )}
      <div className="modal-footer">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="primary" type="submit" icon={<CloudUpload size={16} />} disabled={!options}>
          {request.submitLabel}
        </Button>
      </div>
    </form>
  );
}
