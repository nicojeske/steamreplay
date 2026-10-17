import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";

/** "Show in folder" and "Copy path" for an exported file. */
export function PathActions({ path, onError }: { path: string; onError: (e: unknown) => void }) {
  return (
    <>
      <button type="button" onClick={() => revealItemInDir(path).catch(onError)}>
        Show in folder
      </button>
      <button type="button" onClick={() => writeText(path).catch(onError)}>
        Copy path
      </button>
    </>
  );
}
