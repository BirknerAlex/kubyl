//! The per-namespace table as text: its columns and CSV records.

use crate::summary::Row;

/// Column ids and titles, in table and CSV order.
pub const COLUMNS: [(&str, &str); 7] = [
    ("namespace", "Namespace"),
    ("cpu", "CPU"),
    ("ram", "Memory"),
    ("storage", "Storage"),
    ("network", "Network"),
    ("total", "Total"),
    ("efficiency", "Efficiency"),
];

/// `$1,234.57`, `$0.0042` (small amounts keep their significant digits).
pub fn money(amount: f64) -> String {
    let amount = if amount.is_finite() {
        amount.max(0.0)
    } else {
        0.0
    };
    if amount > 0.0 && amount < 0.01 {
        return format!("${amount:.4}");
    }
    let cents = (amount * 100.0).round() as u64;
    let (whole, fraction) = (cents / 100, cents % 100);
    let digits = whole.to_string();
    let mut grouped = String::new();
    for (ix, c) in digits.chars().enumerate() {
        if ix > 0 && (digits.len() - ix) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("${grouped}.{fraction:02}")
}

/// `42%` (usage over request); empty for rows that have none.
pub fn percent(efficiency: Option<f64>) -> String {
    efficiency
        .map(|e| format!("{:.0}%", e * 100.0))
        .unwrap_or_default()
}

pub fn cell(row: &Row, column: &str) -> String {
    match column {
        "namespace" => row.label().to_string(),
        "cpu" => money(row.cpu),
        "ram" => money(row.ram),
        "storage" => money(row.storage),
        "network" => money(row.network),
        "total" => money(row.total),
        "efficiency" => percent(row.efficiency),
        _ => String::new(),
    }
}

pub fn header() -> Vec<String> {
    COLUMNS.iter().map(|(_, title)| title.to_string()).collect()
}

/// CSV records with plain numbers (two decimals, no `$` or separators), the way a spreadsheet
/// wants them; efficiency as a fraction.
pub fn records<'a>(rows: impl IntoIterator<Item = &'a Row>) -> Vec<Vec<String>> {
    rows.into_iter()
        .map(|r| {
            vec![
                r.label().to_string(),
                format!("{:.4}", r.cpu),
                format!("{:.4}", r.ram),
                format!("{:.4}", r.storage),
                format!("{:.4}", r.network),
                format!("{:.4}", r.total),
                r.efficiency.map(|e| format!("{e:.4}")).unwrap_or_default(),
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str, total: f64, eff: Option<f64>) -> Row {
        Row {
            name: name.into(),
            cpu: total / 2.0,
            ram: total / 4.0,
            storage: 0.0,
            network: total / 4.0,
            total,
            efficiency: eff,
        }
    }

    #[test]
    fn money_formats() {
        assert_eq!(money(1234.5678), "$1,234.57");
        assert_eq!(money(0.0), "$0.00");
        assert_eq!(money(0.0042), "$0.0042");
        assert_eq!(money(1_000_000.0), "$1,000,000.00");
        assert_eq!(money(-3.0), "$0.00");
        assert_eq!(money(f64::NAN), "$0.00");
        assert_eq!(percent(Some(0.425)), "42%");
        assert_eq!(percent(None), "");
    }

    #[test]
    fn records_are_plain_numbers_and_guarded() {
        let rows = [
            row("shop", 4.0, Some(0.5)),
            row("=cmd|' /C calc'!A0", 2.0, Some(1.0)),
            row("__idle__", 1.0, None),
        ];
        let records = records(&rows);
        assert_eq!(
            records[0],
            [
                "shop", "2.0000", "1.0000", "0.0000", "1.0000", "4.0000", "0.5000"
            ]
        );
        assert_eq!(records[2][0], "idle");
        assert_eq!(records[2][6], "");
        let csv = kubyl_base::csv::write(&header(), &records);
        assert!(csv.starts_with("Namespace,CPU,Memory,Storage,Network,Total,Efficiency\r\n"));
        assert!(csv.contains("\r\n'=cmd|"), "{csv}");
        assert_eq!(cell(&rows[0], "total"), "$4.00");
        assert_eq!(cell(&rows[2], "efficiency"), "");
    }
}
