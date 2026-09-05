//! JavaScript and TypeScript (JSX/TSX included).
//!
//! Definitions: `function` declarations (also `export`, `export default`, `async`,
//! generators), `class` (with its methods, at any depth of nesting — `constructor`,
//! `static`, `async`, getters, setters, `#private`), arrow and function-expression
//! assignments at any indentation (`const handleClick = () => {}` inside a component is
//! a definition an agent asks for), module-level `const`/`let`/`var` at column 0, and in
//! TypeScript `interface`, `type`, `enum`, `namespace`/`module`, `abstract class`.
//!
//! Left out: object-literal methods outside classes, interface members, `prototype`
//! assignments, anything inside a minified single line. A method is only recognised at
//! one indentation level below its class, which is what rules out call sites inside
//! method bodies. Doc blocks (`/** */`, `//`) and decorators directly above are included
//! in the range.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{Lexed, Style, Terminator, extend_up, indent_of, item_extent};
use regex::Regex;
use std::sync::LazyLock;

const IDENT: &str = r"[A-Za-z_$][\w$]*";

static FUNC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+(?:default\s+)?)?(?:declare\s+)?(?:async\s+)?function\s*\*?\s*({IDENT})"
    ))
    .unwrap()
});
static CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+(?:default\s+)?)?(?:declare\s+)?(?:abstract\s+)?class\s+({IDENT})"
    ))
    .unwrap()
});
static IFACE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+)?(?:declare\s+)?interface\s+({IDENT})"
    ))
    .unwrap()
});
static TYPE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+)?(?:declare\s+)?type\s+({IDENT})\s*(?:<|=)"
    ))
    .unwrap()
});
static ENUM: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+)?(?:declare\s+)?(?:const\s+)?enum\s+({IDENT})"
    ))
    .unwrap()
});
static NS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+)?(?:declare\s+)?(?:namespace|module)\s+({IDENT}(?:\.{IDENT})*)\s*\{{"
    ))
    .unwrap()
});
/// `const f = (a) => ...`, `const f = async function`, `const F = memo(() => ...)`,
/// `const f = <T,>(a: T) =>`, `const f = (\n` (params continue on the next line).
static VARFN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:export\s+)?(?:const|let|var)\s+({IDENT})\s*(?::\s*[^=]+?)?\s*=\s*(?:(?:memo|forwardRef|React\.memo|React\.forwardRef|useCallback|styled\.\w+|styled\([^)]*\))\s*\(\s*)?(?:async\s+)?(?:function\b|\([^()]*\)\s*(?::[^=]+?)?\s*=>|{IDENT}\s*=>|<[^>]*>\s*\(|\(\s*$)"
    ))
    .unwrap()
});
static VAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!(r"^(?:export\s+)?(const|let|var)\s+({IDENT})")).unwrap());
static METHOD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(\s*)(?:(?:public|private|protected|static|async|readonly|override|abstract|declare|get|set)\s+)*\*?\s*(#?{IDENT})\s*(?:<[^>]*>)?\s*\("
    ))
    .unwrap()
});

const KEYWORDS: &[&str] = &[
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "return",
    "function",
    "const",
    "let",
    "var",
    "new",
    "await",
    "else",
    "do",
    "try",
    "throw",
    "typeof",
    "delete",
    "void",
    "yield",
    "import",
    "export",
    "super",
    "this",
    "case",
    "default",
    "with",
    "in",
    "of",
    "instanceof",
    "class",
    "interface",
    "enum",
    "declare",
    "type",
    "as",
    "satisfies",
];

fn is_item_start(l: &str) -> bool {
    FUNC.is_match(l)
        || CLASS.is_match(l)
        || IFACE.is_match(l)
        || TYPE.is_match(l)
        || ENUM.is_match(l)
        || NS.is_match(l)
        || VARFN.is_match(l)
        || VAR.is_match(l)
}

fn is_doc_line(l: &str) -> bool {
    let t = l.trim_start();
    t.starts_with("//") || t.starts_with("/*") || t.starts_with('*') || t.starts_with('@')
}

/// A method header: `name(...)` followed, on the same line, by `{` (after an optional
/// return type), or by an unclosed parameter list. Anything ending in `;` or `,` or with
/// nothing after the closing paren is a call or a signature, not a body.
fn method_header_ok(line: &str, paren_at: usize) -> bool {
    let b = line.as_bytes();
    let mut depth = 0usize;
    let mut close = None;
    for (k, &c) in b.iter().enumerate().skip(paren_at) {
        match c {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(k);
                    break;
                }
            }
            _ => {}
        }
    }
    match close {
        None => true,
        Some(k) => {
            let rest = line[k + 1..].trim();
            if rest.starts_with('{') {
                return true; // `get value() { return this.a; }`
            }
            if rest.is_empty() || rest.ends_with(';') || rest.ends_with(',') || rest.contains("=>")
            {
                return false;
            }
            rest.ends_with('{')
        }
    }
}

pub(crate) fn extract(lines: &[&str], typescript: bool, out: &mut Vec<Def>) {
    let term = Terminator::SemicolonOrNewline;
    let lx = Lexed::new(lines, Style::CLike);
    // Classes whose range is still open: (end, indent, name).
    let mut classes: Vec<(usize, usize, String)> = Vec::new();

    for (i, &line) in lines.iter().enumerate() {
        while classes.last().is_some_and(|c| c.0 < i) {
            classes.pop();
        }

        if let Some(c) = caps(&CLASS, line) {
            let indent = indent_of(&c[1]);
            let end = item_extent(lines, &lx, i, term, &is_item_start);
            let name = c[2].to_string();
            out.push(Def {
                kind: DefKind::Class,
                name: name.clone(),
                scope: classes.last().map(|c| c.2.clone()),
                line: i,
                start: extend_up(lines, i, is_doc_line),
                end,
            });
            classes.push((end, indent, name));
            continue;
        }
        if let Some(c) = caps(&FUNC, line) {
            out.push(Def {
                kind: DefKind::Function,
                name: c[2].to_string(),
                scope: None,
                line: i,
                start: extend_up(lines, i, is_doc_line),
                end: item_extent(lines, &lx, i, term, &is_item_start),
            });
            continue;
        }
        if typescript {
            let ts = if let Some(c) = caps(&IFACE, line) {
                Some((DefKind::Interface, c[2].to_string()))
            } else if let Some(c) = caps(&ENUM, line) {
                Some((DefKind::Enum, c[2].to_string()))
            } else if let Some(c) = caps(&NS, line) {
                Some((DefKind::Namespace, c[2].to_string()))
            } else {
                caps(&TYPE, line).map(|c| (DefKind::TypeAlias, c[2].to_string()))
            };
            if let Some((kind, name)) = ts {
                out.push(Def {
                    kind,
                    name,
                    scope: None,
                    line: i,
                    start: extend_up(lines, i, is_doc_line),
                    end: item_extent(lines, &lx, i, term, &is_item_start),
                });
                continue;
            }
        }
        if let Some(c) = caps(&VARFN, line) {
            out.push(Def {
                kind: DefKind::Function,
                name: c[2].to_string(),
                scope: None,
                line: i,
                start: extend_up(lines, i, is_doc_line),
                end: item_extent(lines, &lx, i, term, &is_item_start),
            });
            continue;
        }
        if let Some(c) = caps(&VAR, line) {
            out.push(Def {
                kind: if &c[1] == "const" {
                    DefKind::Const
                } else {
                    DefKind::Variable
                },
                name: c[2].to_string(),
                scope: None,
                line: i,
                start: extend_up(lines, i, is_doc_line),
                end: item_extent(lines, &lx, i, term, &is_item_start),
            });
            continue;
        }
        // Methods: one level inside the innermost open class.
        if let Some(class) = classes.last()
            && let Some(c) = caps(&METHOD, line)
        {
            let indent = indent_of(&c[1]);
            let name = c[2].to_string();
            let bare = name.trim_start_matches('#');
            if indent > class.1
                && indent <= class.1 + 4
                && !KEYWORDS.contains(&bare)
                && method_header_ok(line, c.get(2).unwrap().end())
            {
                out.push(Def {
                    kind: DefKind::Method,
                    name,
                    scope: Some(class.2.clone()),
                    line: i,
                    start: extend_up(lines, i, is_doc_line),
                    end: item_extent(lines, &lx, i, term, &is_item_start),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn javascript_definitions() {
        let src = r#"import x from "y";

export const ACTIVE_ENGINES = ["carry"];
let counter = 0;

/**
 * Fetch.
 */
export async function fetchUser(id) {
  const r = await fetch(`/u/${id}`);
  return r.json();
}

export default function Dashboard() {
  const handleClick = () => {
    fetchUser(1);
  };
  fetchUser(2);
  return <p>don't {counter}</p>;
}

class Store {
  constructor(a) {
    this.a = a;
  }

  static create(
    a,
    b,
  ) {
    return new Store(a);
  }

  async load() {
    fetchUser(3);
    if (this.a) {
      other(1);
    }
  }

  get value() { return this.a; }
}
const noop = function () {};
"#;
        let d = defs(Language::JavaScript, src);
        let get = |name: &str| {
            d.iter()
                .find(|x| x.1 == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} missing in {d:?}"))
        };
        assert_eq!(
            get("ACTIVE_ENGINES"),
            (DefKind::Const, "ACTIVE_ENGINES".into(), None, 3, 3, 3)
        );
        assert_eq!(get("counter").0, DefKind::Variable);
        assert_eq!(
            get("fetchUser"),
            (DefKind::Function, "fetchUser".into(), None, 6, 9, 12)
        );
        assert_eq!(
            get("Dashboard"),
            (DefKind::Function, "Dashboard".into(), None, 14, 14, 20)
        );
        assert_eq!(
            get("handleClick"),
            (DefKind::Function, "handleClick".into(), None, 15, 15, 17)
        );
        assert_eq!(
            get("Store"),
            (DefKind::Class, "Store".into(), None, 22, 22, 42)
        );
        assert_eq!(
            get("constructor"),
            (
                DefKind::Method,
                "constructor".into(),
                Some("Store".into()),
                23,
                23,
                25
            )
        );
        assert_eq!(
            get("create"),
            (
                DefKind::Method,
                "create".into(),
                Some("Store".into()),
                27,
                27,
                32
            )
        );
        assert_eq!(
            get("load"),
            (
                DefKind::Method,
                "load".into(),
                Some("Store".into()),
                34,
                34,
                39
            )
        );
        assert_eq!(
            get("value"),
            (
                DefKind::Method,
                "value".into(),
                Some("Store".into()),
                41,
                41,
                41
            )
        );
        assert_eq!(get("noop").0, DefKind::Function);
        assert_eq!(
            d.iter().filter(|x| x.1 == "fetchUser").count(),
            1,
            "call sites are not hits: {d:?}"
        );
        assert!(d.iter().all(|x| x.1 != "other" && x.1 != "fetch"));
    }

    #[test]
    fn typescript_definitions() {
        let src = r#"export interface User {
  id: number;
  name(): string;
}
export type Id = string | number;
type Union =
  | "a"
  | "b";
export enum Kind { A, B }
export namespace Api.V1 {
  export const x = 1;
}
export abstract class Base<T> {
  private readonly items: T[] = [];
  protected abstract run(): void;
  public async start(arg: string): Promise<void> {
    this.run();
  }
}
export const useThing = <T,>(x: T): T => x;
"#;
        let d = defs(Language::TypeScript, src);
        let get = |name: &str| {
            d.iter()
                .find(|x| x.1 == name)
                .cloned()
                .unwrap_or_else(|| panic!("{name} missing in {d:?}"))
        };
        assert_eq!(
            get("User"),
            (DefKind::Interface, "User".into(), None, 1, 1, 4)
        );
        assert_eq!(get("Id"), (DefKind::TypeAlias, "Id".into(), None, 5, 5, 5));
        assert_eq!(
            get("Union"),
            (DefKind::TypeAlias, "Union".into(), None, 6, 6, 8)
        );
        assert_eq!(get("Kind").0, DefKind::Enum);
        assert_eq!(get("Api.V1").0, DefKind::Namespace);
        assert_eq!(
            get("Base"),
            (DefKind::Class, "Base".into(), None, 13, 13, 19)
        );
        assert_eq!(
            get("start"),
            (
                DefKind::Method,
                "start".into(),
                Some("Base".into()),
                16,
                16,
                18
            )
        );
        assert_eq!(get("useThing").0, DefKind::Function);
        assert!(
            d.iter().all(|x| x.1 != "name" && x.1 != "run"),
            "signatures without bodies are not hits: {d:?}"
        );
    }
}
