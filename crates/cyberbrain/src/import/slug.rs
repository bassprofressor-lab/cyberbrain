//! Names. A note name is a kebab-case slug (SPEC §3.1, `frontmatter::validate_name`);
//! this module derives one from a heading or file name and fills a template.
//!
//! The corpus is German, so umlauts are transliterated (`ä` → `ae`) rather than dropped:
//! `Nächste` becoming `nchste` is unreadable, and unreadable names are ones the operator
//! cannot type back. Other Latin diacritics fold to their base letter; everything else
//! that is not a letter or digit becomes a hyphen. The result can be empty (a heading of
//! emoji only); the caller falls back and says so.

use cyberbrain_core::frontmatter::validate_name;

pub const MAX_NAME: usize = 120;

pub fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pending_hyphen = false;
    for ch in text.chars() {
        let mapped: &str = match ch {
            'ä' | 'Ä' => "ae",
            'ö' | 'Ö' => "oe",
            'ü' | 'Ü' => "ue",
            'ß' => "ss",
            'à' | 'á' | 'â' | 'ã' | 'å' | 'À' | 'Á' | 'Â' | 'Ã' | 'Å' => "a",
            'è' | 'é' | 'ê' | 'ë' | 'È' | 'É' | 'Ê' | 'Ë' => "e",
            'ì' | 'í' | 'î' | 'ï' | 'Ì' | 'Í' | 'Î' | 'Ï' => "i",
            'ò' | 'ó' | 'ô' | 'õ' | 'ø' | 'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ø' => "o",
            'ù' | 'ú' | 'û' | 'Ù' | 'Ú' | 'Û' => "u",
            'ç' | 'Ç' => "c",
            'ñ' | 'Ñ' => "n",
            c if c.is_ascii_alphanumeric() => {
                if pending_hyphen && !out.is_empty() {
                    out.push('-');
                }
                pending_hyphen = false;
                out.push(c.to_ascii_lowercase());
                continue;
            }
            _ => {
                pending_hyphen = true;
                continue;
            }
        };
        if pending_hyphen && !out.is_empty() {
            out.push('-');
        }
        pending_hyphen = false;
        out.push_str(mapped);
    }
    truncate(&out)
}

/// Cut at a hyphen so the name stays readable, never in the middle of a word.
fn truncate(s: &str) -> String {
    if s.len() <= MAX_NAME {
        return s.to_string();
    }
    let head = &s[..MAX_NAME];
    match head.rfind('-') {
        Some(i) if i > 0 => head[..i].to_string(),
        _ => head.trim_end_matches('-').to_string(),
    }
}

/// The values a template can draw on.
#[derive(Debug, Clone, Default)]
pub struct NameParts {
    pub stem: String,
    pub dir: String,
    pub heading: String,
    pub index: usize,
    pub hash: String,
}

/// Fill `template` and normalise the result. Placeholders that are empty vanish together
/// with a neighbouring hyphen, so `{dir}-{stem}` for a root file is just the stem.
pub fn render(template: &str, parts: &NameParts) -> String {
    let filled = template
        .replace("{stem}", &parts.stem)
        .replace("{dir}", &parts.dir)
        .replace("{heading}", &parts.heading)
        .replace("{index}", &parts.index.to_string())
        .replace("{hash}", &parts.hash);
    slugify(&filled)
}

/// A short, deterministic content hash for disambiguation (FNV-1a, 8 hex chars). Not a
/// security primitive; two items with identical text collide on purpose.
pub fn short_hash(text: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:08x}", (h >> 32) as u32 ^ h as u32)
}

pub fn is_valid(name: &str) -> bool {
    validate_name(name).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(
            slugify("Session: 2026-04-16 15:53"),
            "session-2026-04-16-15-53"
        );
        assert_eq!(
            slugify("🚀 NÄCHSTE SITZUNG — hier anfangen (Stand 04.09.2026, 06:15 UTC)"),
            "naechste-sitzung-hier-anfangen-stand-04-09-2026-06-15-utc"
        );
        assert_eq!(
            slugify("PREREG_carry_2026-08-21"),
            "prereg-carry-2026-08-21"
        );
        assert_eq!(slugify("**bold** `code` façade"), "bold-code-facade");
        assert_eq!(slugify("./"), "");
        assert_eq!(slugify("🚀"), "");
        assert_eq!(slugify("Straße Öl"), "strasse-oel");
        let long = slugify(&"word ".repeat(50));
        assert!(
            long.len() <= MAX_NAME && is_valid(&long) && !long.ends_with('-'),
            "{long}"
        );
    }

    #[test]
    fn templates() {
        let parts = NameParts {
            stem: "memory".into(),
            dir: String::new(),
            heading: "Session: 2026-04-16 15:53".into(),
            index: 7,
            hash: "abcd1234".into(),
        };
        assert_eq!(
            render("{stem}-{heading}", &parts),
            "memory-session-2026-04-16-15-53"
        );
        assert_eq!(render("{dir}-{stem}", &parts), "memory");
        assert_eq!(render("{stem}-{index}", &parts), "memory-7");
        assert_eq!(
            render("{stem}-{heading}-{hash}", &parts),
            "memory-session-2026-04-16-15-53-abcd1234"
        );
        assert_eq!(short_hash("a"), short_hash("a"));
        assert_ne!(short_hash("a"), short_hash("b"));
        assert_eq!(short_hash("x").len(), 8);
    }
}
