import { SHORTCUTS, SHORTCUT_GROUPS } from "../lib/shortcuts";
import { Modal } from "./ui/Modal";
import { Kbd } from "./ui/misc";

/** Every keyboard shortcut, opened with `?`. */
export function ShortcutsOverlay({ open, onClose }: { open: boolean; onClose: () => void }) {
  return (
    <Modal open={open} onClose={onClose} title="Keyboard shortcuts" className="shortcuts-modal">
      <div className="shortcuts-grid">
        {SHORTCUT_GROUPS.map((g) => (
          <section key={g}>
            <h3>{g}</h3>
            <dl>
              {SHORTCUTS.filter((s) => s.group === g).map((s) => (
                <div key={s.id} className="shortcut-row">
                  <dt>{s.action}</dt>
                  <dd>
                    <Kbd>{s.keys}</Kbd>
                  </dd>
                </div>
              ))}
            </dl>
          </section>
        ))}
      </div>
    </Modal>
  );
}
