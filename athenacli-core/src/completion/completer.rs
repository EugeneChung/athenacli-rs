//! reedline `Completer`: turns `suggest_type` scopes into concrete candidates
//! drawn from the metadata cache and static keyword/function tables. Port of
//! Python `completer.py` (`AthenaCompleter`).

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use arc_swap::ArcSwap;
use reedline::{Completer as ReedlineCompleter, Span, Suggestion};

use super::engine::{suggest_type, Suggestion as Suggest, TableRef};
use super::metadata::Metadata;
use crate::parse::scanner::{last_word, WordClass};

/// Keyword casing applied to keyword/function completions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casing {
    Upper,
    Lower,
    Auto,
}

impl Casing {
    pub fn parse(s: &str) -> Self {
        match s.to_ascii_lowercase().as_str() {
            "lower" => Casing::Lower,
            "auto" => Casing::Auto,
            _ => Casing::Upper,
        }
    }

    /// Resolve `Auto` against the partial word the user is typing.
    fn resolve(self, last: &str) -> Casing {
        match self {
            Casing::Auto => {
                if last.chars().last().is_some_and(|c| c.is_lowercase()) {
                    Casing::Lower
                } else {
                    Casing::Upper
                }
            }
            other => other,
        }
    }
}

pub struct AthenaCompleter {
    metadata: Arc<ArcSwap<Metadata>>,
    dbname: String,
    keyword_casing: Casing,
}

impl AthenaCompleter {
    pub fn new(metadata: Arc<ArcSwap<Metadata>>, dbname: String, keyword_casing: Casing) -> Self {
        Self {
            metadata,
            dbname,
            keyword_casing,
        }
    }

    fn match_suggestion(
        &self,
        suggestion: &Suggest,
        word_before_cursor: &str,
        meta: &Metadata,
        out: &mut Vec<Suggestion>,
    ) {
        match suggestion {
            Suggest::Column {
                tables,
                drop_unique,
            } => {
                let cols = self.scoped_columns(tables, *drop_unique, meta);
                find_matches(
                    word_before_cursor,
                    cols.iter().map(String::as_str),
                    false,
                    true,
                    None,
                    out,
                );
            }
            Suggest::Function { schema } => {
                if schema.is_none() {
                    find_matches(
                        word_before_cursor,
                        FUNCTIONS.iter().copied(),
                        true,
                        false,
                        Some(self.keyword_casing),
                        out,
                    );
                }
            }
            Suggest::Table { schema } => {
                // Only the current database is cached; a schema/alias qualifier
                // that isn't the current db yields nothing (matches Python).
                if schema.is_none() {
                    find_matches(
                        word_before_cursor,
                        meta.tables.iter().map(String::as_str),
                        false,
                        true,
                        None,
                        out,
                    );
                }
            }
            Suggest::View { .. } => {} // no view metadata cached
            Suggest::Alias { aliases } => {
                find_matches(
                    word_before_cursor,
                    aliases.iter().map(String::as_str),
                    false,
                    true,
                    None,
                    out,
                );
            }
            Suggest::Database => {
                find_matches(
                    word_before_cursor,
                    meta.databases.iter().map(String::as_str),
                    false,
                    true,
                    None,
                    out,
                );
            }
            Suggest::Schema => {}
            Suggest::Keyword { last_token } => {
                let kws = keyword_candidates(last_token.as_deref());
                find_matches(
                    word_before_cursor,
                    kws.into_iter(),
                    true,
                    false,
                    Some(self.keyword_casing),
                    out,
                );
            }
            // Special / Show / TableFormat / FileName / FavoriteQuery are wired
            // in Phase 3 (special commands, paths, favorites).
            _ => {}
        }
    }

    /// Columns visible for a list of scoped tables, mirroring
    /// `populate_scoped_cols` + the `drop_unique` USING(...) filter.
    fn scoped_columns(
        &self,
        tables: &[TableRef],
        drop_unique: bool,
        meta: &Metadata,
    ) -> Vec<String> {
        let mut columns: Vec<String> = Vec::new();
        for (schema, table, _alias) in tables {
            // schema qualifier other than the current db has no cached columns.
            if let Some(s) = schema {
                if !s.eq_ignore_ascii_case(&self.dbname) {
                    continue;
                }
            }
            if let Some(cols) = meta.columns.get(&table.to_ascii_lowercase()) {
                columns.extend(cols.iter().cloned());
            }
        }
        if drop_unique {
            let mut counts: HashMap<&str, usize> = HashMap::new();
            for c in &columns {
                *counts.entry(c.as_str()).or_default() += 1;
            }
            columns
                .iter()
                .filter(|c| c.as_str() != "*" && counts.get(c.as_str()).copied().unwrap_or(0) > 1)
                .cloned()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect()
        } else {
            columns
        }
    }
}

impl ReedlineCompleter for AthenaCompleter {
    fn complete(&mut self, line: &str, pos: usize) -> Vec<Suggestion> {
        let text_before = &line[..pos];
        let word_before_cursor = last_word(text_before, WordClass::AllPunctuations);

        let meta = self.metadata.load();
        let mut out: Vec<Suggestion> = Vec::new();
        for suggestion in suggest_type(line, text_before) {
            self.match_suggestion(&suggestion, word_before_cursor, &meta, &mut out);
        }

        // Stable dedup by replacement value.
        let mut seen = std::collections::HashSet::new();
        out.retain(|s| seen.insert(s.value.clone()));
        // Set the replacement span (constant across candidates).
        let replace_len = last_word(word_before_cursor, WordClass::MostPunctuations).len();
        let span = Span::new(pos.saturating_sub(replace_len), pos);
        for s in &mut out {
            s.span = span;
        }
        out
    }
}

/// Port of Python `AthenaCompleter.find_matches`. Appends matching candidates,
/// fuzzy or prefix, with optional keyword casing, ordered by match quality.
fn find_matches<'a>(
    word_before_cursor: &str,
    collection: impl Iterator<Item = &'a str>,
    start_only: bool,
    fuzzy: bool,
    casing: Option<Casing>,
    out: &mut Vec<Suggestion>,
) {
    let last = last_word(word_before_cursor, WordClass::MostPunctuations);
    let text = last.to_ascii_lowercase();

    // (match_len, match_start, item)
    let mut scored: Vec<(usize, usize, &str)> = Vec::new();
    for item in collection {
        let item_lower = item.to_ascii_lowercase();
        if fuzzy {
            if let Some((len, start)) = fuzzy_match(&text, &item_lower) {
                scored.push((len, start, item));
            }
        } else {
            let hay = if start_only {
                item_lower
                    .get(..text.len().min(item_lower.len()))
                    .unwrap_or("")
            } else {
                item_lower.as_str()
            };
            if let Some(start) = hay.find(&text) {
                scored.push((text.len(), start, item));
            }
        }
    }
    scored.sort_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)));

    let resolved = casing.map(|c| c.resolve(last));
    for (_, _, item) in scored {
        let value = match resolved {
            Some(Casing::Upper) => item.to_ascii_uppercase(),
            Some(Casing::Lower) => item.to_ascii_lowercase(),
            _ => item.to_string(),
        };
        out.push(Suggestion {
            value,
            span: Span::new(0, 0), // set by caller once
            append_whitespace: false,
            ..Suggestion::default()
        });
    }
}

/// Leftmost lazy subsequence match (`c0.*?c1.*?...`). Returns
/// `(matched_span_len, start)`; an empty pattern matches everything at `(0, 0)`.
fn fuzzy_match(pattern: &str, item: &str) -> Option<(usize, usize)> {
    let pat: Vec<char> = pattern.chars().collect();
    if pat.is_empty() {
        return Some((0, 0));
    }
    let chars: Vec<char> = item.chars().collect();
    for start in 0..chars.len() {
        if chars[start] != pat[0] {
            continue;
        }
        let mut pi = 1;
        let mut ci = start + 1;
        let mut end = start;
        while pi < pat.len() && ci < chars.len() {
            if chars[ci] == pat[pi] {
                end = ci;
                pi += 1;
            }
            ci += 1;
        }
        if pi == pat.len() {
            return Some((end - start + 1, start));
        }
    }
    None
}

/// Candidate keywords for a `Keyword` suggestion: the well-known followers of
/// `last_token` if any, else every top-level keyword.
fn keyword_candidates(last_token: Option<&str>) -> Vec<&'static str> {
    if let Some(tok) = last_token {
        if let Some(next) = KEYWORD_TREE.get(tok) {
            if !next.is_empty() {
                return next.clone();
            }
        }
    }
    KEYWORD_TREE.keys().copied().collect()
}

/// `keywords` tree from Python `literals/literals.json`: key -> well-known
/// following keywords.
static KEYWORD_TREE: LazyLock<HashMap<&'static str, Vec<&'static str>>> = LazyLock::new(|| {
    HashMap::from([
        ("ALTER", vec!["DATABASE", "SCHEMA", "TABLE"]),
        ("CREATE", vec!["DATABASE", "EXTERNAL", "TABLE", "VIEW"]),
        ("EXTERNAL", vec!["TABLE"]),
        ("DESCRIBE", vec!["TABLE", "VIEW"]),
        ("DROP", vec!["DATABASE", "TABLE", "VIEW"]),
        ("MSCK", vec!["REPAIR TABLE"]),
        (
            "SHOW",
            vec![
                "COLUMNS IN",
                "CREATE TABLE",
                "CREATE VIEW",
                "DATABASES",
                "SCHEMAS",
                "PARTITIONS",
                "TABLES",
                "TBLPROPERTIES",
                "VIEWS",
            ],
        ),
        ("REPLACE", vec!["VIEW"]),
        ("WITH", vec![]),
        ("SELECT", vec![]),
        ("ALL", vec![]),
        ("DISTINCT", vec![]),
        ("FROM", vec![]),
        ("WHERE", vec![]),
        ("INNER", vec!["JOIN"]),
        ("OUTER", vec!["JOIN"]),
        ("CROSS", vec!["JOIN"]),
        ("LEFT", vec!["JOIN", "OUTER JOIN"]),
        ("RIGHT", vec!["JOIN", "OUTER JOIN"]),
        ("FULL", vec!["JOIN", "OUTER JOIN"]),
        ("JOIN", vec![]),
        ("ON", vec![]),
        ("USING", vec![]),
        ("GROUP BY", vec![]),
        ("HAVING", vec![]),
        ("UNION", vec![]),
        ("ORDER BY", vec![]),
        ("ASC", vec![]),
        ("DESC", vec![]),
        ("NULLS FIRST", vec![]),
        ("NULLS LAST", vec![]),
        ("LIMIT", vec![]),
        ("AND", vec![]),
        ("OR", vec![]),
        ("NOT", vec![]),
        ("CAST", vec![]),
        ("CASE", vec![]),
        ("WHEN", vec![]),
        ("THEN", vec![]),
        ("ELSE", vec![]),
        ("END", vec![]),
        ("JSON", vec![]),
        ("IF NOT EXISTS", vec![]),
    ])
});

/// `functions` list from Python `literals/literals.json`.
static FUNCTIONS: &[&str] = &[
    "AVG",
    "CONCAT",
    "COUNT",
    "EVERY",
    "FIRST",
    "FORMAT",
    "LAST",
    "LCASE",
    "LEN",
    "MAX",
    "MIN",
    "MID",
    "NOW",
    "ROUND",
    "SUM",
    "TOP",
    "UCASE",
    "IF",
    "COALESCE",
    "NULLIF",
    "TRY",
    "CAST",
    "TRY_CAST",
    "TYPEOF",
    "ABS",
    "CEIL",
    "FLOOR",
    "LOG",
    "POW",
    "LENGTH",
    "LOWER",
    "REPLACE",
    "UPPER",
    "TRIM",
    "SUBSTR",
    "DAY",
    "YEAR",
    "WEEK",
    "REGEXP_EXTRACT_ALL",
    "REGEXP_EXTRACT",
    "REGEXP_LIKE",
    "REGEXP_REPLACE",
    "REGEXP_SPLIT",
    "URL_EXTRACT_PATH",
    "URL_EXTRACT_HOST",
    "URL_EXTRACT_PARAMETER",
    "URL_EXTRACT_QUERY",
    "MAP",
    "REDUCE",
    "FILTER",
    "TRANSFORM",
    "ZIP_WITH",
    "INDEX",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn completer_with(meta: Metadata) -> AthenaCompleter {
        AthenaCompleter::new(
            Arc::new(ArcSwap::from_pointee(meta)),
            "mydb".to_string(),
            Casing::Upper,
        )
    }

    fn values(c: &mut AthenaCompleter, line: &str) -> Vec<String> {
        c.complete(line, line.len())
            .into_iter()
            .map(|s| s.value)
            .collect()
    }

    #[test]
    fn from_completes_tables() {
        let meta = Metadata {
            tables: vec!["users".into(), "orders".into()],
            ..Default::default()
        };
        let mut c = completer_with(meta);
        let got = values(&mut c, "SELECT * FROM ");
        assert!(got.contains(&"users".to_string()));
        assert!(got.contains(&"orders".to_string()));
    }

    #[test]
    fn from_prefix_filters_tables() {
        let meta = Metadata {
            tables: vec!["users".into(), "orders".into()],
            ..Default::default()
        };
        let mut c = completer_with(meta);
        let got = values(&mut c, "SELECT * FROM us");
        assert_eq!(got, vec!["users".to_string()]);
    }

    #[test]
    fn where_completes_scoped_columns() {
        let mut columns = HashMap::new();
        columns.insert("users".to_string(), vec!["id".into(), "name".into()]);
        let meta = Metadata {
            tables: vec!["users".into()],
            columns,
            ..Default::default()
        };
        let mut c = completer_with(meta);
        // Cursor sits right after the first "SELECT " (the column position).
        let got: Vec<String> = c
            .complete("SELECT  FROM users", 7)
            .into_iter()
            .map(|s| s.value)
            .collect();
        assert!(got.contains(&"id".to_string()));
        assert!(got.contains(&"name".to_string()));
    }

    #[test]
    fn use_completes_databases() {
        let meta = Metadata {
            databases: vec!["analytics".into(), "raw".into()],
            ..Default::default()
        };
        let mut c = completer_with(meta);
        let got = values(&mut c, "USE ");
        assert!(got.contains(&"analytics".to_string()));
        assert!(got.contains(&"raw".to_string()));
    }

    #[test]
    fn join_keywords_after_left() {
        let mut c = completer_with(Metadata::default());
        let got = values(&mut c, "SELECT foo FROM bar LEFT ");
        assert!(got.contains(&"JOIN".to_string()));
        assert!(got.contains(&"OUTER JOIN".to_string()));
    }

    #[test]
    fn outer_join_after_inner() {
        let mut c = completer_with(Metadata::default());
        let got = values(&mut c, "SELECT foo FROM bar INNER ");
        assert!(got.contains(&"JOIN".to_string()));
        assert!(!got.contains(&"OUTER JOIN".to_string()));
    }
}
