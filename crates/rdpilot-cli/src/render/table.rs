//! A small hand-rolled column-aligned table printer — no external
//! table-formatting crate (research "Don't Hand-Roll" scope explicitly
//! excludes this: a handful of columns doesn't justify a dependency).

/// Render `rows` (each row already-stringified per column, in `headers`
/// order) as a column-aligned, whitespace-padded table. Column widths are
/// the max cell width (including the header) per column; columns are
/// separated by a fixed two-space gutter, and trailing padding on the last
/// column of each line is trimmed.
#[must_use]
pub fn render_table(headers: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            if let Some(w) = widths.get_mut(i) {
                *w = (*w).max(cell.len());
            }
        }
    }

    let header_cells: Vec<String> = headers.iter().map(|h| (*h).to_owned()).collect();
    let mut out = String::new();
    push_row(&mut out, &header_cells, &widths);
    for row in rows {
        push_row(&mut out, row, &widths);
    }
    out
}

fn push_row(out: &mut String, cells: &[String], widths: &[usize]) {
    let mut parts = Vec::with_capacity(cells.len());
    for (i, cell) in cells.iter().enumerate() {
        let width = widths.get(i).copied().unwrap_or(cell.len());
        parts.push(format!("{cell:<width$}"));
    }
    out.push_str(parts.join("  ").trim_end());
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_header_and_column_aligned_rows() {
        let out = render_table(
            &["id", "name"],
            &[vec!["brave-otter".to_owned(), "web".to_owned()], vec!["x".to_owned(), "y".to_owned()]],
        );
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("id "));
        assert!(lines[1].starts_with("brave-otter"));
        // The "id" column is padded to "brave-otter"'s width (11 chars) on
        // every row, including the header.
        assert!(lines[0].starts_with("id         "));
    }

    #[test]
    fn renders_header_only_for_an_empty_row_set() {
        let out = render_table(&["id", "status"], &[]);
        assert_eq!(out, "id  status\n");
    }
}
