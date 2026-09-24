//! Translates a GPUI keystroke into the bytes a PTY expects. Covers printable characters,
//! control keys, arrows/navigation and common Ctrl combinations — enough for shells, `vim` and
//! `htop`. Exotic function-key/modifier combinations are best-effort; see the phase 05 handoff
//! log.

/// A minimal, GPUI-independent description of a keystroke (kept separate from
/// `gpui::Keystroke` so the encoder is unit-testable without a GPUI context).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyInput {
    /// GPUI's key name: `"a"`, `"enter"`, `"up"`, `"backspace"`…
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// The characters GPUI resolved for this keystroke (layout-aware), when it produced any.
    pub ime_key: Option<String>,
}

/// Encodes a keystroke to the bytes to write to the PTY's stdin, or `None` for keys that don't
/// produce input (a bare modifier, `Escape` handled elsewhere, etc. still return `Some`).
pub fn encode(key: &KeyInput) -> Option<Vec<u8>> {
    if key.control
        && let Some(byte) = control_byte(&key.key)
    {
        return Some(vec![byte]);
    }
    let sequence = match key.key.as_str() {
        "enter" => "\r",
        "tab" => "\t",
        "backspace" => "\x7f",
        "escape" => "\x1b",
        "up" => "\x1b[A",
        "down" => "\x1b[B",
        "right" => "\x1b[C",
        "left" => "\x1b[D",
        "home" => "\x1b[H",
        "end" => "\x1b[F",
        "pageup" => "\x1b[5~",
        "pagedown" => "\x1b[6~",
        "insert" => "\x1b[2~",
        "delete" => "\x1b[3~",
        "f1" => "\x1bOP",
        "f2" => "\x1bOQ",
        "f3" => "\x1bOR",
        "f4" => "\x1bOS",
        _ => "",
    };
    if !sequence.is_empty() {
        return Some(sequence.as_bytes().to_vec());
    }
    if let Some(text) = &key.ime_key
        && !text.is_empty()
    {
        return Some(text.as_bytes().to_vec());
    }
    if key.key.chars().count() == 1 {
        let mut c = key.key.chars().next().unwrap();
        if key.shift {
            c = c.to_ascii_uppercase();
        }
        let mut buf = [0u8; 4];
        return Some(c.encode_utf8(&mut buf).as_bytes().to_vec());
    }
    None
}

/// `Ctrl-<letter>` and friends map to bytes 0x00-0x1f.
fn control_byte(key: &str) -> Option<u8> {
    if key.len() == 1 {
        let c = key.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() {
            return Some((c as u8) - b'A' + 1);
        }
    }
    match key {
        "[" => Some(0x1b),
        "\\" => Some(0x1c),
        "]" => Some(0x1d),
        "space" => Some(0x00),
        _ => None,
    }
}

/// The bracketed-paste wrapper (`ESC [200~ ... ESC [201~`), so pasted multi-line text isn't
/// auto-indented by the shell.
pub fn bracketed_paste(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + 12);
    out.extend_from_slice(b"\x1b[200~");
    out.extend_from_slice(text.as_bytes());
    out.extend_from_slice(b"\x1b[201~");
    out
}

/// The control sequence sent when the terminal window is resized, if the application asked for
/// it (`\x1b[8;{rows};{cols}t`, xterm-style). Kubyl sends the size out-of-band over the exec
/// channel's `TerminalSize` message instead; this is only for apps that also poll `SIGWINCH`
/// via the terminal report.
pub fn resize_report(columns: u16, rows: u16) -> Vec<u8> {
    format!("\x1b[8;{rows};{columns}t").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> KeyInput {
        KeyInput {
            key: k.to_string(),
            shift: false,
            control: false,
            alt: false,
            ime_key: None,
        }
    }

    #[test]
    fn printable_letters_round_trip() {
        assert_eq!(encode(&key("a")), Some(b"a".to_vec()));
        let mut shifted = key("a");
        shifted.shift = true;
        assert_eq!(encode(&shifted), Some(b"A".to_vec()));
    }

    #[test]
    fn navigation_keys_use_csi_sequences() {
        assert_eq!(encode(&key("up")), Some(b"\x1b[A".to_vec()));
        assert_eq!(encode(&key("left")), Some(b"\x1b[D".to_vec()));
        assert_eq!(encode(&key("enter")), Some(b"\r".to_vec()));
        assert_eq!(encode(&key("backspace")), Some(b"\x7f".to_vec()));
    }

    #[test]
    fn ctrl_letters_map_to_control_bytes() {
        let mut ctrl_c = key("c");
        ctrl_c.control = true;
        assert_eq!(encode(&ctrl_c), Some(vec![0x03]));
        let mut ctrl_d = key("d");
        ctrl_d.control = true;
        assert_eq!(encode(&ctrl_d), Some(vec![0x04]));
    }

    #[test]
    fn ime_text_is_used_when_present() {
        let mut k = key("unidentified");
        k.ime_key = Some("é".into());
        assert_eq!(encode(&k), Some("é".as_bytes().to_vec()));
    }

    #[test]
    fn bracketed_paste_wraps_the_text() {
        let wrapped = bracketed_paste("hi");
        assert_eq!(wrapped, b"\x1b[200~hi\x1b[201~");
    }
}
