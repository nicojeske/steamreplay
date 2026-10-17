import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { commands, type FfmpegPaths } from "../../bindings";
import { errorMessage, unwrap } from "../../lib/ipc";
import { toast } from "../../state/toasts";
import { Button } from "../ui/Button";
import { Field } from "../ui/Field";
import { Callout } from "../ui/misc";

/** Which ffmpeg is used, and choosing another one. */
export function FfmpegSection() {
  const [ffmpeg, setFfmpeg] = useState<FfmpegPaths | string | null>(null);

  useEffect(() => {
    unwrap(commands.getFfmpegInfo()).then(setFfmpeg, (e: unknown) => setFfmpeg(errorMessage(e)));
  }, []);

  const setDir = async (path: string | null) => {
    try {
      setFfmpeg(await unwrap(commands.setFfmpegDir(path)));
    } catch (e) {
      toast.error(e);
    }
  };

  const pick = async () => {
    const dir = await open({ directory: true, title: "Choose the folder with ffmpeg and ffprobe" });
    if (typeof dir === "string") await setDir(dir);
  };

  return (
    <>
      {typeof ffmpeg === "string" && <Callout tone="danger">{ffmpeg}</Callout>}
      <Field
        label="ffmpeg"
        description={
          ffmpeg === null ? (
            "Checking…"
          ) : typeof ffmpeg === "string" ? (
            "Not found."
          ) : (
            <>
              <span className="mono break">{ffmpeg.ffmpeg}</span> ({ffmpeg.source})
              <br />
              {ffmpeg.version}
            </>
          )
        }
      >
        <div className="row">
          <Button onClick={() => void pick()}>Choose folder…</Button>
          {typeof ffmpeg === "object" && ffmpeg?.source === "settings" && (
            <Button variant="ghost" onClick={() => void setDir(null)}>
              Bundled / PATH
            </Button>
          )}
        </div>
      </Field>
    </>
  );
}
