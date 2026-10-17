import { useEffect, useState } from "react";
import { commands } from "../../bindings";
import { errorMessage, unwrap } from "../../lib/ipc";
import { useFireshare } from "../../state/fireshare";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Callout } from "../ui/misc";

type Check = { ok: true; user: string } | { ok: false; message: string } | "checking" | null;

/** Server and upload token for uploads to Fireshare. */
export function FireshareSection() {
  const config = useFireshare((s) => s.config);
  const setConfig = useFireshare((s) => s.setConfig);
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [check, setCheck] = useState<Check>(null);
  const [saving, setSaving] = useState(false);

  useEffect(() => {
    useFireshare
      .getState()
      .loadConfig()
      .then((c) => setUrl(c.url ?? ""), toast.error);
  }, []);

  const test = async () => {
    setCheck("checking");
    try {
      setCheck({ ok: true, user: await unwrap(commands.fireshareCheck()) });
    } catch (e) {
      setCheck({ ok: false, message: errorMessage(e) });
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
      if (c.url && c.hasToken) await test();
      else setCheck(null);
    } catch (e) {
      toast.error(e);
    } finally {
      setSaving(false);
    }
  };

  const removeToken = async () => {
    try {
      setConfig(await unwrap(commands.setFireshareConfig(config?.url ?? null, "")));
      setCheck(null);
    } catch (e) {
      toast.error(e);
    }
  };

  const dirty = url.trim() !== (config?.url ?? "") || token.trim() !== "";

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
        />
      </Field>
      <Field
        label="Upload token"
        description="Create one in Fireshare under Account → Upload tokens. It can only upload, and you can revoke it there."
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
          {config?.hasToken && (
            <Button variant="ghost" onClick={() => void removeToken()}>
              Remove
            </Button>
          )}
        </div>
      </Field>
      <div className="row settings-actions">
        <Button variant="primary" onClick={() => void save()} disabled={!dirty || saving}>
          Save
        </Button>
        <Button onClick={() => void test()} disabled={!config?.url || !config.hasToken || dirty || check === "checking"}>
          Test connection
        </Button>
      </div>
      {check === "checking" && <p className="muted">Connecting…</p>}
      {check && check !== "checking" && (
        <Callout tone={check.ok ? "info" : "danger"}>{check.ok ? `Connected as ${check.user}.` : check.message}</Callout>
      )}
    </>
  );
}
