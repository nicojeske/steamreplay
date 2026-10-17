import type { ButtonHTMLAttributes, ReactNode, Ref } from "react";
import type { ShortcutId } from "../../lib/shortcuts";
import { Tip } from "./Tooltip";

type Variant = "primary" | "secondary" | "ghost" | "danger";

type ButtonProps = ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: Variant;
  size?: "sm" | "md" | "lg";
  icon?: ReactNode;
  ref?: Ref<HTMLButtonElement>;
};

export function Button({ variant = "secondary", size = "md", icon, className, children, type = "button", ...rest }: ButtonProps) {
  return (
    <button type={type} className={`btn btn-${variant} btn-${size} ${className ?? ""}`} {...rest}>
      {icon}
      {children}
    </button>
  );
}

type IconButtonProps = Omit<ButtonHTMLAttributes<HTMLButtonElement>, "children"> & {
  /** Accessible name and tooltip. */
  label: string;
  shortcut?: ShortcutId;
  icon: ReactNode;
  variant?: "ghost" | "secondary" | "primary";
  size?: "sm" | "md" | "lg";
  /** Pressed state of a toggle. */
  active?: boolean;
  tooltipSide?: "top" | "bottom" | "left" | "right";
  ref?: Ref<HTMLButtonElement>;
};

/**
 * A square icon button with a tooltip. A mouse click does not move the focus, so Space keeps
 * playing the video instead of clicking the button again (keyboard focus via Tab still works).
 */
export function IconButton({
  label,
  shortcut,
  icon,
  variant = "ghost",
  size = "md",
  active,
  tooltipSide,
  className,
  onMouseDown,
  type = "button",
  ...rest
}: IconButtonProps) {
  return (
    <Tip label={label} shortcut={shortcut} side={tooltipSide}>
      <button
        type={type}
        aria-label={label}
        aria-pressed={active}
        className={`btn btn-icon btn-${variant} btn-${size} ${active ? "is-active" : ""} ${className ?? ""}`}
        onMouseDown={(e) => {
          e.preventDefault();
          onMouseDown?.(e);
        }}
        {...rest}
      >
        {icon}
      </button>
    </Tip>
  );
}
