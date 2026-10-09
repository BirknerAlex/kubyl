//! CSV export of tables: an RFC 4180 writer that is safe to open in a spreadsheet.
//!
//! Every table that offers "Export CSV" (resource lists, Security Center, Cost) goes through
//! [`write`], so the formula guard and the quoting rules live in one place.
//!
//! - Fields are quoted when they hold a comma, a quote, CR or LF (quotes doubled), records end
//!   in CRLF, and every record has the header's number of fields.
//! - **Formula injection:** a cell that starts with `=`, `+`, `-`, `@`, a tab or a CR would run
//!   as a formula in Excel, LibreOffice or Sheets, and object names, labels and annotations
//!   are written by whoever can create the object. [`guard`] puts a `'` in front of such a
//!   cell, which spreadsheets show as plain text (OWASP's CSV injection advice).
//! - Callers decide which columns they export. Secret lists have key counts only; nothing here
//!   looks into cells.

use std::borrow::Cow;

use crate::columns::CellValue;

/// The characters that make a spreadsheet read a cell as a formula.
const FORMULA_STARTS: [char; 6] = ['=', '+', '-', '@', '\t', '\r'];

/// `text` as plain text in a spreadsheet: a leading `'` when it would be read as a formula.
pub fn guard(text: &str) -> Cow<'_, str> {
    if text.starts_with(FORMULA_STARTS) {
        Cow::Owned(format!("'{text}"))
    } else {
        Cow::Borrowed(text)
    }
}

/// One field: guarded against formulas, quoted when it needs to be.
pub fn field(text: &str) -> String {
    let text = guard(text);
    if text.contains([',', '"', '\r', '\n']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.into_owned()
    }
}

/// One record without its line ending.
pub fn record<S: AsRef<str>>(fields: &[S]) -> String {
    let mut line = String::new();
    for (ix, value) in fields.iter().enumerate() {
        if ix > 0 {
            line.push(',');
        }
        line.push_str(&field(value.as_ref()));
    }
    line
}

/// A table as CSV: the header, then one record per row (CRLF after each). Rows shorter than
/// the header are padded with empty fields and longer ones cut, so the file stays rectangular.
pub fn write<H: AsRef<str>, S: AsRef<str>>(header: &[H], rows: &[Vec<S>]) -> String {
    let mut out = record(header);
    out.push_str("\r\n");
    for row in rows {
        let mut fields: Vec<&str> = row.iter().map(AsRef::as_ref).take(header.len()).collect();
        fields.resize(header.len(), "");
        out.push_str(&record(&fields));
        out.push_str("\r\n");
    }
    out
}

/// What a cell shows, as text. Buttons (one per port, links to other objects) join with
/// `, ` by their labels; `button_label` reads a button's label.
pub fn cell_text<B>(cell: &CellValue<B>, button_label: impl Fn(&B) -> &str) -> String {
    match cell {
        CellValue::Text(label)
        | CellValue::Tinted { label, .. }
        | CellValue::Status { label, .. }
        | CellValue::Usage { label, .. } => label.to_string(),
        CellValue::Buttons(buttons) => buttons
            .iter()
            .map(button_label)
            .collect::<Vec<_>>()
            .join(", "),
        CellValue::Empty => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tone;

    #[test]
    fn plain_fields_are_not_quoted() {
        assert_eq!(field("nginx-7d9"), "nginx-7d9");
        assert_eq!(field(""), "");
        assert_eq!(field("1/1"), "1/1");
    }

    #[test]
    fn quotes_commas_and_line_breaks() {
        assert_eq!(field("a,b"), "\"a,b\"");
        assert_eq!(field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(field("one\ntwo"), "\"one\ntwo\"");
        assert_eq!(field("one\r\ntwo"), "\"one\r\ntwo\"");
    }

    #[test]
    fn formula_starts_are_neutralized() {
        for text in ["=1+1", "+1", "-1", "@SUM(A1)", "\tcmd", "\rcmd"] {
            let out = field(text);
            assert!(
                out.starts_with('\'') || out.starts_with("\"'"),
                "{text:?} -> {out:?}"
            );
        }
        // Only the start matters.
        assert_eq!(field("a=b"), "a=b");
        assert_eq!(field("5-2"), "5-2");
        // A classic payload, also when it needs quoting.
        assert_eq!(
            field("=HYPERLINK(\"http://evil\",\"x\")"),
            "\"'=HYPERLINK(\"\"http://evil\"\",\"\"x\"\")\""
        );
    }

    #[test]
    fn writes_crlf_records_and_keeps_the_table_rectangular() {
        let out = write(
            &["Name", "Ready"],
            &[vec!["a", "1/1"], vec!["b"], vec!["c", "0/1", "extra"]],
        );
        assert_eq!(out, "Name,Ready\r\na,1/1\r\nb,\r\nc,0/1\r\n");
    }

    #[test]
    fn header_cells_are_guarded_too() {
        assert_eq!(record(&["=x", "ok"]), "'=x,ok");
    }

    #[test]
    fn cells_flatten_to_their_label() {
        let text = |cell: CellValue<&str>| cell_text(&cell, |b| b);
        assert_eq!(text(CellValue::Text("Running".into())), "Running");
        assert_eq!(
            text(CellValue::Status {
                label: "Failed".into(),
                tone: Tone::Bad
            }),
            "Failed"
        );
        assert_eq!(
            text(CellValue::Usage {
                label: "184m".into(),
                percent: 42.0
            }),
            "184m"
        );
        assert_eq!(text(CellValue::Buttons(vec!["80", "443"])), "80, 443");
        assert_eq!(text(CellValue::Empty), "");
    }
}
