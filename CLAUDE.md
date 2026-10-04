# Kubyl

Native Kubernetes desktop client (macOS, Windows, Linux) in Rust on GPUI, styled like Zed.

- Plans: start with `plans/README.md` (architecture, conventions, crate ownership), then the phase file you were given. Keep its checkboxes, **Status** line and **Handoff log** current.
- Mockups: https://claude.ai/artifact/VfLbzAtjsCjQJgVM1cEW4H (source: `design/mockups/generate.py`). New UI must match its board.
- Logo and icons: `assets/logo/` (regenerate with `python3 assets/logo/generate.py`, needs `rsvg-convert`).
- Never depend on GPL crates from Zed (`editor`, `workspace`, `terminal_view`…). `cargo deny check` enforces this.
- No network or blocking calls on the UI thread. Kube work runs on the Tokio runtime from `kubyl_core`.
- Never log or persist tokens, refresh tokens or Secret data.
- Logic without UI goes in the crate's GPUI-free `kubyl_<name>_core`; views stay in the GPUI crate. `script/check-core-crates.sh` (CI job `core-crates`) fails when a core crate depends on `gpui`, `gpui-component` or the UI crates.
- Changes go through PRs from a branch (one worktree per branch for parallel sessions); core maintainers may push small fixes to `main`. Only touch the crates your phase owns.

See AGENTS.md for commands, git workflow and gotchas shared by all coding agents.
