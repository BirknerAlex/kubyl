//! Translates keystrokes, pastes and mouse events into the bytes a PTY expects (xterm
//! conventions): printable text, control keys, navigation keys with modifiers, the
//! application-cursor mode, function keys, meta (Alt) prefixes, bracketed paste and SGR/X10
//! mouse reports. GPUI-free and unit-tested.

/// A minimal, GPUI-independent description of a keystroke.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct KeyInput {
    /// GPUI's key name: `"a"`, `"enter"`, `"up"`, `"backspace"`…
    pub key: String,
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    /// Cmd on macOS, the Windows/Super key elsewhere. Never sent to the program.
    pub platform: bool,
    /// The characters GPUI resolved for this keystroke (layout-aware), when it produced any.
    pub key_char: Option<String>,
}

impl KeyInput {
    /// Parses GPUI keystroke syntax (`ctrl-c`, `alt-left`, `shift-tab`) for bindings that send
    /// a key to the terminal.
    pub fn parse(keystroke: &str) -> Self {
        let mut input = KeyInput::default();
        let mut parts: Vec<&str> = keystroke.split('-').collect();
        // `ctrl--` (minus) splits into an empty last part.
        let key = if keystroke.ends_with("--") {
            parts.truncate(parts.len().saturating_sub(2));
            "-".to_string()
        } else {
            parts.pop().unwrap_or_default().to_string()
        };
        for modifier in parts {
            match modifier {
                "ctrl" => input.control = true,
                "alt" => input.alt = true,
                "shift" => input.shift = true,
                "cmd" | "super" | "win" | "platform" => input.platform = true,
                _ => {}
            }
        }
        input.key = key;
        input
    }
}

/// Terminal state that changes how keys are encoded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyModes {
    /// DECCKM: arrows send `ESC O A` instead of `ESC [ A`.
    pub app_cursor: bool,
    /// Alt/Option sends an `ESC` prefix (meta). Otherwise the layout's character is sent (for
    /// macOS Option compositions like `@` or `€`).
    pub alt_is_meta: bool,
}

/// The xterm modifier parameter: 1 + shift + 2·alt + 4·ctrl.
fn modifier_param(key: &KeyInput) -> u8 {
    1 + key.shift as u8 + 2 * key.alt as u8 + 4 * key.control as u8
}

/// Encodes a keystroke to the bytes to write to the PTY's stdin, or `None` for keys that
/// don't produce input (bare modifiers, Cmd shortcuts).
pub fn encode(key: &KeyInput, modes: KeyModes) -> Option<Vec<u8>> {
    if key.platform {
        return None;
    }
    let modified = key.shift || key.alt || key.control;
    let meta = |mut bytes: Vec<u8>| {
        if key.alt {
            bytes.insert(0, 0x1b);
        }
        bytes
    };

    // Cursor keys: `ESC [ A`, `ESC O A` (application mode) or `ESC [ 1 ; m A`.
    let cursor = match key.key.as_str() {
        "up" => Some('A'),
        "down" => Some('B'),
        "right" => Some('C'),
        "left" => Some('D'),
        "home" => Some('H'),
        "end" => Some('F'),
        _ => None,
    };
    if let Some(letter) = cursor {
        let sequence = if modified {
            format!("\x1b[1;{}{letter}", modifier_param(key))
        } else if modes.app_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        };
        return Some(sequence.into_bytes());
    }

    // `ESC [ n ~` keys.
    let tilde = match key.key.as_str() {
        "insert" => Some(2),
        "delete" => Some(3),
        "pageup" => Some(5),
        "pagedown" => Some(6),
        "f5" => Some(15),
        "f6" => Some(17),
        "f7" => Some(18),
        "f8" => Some(19),
        "f9" => Some(20),
        "f10" => Some(21),
        "f11" => Some(23),
        "f12" => Some(24),
        _ => None,
    };
    if let Some(n) = tilde {
        let sequence = if modified {
            format!("\x1b[{n};{}~", modifier_param(key))
        } else {
            format!("\x1b[{n}~")
        };
        return Some(sequence.into_bytes());
    }

    // F1-F4: `ESC O P` or `ESC [ 1 ; m P`.
    let ss3 = match key.key.as_str() {
        "f1" => Some('P'),
        "f2" => Some('Q'),
        "f3" => Some('R'),
        "f4" => Some('S'),
        _ => None,
    };
    if let Some(letter) = ss3 {
        let sequence = if modified {
            format!("\x1b[1;{}{letter}", modifier_param(key))
        } else {
            format!("\x1bO{letter}")
        };
        return Some(sequence.into_bytes());
    }

    match key.key.as_str() {
        "enter" => return Some(meta(b"\r".to_vec())),
        "tab" if key.shift => return Some(b"\x1b[Z".to_vec()),
        "tab" => return Some(meta(b"\t".to_vec())),
        "backspace" if key.control => return Some(meta(vec![0x08])),
        "backspace" => return Some(meta(vec![0x7f])),
        "escape" => return Some(meta(vec![0x1b])),
        _ => {}
    }

    if key.control
        && let Some(byte) = control_byte(&key.key, key.shift)
    {
        return Some(meta(vec![byte]));
    }

    // Printable text.
    let single = (key.key.chars().count() == 1).then(|| {
        let c = key.key.chars().next().unwrap_or(' ');
        if key.shift { c.to_ascii_uppercase() } else { c }
    });
    if key.alt && modes.alt_is_meta {
        let text = match single {
            Some(c) => c.to_string(),
            None if key.key == "space" => " ".into(),
            None => key.key_char.clone()?,
        };
        return Some(meta(text.into_bytes()));
    }
    if let Some(text) = &key.key_char
        && !text.is_empty()
    {
        return Some(text.as_bytes().to_vec());
    }
    if key.key == "space" {
        return Some(b" ".to_vec());
    }
    let c = single?;
    let mut buf = [0u8; 4];
    Some(c.encode_utf8(&mut buf).as_bytes().to_vec())
}

/// `Ctrl-<key>` and friends map to bytes 0x00-0x1f.
fn control_byte(key: &str, shift: bool) -> Option<u8> {
    if key.len() == 1 {
        let c = key.chars().next()?.to_ascii_uppercase();
        if c.is_ascii_uppercase() {
            return Some((c as u8) - b'A' + 1);
        }
    }
    match key {
        "@" | "space" | "2" => Some(0x00),
        "[" | "3" => Some(0x1b),
        "\\" | "4" => Some(0x1c),
        "]" | "5" => Some(0x1d),
        "^" | "6" => Some(0x1e),
        "_" | "/" | "7" => Some(0x1f),
        "-" if shift => Some(0x1f),
        "8" | "?" => Some(0x7f),
        _ => None,
    }
}

/// The bytes for pasted text: wrapped in `ESC [200~ … ESC [201~` when the program enabled
/// bracketed paste (so shells don't run each line), otherwise with newlines as carriage
/// returns, like a typed Enter.
pub fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // A paste can't end the bracket early.
        let text = text.replace("\x1b[201~", "");
        let mut out = Vec::with_capacity(text.len() + 12);
        out.extend_from_slice(b"\x1b[200~");
        out.extend_from_slice(text.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
        out
    } else {
        text.into_bytes()
    }
}

/// A mouse event to report to a program that enabled mouse mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReport {
    Press {
        button: u8,
    },
    Release {
        button: u8,
    },
    /// Motion while `button` is held (`None`: no button, for any-motion mode).
    Drag {
        button: Option<u8>,
    },
    WheelUp,
    WheelDown,
}

/// Encodes a mouse report at a 0-based cell. `button`: 0 left, 1 middle, 2 right. Modifier
/// bits: 4 shift, 8 alt, 16 ctrl.
pub fn mouse_report(
    report: MouseReport,
    column: usize,
    row: usize,
    modifiers: u8,
    sgr: bool,
) -> Option<Vec<u8>> {
    let (code, release) = match report {
        MouseReport::Press { button } => (button, false),
        MouseReport::Release { button } => (if sgr { button } else { 3 }, true),
        MouseReport::Drag { button } => (32 + button.unwrap_or(3), false),
        MouseReport::WheelUp => (64, false),
        MouseReport::WheelDown => (65, false),
    };
    let code = code + modifiers;
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", column + 1, row + 1).into_bytes());
    }
    // X10/normal encoding only reaches column/row 223.
    if column > 222 || row > 222 {
        return None;
    }
    Some(vec![
        0x1b,
        b'[',
        b'M',
        32 + code,
        32 + column as u8 + 1,
        32 + row as u8 + 1,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(k: &str) -> KeyInput {
        KeyInput {
            key: k.to_string(),
            ..Default::default()
        }
    }

    fn enc(k: &KeyInput) -> Option<Vec<u8>> {
        encode(k, KeyModes::default())
    }

    #[test]
    fn printable_letters_round_trip() {
        assert_eq!(enc(&key("a")), Some(b"a".to_vec()));
        let mut shifted = key("a");
        shifted.shift = true;
        assert_eq!(enc(&shifted), Some(b"A".to_vec()));
        let mut composed = key("2");
        composed.key_char = Some("@".into());
        assert_eq!(enc(&composed), Some(b"@".to_vec()));
    }

    #[test]
    fn navigation_keys_follow_the_cursor_mode() {
        assert_eq!(enc(&key("up")), Some(b"\x1b[A".to_vec()));
        let app = KeyModes {
            app_cursor: true,
            ..Default::default()
        };
        assert_eq!(encode(&key("up"), app), Some(b"\x1bOA".to_vec()));
        assert_eq!(enc(&key("enter")), Some(b"\r".to_vec()));
        assert_eq!(enc(&key("backspace")), Some(b"\x7f".to_vec()));
        assert_eq!(enc(&key("pagedown")), Some(b"\x1b[6~".to_vec()));
        assert_eq!(enc(&key("f1")), Some(b"\x1bOP".to_vec()));
        assert_eq!(enc(&key("f5")), Some(b"\x1b[15~".to_vec()));
    }

    #[test]
    fn modifiers_are_encoded_on_navigation_keys() {
        let mut ctrl_left = key("left");
        ctrl_left.control = true;
        assert_eq!(enc(&ctrl_left), Some(b"\x1b[1;5D".to_vec()));
        let mut alt_right = key("right");
        alt_right.alt = true;
        assert_eq!(enc(&alt_right), Some(b"\x1b[1;3C".to_vec()));
        let mut shift_tab = key("tab");
        shift_tab.shift = true;
        assert_eq!(enc(&shift_tab), Some(b"\x1b[Z".to_vec()));
        let mut shift_f5 = key("f5");
        shift_f5.shift = true;
        assert_eq!(enc(&shift_f5), Some(b"\x1b[15;2~".to_vec()));
    }

    #[test]
    fn ctrl_keys_map_to_control_bytes() {
        assert_eq!(enc(&KeyInput::parse("ctrl-c")), Some(vec![0x03]));
        assert_eq!(enc(&KeyInput::parse("ctrl-d")), Some(vec![0x04]));
        assert_eq!(enc(&KeyInput::parse("ctrl-[")), Some(vec![0x1b]));
        assert_eq!(enc(&KeyInput::parse("ctrl-space")), Some(vec![0x00]));
        assert_eq!(enc(&KeyInput::parse("ctrl-\\")), Some(vec![0x1c]));
    }

    #[test]
    fn alt_sends_meta_when_asked() {
        let meta = KeyModes {
            alt_is_meta: true,
            ..Default::default()
        };
        let mut alt_b = key("b");
        alt_b.alt = true;
        alt_b.key_char = Some("∫".into());
        assert_eq!(encode(&alt_b, meta), Some(b"\x1bb".to_vec()));
        // Without meta, the composed character is typed (macOS Option).
        assert_eq!(enc(&alt_b), Some("∫".as_bytes().to_vec()));
    }

    #[test]
    fn cmd_shortcuts_are_not_sent() {
        assert_eq!(enc(&KeyInput::parse("cmd-v")), None);
    }

    #[test]
    fn parses_keystrokes() {
        let parsed = KeyInput::parse("ctrl-shift-tab");
        assert!(parsed.control && parsed.shift);
        assert_eq!(parsed.key, "tab");
        assert_eq!(KeyInput::parse("ctrl--").key, "-");
    }

    #[test]
    fn paste_wraps_or_converts_newlines() {
        assert_eq!(paste("hi\nthere", true), b"\x1b[200~hi\rthere\x1b[201~");
        assert_eq!(paste("a\r\nb", false), b"a\rb");
        assert_eq!(paste("x\x1b[201~y", true), b"\x1b[200~xy\x1b[201~");
    }

    #[test]
    fn mouse_reports() {
        assert_eq!(
            mouse_report(MouseReport::Press { button: 0 }, 4, 2, 0, true),
            Some(b"\x1b[<0;5;3M".to_vec())
        );
        assert_eq!(
            mouse_report(MouseReport::Release { button: 0 }, 4, 2, 0, true),
            Some(b"\x1b[<0;5;3m".to_vec())
        );
        assert_eq!(
            mouse_report(MouseReport::WheelDown, 0, 0, 0, true),
            Some(b"\x1b[<65;1;1M".to_vec())
        );
        assert_eq!(
            mouse_report(MouseReport::Press { button: 0 }, 0, 0, 0, false),
            Some(vec![0x1b, b'[', b'M', 32, 33, 33])
        );
        assert_eq!(mouse_report(MouseReport::WheelUp, 300, 0, 0, false), None);
    }
}
