import { Tabs } from "radix-ui";
import { Download, Film, HardDrive, Info, Palette, Share2, Terminal, AudioWaveform } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import type { DiscoveryInfo } from "../../bindings";
import type { Update } from "../../lib/updater";
import { Modal } from "../ui/Modal";
import { AboutSection } from "./AboutSection";
import { useFireshare } from "../../state/fireshare";
import { ExportSection } from "./ExportSection";
import { FireshareSection } from "./FireshareSection";
import { FfmpegSection } from "./FfmpegSection";
import { GeneralSection } from "./GeneralSection";
import { PreviewSection } from "./PreviewSection";
import { SpikeSection } from "./SpikeSection";
import { StorageSection } from "./StorageSection";

type Props = {
  open: boolean;
  info: DiscoveryInfo | null;
  onClose: () => void;
  onChanged: () => void;
  onShowShortcuts: () => void;
  /** A newer release was found; the app shows the install prompt. */
  onUpdate: (update: Update) => void;
};

/** The settings, one section per tab in a side nav. */
export function SettingsDialog({ open, info, onClose, onChanged, onShowShortcuts, onUpdate }: Props) {
  const [section, setSection] = useState("general");
  // "Open settings" from the upload dialog.
  useEffect(
    () =>
      useFireshare.subscribe((s, prev) => {
        if (s.settingsSeq !== prev.settingsSeq) setSection("fireshare");
      }),
    [],
  );
  const sections: { id: string; label: string; icon: ReactNode; body: ReactNode }[] = [
    { id: "general", label: "General", icon: <Palette size={16} />, body: <GeneralSection info={info} onChanged={onChanged} /> },
    { id: "export", label: "Export", icon: <Download size={16} />, body: <ExportSection /> },
    { id: "fireshare", label: "Fireshare", icon: <Share2 size={16} />, body: <FireshareSection /> },
    { id: "moments", label: "Moments", icon: <AudioWaveform size={16} />, body: <SpikeSection /> },
    { id: "preview", label: "Video preview", icon: <Film size={16} />, body: <PreviewSection /> },
    { id: "ffmpeg", label: "ffmpeg", icon: <Terminal size={16} />, body: <FfmpegSection /> },
    { id: "storage", label: "Storage", icon: <HardDrive size={16} />, body: <StorageSection /> },
    {
      id: "about",
      label: "About",
      icon: <Info size={16} />,
      body: (
        <AboutSection
          info={info}
          onShowShortcuts={onShowShortcuts}
          onUpdate={(u) => {
            onUpdate(u);
            onClose();
          }}
        />
      ),
    },
  ];

  return (
    <Modal open={open} onClose={onClose} title="Settings" className="settings-modal">
      <Tabs.Root className="settings-body" orientation="vertical" value={section} onValueChange={setSection}>
        <Tabs.List className="settings-nav" aria-label="Settings sections">
          {sections.map((s) => (
            <Tabs.Trigger key={s.id} value={s.id} className="settings-nav-item">
              {s.icon}
              {s.label}
            </Tabs.Trigger>
          ))}
        </Tabs.List>
        {sections.map((s) => (
          <Tabs.Content key={s.id} value={s.id} className="settings-section">
            <h3 className="settings-section-title">{s.label}</h3>
            {s.body}
          </Tabs.Content>
        ))}
      </Tabs.Root>
    </Modal>
  );
}
