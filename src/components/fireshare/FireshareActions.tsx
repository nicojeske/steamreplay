import { openUrl } from "@tauri-apps/plugin-opener";
import { CloudUpload, ExternalLink, Link } from "lucide-react";
import { copyLink, requestUpload } from "../../state/exports";
import { isConfigured, useFireshare } from "../../state/fireshare";
import { toast } from "../../state/toasts";
import { IconButton } from "../ui/Button";

type Props = { path: string; url: string | null; gameName: string; defaultTitle: string };

/** "Copy link" and "Open" once a file is on Fireshare, else "Upload to Fireshare" (when set up). */
export function FireshareActions({ path, url, gameName, defaultTitle }: Props) {
  const configured = useFireshare((s) => isConfigured(s.config));
  if (url) {
    return (
      <span className="path-actions">
        <IconButton label="Copy Fireshare link" size="sm" icon={<Link size={15} />} onClick={() => void copyLink(url)} />
        <IconButton label="Open on Fireshare" size="sm" icon={<ExternalLink size={15} />} onClick={() => void openUrl(url).catch(toast.error)} />
      </span>
    );
  }
  if (!configured) return null;
  return (
    <IconButton
      label="Upload to Fireshare"
      size="sm"
      icon={<CloudUpload size={15} />}
      onClick={() => requestUpload(path, gameName, defaultTitle)}
    />
  );
}
