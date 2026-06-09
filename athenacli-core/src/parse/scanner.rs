//! Hand-written SQL scanner for completion, replacing sqlparse. The strict
//! `sqlparser` errors on incomplete input like `SELECT x FROM t WHERE `, so we
//! tokenize loosely: just enough to find the previous keyword and pull
//! FROM/JOIN tables out of partial statements. Ported from Python
//! `packages/parseutils.py`.

use std::collections::HashSet;
use std::sync::LazyLock;

/// Character class for `last_word`, mirroring Python `cleanup_regex`.
#[derive(Clone, Copy)]
pub enum WordClass {
    /// `(\w+)$` — alphanumerics and underscore.
    AlphanumUnderscore,
    /// `([^():,\s]+)$` — everything except spaces, parens, colon, comma.
    ManyPunctuations,
    /// `([^\.():,\s]+)$` — also excludes period.
    MostPunctuations,
    /// `([^\s]+)$` — everything except whitespace.
    AllPunctuations,
}

impl WordClass {
    fn accepts(self, c: char) -> bool {
        match self {
            WordClass::AlphanumUnderscore => c.is_alphanumeric() || c == '_',
            WordClass::ManyPunctuations => {
                !c.is_whitespace() && !matches!(c, '(' | ')' | ':' | ',')
            }
            WordClass::MostPunctuations => {
                !c.is_whitespace() && !matches!(c, '.' | '(' | ')' | ':' | ',')
            }
            WordClass::AllPunctuations => !c.is_whitespace(),
        }
    }
}

/// Last word before the (implicit) cursor — the maximal trailing run of
/// characters in `class`. Empty when the text ends in whitespace.
/// Port of Python `last_word`.
pub fn last_word(text: &str, class: WordClass) -> &str {
    if text.is_empty() {
        return "";
    }
    // Walk back over accepted chars; the kept suffix is the match.
    let mut start = text.len();
    for (idx, c) in text.char_indices().rev() {
        if class.accepts(c) {
            start = idx;
        } else {
            break;
        }
    }
    &text[start..]
}

/// Split a partial identifier like `schema.partial` or `schema.` into its
/// qualifier and trailing fragment. Returns `(parent, partial)`.
pub fn parse_partial_identifier(word: &str) -> (Option<String>, String) {
    match word.rfind('.') {
        Some(dot) => {
            let parent = &word[..dot];
            let partial = &word[dot + 1..];
            let parent = if parent.is_empty() {
                None
            } else {
                Some(parent.to_string())
            };
            (parent, partial.to_string())
        }
        None => (None, word.to_string()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TokKind {
    Keyword,
    Name,
    Num,
    Str,
    Punct,
}

#[derive(Debug, Clone)]
struct Tok<'a> {
    kind: TokKind,
    text: &'a str,
    end: usize,
}

impl Tok<'_> {
    fn upper(&self) -> String {
        self.text.to_ascii_uppercase()
    }
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// Tokenize loosely, skipping whitespace and comments. Byte offsets are into the
/// original string so callers can slice prefixes.
fn tokenize(sql: &str) -> Vec<Tok<'_>> {
    let mut toks = Vec::new();
    let bytes = sql.as_bytes();
    let mut chars = sql.char_indices().peekable();

    while let Some(&(i, c)) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        // Line / block comments: skip, do not emit.
        if c == '-' && bytes.get(i + 1) == Some(&b'-') {
            while let Some(&(_, c2)) = chars.peek() {
                chars.next();
                if c2 == '\n' {
                    break;
                }
            }
            continue;
        }
        if c == '#' {
            while let Some(&(_, c2)) = chars.peek() {
                chars.next();
                if c2 == '\n' {
                    break;
                }
            }
            continue;
        }
        if c == '/' && bytes.get(i + 1) == Some(&b'*') {
            chars.next();
            chars.next();
            while let Some(&(_, c2)) = chars.peek() {
                chars.next();
                if c2 == '*' && matches!(chars.peek(), Some(&(_, '/'))) {
                    chars.next();
                    break;
                }
            }
            continue;
        }
        // Quoted string / identifier.
        if matches!(c, '\'' | '"' | '`') {
            let quote = c;
            chars.next();
            let mut end = i + c.len_utf8();
            while let Some(&(j, c2)) = chars.peek() {
                chars.next();
                end = j + c2.len_utf8();
                if c2 == quote {
                    // doubled quote escape for ' and "
                    if matches!(quote, '\'' | '"')
                        && matches!(chars.peek(), Some(&(_, q)) if q == quote)
                    {
                        chars.next();
                        continue;
                    }
                    break;
                }
            }
            toks.push(Tok {
                kind: TokKind::Str,
                text: &sql[i..end],
                end,
            });
            continue;
        }
        // Identifier / keyword.
        if is_name_start(c) {
            let mut end = i + c.len_utf8();
            chars.next();
            while let Some(&(j, c2)) = chars.peek() {
                if is_name_char(c2) {
                    end = j + c2.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
            let text = &sql[i..end];
            let kind = if KEYWORDS.contains(text.to_ascii_uppercase().as_str()) {
                TokKind::Keyword
            } else {
                TokKind::Name
            };
            toks.push(Tok { kind, text, end });
            continue;
        }
        // Number.
        if c.is_ascii_digit() {
            let mut end = i + 1;
            chars.next();
            while let Some(&(j, c2)) = chars.peek() {
                if c2.is_ascii_digit() || c2 == '.' {
                    end = j + 1;
                    chars.next();
                } else {
                    break;
                }
            }
            toks.push(Tok {
                kind: TokKind::Num,
                text: &sql[i..end],
                end,
            });
            continue;
        }
        // Single punctuation char.
        let end = i + c.len_utf8();
        toks.push(Tok {
            kind: TokKind::Punct,
            text: &sql[i..end],
            end,
        });
        chars.next();
    }
    toks
}

/// The final significant token of `sql` (whitespace/comments ignored).
pub struct LastTok {
    pub text: String,
    pub is_keyword: bool,
}

/// Last non-whitespace token, or `None` for empty/whitespace-only input.
pub fn last_token(sql: &str) -> Option<LastTok> {
    tokenize(sql).last().map(|t| LastTok {
        text: t.text.to_string(),
        is_keyword: t.kind == TokKind::Keyword,
    })
}

const LOGICAL_OPERATORS: [&str; 4] = ["AND", "OR", "NOT", "BETWEEN"];

/// Last keyword (or opening paren) before the cursor, plus the original text up
/// to and including it. Logical operators are skipped so the *clause* keyword is
/// returned. Port of Python `find_prev_keyword`.
pub fn find_prev_keyword(sql: &str) -> (Option<String>, &str) {
    if sql.trim().is_empty() {
        return (None, "");
    }
    let toks = tokenize(sql);
    for tok in toks.iter().rev() {
        let is_open_paren = tok.kind == TokKind::Punct && tok.text == "(";
        let is_kw =
            tok.kind == TokKind::Keyword && !LOGICAL_OPERATORS.contains(&tok.upper().as_str());
        if is_open_paren || is_kw {
            return (Some(tok.text.to_string()), &sql[..tok.end]);
        }
    }
    (None, "")
}

fn is_table_trigger(uv: &str) -> bool {
    matches!(uv, "FROM" | "INTO" | "UPDATE" | "COPY" | "TABLE") || uv.ends_with("JOIN")
}

/// `(schema, table, alias)` triples from FROM/JOIN/UPDATE/INTO clauses.
/// Port of Python `extract_tables` (best-effort; subqueries/CTEs are a known
/// limitation — master plan risk #3).
pub fn extract_tables(sql: &str) -> Vec<(Option<String>, String, Option<String>)> {
    let toks = tokenize(sql);
    if toks.is_empty() {
        return Vec::new();
    }
    let insert_stmt = toks[0].text.eq_ignore_ascii_case("insert");

    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let uv = toks[i].upper();
        if toks[i].kind == TokKind::Keyword && is_table_trigger(&uv) {
            i += 1;
            let mut region: Vec<&Tok> = Vec::new();
            while i < toks.len() {
                let t = &toks[i];
                if t.kind == TokKind::Keyword {
                    let tv = t.upper();
                    if tv == "FROM" || tv.ends_with("JOIN") {
                        // Another table source: flush and keep collecting.
                        parse_region(&region, insert_stmt, &mut out);
                        region.clear();
                        i += 1;
                        continue;
                    }
                    if tv == "AS" {
                        region.push(t); // part of `table AS alias`
                        i += 1;
                        continue;
                    }
                    break; // a different keyword ends the table list
                }
                if t.kind == TokKind::Punct && t.text == "(" {
                    break; // INSERT col list / subquery boundary (known limit)
                }
                region.push(t);
                i += 1;
            }
            parse_region(&region, insert_stmt, &mut out);
        } else {
            i += 1;
        }
    }
    out
}

/// Parse one comma-separated table region into `(schema, table, alias)` rows.
fn parse_region(
    region: &[&Tok],
    insert_stmt: bool,
    out: &mut Vec<(Option<String>, String, Option<String>)>,
) {
    for group in region.split(|t| t.kind == TokKind::Punct && t.text == ",") {
        let group: Vec<&&Tok> = group.iter().filter(|t| t.kind != TokKind::Num).collect();
        if group.is_empty() {
            continue;
        }
        let mut k = 0;
        // First name.
        if group[k].kind != TokKind::Name && group[k].kind != TokKind::Str {
            continue;
        }
        let first = unquote(group[k].text);
        k += 1;
        // Optional `.name` schema qualifier.
        let (schema, table) = if k < group.len() && group[k].text == "." {
            k += 1;
            if k < group.len() && (group[k].kind == TokKind::Name || group[k].kind == TokKind::Str)
            {
                let t = unquote(group[k].text);
                k += 1;
                (Some(first), t)
            } else {
                (None, first) // trailing dot, no name
            }
        } else {
            (None, first)
        };
        // Optional alias: `AS name` or bare `name`.
        let mut alias = None;
        if k < group.len() && group[k].text.eq_ignore_ascii_case("as") {
            k += 1;
        }
        if k < group.len() && (group[k].kind == TokKind::Name || group[k].kind == TokKind::Str) {
            alias = Some(unquote(group[k].text));
        }
        // INSERT's `tbl (...)` parses as a function in sqlparse, which aliases
        // the table to its own name; replicate that quirk.
        if alias.is_none() && insert_stmt {
            alias = Some(table.clone());
        }
        out.push((schema, table, alias));
    }
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if s.len() >= 2 {
        let first = bytes[0];
        let last = bytes[s.len() - 1];
        if (first == b'"' && last == b'"')
            || (first == b'`' && last == b'`')
            || (first == b'\'' && last == b'\'')
        {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

/// Does any statement in `sql` start with a destructive verb? Port of Python
/// `is_destructive`.
pub fn is_destructive(sql: &str) -> bool {
    const VERBS: [&str; 4] = ["drop", "shutdown", "delete", "truncate"];
    crate::exec::split_statements(sql)
        .iter()
        .any(|stmt| query_starts_with(stmt, &VERBS))
}

/// First keyword of `query` (comments stripped) is in `prefixes`.
pub fn query_starts_with(query: &str, prefixes: &[&str]) -> bool {
    let toks = tokenize(query);
    match toks.first() {
        Some(t) => prefixes.iter().any(|p| t.text.eq_ignore_ascii_case(p)),
        None => false,
    }
}

/// Keyword set used to classify identifier-vs-keyword tokens. Combines the
/// completion keyword tree, the literal keyword list, and the dispatch keywords
/// `suggest_type` relies on.
static KEYWORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "SELECT",
        "FROM",
        "WHERE",
        "HAVING",
        "GROUP",
        "ORDER",
        "BY",
        "JOIN",
        "INNER",
        "OUTER",
        "LEFT",
        "RIGHT",
        "FULL",
        "CROSS",
        "ON",
        "USING",
        "AS",
        "AND",
        "OR",
        "NOT",
        "BETWEEN",
        "IN",
        "IS",
        "LIKE",
        "LIMIT",
        "DISTINCT",
        "ALL",
        "UNION",
        "SET",
        "INTO",
        "UPDATE",
        "INSERT",
        "DELETE",
        "VALUES",
        "CREATE",
        "DROP",
        "ALTER",
        "TABLE",
        "VIEW",
        "DATABASE",
        "SCHEMA",
        "FUNCTION",
        "USE",
        "DESCRIBE",
        "DESC",
        "EXPLAIN",
        "SHOW",
        "TRUNCATE",
        "PARTITIONS",
        "PARTITION",
        "COPY",
        "WITH",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "CAST",
        "EXISTS",
        "TBLPROPERTIES",
        "EXTERNAL",
        "REPLACE",
        "MSCK",
        "REPAIR",
        "TEMPLATE",
        "CONNECT",
        "TABLEFORMAT",
        "COLUMNS",
        "TABLES",
        "DATABASES",
        "SCHEMAS",
        "VIEWS",
        "IF",
        "NULL",
        "ASC",
        "VALUES",
    ]
    .into_iter()
    .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    // --- last_word (Python doctests) ---------------------------------------
    #[test]
    fn last_word_alphanum() {
        let c = WordClass::AlphanumUnderscore;
        assert_eq!(last_word("abc", c), "abc");
        assert_eq!(last_word(" abc", c), "abc");
        assert_eq!(last_word("", c), "");
        assert_eq!(last_word(" ", c), "");
        assert_eq!(last_word("abc ", c), "");
        assert_eq!(last_word("abc def", c), "def");
        assert_eq!(last_word("abc def ", c), "");
        assert_eq!(last_word("abc def;", c), "");
        assert_eq!(last_word("bac $def", c), "def");
    }

    #[test]
    fn last_word_most_punctuations() {
        let c = WordClass::MostPunctuations;
        assert_eq!(last_word("bac $def", c), "$def");
        assert_eq!(last_word("bac \\def", c), "\\def");
        assert_eq!(last_word("bac \\def;", c), "\\def;");
        assert_eq!(last_word("bac::def", c), "def");
    }

    #[test]
    fn partial_identifier_split() {
        assert_eq!(
            parse_partial_identifier("sch.tabl"),
            (Some("sch".into()), "tabl".into())
        );
        assert_eq!(
            parse_partial_identifier("a."),
            (Some("a".into()), "".into())
        );
        assert_eq!(parse_partial_identifier("tabl"), (None, "tabl".into()));
    }

    // --- extract_tables (Python test_parseutils.py) -------------------------
    fn et(sql: &str) -> Vec<(Option<String>, String, Option<String>)> {
        let mut v = extract_tables(sql);
        v.sort();
        v
    }
    fn row(s: Option<&str>, t: &str, a: Option<&str>) -> (Option<String>, String, Option<String>) {
        (s.map(String::from), t.to_string(), a.map(String::from))
    }

    #[test]
    fn extract_empty() {
        assert_eq!(extract_tables(""), Vec::new());
    }

    #[test]
    fn extract_simple() {
        assert_eq!(
            extract_tables("select * from abc"),
            vec![row(None, "abc", None)]
        );
        assert_eq!(
            extract_tables("select * from abc.def"),
            vec![row(Some("abc"), "def", None)]
        );
        assert_eq!(
            et("select * from abc, def"),
            vec![row(None, "abc", None), row(None, "def", None)]
        );
    }

    #[test]
    fn extract_with_cols() {
        assert_eq!(
            extract_tables("select a,b from abc"),
            vec![row(None, "abc", None)]
        );
        assert_eq!(
            et("select a,b from abc.def, def.ghi"),
            vec![row(Some("abc"), "def", None), row(Some("def"), "ghi", None)]
        );
    }

    #[test]
    fn extract_hanging_comma_and_period() {
        assert_eq!(
            extract_tables("select a, from abc"),
            vec![row(None, "abc", None)]
        );
        assert_eq!(
            et("SELECT t1. FROM tabl1 t1, tabl2 t2"),
            vec![
                row(None, "tabl1", "t1".into()),
                row(None, "tabl2", "t2".into())
            ]
        );
    }

    #[test]
    fn extract_insert_and_update() {
        assert_eq!(
            extract_tables("insert into abc (id, name) values (1, \"def\")"),
            vec![row(None, "abc", Some("abc"))]
        );
        assert_eq!(
            extract_tables("update abc set id = 1"),
            vec![row(None, "abc", None)]
        );
        assert_eq!(
            extract_tables("update abc.def set id = 1"),
            vec![row(Some("abc"), "def", None)]
        );
    }

    #[test]
    fn extract_joins() {
        assert_eq!(
            et("SELECT * FROM abc a JOIN def d ON a.id = d.num"),
            vec![row(None, "abc", Some("a")), row(None, "def", Some("d"))]
        );
        assert_eq!(
            extract_tables("SELECT * FROM abc.def x JOIN ghi.jkl y ON x.id = y.num"),
            vec![
                row(Some("abc"), "def", Some("x")),
                row(Some("ghi"), "jkl", Some("y"))
            ]
        );
        assert_eq!(
            extract_tables("SELECT * FROM my_table AS m WHERE m.a > 5"),
            vec![row(None, "my_table", Some("m"))]
        );
    }

    // --- find_prev_keyword --------------------------------------------------
    #[test]
    fn prev_keyword_basic() {
        assert_eq!(find_prev_keyword("SELECT ").0, Some("SELECT".into()));
        assert_eq!(
            find_prev_keyword("SELECT * FROM tabl WHERE ").0,
            Some("WHERE".into())
        );
        // logical operators are skipped
        assert_eq!(
            find_prev_keyword("SELECT * FROM tabl WHERE foo = 1 AND ").0,
            Some("WHERE".into())
        );
        // opening paren counts
        assert_eq!(
            find_prev_keyword("SELECT * FROM tabl WHERE (").0,
            Some("(".into())
        );
        assert_eq!(find_prev_keyword("").0, None);
    }

    // --- is_destructive (Python test_parseutils.py) -------------------------
    #[test]
    fn destructive_detection() {
        assert!(is_destructive(
            "use test;\nshow databases;\ndrop database foo;"
        ));
        assert!(!is_destructive("use test;\nshow databases;"));
        assert!(query_starts_with("USE test;", &["use"]));
        assert!(!query_starts_with("DROP DATABASE test;", &["use"]));
        assert!(query_starts_with("# comment\nUSE test;", &["use"]));
    }
}
