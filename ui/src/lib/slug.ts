/**
 * A note name from a title somebody typed.
 *
 * A name is a kebab-case ASCII slug (`cyberbrain_core::validate_name`), because it is also the
 * file name and part of every citation. A title is not, and the person typing it should not
 * have to know that: "Auslieferung für Kunden" becomes `auslieferung-fuer-kunden`. German
 * letters are spelled out the way a German reader writes them without the key for it
 * (ü → ue, ß → ss) instead of being stripped to `u`, which would change the word. Other
 * accents are dropped.
 */
const SPELLED_OUT: Record<string, string> = { ä: "ae", ö: "oe", ü: "ue", ß: "ss", Ä: "ae", Ö: "oe", Ü: "ue", ẞ: "ss", æ: "ae", œ: "oe", ø: "o", å: "a", ł: "l", đ: "d", þ: "th" };

export const NAME_MAX = 120;
const VALID_NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;

export function slugFromTitle(title: string): string {
  const slug = title
    .replace(/[äöüßÄÖÜẞæœøåłđþ]/g, (c) => SPELLED_OUT[c] ?? c)
    .normalize("NFKD")
    .replace(/\p{M}+/gu, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return slug.slice(0, NAME_MAX).replace(/-+$/, "");
}

/** The same rule as `validate_name`, so a hand-edited name is refused before it is sent. */
export function isValidName(name: string): boolean {
  return name.length <= NAME_MAX && VALID_NAME.test(name);
}
