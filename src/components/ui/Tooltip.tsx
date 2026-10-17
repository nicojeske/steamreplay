import type { ReactElement, ReactNode } from "react";
import { Tooltip } from "radix-ui";
import { shortcutKeys, type ShortcutId } from "../../lib/shortcuts";
import { Kbd } from "./misc";

type Props = {
  label: ReactNode;
  /** Shown as a key hint next to the label. */
  shortcut?: ShortcutId;
  side?: "top" | "bottom" | "left" | "right";
  children: ReactElement;
};

/** A small label on hover or focus. Needs the `Tooltip.Provider` in `App`. */
export function Tip({ label, shortcut, side = "top", children }: Props) {
  return (
    <Tooltip.Root>
      <Tooltip.Trigger asChild>{children}</Tooltip.Trigger>
      <Tooltip.Portal>
        <Tooltip.Content className="tooltip" side={side} sideOffset={6} collisionPadding={8}>
          {label}
          {shortcut && <Kbd>{shortcutKeys(shortcut)}</Kbd>}
        </Tooltip.Content>
      </Tooltip.Portal>
    </Tooltip.Root>
  );
}
