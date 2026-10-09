//! Saving exported data (CSV of a table) through the platform's save dialog.

use std::path::PathBuf;

use gpui::App;

use crate::notify::{Notification, NotificationCenter};

/// Asks where to save `contents` as `file_name` and writes it there on the background
/// executor. Says how it went in a toast; cancelling the dialog does nothing. `what` names
/// the data in the toast ("42 rows").
pub fn save_text(cx: &mut App, file_name: &str, what: String, contents: String) {
    let directory = dirs::download_dir()
        .or_else(dirs::home_dir)
        .unwrap_or_else(|| PathBuf::from("."));
    let path = cx.prompt_for_new_path(&directory, Some(file_name));
    cx.spawn(async move |cx| {
        let Ok(Ok(Some(path))) = path.await else {
            return;
        };
        let target = path.clone();
        let result = cx
            .background_executor()
            .spawn(async move { std::fs::write(&target, contents) })
            .await;
        cx.update(|cx| {
            NotificationCenter::push(
                cx,
                match result {
                    Ok(()) => Notification::success(format!("Saved {what} to {}", path.display())),
                    Err(err) => Notification::error(format!("Couldn't save {what}: {err}")),
                },
            )
        });
    })
    .detach();
}

/// A file name for an export: `<name>-<yyyymmdd-hhmmss>.csv` with anything but letters,
/// digits, `-`, `_` and `.` in `name` replaced.
pub fn file_name(name: &str, extension: &str, now: jiff::Timestamp) -> String {
    let name: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    let stamp = now.strftime("%Y%m%d-%H%M%S");
    format!("{name}-{stamp}.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names_are_safe_and_stamped() {
        let at: jiff::Timestamp = "2026-10-09T13:05:09Z".parse().unwrap();
        assert_eq!(
            file_name("pods (shop/*)", "csv", at),
            "pods--shop----20261009-130509.csv"
        );
        assert_eq!(file_name("../x", "csv", at), "..-x-20261009-130509.csv");
    }
}
