//! SQL data definition.
//!
//! Definitions: `CREATE [OR REPLACE] [TEMP|UNLOGGED|UNIQUE|MATERIALIZED] TABLE | VIEW |
//! INDEX | FUNCTION | PROCEDURE | TRIGGER | TYPE | SCHEMA | SEQUENCE | DOMAIN [IF NOT
//! EXISTS] name`. A schema-qualified name (`public.users`) yields `users` with scope
//! `public`. Quoted identifiers are unquoted.
//!
//! In `.sql` files the keyword is matched case-insensitively and a statement runs to its
//! `;` (dollar-quoted bodies are skipped, so a `;` inside a PL/pgSQL function does not
//! end it). Embedded in another language the keyword must be upper case, and the
//! statement ends at the `;` or, failing that, where its bracket depth returns to zero —
//! a `CREATE TABLE (...)` inside a Python string rarely carries a semicolon.
//!
//! Left out: `ALTER`, `DROP`, `INSERT`, columns, constraints.

use super::{Def, caps};
use crate::DefKind;
use crate::extent::{LexState, Style, code_chars};
use regex::Regex;
use std::sync::LazyLock;

const PATTERN: &str = r"^\s*CREATE\s+(?:OR\s+REPLACE\s+)?(?:(?:TEMP|TEMPORARY|UNLOGGED|UNIQUE|MATERIALIZED|GLOBAL|LOCAL|RECURSIVE)\s+)*(TABLE|VIEW|INDEX|FUNCTION|PROCEDURE|TRIGGER|TYPE|SCHEMA|SEQUENCE|DOMAIN)\s+(?:IF\s+NOT\s+EXISTS\s+)?(?:CONCURRENTLY\s+)?([\x22`\[]?[A-Za-z_][\w$]*[\x22`\]]?(?:\.[\x22`\[]?[A-Za-z_][\w$]*[\x22`\]]?)*)";

static SQL_FILE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!("(?i){PATTERN}")).unwrap());
static EMBEDDED: LazyLock<Regex> = LazyLock::new(|| Regex::new(PATTERN).unwrap());

fn unquote(s: &str) -> &str {
    s.trim_matches(['"', '`', '[', ']'])
}

fn statement_extent(lines: &[&str], start: usize, require_semicolon: bool) -> usize {
    let mut st = LexState::default();
    let mut depth: usize = 0;
    let mut was_open = false;
    let mut buf = Vec::new();
    let last = lines.len().saturating_sub(1);
    let stop = (start + 2_000).min(last);
    let mut i = start;
    while i <= stop {
        buf.clear();
        code_chars(lines[i], &mut st, Style::Sql, &mut buf);
        for &c in &buf {
            match c {
                b'(' => {
                    depth += 1;
                    was_open = true;
                }
                b')' => depth = depth.saturating_sub(1),
                b';' if depth == 0 && !st.is_open() => return i,
                _ => {}
            }
        }
        if !require_semicolon && was_open && depth == 0 && !st.is_open() {
            return i;
        }
        i += 1;
    }
    start
}

pub(crate) fn extract(lines: &[&str], sql_file: bool, out: &mut Vec<Def>) {
    let re: &Regex = if sql_file { &SQL_FILE } else { &EMBEDDED };
    for (i, &line) in lines.iter().enumerate() {
        let Some(c) = caps(re, line) else {
            continue;
        };
        let kind = match c[1].to_ascii_uppercase().as_str() {
            "TABLE" => DefKind::Table,
            "VIEW" => DefKind::View,
            "INDEX" => DefKind::Index,
            "FUNCTION" | "PROCEDURE" => DefKind::Function,
            "TRIGGER" => DefKind::Trigger,
            "TYPE" | "DOMAIN" => DefKind::TypeAlias,
            "SCHEMA" => DefKind::Schema,
            "SEQUENCE" => DefKind::Variable,
            _ => continue,
        };
        let full = &c[2];
        let (scope, name) = match full.rsplit_once('.') {
            Some((s, n)) => (Some(unquote(s).to_string()), unquote(n).to_string()),
            None => (None, unquote(full).to_string()),
        };
        out.push(Def {
            kind,
            name,
            scope,
            line: i,
            start: i,
            end: statement_extent(lines, i, sql_file),
        });
    }
}

#[cfg(test)]
mod tests {
    use super::super::defs;
    use crate::{DefKind, Language};

    #[test]
    fn sql_file() {
        let src = r#"-- schema
create table if not exists users (
    id serial primary key,
    email text -- ; not a terminator
);
CREATE UNIQUE INDEX users_email ON users(email);
CREATE OR REPLACE VIEW public."active" AS
  SELECT * FROM users;
CREATE OR REPLACE FUNCTION bump() RETURNS trigger AS $$
BEGIN
  NEW.n := NEW.n + 1; RETURN NEW;
END;
$$ LANGUAGE plpgsql;
CREATE TRIGGER t BEFORE UPDATE ON users FOR EACH ROW EXECUTE FUNCTION bump();
"#;
        let d = defs(Language::Sql, src);
        assert_eq!(d[0], (DefKind::Table, "users".into(), None, 2, 2, 5));
        assert_eq!(d[1], (DefKind::Index, "users_email".into(), None, 6, 6, 6));
        assert_eq!(
            d[2],
            (
                DefKind::View,
                "active".into(),
                Some("public".into()),
                7,
                7,
                8
            )
        );
        assert_eq!(d[3], (DefKind::Function, "bump".into(), None, 9, 9, 13));
        assert_eq!(d[4], (DefKind::Trigger, "t".into(), None, 14, 14, 14));
    }

    #[test]
    fn embedded_in_python_upper_case_only() {
        let src = r#"async def init():
    await conn.execute("""
        CREATE TABLE IF NOT EXISTS payments (
            id serial,
            amount numeric(12, 2)
        )
    """)
    # create table for users later
"#;
        let d = defs(Language::Python, src);
        let t = d.iter().find(|x| x.0 == DefKind::Table).unwrap();
        assert_eq!(t, &(DefKind::Table, "payments".into(), None, 3, 3, 6));
        assert!(d.iter().all(|x| x.1 != "for"), "{d:?}");
    }
}
