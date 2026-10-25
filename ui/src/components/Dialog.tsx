import { useEffect, useRef, type KeyboardEvent, type ReactNode } from "react";
import { FOCUSABLE, nextFocus } from "@/lib/focus";
import { useModal } from "@/lib/keys";

/**
 * The one shell for a modal question.
 *
 * Focus moves into it on opening and back to where it was on closing, Tab cannot leave it,
 * Escape asks the caller to close it, and the page behind it hears no shortcuts while it is
 * open. The box is never taller than the window: `body` scrolls, `actions` stay in view, so
 * the buttons that answer the question cannot be pushed off the screen by a long table.
 */
export function Dialog({
  label,
  onEscape,
  title,
  actions,
  children,
  className = "w-[min(36rem,100%)]",
  top = false,
}: {
  label: string;
  /** What Escape means here. Always the safe answer: back out, discard, do nothing. */
  onEscape: () => void;
  title: ReactNode;
  actions: ReactNode;
  children: ReactNode;
  className?: string;
  /** Near the top of the window instead of the middle, for an overlay such as the help. */
  top?: boolean;
}) {
  useModal();
  const box = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const before = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    const first = box.current?.querySelector<HTMLElement>(FOCUSABLE);
    (first ?? box.current)?.focus();
    return () => before?.focus?.();
  }, []);

  const onKeyDown = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "Escape") {
      // Handled here, so the document-level shortcut listener sees a handled event.
      e.preventDefault();
      e.stopPropagation();
      onEscape();
      return;
    }
    if (e.key !== "Tab" || !box.current) return;
    const items = [...box.current.querySelectorAll<HTMLElement>(FOCUSABLE)];
    const at = items.indexOf(document.activeElement as HTMLElement);
    const to = nextFocus(at, items.length, e.shiftKey);
    e.preventDefault();
    if (to !== null) items[to]?.focus();
    else box.current.focus();
  };

  return (
    <div className={`scrim ${top ? "items-start pt-[8vh]" : "items-center"} justify-center`} role="dialog" aria-modal="true" aria-label={label}>
      <div ref={box} tabIndex={-1} className={`dialog dialog-box ${className}`} onKeyDown={onKeyDown}>
        <header className="section-head justify-start shrink-0">{title}</header>
        <div className="dialog-body p-4 text-sm space-y-3">{children}</div>
        <div className="dialog-actions shrink-0 flex flex-wrap gap-1.5 justify-end px-4 py-3 border-t">{actions}</div>
      </div>
    </div>
  );
}
