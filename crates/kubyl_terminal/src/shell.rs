//! Shell auto-detection: try `/bin/bash`, then `/bin/sh`, then `sh`, unless the user gave an
//! override. Probing which of these exist in the target container needs a real exec call
//! (`probe`, below); [`pick`] is the pure decision so it's unit-testable on its own.

/// Shells tried in order when none is configured.
pub const CANDIDATES: [&str; 3] = ["/bin/bash", "/bin/sh", "sh"];

/// Picks the shell to run: the override if set, otherwise the first candidate that `available`
/// reports as present, otherwise the last candidate (best effort — the exec still tries it).
pub fn pick(override_shell: Option<&str>, available: impl Fn(&str) -> bool) -> String {
    if let Some(shell) = override_shell
        && !shell.is_empty()
    {
        return shell.to_string();
    }
    CANDIDATES
        .iter()
        .find(|c| available(c))
        .copied()
        .unwrap_or_else(|| CANDIDATES[CANDIDATES.len() - 1])
        .to_string()
}

/// The exec command used to probe whether `shell` exists in the container: `test -x <path>` for
/// absolute paths, `command -v <name>` otherwise (works even when the container has no `test`
/// on `PATH` in the exact spot expected).
pub fn probe_command(shell: &str) -> Vec<String> {
    if shell.starts_with('/') {
        vec!["test".into(), "-x".into(), shell.into()]
    } else {
        vec!["sh".into(), "-c".into(), format!("command -v {shell}")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_wins_even_if_not_probed() {
        assert_eq!(pick(Some("/bin/zsh"), |_| false), "/bin/zsh");
    }

    #[test]
    fn falls_back_through_candidates_in_order() {
        assert_eq!(pick(None, |c| c == "/bin/sh"), "/bin/sh");
        assert_eq!(pick(None, |c| c == "/bin/bash"), "/bin/bash");
    }

    #[test]
    fn falls_back_to_sh_when_nothing_is_available() {
        assert_eq!(pick(None, |_| false), "sh");
    }

    #[test]
    fn probe_command_uses_test_for_absolute_paths() {
        assert_eq!(probe_command("/bin/bash"), ["test", "-x", "/bin/bash"]);
        assert_eq!(probe_command("sh"), ["sh", "-c", "command -v sh"]);
    }
}
