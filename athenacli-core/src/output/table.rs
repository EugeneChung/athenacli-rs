//! Result rendering: ASCII table (REPL default), CSV (`-e` default), and
//! vertical (`\G`). Replaces Python `cli_helpers.tabular_output`.

use comfy_table::{presets, ContentArrangement, Table, TableComponent};

/// Warn before printing more rows than this (Python `threshold = 1000`).
pub const ROW_THRESHOLD: usize = 1000;

fn cell(value: &Option<String>) -> &str {
    value.as_deref().unwrap_or("")
}

/// Render a result set. Returns an empty string when there are no rows
/// (Python prints only the status line in that case).
pub fn render(
    headers: &[String],
    rows: &[Vec<Option<String>>],
    format: &str,
    expanded: bool,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    if expanded || format.eq_ignore_ascii_case("vertical") {
        render_vertical(headers, rows)
    } else if format.eq_ignore_ascii_case("csv") {
        render_csv(headers, rows)
    } else {
        render_ascii(headers, rows)
    }
}

fn render_ascii(headers: &[String], rows: &[Vec<Option<String>>]) -> String {
    let mut table = Table::new();
    table.load_preset(presets::ASCII_FULL);
    // MySQL/mycli look: keep outer borders + header underline, drop per-row lines.
    table.remove_style(TableComponent::HorizontalLines);
    table.remove_style(TableComponent::MiddleIntersections);
    table.remove_style(TableComponent::LeftBorderIntersections);
    table.remove_style(TableComponent::RightBorderIntersections);
    table.set_content_arrangement(ContentArrangement::Disabled);
    table.set_header(headers.iter().cloned());
    for row in rows {
        table.add_row(row.iter().map(|c| cell(c).to_string()));
    }
    table.to_string()
}

fn render_csv(headers: &[String], rows: &[Vec<Option<String>>]) -> String {
    let mut lines = Vec::with_capacity(rows.len() + 1);
    lines.push(csv_record(headers.iter().map(String::as_str)));
    for row in rows {
        lines.push(csv_record(row.iter().map(cell)));
    }
    lines.join("\n")
}

fn csv_record<'a>(fields: impl Iterator<Item = &'a str>) -> String {
    fields.map(csv_field).collect::<Vec<_>>().join(",")
}

fn csv_field(field: &str) -> String {
    if field.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

fn render_vertical(headers: &[String], rows: &[Vec<Option<String>>]) -> String {
    let width = headers.iter().map(|h| h.chars().count()).max().unwrap_or(0);
    let mut out = String::new();
    for (i, row) in rows.iter().enumerate() {
        out.push_str(&format!(
            "***************************[ {}. row ]***************************\n",
            i + 1
        ));
        for (header, value) in headers.iter().zip(row) {
            out.push_str(&format!("{header:>width$} | {}\n", cell(value)));
        }
    }
    out.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(vals: &[&str]) -> Vec<Option<String>> {
        vals.iter().map(|v| Some(v.to_string())).collect()
    }

    #[test]
    fn empty_rows_render_nothing() {
        assert_eq!(render(&["a".into()], &[], "ascii", false), "");
    }

    #[test]
    fn ascii_has_borders_headers_and_values() {
        let out = render(
            &["id".into(), "name".into()],
            &[row(&["1", "alice"])],
            "ascii",
            false,
        );
        assert!(out.contains("id"));
        assert!(out.contains("alice"));
        assert!(out.contains('+'));
        assert!(out.contains('|'));
    }

    #[test]
    fn csv_quotes_only_when_needed() {
        let out = render(
            &["a".into(), "b".into()],
            &[row(&["x,y", "plain"])],
            "csv",
            false,
        );
        assert_eq!(out, "a,b\n\"x,y\",plain");
    }

    #[test]
    fn csv_escapes_embedded_quotes() {
        assert_eq!(csv_field("he\"llo"), "\"he\"\"llo\"");
        assert_eq!(csv_field("plain"), "plain");
    }

    #[test]
    fn null_renders_as_empty() {
        let out = render(
            &["a".into(), "b".into()],
            &[vec![None, Some("z".into())]],
            "csv",
            false,
        );
        assert_eq!(out, "a,b\n,z");
    }

    #[test]
    fn vertical_header_and_alignment() {
        let out = render(
            &["id".into(), "name".into()],
            &[row(&["1", "alice"]), row(&["2", "bob"])],
            "ascii",
            true,
        );
        assert!(out.contains("***************************[ 1. row ]***************************"));
        assert!(out.contains("***************************[ 2. row ]***************************"));
        // names right-justified to width 4 ("name")
        assert!(out.contains("  id | 1"));
        assert!(out.contains("name | alice"));
    }
}
