//! Log level detection: JSON `level`/`severity` fields, logfmt (`level=info`), and common
//! bracketed/prefixed text patterns (`[ERROR]`, `ERROR:`, `WARN `…).

use serde_json::Value;

/// A detected (or unknown) log level.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
    Fatal,
    Unknown,
}

impl LogLevel {
    pub const ALL: [LogLevel; 7] = [
        LogLevel::Trace,
        LogLevel::Debug,
        LogLevel::Info,
        LogLevel::Warn,
        LogLevel::Error,
        LogLevel::Fatal,
        LogLevel::Unknown,
    ];

    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
            LogLevel::Unknown => "—",
        }
    }

    fn from_word(word: &str) -> Option<LogLevel> {
        let upper = word.trim().to_ascii_uppercase();
        Some(match upper.as_str() {
            "TRACE" | "TRC" => LogLevel::Trace,
            "DEBUG" | "DBG" => LogLevel::Debug,
            "INFO" | "INF" | "NOTICE" => LogLevel::Info,
            "WARN" | "WARNING" | "WRN" => LogLevel::Warn,
            "ERROR" | "ERR" => LogLevel::Error,
            "FATAL" | "CRITICAL" | "CRIT" | "PANIC" => LogLevel::Fatal,
            _ => return None,
        })
    }
}

/// Detects the level of one raw log line (without the pod/container prefix Kubyl adds).
pub fn detect_level(line: &str) -> LogLevel {
    let trimmed = line.trim_start();

    // JSON: {"level":"info", ...} or {"severity":"ERROR", ...}. Only look at the top level so
    // this stays cheap on high-volume streams.
    if trimmed.starts_with('{')
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed)
    {
        for key in ["level", "severity", "loglevel", "log_level"] {
            if let Some(Value::String(s)) = map.get(key)
                && let Some(level) = LogLevel::from_word(s)
            {
                return level;
            }
        }
    }

    // logfmt: `... level=info ...` or `... severity=ERROR ...`.
    for key in ["level=", "severity=", "loglevel="] {
        if let Some(pos) = trimmed.find(key) {
            let rest = &trimmed[pos + key.len()..];
            let value = rest
                .trim_start_matches('"')
                .split(|c: char| c.is_whitespace() || c == '"')
                .next()
                .unwrap_or("");
            if let Some(level) = LogLevel::from_word(value) {
                return level;
            }
        }
    }

    // Bracketed or prefixed text: `[ERROR]`, `ERROR:`, `WARN  `.
    let candidates = [
        "TRACE", "DEBUG", "INFO", "WARN", "WARNING", "ERROR", "FATAL",
    ];
    for word in candidates {
        let bracketed = format!("[{word}]");
        let colon = format!("{word}:");
        let prefixed = trimmed
            .split(|c: char| !c.is_ascii_alphabetic())
            .next()
            .is_some_and(|first| first.eq_ignore_ascii_case(word));
        if trimmed.starts_with(&bracketed) || trimmed.starts_with(&colon) || prefixed {
            return LogLevel::from_word(word).unwrap_or(LogLevel::Unknown);
        }
    }

    LogLevel::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_json_level() {
        assert_eq!(
            detect_level(r#"{"level":"warn","msg":"disk low"}"#),
            LogLevel::Warn
        );
        assert_eq!(
            detect_level(r#"{"severity":"ERROR","msg":"boom"}"#),
            LogLevel::Error
        );
    }

    #[test]
    fn detects_logfmt_level() {
        assert_eq!(
            detect_level(r#"time=2026-09-25 level=info msg="started""#),
            LogLevel::Info
        );
    }

    #[test]
    fn detects_bracketed_and_prefixed_text() {
        assert_eq!(detect_level("[ERROR] connection refused"), LogLevel::Error);
        assert_eq!(detect_level("WARN: retrying in 5s"), LogLevel::Warn);
        assert_eq!(detect_level("FATAL could not bind port"), LogLevel::Fatal);
    }

    #[test]
    fn unknown_when_nothing_matches() {
        assert_eq!(detect_level("hello world"), LogLevel::Unknown);
    }
}
