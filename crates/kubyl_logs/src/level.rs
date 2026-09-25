//! Log level detection: JSON `level`/`severity` fields (names or pino/bunyan numbers), logfmt
//! (`level=info`), klog and Ruby prefixes, and level words near the start of text lines, after a
//! timestamp, thread or logger name (`2026-09-25 10:25:01,798 WARN [x]`, `[error]`, `ERRO[0000]`).
//! [`is_continuation`] recognizes stack-trace lines, which take the level of the line before.

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

    /// Level names, their common abbreviations (`ERR`, logrus' `ERRO`) and the names of Java
    /// util logging, syslog and MySQL.
    fn from_word(word: &str) -> Option<LogLevel> {
        let upper = word.trim().to_ascii_uppercase();
        Some(match upper.as_str() {
            "TRACE" | "TRC" | "TRAC" | "FINER" | "FINEST" => LogLevel::Trace,
            "DEBUG" | "DBG" | "DEBU" | "FINE" | "VERBOSE" => LogLevel::Debug,
            "INFO" | "INF" | "NOTICE" | "NOTE" | "SYSTEM" => LogLevel::Info,
            "WARN" | "WARNING" | "WRN" => LogLevel::Warn,
            "ERROR" | "ERR" | "ERRO" | "SEVERE" => LogLevel::Error,
            "FATAL" | "FATA" | "CRITICAL" | "CRIT" | "PANIC" | "PANI" | "EMERG" | "ALERT" => {
                LogLevel::Fatal
            }
            _ => return None,
        })
    }

    /// pino/bunyan numeric levels: 10 trace … 60 fatal.
    fn from_number(number: f64) -> Option<LogLevel> {
        Some(match number as i64 {
            60.. => LogLevel::Fatal,
            50.. => LogLevel::Error,
            40.. => LogLevel::Warn,
            30.. => LogLevel::Info,
            20.. => LogLevel::Debug,
            10.. => LogLevel::Trace,
            _ => return None,
        })
    }
}

/// How far into a text line the level may be: a timestamp, a pid, a thread and a logger name
/// usually come first; words later belong to the message.
const PREFIX_WORDS: usize = 12;
const PREFIX_BYTES: usize = 200;

/// Detects the level of one raw log line (without the pod/container prefix Kubyl adds).
pub fn detect_level(line: &str) -> LogLevel {
    let trimmed = line.trim_start();

    // JSON: {"level":"info", ...}, {"severity":"ERROR", ...}, {"level":50, ...}. Only the top
    // level, so this stays cheap on high-volume streams.
    if trimmed.starts_with('{')
        && let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed)
    {
        for key in [
            "level",
            "severity",
            "loglevel",
            "log_level",
            "log.level",
            "levelname",
            "lvl",
            "@l",
        ] {
            let level = match map.get(key) {
                Some(Value::String(s)) => LogLevel::from_word(s),
                Some(Value::Number(n)) => n.as_f64().and_then(LogLevel::from_number),
                _ => None,
            };
            if let Some(level) = level {
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

    let bytes = trimmed.as_bytes();
    // klog (Kubernetes components): `I0925 10:42:17.902123 ...`.
    if bytes.len() > 5
        && matches!(bytes[0], b'I' | b'W' | b'E' | b'F')
        && bytes[1..5].iter().all(u8::is_ascii_digit)
        && bytes[5] == b' '
    {
        return match bytes[0] {
            b'I' => LogLevel::Info,
            b'W' => LogLevel::Warn,
            b'E' => LogLevel::Error,
            _ => LogLevel::Fatal,
        };
    }
    // Ruby's Logger: `E, [2026-09-25T10:25:01.798 #1] ERROR -- : ...`.
    if bytes.len() > 3 && &bytes[1..4] == b", [" {
        match bytes[0] {
            b'D' => return LogLevel::Debug,
            b'I' => return LogLevel::Info,
            b'W' => return LogLevel::Warn,
            b'E' => return LogLevel::Error,
            b'F' => return LogLevel::Fatal,
            _ => {}
        }
    }

    text_level(trimmed).unwrap_or(LogLevel::Unknown)
}

/// A level word near the start of a text line. Accepted when it's uppercase (`WARN`, `ERRO[0000]`,
/// `SEVERE:`), enclosed in brackets or angle brackets in any case (`[error]`, `[Warning]`,
/// `<warn>`), between tabs (`\twarn\t`), or the line's first word (`Error: …`). Postgres'
/// `LOG:` counts as info.
fn text_level(line: &str) -> Option<LogLevel> {
    let prefix = match line.char_indices().nth(PREFIX_BYTES) {
        Some((end, _)) => &line[..end],
        None => line,
    };
    let bytes = prefix.as_bytes();
    let mut words = 0;
    let mut start = None;
    for (ix, c) in prefix
        .char_indices()
        .chain(std::iter::once((prefix.len(), ' ')))
    {
        if c.is_ascii_alphabetic() {
            start.get_or_insert(ix);
            continue;
        }
        let Some(from) = start.take() else {
            continue;
        };
        let word = &prefix[from..ix];
        let before = from.checked_sub(1).map(|i| bytes[i]);
        // The character after the word, skipping the padding of `[WARN ]`.
        let after = prefix[ix..].trim_start_matches(' ').bytes().next();
        let uppercase = word.bytes().all(|b| b.is_ascii_uppercase());
        let enclosed =
            matches!(before, Some(b'[' | b'<' | b'(')) && matches!(after, Some(b']' | b'>' | b')'));
        // Tab-separated console encoders (zap, Istio): `…Z\tinfo\tads\t…`.
        let tabbed = before == Some(b'\t') && prefix[ix..].starts_with('\t');
        // Part of a longer token (`com.example.Error`, `ErrorHandler`, `x_error`) isn't a level.
        let glued = matches!(before, Some(b'.' | b'_' | b'/' | b'$'))
            || prefix[ix..].starts_with(|c: char| c.is_ascii_digit() && !uppercase);
        if !glued {
            if uppercase && word == "LOG" && after == Some(b':') {
                return Some(LogLevel::Info);
            }
            if (uppercase || enclosed || tabbed || words == 0)
                && let Some(level) = LogLevel::from_word(word)
            {
                return Some(level);
            }
        }
        words += 1;
        if words >= PREFIX_WORDS {
            break;
        }
    }
    None
}

/// A line that continues the one before (a stack trace): indented, or a Java `Caused by:` /
/// `Suppressed:` / `... 12 more`, or the start of a Python traceback. It takes the level of
/// the line before it from the same container.
pub fn is_continuation(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    line.starts_with([' ', '\t'])
        || line.starts_with("Caused by:")
        || line.starts_with("Suppressed:")
        || line.starts_with("Traceback (most recent call last)")
        || (line.starts_with("... ") && line.trim_end().ends_with(" more"))
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
        // pino / bunyan.
        assert_eq!(detect_level(r#"{"level":50,"msg":"x"}"#), LogLevel::Error);
        assert_eq!(detect_level(r#"{"level":30,"msg":"x"}"#), LogLevel::Info);
        // ECS.
        assert_eq!(
            detect_level(r#"{"@timestamp":"2026","log.level":"warn","message":"x"}"#),
            LogLevel::Warn
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
        assert_eq!(detect_level("Error: listen EADDRINUSE"), LogLevel::Error);
    }

    #[test]
    fn detects_klog_prefixes() {
        assert_eq!(
            detect_level("E0925 10:42:17.902123       1 reflector.go:123] failed"),
            LogLevel::Error
        );
        assert_eq!(
            detect_level("I0925 10:42:17.902123 1 main.go:1] ok"),
            LogLevel::Info
        );
    }

    #[test]
    fn detects_levels_after_timestamps_and_threads() {
        let cases = [
            // Keycloak / Quarkus (JBoss log manager).
            (
                "2026-09-25 10:25:01,798 WARN  [org.keycloak.events] (executor-thread-88) type=\"REFRESH_TOKEN_ERROR\"",
                LogLevel::Warn,
            ),
            (
                "2026-09-25 10:25:16,400 ERROR [org.keycloak.services.resources.IdentityBrokerService] (executor-thread-97) invalidRequestMessage",
                LogLevel::Error,
            ),
            // Spring Boot / Logback.
            (
                "2026-09-25T10:25:01.798Z  INFO 1 --- [main] o.s.b.w.e.tomcat.TomcatWebServer : Tomcat started",
                LogLevel::Info,
            ),
            // log4j pattern with the thread first.
            (
                "10:25:01.798 [http-nio-8080-exec-1] ERROR c.e.web.Controller - request failed",
                LogLevel::Error,
            ),
            // Python logging.
            (
                "2026-09-25 10:25:01,798 - payments.worker - WARNING - retrying",
                LogLevel::Warn,
            ),
            ("ERROR:root:something broke", LogLevel::Error),
            // Go zap console encoder.
            (
                "2026-09-25T10:25:01.798Z\tDEBUG\tcontroller/reconcile.go:88\tqueued",
                LogLevel::Debug,
            ),
            // logrus text formatter.
            (
                "ERRO[0003] failed to pull image                 image=nginx",
                LogLevel::Error,
            ),
            // nginx error log.
            (
                "2026/09/25 10:25:01 [error] 29#29: *1 connect() failed",
                LogLevel::Error,
            ),
            // Envoy.
            (
                "[2026-09-25 10:25:01.798][1][warning][config] [source/common/config/grpc.cc:91] gRPC config stream closed",
                LogLevel::Warn,
            ),
            // Elasticsearch.
            (
                "[2026-09-25T10:25:01,798][WARN ][o.e.c.r.a.DiskThresholdMonitor] [node-1] high disk watermark",
                LogLevel::Warn,
            ),
            // MySQL.
            (
                "2026-09-25T10:25:01.798Z 0 [Warning] [MY-010068] [Server] CA certificate is self signed.",
                LogLevel::Warn,
            ),
            (
                "2026-09-25T10:25:01.798Z 0 [System] [MY-010931] [Server] ready for connections",
                LogLevel::Info,
            ),
            // PostgreSQL.
            (
                "2026-09-25 10:25:01.798 UTC [1] LOG:  database system is ready to accept connections",
                LogLevel::Info,
            ),
            (
                "2026-09-25 10:25:01.798 UTC [42] ERROR:  relation \"users\" does not exist",
                LogLevel::Error,
            ),
            // Java util logging.
            ("SEVERE: Servlet.service() threw exception", LogLevel::Error),
            // Ruby Logger.
            (
                "E, [2026-09-25T10:25:01.798 #1] ERROR -- : boom",
                LogLevel::Error,
            ),
            // Istio / zap production console.
            (
                "2026-09-25T10:25:01.798Z\twarn\tads\tADS: \"10.0.0.1:43210\" terminated",
                LogLevel::Warn,
            ),
            // pino-pretty.
            (
                "[10:25:01.798] WARN (payments/123): slow request",
                LogLevel::Warn,
            ),
        ];
        for (line, level) in cases {
            assert_eq!(detect_level(line), level, "{line}");
        }
    }

    #[test]
    fn ignores_level_words_inside_names_and_messages() {
        // Lowercase words in the message aren't levels…
        assert_eq!(
            detect_level("2026-09-25 10:25:01 connection error from 10.0.0.1"),
            LogLevel::Unknown
        );
        // …nor parts of class or file names.
        assert_eq!(
            detect_level("2026-09-25 10:25:01 [com.example.ErrorHandler] started"),
            LogLevel::Unknown
        );
        assert_eq!(detect_level("GET /api/errors 200 12ms"), LogLevel::Unknown);
        // A level word far into the message doesn't count.
        let late = format!("{} ERROR", "word ".repeat(20));
        assert_eq!(detect_level(&late), LogLevel::Unknown);
        assert_eq!(detect_level("hello world"), LogLevel::Unknown);
    }

    #[test]
    fn recognizes_stack_trace_continuations() {
        assert!(is_continuation("\tat org.keycloak.Foo.bar(Foo.java:12)"));
        assert!(is_continuation("    at async handler (/app/index.js:4:10)"));
        assert!(is_continuation("Caused by: java.io.IOException: closed"));
        assert!(is_continuation("... 42 more"));
        assert!(is_continuation("Traceback (most recent call last):"));
        assert!(!is_continuation("2026-09-25 10:25:01 INFO next line"));
        assert!(!is_continuation("   "));
    }
}
