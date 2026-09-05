//! Matching a symbol against the extracted definitions, and ordering the hits.
//!
//! Three match tiers, best first: exact name, exact name ignoring case, name containing
//! the query (only for queries of three characters or more; `fn` would otherwise match
//! everything). Within a tier, code definitions sort before configuration keys and
//! headings, then by path and line, so `find backend` lists a Python `backend` before
//! the docker-compose service of that name.
//!
//! A query may carry a scope: `App::find`, `App.find`, `App#find`, `services.backend`.
//! The dot form is only split when both halves look like identifiers, so a heading such
//! as `10. Code index` stays whole.

use crate::{DefKind, Definition, Hit, MatchKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Query {
    pub name: String,
    pub scope: Option<String>,
}

fn ident_like(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '$' | '-'))
}

pub(crate) fn parse(symbol: &str) -> Query {
    let s = symbol.trim();
    if let Some((scope, name)) = s.rsplit_once("::") {
        return Query {
            name: name.trim().to_string(),
            scope: Some(scope.trim().trim_start_matches("::").to_string())
                .filter(|x| !x.is_empty()),
        };
    }
    if let Some((scope, name)) = s.rsplit_once('#')
        && ident_like(name)
        && !scope.is_empty()
    {
        return Query {
            name: name.to_string(),
            scope: Some(scope.to_string()),
        };
    }
    if let Some((scope, name)) = s.rsplit_once('.')
        && ident_like(name)
        && scope.split('.').all(ident_like)
    {
        return Query {
            name: name.to_string(),
            scope: Some(scope.to_string()),
        };
    }
    Query {
        name: s.to_string(),
        scope: None,
    }
}

pub(crate) const MIN_CONTAINS_LEN: usize = 3;

pub(crate) fn match_name(name: &str, q: &str) -> Option<MatchKind> {
    if name == q {
        return Some(MatchKind::Exact);
    }
    if name.eq_ignore_ascii_case(q) {
        return Some(MatchKind::CaseInsensitive);
    }
    if q.chars().count() >= MIN_CONTAINS_LEN && name.to_lowercase().contains(&q.to_lowercase()) {
        return Some(MatchKind::Contains);
    }
    None
}

/// A definition's scope satisfies the query's scope when it equals it, or ends with it
/// as a dotted suffix (`services.backend` for a query scope of `backend`), ignoring case
/// and treating `::` as `.`.
pub(crate) fn scope_matches(def_scope: Option<&str>, want: &str) -> bool {
    let Some(have) = def_scope else {
        return false;
    };
    let have = have.replace("::", ".").to_lowercase();
    let want = want.replace("::", ".").to_lowercase();
    have == want || have.ends_with(&format!(".{want}"))
}

fn kind_rank(k: DefKind) -> u8 {
    match k {
        DefKind::Key | DefKind::Section => 2,
        DefKind::Heading => 3,
        _ => 1,
    }
}

pub(crate) fn rank(hits: &mut [Hit]) {
    hits.sort_by(|a, b| {
        a.matched
            .cmp(&b.matched)
            .then_with(|| kind_rank(a.def.kind).cmp(&kind_rank(b.def.kind)))
            .then_with(|| a.def.path.cmp(&b.def.path))
            .then_with(|| a.def.line.cmp(&b.def.line))
    });
}

pub(crate) fn matches(defs: &[Definition], q: &Query, use_scope: bool) -> Vec<Hit> {
    defs.iter()
        .filter_map(|d| {
            let m = match_name(&d.name, &q.name)?;
            if use_scope
                && let Some(s) = &q.scope
                && !scope_matches(d.scope.as_deref(), s)
            {
                return None;
            }
            Some(Hit {
                def: d.clone(),
                matched: m,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_parsing() {
        assert_eq!(
            parse("find"),
            Query {
                name: "find".into(),
                scope: None
            }
        );
        assert_eq!(
            parse("App::find"),
            Query {
                name: "find".into(),
                scope: Some("App".into())
            }
        );
        assert_eq!(
            parse("crate::app::App"),
            Query {
                name: "App".into(),
                scope: Some("crate::app".into())
            }
        );
        assert_eq!(
            parse("App.find"),
            Query {
                name: "find".into(),
                scope: Some("App".into())
            }
        );
        assert_eq!(
            parse("services.backend"),
            Query {
                name: "backend".into(),
                scope: Some("services".into())
            }
        );
        assert_eq!(
            parse("App#find"),
            Query {
                name: "find".into(),
                scope: Some("App".into())
            }
        );
        assert_eq!(
            parse("10. Code index"),
            Query {
                name: "10. Code index".into(),
                scope: None
            }
        );
        assert_eq!(
            parse("  x  "),
            Query {
                name: "x".into(),
                scope: None
            }
        );
    }

    #[test]
    fn match_tiers() {
        assert_eq!(match_name("find", "find"), Some(MatchKind::Exact));
        assert_eq!(match_name("Find", "find"), Some(MatchKind::CaseInsensitive));
        assert_eq!(
            match_name("find_report", "report"),
            Some(MatchKind::Contains)
        );
        assert_eq!(
            match_name("fn_x", "fn"),
            None,
            "short queries do not substring-match"
        );
        assert_eq!(match_name("other", "find"), None);
        assert!(MatchKind::Exact < MatchKind::CaseInsensitive);
        assert!(MatchKind::CaseInsensitive < MatchKind::Contains);
    }

    #[test]
    fn scope_suffix() {
        assert!(scope_matches(Some("services.backend"), "backend"));
        assert!(scope_matches(Some("services.backend"), "services.backend"));
        assert!(scope_matches(Some("App"), "app"));
        assert!(!scope_matches(Some("services.backend"), "services"));
        assert!(!scope_matches(None, "App"));
    }
}
