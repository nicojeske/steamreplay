import { Tabs } from "radix-ui";
import type { PreviewInfo, SessionDetail } from "../../bindings";
import { useView } from "../../state/view";
import { useUi, type InspectorTab } from "../../state/ui";
import type { Analysis } from "../editor/useAnalysis";
import { ExportTab } from "./ExportTab";
import { InfoTab } from "./InfoTab";
import { MomentsTab } from "./MomentsTab";

type Props = { detail: SessionDetail; analysis: Analysis | null; preview: PreviewInfo | null };

/**
 * Moments, Export and Info. All tabs stay mounted (hidden when inactive), so their shortcuts
 * work from anywhere and the export form keeps its state.
 */
export function Inspector({ detail, analysis, preview }: Props) {
  const tab = useUi((s) => s.inspectorTab);
  const hasSelection = useView((s) => s.inS !== null);
  const momentCount = (analysis?.spikes.length ?? 0) + detail.markers.length;
  // A recording without media only has the Info tab.
  const value: InspectorTab = analysis ? tab : "info";

  return (
    <Tabs.Root className="inspector" value={value} onValueChange={(v) => useUi.getState().set({ inspectorTab: v as InspectorTab })}>
      <Tabs.List className="tabs-list" aria-label="Inspector">
        <Tabs.Trigger className="tab" value="moments" disabled={!analysis}>
          Moments {analysis && <span className="tab-count num">{momentCount}</span>}
        </Tabs.Trigger>
        <Tabs.Trigger className="tab" value="export" disabled={!analysis}>
          Export {hasSelection && <span className="tab-dot" aria-label="selection set" />}
        </Tabs.Trigger>
        <Tabs.Trigger className="tab" value="info">
          Info
        </Tabs.Trigger>
      </Tabs.List>
      {analysis && (
        <>
          <Tabs.Content className="tab-panel" value="moments" forceMount>
            <MomentsTab detail={detail} analysis={analysis} />
          </Tabs.Content>
          <Tabs.Content className="tab-panel" value="export" forceMount>
            <ExportTab detail={detail} />
          </Tabs.Content>
        </>
      )}
      <Tabs.Content className="tab-panel" value="info" forceMount>
        <InfoTab detail={detail} preview={preview} />
      </Tabs.Content>
    </Tabs.Root>
  );
}
