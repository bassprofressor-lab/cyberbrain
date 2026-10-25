/**
 * A note name from a title somebody typed.
 *
 * A name is a slug of lowercase Latin letters, accents allowed, digits and single hyphens, in
 * Unicode NFC, at most 120 characters and 240 bytes (`cyberbrain_core::validate_name`). So
 * "Auslieferung für Kunden" becomes `auslieferung-für-kunden`: the umlaut stays, capitals
 * fold, spaces become hyphens. Letters of other scripts are not guessed at; they separate
 * words like punctuation does, and the name field shows what is left, where it can be edited.
 */
export const NAME_MAX = 120;
const NAME_BYTES_MAX = 240;

const DIGIT = /^[0-9]$/;
const LATIN_BLOCKS = /^[ß-ÿĀ-ɏḀ-ỿ]$/u;
const LOWERCASE = /^\p{Ll}$/u;
const MARK = /^\p{M}$/u;

/** The same letters `is_name_letter` accepts: a-z, and lowercase letters of the Latin blocks. */
function isNameLetter(c: string): boolean {
  return (c >= "a" && c <= "z") || (LATIN_BLOCKS.test(c) && LOWERCASE.test(c));
}

const bytes = (s: string) => new TextEncoder().encode(s).length;

export function slugFromTitle(title: string): string {
  let out = "";
  let pendingHyphen = false;
  let count = 0;
  for (const c of title.normalize("NFC").toLowerCase().normalize("NFC")) {
    if (DIGIT.test(c) || isNameLetter(c)) {
      const piece = pendingHyphen && out ? `-${c}` : c;
      if (count + [...piece].length > NAME_MAX || bytes(out + piece) > NAME_BYTES_MAX) break;
      out += piece;
      count += [...piece].length;
      pendingHyphen = false;
    } else if (!MARK.test(c)) {
      // A mark NFC could not compose onto its letter is dropped; the letter stays.
      pendingHyphen = true;
    }
  }
  return out.replace(/-+$/, "");
}

/** The same rule as `validate_name`, so a hand-edited name is refused before it is sent. */
export function isValidName(name: string): boolean {
  if (!name || name !== name.normalize("NFC")) return false;
  if ([...name].length > NAME_MAX || bytes(name) > NAME_BYTES_MAX) return false;
  return name.split("-").every((part) => part !== "" && [...part].every((c) => DIGIT.test(c) || isNameLetter(c)));
}

/** `keine-auslieferung-am-freitag` reads as a file name. People do not name things that way. */
export function titleOf(name: string): string {
  const words = name.replace(/[-_]+/g, " ").trim();
  return words ? words[0]!.toUpperCase() + words.slice(1) : name;
}

/**
 * The body to store for a note written in the simple view, where only a title and a text are
 * asked for. The file name is the title folded into a slug, which loses capitals, commas and
 * question marks; when the name does not read back as the title, the title is kept as the
 * note's first heading rather than thrown away.
 */
export function bodyWithTitle(title: string, name: string, body: string): string {
  const t = title.trim();
  if (!t || titleOf(name) === t || /^\s*#\s/.test(body)) return body;
  return `# ${t}\n\n${body}`;
}

/**
 * The heading and the text of a note as the simple view shows it: a note that starts with its
 * own `# Title` (see [`bodyWithTitle`]) is shown under that title, once, instead of under the
 * title made from its file name with the same heading repeated in the text.
 */
export function splitTitle(name: string, body: string): { title: string; text: string; skipped: number } {
  const m = /^\s*#[ \t]+([^\n]+?)[ \t]*(?:\r?\n+|$)/.exec(body);
  if (!m || !m[1]) return { title: titleOf(name), text: body, skipped: 0 };
  // `skipped`: lines taken off the front, so line numbers of the rest can be put back.
  return { title: m[1], text: body.slice(m[0].length), skipped: m[0].split("\n").length - 1 };
}
