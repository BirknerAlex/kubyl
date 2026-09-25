# AGENTS.md

Guidance for coding agents (Claude Code, Codex, …) working on Kubyl, a native Kubernetes
desktop client (macOS, Windows, Linux) in Rust on GPUI, styled like Zed.

## Start here
- `plans/README.md`: architecture, crate ownership, extension points, decision table, definition
  of done. Then the phase file you were given (`plans/NN-*.md`) and the **Handoff log** of the
  phases before it (what exists, stubs, gotchas).
- Keep the phase file's checkboxes, **Status** line and **Handoff log** current. Cross-cutting
  decisions go into `plans/README.md`.
- Mockups: https://claude.ai/artifact/VfLbzAtjsCjQJgVM1cEW4H (source:
  `design/mockups/generate.py`). New UI must match its board.

## Hard rules
- Never depend on GPL crates from Zed (`editor`, `workspace`, `terminal_view`…).
  `cargo deny check` enforces this.
- No network or blocking calls on the UI thread. Kubernetes work runs on the shared Tokio runtime
  via `kubyl_core::spawn_kube`.
- Never log or persist tokens, refresh tokens, client keys or Secret data. Secrets go to the OS
  keychain (`kubyl_kube::auth::store`), never into settings.json or state.json.
- Only touch the crates your phase owns. Feature crates plug in through their `init(cx)` and the
  registries in `kubyl_core` (views, actions, chrome, columns) instead of editing the shell.
  Keep shared-crate and root `Cargo.toml` changes minimal and in their own commits.
- New dependencies: latest release, declared once in `[workspace.dependencies]`.

## Git
- The repo is private and single-owner: committing and pushing straight to `main` is fine. Phase
  branches (`phase/NN-name`) and PRs are optional; use them for parallel sessions.
- Push over HTTPS (SSH keys aren't loaded in agent sessions):
  `git -c credential.helper='!gh auth git-credential' push origin <branch>`.
  Always name the branch explicitly, and check `git branch --show-current` before committing:
  the IDE may switch the checkout under you.
- Small, logical commits. CI (`.github/workflows/ci.yml`) must pass on macOS, Windows and Linux;
  fix failures, don't skip them.

## Commands
```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check
cargo run -p kubyl
```

- Screenshots: `KUBYL_SCREENSHOT=out.png KUBYL_CONFIG_DIR=<tmp> cargo run -p kubyl --features screenshot`.
  To avoid showing real clusters, also set `HOME=<tmp>/home` (run the built binary, not
  `cargo run`, when overriding `HOME`) and `KUBYL_CREDENTIAL_STORE=memory`. A `state.json` in the
  config dir picks the open tabs (e.g. `{"kind":{"custom":"clusters"}}`) and `kube.active`.
  `KUBYL_SCREENSHOT_ACTIONS` takes comma-separated steps: `split`, `bottom`, `zoom`, `light`,
  `about`, `toast` (or `toast=<text>` for an error), `palette=:cert`, `action=pane::GoBack`,
  `keys=: p o enter` (typed through GPUI's key dispatch), `mouse=640:380` (hover),
  `click=640:380` (logical window pixels), `scroll=640:380:-600` (wheel delta at a point;
  negative scrolls down) and `wait=800`, which is enough to click through keyboard flows. Steps
  are comma-separated, so `keys=` can't type commas; type `:`/`#` as themselves (not
  `shift-;`), and close completion menus with `escape` before `enter` when typing into the YAML
  editor.
- Local clusters: `script/dev-cluster.sh` (kind, sample workloads), `script/prometheus-dev.sh`
  (metrics-server and kube-prometheus-stack on it; `--metrics-server-only` for the fallback) and
  `script/oidc-dev.sh` (Dex + an OIDC-enabled kind cluster). `script/load-pods.sh` adds 5,000
  pods (and `--churn`) for list performance checks. Point `KUBECONFIG` at a scratch file so the
  user's `~/.kube/config` isn't modified.
- Live tests against those clusters are ignored by default: see the header of
  `crates/kubyl_kube/tests/live.rs`.
- Web views: `script/webview-dev.sh` (after `prometheus-dev.sh`) adds Grafana, an Ingress, a
  self-signed HTTPS service and a non-HTTP-looking port. Real web views are tested by
  `KUBYL_TEST_WEBVIEW=1 cargo test -p kubyl_webview --test live_webview` (needs a display; on
  Linux use `xvfb-run`); the forward lifecycle by the ignored tests in
  `crates/kubyl_webview/tests/live.rs`.

## Gotchas
- GPUI tests must not start OS threads that wake GPUI tasks (file watchers, network on Tokio):
  use `kubyl_settings::init_with_dir` and `ConnectionManager::install(.., false)`, and don't
  connect to clusters in GPUI tests.
- A GPUI `Task` must not drop itself (clearing the field that holds it from inside the task
  cancels it).
- Sizes use `kubyl_ui::u(px)`; colors come from `cx.colors()`.
- Global `cx.on_action` handlers run while the dispatching window is being updated:
  `window.update` on it fails ("window not found"). Wrap it in `cx.defer`.
- Inputs inside gpui-component dialogs: bind `enter` in your own context (`MyView > Input`),
  otherwise the dialog's `enter` closes it first.
- gpui-component dialogs: pass your view with `.content(..)`, not `.child(..)`. Children go into
  a scroll body whose height collapses, so presses on the footer reach the backdrop and close
  the dialog instead of clicking its buttons.
- Dev builds on macOS trigger keychain prompts; use `KUBYL_CREDENTIAL_STORE=file` (plain text,
  dev only) or `memory`.
