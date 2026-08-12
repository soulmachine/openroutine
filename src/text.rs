//! Making text written by whoever can commit safe to display.
//!
//! Descriptions and errors end up in a terminal table, in a committed
//! markdown file, and on a web page. A newline or a pipe in one of them must
//! reshape none of those.

/// Collapses anything multi-line into a single readable line.
pub fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// One line, with the markdown table separator escaped as well.
pub fn table_cell(value: &str) -> String {
    one_line(value).replace('|', "\\|")
}
