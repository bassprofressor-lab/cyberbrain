/**
 * Where Tab goes inside a dialog. Pure, so the wrap-around can be tested without a browser.
 *
 * `index` is the position of the focused element among the dialog's focusable ones, or -1
 * when focus is somewhere else (outside the dialog, or on the dialog box itself). Returns the
 * position to focus next, or null when there is nothing to focus at all.
 */
export function nextFocus(index: number, count: number, backwards: boolean): number | null {
  if (count <= 0) return null;
  if (index < 0) return backwards ? count - 1 : 0;
  if (backwards) return index === 0 ? count - 1 : index - 1;
  return index === count - 1 ? 0 : index + 1;
}

export const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
