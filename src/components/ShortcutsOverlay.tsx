import { useEffect, useRef } from "react";
import { SHORTCUTS, type ShortcutGroup } from "../lib/keys";

const GROUPS: ShortcutGroup[] = ["Playback", "Navigation", "Selection", "General"];

/** Every keyboard shortcut, opened with `?`. Esc closes it (native `<dialog>`). */
export function ShortcutsOverlay({ onClose }: { onClose: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);

  useEffect(() => {
    ref.current?.showModal();
  }, []);

  return (
    <dialog ref={ref} className="settings shortcuts" onClose={onClose}>
      <h2>Keyboard shortcuts</h2>
      {GROUPS.map((g) => (
        <section key={g}>
          <h3>{g}</h3>
          <table>
            <tbody>
              {SHORTCUTS.filter((s) => s.group === g).map((s) => (
                <tr key={s.keys}>
                  <td>
                    <kbd>{s.keys}</kbd>
                  </td>
                  <td>{s.action}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </section>
      ))}
      <div className="buttons">
        <span className="spacer" />
        <button type="button" onClick={() => ref.current?.close()}>
          Close
        </button>
      </div>
    </dialog>
  );
}
