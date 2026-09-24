//! Formatting shared by columns, details and describe: ages like kubectl, quantities, JSON access.

use jiff::Timestamp;
use serde_json::Value;

/// A string at a JSON pointer (`/status/phase`), or `""`.
pub fn str_at<'a>(object: &'a Value, pointer: &str) -> &'a str {
    object
        .pointer(pointer)
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// An integer at a JSON pointer, or 0.
pub fn int_at(object: &Value, pointer: &str) -> i64 {
    object
        .pointer(pointer)
        .and_then(Value::as_i64)
        .unwrap_or_default()
}

/// An array at a JSON pointer (empty when missing).
pub fn array_at<'a>(object: &'a Value, pointer: &str) -> &'a [Value] {
    object
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

pub fn name(object: &Value) -> &str {
    str_at(object, "/metadata/name")
}

pub fn namespace(object: &Value) -> Option<&str> {
    object
        .pointer("/metadata/namespace")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
}

/// Parses an RFC 3339 timestamp (`metadata.creationTimestamp`).
pub fn timestamp(value: &str) -> Option<Timestamp> {
    value.parse().ok()
}

pub fn creation(object: &Value) -> Option<Timestamp> {
    timestamp(str_at(object, "/metadata/creationTimestamp"))
}

/// Seconds between `then` and `now`.
pub fn seconds_since(then: Timestamp, now: Timestamp) -> i64 {
    now.as_second() - then.as_second()
}

/// kubectl's `duration.HumanDuration`: `45s`, `3m20s`, `47m`, `3h12m`, `3d4h`, `12d`, `2y30d`.
pub fn human_duration(seconds: i64) -> String {
    if seconds < -1 {
        return "<invalid>".into();
    }
    if seconds < 0 {
        return "0s".into();
    }
    if seconds < 60 * 2 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 10 {
        let s = seconds % 60;
        return if s == 0 {
            format!("{minutes}m")
        } else {
            format!("{minutes}m{s}s")
        };
    }
    if minutes < 60 * 3 {
        return format!("{minutes}m");
    }
    let hours = minutes / 60;
    if hours < 8 {
        let m = minutes % 60;
        return if m == 0 {
            format!("{hours}h")
        } else {
            format!("{hours}h{m}m")
        };
    }
    if hours < 48 {
        return format!("{hours}h");
    }
    if hours < 24 * 8 {
        let h = hours % 24;
        return if h == 0 {
            format!("{}d", hours / 24)
        } else {
            format!("{}d{h}h", hours / 24)
        };
    }
    if hours < 24 * 365 * 2 {
        return format!("{}d", hours / 24);
    }
    if hours < 24 * 365 * 8 {
        let days = (hours / 24) % 365;
        return if days == 0 {
            format!("{}y", hours / 24 / 365)
        } else {
            format!("{}y{days}d", hours / 24 / 365)
        };
    }
    format!("{}y", hours / 24 / 365)
}

/// Age of a timestamp string, or `<unknown>`.
pub fn age(value: &str, now: Timestamp) -> String {
    match timestamp(value) {
        Some(then) => human_duration(seconds_since(then, now)),
        None => "<unknown>".into(),
    }
}

/// Age of the object (`metadata.creationTimestamp`).
pub fn object_age(object: &Value, now: Timestamp) -> String {
    age(str_at(object, "/metadata/creationTimestamp"), now)
}

/// Parses a Kubernetes quantity (`250m`, `1.5Gi`, `2`, `1e3`) into a plain number.
pub fn parse_quantity(quantity: &str) -> Option<f64> {
    let quantity = quantity.trim();
    let split = quantity
        .find(|c: char| c.is_ascii_alphabetic() && c != 'e' && c != 'E')
        .unwrap_or(quantity.len());
    let (number, suffix) = quantity.split_at(split);
    let number: f64 = number.parse().ok()?;
    let factor = match suffix {
        "" => 1.0,
        "n" => 1e-9,
        "u" => 1e-6,
        "m" => 1e-3,
        "k" => 1e3,
        "M" => 1e6,
        "G" => 1e9,
        "T" => 1e12,
        "P" => 1e15,
        "E" => 1e18,
        "Ki" => 1024.0,
        "Mi" => 1024.0_f64.powi(2),
        "Gi" => 1024.0_f64.powi(3),
        "Ti" => 1024.0_f64.powi(4),
        "Pi" => 1024.0_f64.powi(5),
        "Ei" => 1024.0_f64.powi(6),
        _ => return None,
    };
    Some(number * factor)
}

/// CPU cores as millicores (`184m`) or cores (`2.5`).
pub fn format_cpu(cores: f64) -> String {
    let millis = (cores * 1000.0).round();
    if millis < 1000.0 {
        format!("{millis}m")
    } else {
        let cores = millis / 1000.0;
        if cores.fract() == 0.0 {
            format!("{cores}")
        } else {
            format!("{cores:.1}")
        }
    }
}

/// Bytes in binary units like kubectl top (`312Mi`, `1.1Gi`).
pub fn format_bytes(bytes: f64) -> String {
    const UNITS: [&str; 5] = ["Ki", "Mi", "Gi", "Ti", "Pi"];
    if bytes < 1024.0 {
        return format!("{bytes:.0}");
    }
    let mut value = bytes / 1024.0;
    for unit in UNITS {
        if value < 1024.0 {
            return if value >= 10.0 || value.fract() < 0.05 {
                format!("{value:.0}{unit}")
            } else {
                format!("{value:.1}{unit}")
            };
        }
        value /= 1024.0;
    }
    format!("{value:.0}Ei")
}

/// `a,b,c` or `<none>`.
pub fn join_or_none<S: AsRef<str>>(items: &[S]) -> String {
    if items.is_empty() {
        "<none>".into()
    } else {
        items
            .iter()
            .map(AsRef::as_ref)
            .collect::<Vec<_>>()
            .join(",")
    }
}

/// `key=value` pairs of a label/selector map, sorted by key.
pub fn map_pairs(map: Option<&Value>) -> Vec<String> {
    let mut pairs: Vec<String> = map
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// Converts a JSON value to YAML (Copy YAML, describe).
pub fn to_yaml(value: &Value) -> String {
    serde_saphyr::to_string(value).unwrap_or_else(|err| format!("# {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn human_durations_match_kubectl() {
        let cases = [
            (0, "0s"),
            (45, "45s"),
            (119, "119s"),
            (120, "2m"),
            (200, "3m20s"),
            (600, "10m"),
            (47 * 60, "47m"),
            (3 * 3600, "3h"),
            (3 * 3600 + 12 * 60, "3h12m"),
            (9 * 3600, "9h"),
            (47 * 3600, "47h"),
            (3 * 86400 + 4 * 3600, "3d4h"),
            (12 * 86400, "12d"),
            (400 * 86400, "400d"),
            (2 * 365 * 86400 + 30 * 86400, "2y30d"),
            (9 * 365 * 86400, "9y"),
            (-5, "<invalid>"),
        ];
        for (seconds, expected) in cases {
            assert_eq!(human_duration(seconds), expected, "{seconds}s");
        }
    }

    #[test]
    fn parses_quantities() {
        assert_eq!(parse_quantity("250m"), Some(0.25));
        assert_eq!(parse_quantity("2"), Some(2.0));
        assert_eq!(parse_quantity("1Gi"), Some(1024.0 * 1024.0 * 1024.0));
        assert_eq!(parse_quantity("100M"), Some(1e8));
        assert_eq!(parse_quantity("1e3"), Some(1000.0));
        assert_eq!(parse_quantity("12Xi"), None);
        assert_eq!(format_cpu(0.184), "184m");
        assert_eq!(format_cpu(2.0), "2");
        assert_eq!(format_bytes(312.0 * 1024.0 * 1024.0), "312Mi");
        assert_eq!(format_bytes(1.1 * 1024.0 * 1024.0 * 1024.0), "1.1Gi");
    }
}
