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

/// Render a slice of [`rdpilot_ipc::WireWindowInfo`] as a table
/// (hwnd/title/rect/z_order/state/class_name/pid), reusing [`render_table`].
#[must_use]
pub fn render_window_table(windows: &[rdpilot_ipc::WireWindowInfo]) -> String {
    const HEADERS: [&str; 7] = ["hwnd", "title", "rect", "z_order", "state", "class_name", "pid"];
    let rows: Vec<Vec<String>> = windows
        .iter()
        .map(|w| {
            vec![
                w.hwnd.to_string(),
                w.title.clone(),
                format!("{}x{}+{}+{}", w.rect.w, w.rect.h, w.rect.x, w.rect.y),
                w.z_order.to_string(),
                window_state_str(w.state).to_owned(),
                w.class_name.clone(),
                w.pid.to_string(),
            ]
        })
        .collect();
    render_table(&HEADERS, &rows)
}

fn window_state_str(state: rdpilot_ipc::WireWindowState) -> &'static str {
    match state {
        rdpilot_ipc::WireWindowState::Normal => "normal",
        rdpilot_ipc::WireWindowState::Minimized => "minimized",
        rdpilot_ipc::WireWindowState::Maximized => "maximized",
    }
}

/// Render a slice of [`rdpilot_ipc::WireProcessInfo`] as a table
/// (pid/parent_pid/name/path/command_line/owner), reusing [`render_table`].
#[must_use]
pub fn render_process_table(processes: &[rdpilot_ipc::WireProcessInfo]) -> String {
    const HEADERS: [&str; 6] = ["pid", "parent_pid", "name", "path", "command_line", "owner"];
    let rows: Vec<Vec<String>> = processes
        .iter()
        .map(|p| {
            vec![
                p.pid.to_string(),
                p.parent_pid.to_string(),
                p.name.clone(),
                p.path.clone(),
                p.command_line.clone().unwrap_or_default(),
                p.owner.clone().unwrap_or_default(),
            ]
        })
        .collect();
    render_table(&HEADERS, &rows)
}

/// Render a slice of [`rdpilot_ipc::WireUiaElement`] as a table
/// (id/role/name/bbox/depth), reusing [`render_table`].
#[must_use]
pub fn render_uia_table(elements: &[rdpilot_ipc::WireUiaElement]) -> String {
    const HEADERS: [&str; 5] = ["id", "role", "name", "bbox", "depth"];
    let rows: Vec<Vec<String>> = elements
        .iter()
        .map(|e| {
            vec![
                e.id.clone(),
                e.role.clone(),
                e.name.clone(),
                format!("{}x{}+{}+{}", e.bbox.w, e.bbox.h, e.bbox.x, e.bbox.y),
                e.depth.to_string(),
            ]
        })
        .collect();
    render_table(&HEADERS, &rows)
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
