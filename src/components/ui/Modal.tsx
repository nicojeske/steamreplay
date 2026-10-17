import type { ReactNode } from "react";
import { Dialog } from "radix-ui";
import { X } from "lucide-react";
import { IconButton } from "./Button";

type Props = {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: string;
  className?: string;
  children: ReactNode;
};

/** A centred modal dialog. Esc and a click outside close it. */
export function Modal({ open, onClose, title, description, className, children }: Props) {
  return (
    <Dialog.Root open={open} onOpenChange={(o) => !o && onClose()}>
      <Dialog.Portal>
        <Dialog.Overlay className="modal-overlay" />
        <Dialog.Content
          className={`modal ${className ?? ""}`}
          // Focus the dialog itself, not the close button (its tooltip would pop up at once).
          onOpenAutoFocus={(e) => {
            e.preventDefault();
            (e.currentTarget as HTMLElement).focus();
          }}
        >
          <header className="modal-header">
            <Dialog.Title className="modal-title">{title}</Dialog.Title>
            <Dialog.Close asChild>
              <IconButton label="Close" icon={<X size={18} />} />
            </Dialog.Close>
          </header>
          <Dialog.Description className="sr-only">{description ?? (typeof title === "string" ? title : "")}</Dialog.Description>
          {children}
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}
