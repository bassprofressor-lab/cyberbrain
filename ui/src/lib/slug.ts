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
