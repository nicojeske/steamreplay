import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { Copy, FolderOpen } from "lucide-react";
import { toast } from "../state/toasts";
import { IconButton } from "./ui/Button";

/** "Show in folder" and "Copy path" for a file. */
export function PathActions({ path }: { path: string }) {
  return (
    <span className="path-actions">
      <IconButton label="Show in folder" size="sm" icon={<FolderOpen size={15} />} onClick={() => revealItemInDir(path).catch(toast.error)} />
      <IconButton
        label="Copy path"
        size="sm"
        icon={<Copy size={15} />}
        onClick={() =>
          writeText(path).then(
            () => toast.info("Path copied"),
            (e: unknown) => toast.error(e),
          )
        }
      />
    </span>
  );
}
