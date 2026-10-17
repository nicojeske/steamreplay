import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useRef, useState } from "react";
import { commands, type ConnectInfo } from "../../bindings";
import { errorMessage, IpcError, unwrap } from "../../lib/ipc";
import { isConfigured, useFireshare } from "../../state/fireshare";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Callout } from "../ui/misc";

type Check = { ok: true; user: string } | { ok: false; message: string } | "checking" | null;

/** One wait at a time; a new Connect cancels the previous one in Rust. */
const CONNECT_JOB = "fireshare-connect";

/** The Fireshare server, connected through the browser (M12) or with a pasted upload token. */
export function FireshareSection() {
  const config = useFireshare((s) => s.config);
  const setConfig = useFireshare((s) => s.setConfig);
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [check, setCheck] = useState<Check>(null);
  const [saving, setSaving] = useState(false);
  const [connecting, setConnecting] = useState<ConnectInfo | "starting" | null>(null);
  const waiting = useRef(false);

  const test = async () => {
    setCheck("checking");
    try {
      setCheck({ ok: true, user: await unwrap(commands.fireshareCheck()) });
    } catch (e) {
      setCheck({ ok: false, message: errorMessage(e) });
    }
  };

  useEffect(() => {
    useFireshare
      .getState()
      .loadConfig()
      .then((c) => {
        setUrl(c.url ?? "");
        if (isConfigured(c)) void test();
      }, toast.error);
    // Closing the settings ends a wait for the browser.
    return () => {
      if (waiting.current) void commands.cancelJob(CONNECT_JOB);
    };
  }, []);

  const connect = async () => {
    setConnecting("starting");
    setCheck(null);
    try {
      const info = await unwrap(commands.fireshareConnectStart(url));
      setConnecting(info);
      await openUrl(info.verificationUrl).catch(toast.error);
      waiting.current = true;
      const user = await unwrap(commands.fireshareConnectWait(CONNECT_JOB));
      const c = await useFireshare.getState().loadConfig();
      setConfig(c);
      setUrl(c.url ?? "");
      setCheck({ ok: true, user });
    } catch (e) {
      if (!(e instanceof IpcError && e.kind === "cancelled")) setCheck({ ok: false, message: errorMessage(e) });
      // The URL was stored by the start.
      await useFireshare.getState().loadConfig().catch(() => undefined);
    } finally {
      waiting.current = false;
      setConnecting(null);
    }
  };

  const disconnect = async () => {
    try {
      setConfig(await unwrap(commands.fireshareDisconnect()));
      setCheck(null);
    } catch (e) {
      toast.error(e);
    }
  };

  const save = async () => {
    setSaving(true);
    try {
      // An empty token field keeps the stored token.
      const c = await unwrap(commands.setFireshareConfig(url.trim() || null, token.trim() ? token : null));
      setConfig(c);
      setUrl(c.url ?? "");
      setToken("");
      if (isConfigured(c)) await test();
      else setCheck(null);
    } catch (e) {
      toast.error(e);
    } finally {
      setSaving(false);
    }
  };

  const connected = isConfigured(config);
  const urlChanged = url.trim() !== (config?.url ?? "");
  const dirty = urlChanged || token.trim() !== "";

  return (
    <>
      <Field
        label="Server"
        description="The address of your Fireshare, e.g. https://clips.example.com."
        stacked
      >
        <input
          className="input wide-input"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          placeholder="https://"
          spellCheck={false}
          aria-label="Fireshare server URL"
          disabled={connecting !== null}
        />
      </Field>
      <div className="row settings-actions">
        {(!connected || urlChanged) && (
          <Button variant="primary" onClick={() => void connect()} disabled={!url.trim() || connecting !== null}>
            Connect
          </Button>
        )}
        {connected && !urlChanged && (
          <>
            <Button onClick={() => void test()} disabled={check === "checking" || connecting !== null}>
              Test connection
            </Button>
            <Button variant="ghost" onClick={() => void disconnect()} disabled={connecting !== null}>
              Disconnect
            </Button>
          </>
        )}
      </div>
      {connecting === "starting" && <p className="muted">Contacting the server…</p>}
      {connecting && connecting !== "starting" && (
        <Callout>
          <p>
            Approve SteamReplay in your browser. Code: <strong className="mono">{connecting.userCode}</strong>
          </p>
          <div className="row">
            <Button size="sm" onClick={() => void openUrl(connecting.verificationUrl).catch(toast.error)}>
              Open again
            </Button>
            <Button size="sm" variant="ghost" onClick={() => void commands.cancelJob(CONNECT_JOB)}>
              Cancel
            </Button>
          </div>
        </Callout>
      )}
      {check === "checking" && <p className="muted">Connecting…</p>}
      {check && check !== "checking" && (
        <Callout tone={check.ok ? "info" : "danger"}>{check.ok ? `Connected as ${check.user}.` : check.message}</Callout>
      )}
      <details className="settings-advanced">
        <summary>Advanced: paste an upload token</summary>
        <Field
          label="Upload token"
          description="For servers without Connect. Create one in Fireshare under Settings → Security → Upload tokens."
          stacked
        >
          <div className="row">
            <input
              className="input wide-input"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder={config?.hasToken ? "•••••••• saved" : "fsk_…"}
              spellCheck={false}
              autoComplete="off"
              aria-label="Fireshare upload token"
            />
          </div>
        </Field>
        <div className="row settings-actions">
          <Button onClick={() => void save()} disabled={!dirty || saving || connecting !== null}>
            Save
          </Button>
        </div>
      </details>
    </>
  );
}
