//! `suggest_type`: from the text around the cursor, decide *what kind* of
//! completion to offer (tables, columns, keywords, ...) and its scope. A
//! faithful port of Python `packages/completion_engine.py`, kept structurally
//! parallel so the Python tests port directly.

use crate::parse::scanner::{
    self, extract_tables, find_prev_keyword, last_word, parse_partial_identifier, WordClass,
};

/// `(schema, table, alias)` — a table reference in a column scope.
pub type TableRef = (Option<String>, String, Option<String>);

/// What to complete at the cursor, plus the scope needed to enumerate it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Suggestion {
    /// Columns from `tables`. `drop_unique` keeps only columns shared by more
    /// than one table (the `JOIN ... USING (` case).
    Column {
        tables: Vec<TableRef>,
        drop_unique: bool,
    },
    Function {
        schema: Option<String>,
    },
    Table {
        schema: Option<String>,
    },
    View {
        schema: Option<String>,
    },
    Alias {
        aliases: Vec<String>,
    },
    Database,
    Schema,
    Keyword {
        last_token: Option<String>,
    },
    Special,
    Show,
    TableFormat,
    FileName,
    FavoriteQuery,
}

/// Entry point. `full_text` is the whole line; `text_before_cursor` is the part
/// left of the cursor.
pub fn suggest_type(full_text: &str, text_before_cursor: &str) -> Vec<Suggestion> {
    // Isolate the current statement (Python's multi-statement handling).
    let off = text_before_cursor.rfind(';').map(|i| i + 1).unwrap_or(0);
    let text_before_cursor = &text_before_cursor[off..];
    let full_text = &full_text[off..];

    let word = last_word(text_before_cursor, WordClass::ManyPunctuations);

    let mut parent: Option<String> = None;
    let scan_text: &str = if word.is_empty() || word.ends_with('(') || word.starts_with('\\') {
        text_before_cursor
    } else {
        // Strip the partial word before scanning, and pull its schema qualifier.
        let (p, _partial) = parse_partial_identifier(word);
        parent = p;
        &text_before_cursor[..text_before_cursor.len() - word.len()]
    };

    // Special backslash commands are handled separately.
    let trimmed = scan_text.trim_start();
    if trimmed.starts_with('\\') {
        return suggest_special(trimmed);
    }

    match scanner::last_token(scan_text) {
        None => vec![
            Suggestion::Keyword { last_token: None },
            Suggestion::Special,
        ],
        Some(t) => suggest_on_token(&t.text, t.is_keyword, scan_text, full_text, &parent),
    }
}

fn suggest_special(text: &str) -> Vec<Suggestion> {
    let text = text.trim_start();
    // cmd = first whitespace-delimited token; arg = the rest.
    let (cmd, arg) = match text.split_once(char::is_whitespace) {
        Some((c, rest)) => (c, rest.trim_start()),
        None => (text, ""),
    };

    // Still typing the command name itself.
    if arg.is_empty() && !text[cmd.len()..].starts_with(char::is_whitespace) {
        return vec![Suggestion::Special];
    }

    match cmd {
        "\\u" | "\\r" => vec![Suggestion::Database],
        "\\T" => vec![Suggestion::TableFormat],
        "\\f" | "\\fs" | "\\fd" => vec![Suggestion::FavoriteQuery],
        "\\dt" | "\\dt+" => vec![
            Suggestion::Table { schema: None },
            Suggestion::View { schema: None },
            Suggestion::Schema,
        ],
        "\\." | "source" => vec![Suggestion::FileName],
        _ => vec![
            Suggestion::Keyword { last_token: None },
            Suggestion::Special,
        ],
    }
}

fn suggest_on_token(
    token: &str,
    is_keyword: bool,
    text_before_cursor: &str,
    full_text: &str,
    parent: &Option<String>,
) -> Vec<Suggestion> {
    use Suggestion::*;
    let tv = token.to_ascii_lowercase();

    if tv.ends_with('(') {
        return suggest_paren(text_before_cursor, full_text, parent);
    }
    if matches!(tv.as_str(), "set" | "by" | "distinct") {
        return vec![Column {
            tables: extract_tables(full_text),
            drop_unique: false,
        }];
    }
    if tv == "as" {
        return Vec::new();
    }
    if matches!(tv.as_str(), "select" | "where" | "having") {
        let tables = extract_tables(full_text);
        return match parent {
            Some(p) => {
                let tables = tables.into_iter().filter(|t| identifies(p, t)).collect();
                vec![
                    Column {
                        tables,
                        drop_unique: false,
                    },
                    Table {
                        schema: Some(p.clone()),
                    },
                    View {
                        schema: Some(p.clone()),
                    },
                    Function {
                        schema: Some(p.clone()),
                    },
                ]
            }
            None => {
                let aliases = alias_list(&tables);
                vec![
                    Column {
                        tables,
                        drop_unique: false,
                    },
                    Function { schema: None },
                    Alias { aliases },
                    Keyword {
                        last_token: Some(tv.to_ascii_uppercase()),
                    },
                ]
            }
        };
    }
    if (tv.ends_with("join") && is_keyword)
        || matches!(
            tv.as_str(),
            "copy"
                | "from"
                | "update"
                | "into"
                | "describe"
                | "truncate"
                | "desc"
                | "explain"
                | "partitions"
        )
    {
        let schema = parent.clone();
        let mut v = Vec::new();
        if schema.is_none() {
            v.push(Schema);
        }
        v.push(Table {
            schema: schema.clone(),
        });
        if tv != "truncate" {
            v.push(View { schema });
        }
        return v;
    }
    if matches!(tv.as_str(), "table" | "view" | "function" | "tblproperties") {
        let schema = parent.clone();
        let mk = |s: Option<String>| match tv.as_str() {
            "view" => View { schema: s },
            "function" => Function { schema: s },
            _ => Table { schema: s },
        };
        return match schema {
            Some(_) => vec![mk(schema)],
            None => vec![Schema, mk(None)],
        };
    }
    if tv == "on" {
        let tables = extract_tables(full_text);
        return match parent {
            Some(p) => {
                let tables = tables.into_iter().filter(|t| identifies(p, t)).collect();
                vec![
                    Column {
                        tables,
                        drop_unique: false,
                    },
                    Table {
                        schema: Some(p.clone()),
                    },
                    View {
                        schema: Some(p.clone()),
                    },
                    Function {
                        schema: Some(p.clone()),
                    },
                ]
            }
            None => {
                let aliases = alias_list(&tables);
                let mut v = vec![Alias {
                    aliases: aliases.clone(),
                }];
                if aliases.is_empty() {
                    v.push(Table { schema: None });
                }
                v
            }
        };
    }
    if matches!(tv.as_str(), "use" | "database" | "template" | "connect") {
        return vec![Database];
    }
    if tv == "tableformat" {
        return vec![TableFormat];
    }
    if tv.ends_with(',') || is_operator_token(&tv) || matches!(tv.as_str(), "and" | "or") {
        let (prev, before) = find_prev_keyword(text_before_cursor);
        return match prev {
            Some(kw) => suggest_on_token(&kw, true, before, full_text, parent),
            None => Vec::new(),
        };
    }
    // alter/create/drop/show and everything else -> keyword scope.
    vec![Keyword {
        last_token: Some(tv.to_ascii_uppercase()),
    }]
}

fn suggest_paren(
    text_before_cursor: &str,
    full_text: &str,
    parent: &Option<String>,
) -> Vec<Suggestion> {
    use Suggestion::*;
    let trimmed = text_before_cursor.trim_end();
    let before_paren = trimmed.strip_suffix('(').unwrap_or(trimmed);

    match nearest_clause_keyword(before_paren) {
        Some((kw, before)) => {
            let kl = kw.to_ascii_lowercase();
            if kl == "using" {
                return vec![Column {
                    tables: extract_tables(full_text),
                    drop_unique: true,
                }];
            }
            if kl == "exists" {
                return vec![Keyword { last_token: None }];
            }
            if matches!(kl.as_str(), "where" | "having") {
                return suggest_on_token("where", true, &before, full_text, parent);
            }
            vec![Column {
                tables: extract_tables(full_text),
                drop_unique: false,
            }]
        }
        None => vec![Column {
            tables: extract_tables(full_text),
            drop_unique: false,
        }],
    }
}

/// Like `find_prev_keyword`, but treats `(` as transparent so a WHERE clause is
/// still found through nested parentheses.
fn nearest_clause_keyword(text: &str) -> Option<(String, String)> {
    let mut cur = text.to_string();
    loop {
        let (prev, before) = find_prev_keyword(&cur);
        match prev {
            Some(k) if k == "(" => {
                let b = before
                    .trim_end()
                    .strip_suffix('(')
                    .unwrap_or_else(|| before.trim_end())
                    .to_string();
                if b.len() >= cur.len() {
                    return None;
                }
                cur = b;
            }
            Some(k) => return Some((k, before.to_string())),
            None => return None,
        }
    }
}

fn alias_list(tables: &[TableRef]) -> Vec<String> {
    tables
        .iter()
        .map(|(_, table, alias)| alias.clone().unwrap_or_else(|| table.clone()))
        .collect()
}

fn identifies(id: &str, t: &TableRef) -> bool {
    let (schema, table, alias) = t;
    Some(id) == alias.as_deref()
        || id == table
        || schema
            .as_ref()
            .is_some_and(|s| id == format!("{s}.{table}"))
}

fn is_operator_token(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| "+-*/%=<>!~".contains(c))
}

#[cfg(test)]
mod tests {
    use super::Suggestion::*;
    use super::*;

    fn col(tables: Vec<TableRef>) -> Suggestion {
        Column {
            tables,
            drop_unique: false,
        }
    }
    fn row(s: Option<&str>, t: &str, a: Option<&str>) -> TableRef {
        (s.map(String::from), t.to_string(), a.map(String::from))
    }

    #[test]
    fn select_suggests_cols_with_visible_table_scope() {
        assert_eq!(
            suggest_type("SELECT  FROM tabl", "SELECT "),
            vec![
                col(vec![row(None, "tabl", None)]),
                Function { schema: None },
                Alias {
                    aliases: vec!["tabl".into()]
                },
                Keyword {
                    last_token: Some("SELECT".into())
                },
            ]
        );
    }

    #[test]
    fn select_suggests_cols_with_qualified_table_scope() {
        assert_eq!(
            suggest_type("SELECT  FROM sch.tabl", "SELECT "),
            vec![
                col(vec![row(Some("sch"), "tabl", None)]),
                Function { schema: None },
                Alias {
                    aliases: vec!["tabl".into()]
                },
                Keyword {
                    last_token: Some("SELECT".into())
                },
            ]
        );
    }

    #[test]
    fn join_suggests_cols_with_qualified_table_scope() {
        let e = "SELECT * FROM tabl a JOIN tabl b on a.";
        assert_eq!(
            suggest_type(e, e),
            vec![
                col(vec![row(None, "tabl", Some("a"))]),
                Table {
                    schema: Some("a".into())
                },
                View {
                    schema: Some("a".into())
                },
                Function {
                    schema: Some("a".into())
                },
            ]
        );
    }

    #[test]
    fn where_suggests_columns_functions() {
        let expressions = [
            "SELECT * FROM tabl WHERE ",
            "SELECT * FROM tabl WHERE (",
            "SELECT * FROM tabl WHERE foo = ",
            "SELECT * FROM tabl WHERE bar OR ",
            "SELECT * FROM tabl WHERE foo = 1 AND ",
            "SELECT * FROM tabl WHERE (bar > 10 AND ",
            "SELECT * FROM tabl WHERE (bar AND (baz OR (qux AND (",
            "SELECT * FROM tabl WHERE foo BETWEEN foo AND ",
        ];
        for e in expressions {
            assert_eq!(
                suggest_type(e, e),
                vec![
                    col(vec![row(None, "tabl", None)]),
                    Function { schema: None },
                    Alias {
                        aliases: vec!["tabl".into()]
                    },
                    Keyword {
                        last_token: Some("WHERE".into())
                    },
                ],
                "expression: {e:?}"
            );
        }
    }

    #[test]
    fn from_suggests_tables_views_schema() {
        assert_eq!(
            suggest_type("SELECT * FROM ", "SELECT * FROM "),
            vec![Schema, Table { schema: None }, View { schema: None }]
        );
    }

    #[test]
    fn special_dispatch() {
        assert_eq!(suggest_type("\\u ", "\\u "), vec![Database]);
        assert_eq!(
            suggest_type("\\dt ", "\\dt "),
            vec![Table { schema: None }, View { schema: None }, Schema]
        );
    }

    #[test]
    fn bare_left_suggests_keyword() {
        let e = "SELECT foo FROM bar LEFT ";
        assert_eq!(
            suggest_type(e, e),
            vec![Keyword {
                last_token: Some("LEFT".into())
            }]
        );
    }
}
