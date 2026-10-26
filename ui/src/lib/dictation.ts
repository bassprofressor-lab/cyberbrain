/** Pure helpers for dictation, apart from the component so they test without a DOM. */

/** Join dictated text onto what is already there, with one space or line between. */
export function appendDictation(current: string, spoken: string, multiline: boolean): string {
  const s = spoken.trim();
  if (!s) return current;
  if (!current.trim()) return s;
  const sep = multiline ? (current.endsWith("\n") ? "" : "\n") : current.endsWith(" ") ? "" : " ";
  return current + sep + s;
}
