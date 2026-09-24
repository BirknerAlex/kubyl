# Kubyl: implementation plan

Kubyl is a native Kubernetes desktop client for macOS, Windows and Linux. It is written in Rust on
[GPUI](https://gpui.rs) (Zed's UI framework) and styled like the Zed editor (One Dark).
In scope: everything k9s and the old Kubernetes Dashboard do, plus selected OpenShift console
features (operators, cluster updates).

- **Mockups:** https://claude.ai/artifact/VfLbzAtjsCjQJgVM1cEW4H (source: `design/mockups/generate.py`)
- **Logo and icons:** `assets/logo/` (source: `assets/logo/generate.py`)

## How to use these plans (multi-session workflow)

Each phase file is written so one Claude Code session can own it from start to finish.

1. Start a session with: *"Implement `plans/NN-name.md`. Read `plans/README.md` first."*
2. The session reads this README (architecture, conventions), then its phase file.
3. Tick task checkboxes as work lands. Set the phase **Status** line
   (`not started` → `in progress` → `done`) and add dated notes under **Handoff log** at the
   bottom of the phase file: decisions made, what is stubbed, what the next session must know.
4. Parallel sessions must each use their own git worktree/branch (`phase/NN-name`) and only touch
   the crates their phase owns (see the ownership table). Changes to a shared crate
   (`kubyl_ui`, `kubyl_kube`, `kubyl_core`) go in a small separate PR that lands first.
5. When a phase changes a cross-cutting decision, update **this README**, not only the phase file.

## Phases and dependencies

| # | Phase | Depends on | Owns crates | Mockup board |
|---|-------|-----------|-------------|--------------|
| 00 | [Foundation: workspace, app shell, theme](00-foundation.md) | none | `kubyl`, `kubyl_ui`, `kubyl_core`, `kubyl_settings` | all (chrome) |
| 01 | [Cluster connectivity: kubeconfigs, auth, discovery](01-cluster-connectivity.md) | 00 | `kubyl_kube` | 5 · Clusters |
| 02 | [Resource engine, explorer, tables, favorites](02-resource-explorer.md) | 01 | `kubyl_resources`, `kubyl_explorer` | 1 · Pods |
| 03 | [Command palette, navigation, keymaps](03-command-palette.md) | 02 | `kubyl_palette`, `kubyl_keymap` | 6 · Palette |
| 04 | [YAML editor, schema validation, apply](04-yaml-editor.md) | 02 | `kubyl_yaml` | 3 · YAML |
| 05 | [Logs, exec terminal, port-forwarding](05-logs-exec-portforward.md) | 02 | `kubyl_logs`, `kubyl_terminal`, `kubyl_portforward` | 2 · Logs |
| 06 | [Pod file browser, drag and drop transfers](06-file-browser.md) | 05 (exec) | `kubyl_files` | 9 · Files |
| 07 | [Overview, metrics (Prometheus), events](07-metrics-events-overview.md) | 02 | `kubyl_metrics`, `kubyl_charts`, `kubyl_overview` | 4 · Overview |
| 08 | [Operators (OLM) and Helm releases](08-operators-olm.md) | 02, 04 | `kubyl_operators` | 7 · Operators |
| 09 | [Cluster updates](09-cluster-updates.md) | 02, 07, 08 | `kubyl_updates` | 8 · Updates |
| 10 | [Packaging, release, auto-update, hardening](10-packaging-release.md) | 00 (CI), then all | `script/`, `.github/`, `crates/kubyl` bundling | none |
| 11 | [Service web views over temporary port-forwards](11-service-webview.md) | 02, 05 | `kubyl_webview` (new) | 10 · Web view |
| 12 | [Argo CD: applications, sync, history, rollback](12-argocd.md) | 02, 04, 05 (11 optional) | `kubyl_argocd` (new) | none yet |

```
00 ─▶ 01 ─▶ 02 ─┬─▶ 03
                ├─▶ 04 ──────┐
                ├─▶ 05 ─▶ 06 │
                ├─▶ 07 ──────┼─▶ 09 (also needs 08)
                └────────────┴─▶ 08 (needs 04 for install YAML/diff)
05 ─▶ 11 (web views)        02 + 04 + 05 ─▶ 12 (Argo CD; uses 11 for "Open Argo CD UI" if present)
10: CI part runs from 00 onward; packaging and release after the feature phases
```

After phase 02, phases 03, 04, 05 and 07 can run in parallel sessions. Phase 11 can start once 05 is done, and phase 12 once 04 and 05 are done.
Phases 11 and 12 add crates that phase 00 didn't stub (`kubyl_webview`, `kubyl_argocd`): their first commit adds the stub crate (workspace member plus the `init` line in `crates/kubyl/src/main.rs`) in a tiny PR that lands on `main` before the feature work, so parallel sessions don't conflict.
Phase 00 must leave stub crates and registration traits so that parallel phases never edit
the same files. See "Extension points" below.

## Architecture

### Workspace layout (Zed-style `crates/`)

```
Cargo.toml                  # [workspace], shared deps pinned in [workspace.dependencies]
crates/
  kubyl/                    # binary: main(), app init, window, menus, bundling metadata
  kubyl_core/               # shared types: ClusterId, ResourceRef, GVK/GVR, errors, Tokio bridge
  kubyl_ui/                 # theme tokens, Zed-like components (TitleBar, Sidebar, Tabs, StatusBar,
                            #   Dock, Table, KeyHints, Chip, Pill, Toast, Modal), icons
  kubyl_settings/           # settings + state persistence (JSON in platform config dir)
  kubyl_kube/               # kubeconfig sources, auth, ConnectionManager, discovery, client pool
  kubyl_resources/          # resource registry, watch cache (reflectors), columns, actions
  kubyl_explorer/           # sidebar tree, favorites, resource list views, details dock
  kubyl_palette/            # command palette + modes (: @ # > * /)
  kubyl_keymap/             # keybinding tables (k9s-compatible preset + Zed-like preset)
  kubyl_yaml/               # YAML editor view, schema validation, diff, apply
  kubyl_logs/               # log streaming, search, level parsing
  kubyl_terminal/           # exec terminal view (alacritty_terminal backend)
  kubyl_portforward/        # port-forward manager
  kubyl_files/              # pod filesystem browser, tar transfers, DnD
  kubyl_metrics/            # Prometheus discovery + PromQL client, metrics-server fallback
  kubyl_charts/             # GPUI-native line/area/sparkline charts
  kubyl_overview/           # cluster overview dashboard + events stream
  kubyl_operators/          # OLM v0/v1, OperatorHub, InstallPlans, Helm releases
  kubyl_updates/            # cluster update providers + preflight checks
  kubyl_webview/            # embedded web views over temporary port-forwards (phase 11)
  kubyl_argocd/             # Argo CD applications, sync, history, rollback (phase 12)
assets/                     # logo, icons, fonts, keymaps, themes
design/mockups/             # mockup generator (HTML design canvas)
plans/                      # these plans
```

### Core technical decisions

| Topic | Decision | Notes |
|---|---|---|
| UI framework | `gpui = { package = "gpui-pre", version = "=0.3.6" }` and `gpui_platform = { package = "gpui-pre-platform", version = "=0.3.6" }` (decided 2026-09-24). `gpui-pre-*` are crates.io snapshots of `zed-industries/zed` (0.3.6 = zed@`bcf6582`) published by the gpui-component maintainers; the crates.io `gpui` (0.2.2, Oct 2025) is stale | GPUI's API changes often. Bump `gpui-pre-*`, `gpui-component`, `gpui-base` and `gpui-kit-assets` together, in their own PR (gpui-component pins an exact snapshot). `gpui_platform` uses `runtime_shaders`, so macOS builds don't need Xcode's Metal toolchain. |
| Components | `gpui-component =0.6.6` + `gpui-base =0.6.6` (longbridge/gpui-kit, Apache-2.0) for inputs, menus, dialogs, toasts, resizable splits, title bar window controls | Licenses verified at phase 00 (Apache-2.0). Kubyl's own chrome (tabs, tree rows, status bar, table) lives in `kubyl_ui` to match the mockups exactly. **Do not depend on Zed's `editor`, `terminal_view`, `workspace` crates: they are GPL-3.0.** `cargo deny check` enforces this. |
| Kubernetes client | `kube` + `k8s-openapi` (latest), features `runtime`, `ws`, `oauth`, `oidc`, `gzip`, `rustls-tls` + `ring`, `http-proxy`, `socks5` | Dynamic API (`DynamicObject`, `discovery`) is the default path. Typed APIs only where needed. Get clients, discovery, namespaces and caps from `kubyl_kube::ConnectionManager::global(cx)` (phase 01), never build your own. Exec and OIDC credentials are injected by Kubyl's auth layer, not by kube. |
| Async | Dedicated multi-thread **Tokio** runtime on background threads. GPUI's executor drives UI only. | Bridge in `kubyl_core::runtime`: `spawn_kube(fut) -> Task<T>` plus channels into `cx.spawn`. No Tokio calls on the UI thread. |
| State model | Per-cluster **watch caches** (kube `reflector` stores) feed GPUI `Entity<…>` models via batched diffs (≤ 60 Hz) | Views never await network calls directly. |
| Secrets | `keyring` 4 (macOS Keychain, Windows Credential Manager, Secret Service) | OIDC refresh/ID tokens (keyed by issuer + client id). Exec credentials are cached in memory only. Never written to disk in plain text, except the opt-in dev store `KUBYL_CREDENTIAL_STORE=file` (`memory` for tests). |
| OIDC | `openidconnect` crate: auth-code + PKCE with loopback redirect, device-code fallback | Compatible with kubelogin-style kubeconfigs: redirect `http://localhost:8000` (then `18000`), like kubelogin (decided in phase 01). `script/oidc-dev.sh` runs Dex + an OIDC kind cluster. |
| YAML | `granit-parser` 1.3 (decided in phase 04): the saphyr-parser fork that `serde-saphyr` 1.3 uses, events with byte spans and comments. `kubyl_yaml::parse` builds a spanned tree on it; output goes through `serde-saphyr` (keys sorted like `kubectl get -o yaml`) | The editor never re-serializes the user's buffer: diagnostics, hover, completion, Secret masking and the managedFields toggle work on spans and text edits, so comments and key order survive. `serde_yaml` is unmaintained; `serde_yaml_ng` has no node spans. |
| Syntax | `tree-sitter-yaml` through gpui-component's `tree-sitter-yaml` feature (phase 04) | Highlighting and folding in gpui-component's code editor (`EditorState`); editor colors come from `kubyl_ui` (One Dark). |
| YAML editor | gpui-component `EditorState` (Apache-2.0), not Zed's GPL `editor` (phase 04) | Diagnostics, hover and completion use its LSP-style providers; gutter markers, code lenses and end-of-line messages are painted by `kubyl_yaml` over the editor. Diffs: `similar` 3 (line diff, three-way merge). |
| Apply | Server-side apply, field manager `kubyl`, strict field validation; `force` only after the user saw the conflicting managers; replace/create fallback on 415 (phase 04) | `kubyl_yaml::apply`. PROD clusters confirm with the change summary and the typed object name. Kubyl's own applies are kept in state.json (`yaml_history`, never Secrets). |
| Terminal | `alacritty_terminal` (Apache-2.0) with a custom GPUI renderer | Same approach as Zed, without Zed's GPL view code. |
| Charts | Own GPUI `canvas`/path renderer in `kubyl_charts` | No webviews. |
| Web views | `wry` (MIT/Apache-2.0) as a child view of the GPUI window; separate window or system browser as fallback (phase 11 spike decides per platform) | Only for phase 11 service web views, loaded lazily. |
| File watching | `notify` | Kubeconfig hot reload. |
| Settings | JSON (`serde_json`) in `dirs::config_dir()/kubyl/` (override with `$KUBYL_CONFIG_DIR`) | `settings.json` (user, hot-reloaded, with a generated `settings.schema.json`), `state.json` (UI state, favorites, tabs). Typed sections: `kubyl_settings::{SettingsSection, StateSection}`. |
| UI units | Sizes use `kubyl_ui::u(px)` (rems); the window's rem size follows `ui_font_size` | Zoom (⌘+/⌘-) scales the whole UI. Colors come from `cx.colors()`. |
| Watch caches | `kubyl_resources::ResourceStores::acquire(cx, StoreKey)` → shared `StoreHandle` (decided in phase 02) | One kube `watcher` per (cluster, GVR, namespace, selectors, full/metadata) shared by every view; events are batched on Tokio per 16 ms frame; stopped 30 s after the last handle drops. Views never build their own watchers. |
| Favorites | Keyed by context name + API server URL, kubeconfig file as a hint (decided in phase 02) | Stored in `state.json` (`favorites`). Resolution order: context+server+file, context+server, context+file, server+file. `ClusterId` isn't stored (it contains the file path). |
| Resource selection | `kubyl_resources::ResourceSelection` global (phase 02) | Lists publish their selection; actions from any crate bind in the `ResourceList` key context and read their targets from it. |
| Command palette | `kubyl_palette` lists `ActionRegistry` actions whose key context matches the focus it was opened from (decided in phase 03) | Register every user-facing action in the registry with a context; the palette, key-hint bar and keymaps pick it up. Fuzzy matching: `nucleo-matcher` (MPL-2.0). |
| Keymaps | Crates bind defaults in code; `kubyl_keymap` layers `assets/keymaps/{default,k9s}.json` and the user's `"keymap"` section of settings.json on top (decided in phase 03) | Not a separate keymap.json: settings.json already hot-reloads and has a schema. `null` unbinds. `ActionRegistry` keystrokes are kept in sync with what is bound. |
| Logging | `tracing` + `tracing-subscriber`, rolling file in the platform log dir | Never log tokens or Secret data. |
| License | **`MIT OR Apache-2.0`** (decided 2026-09-24). Set once in `[workspace.package]`; every crate uses `license.workspace = true` | Compatible with GPUI and all key deps (Apache-2.0). GPL crates are banned via `cargo-deny`. Releases ship `THIRD_PARTY_LICENSES` (cargo-about) plus the font/icon licenses. |

### Extension points (these keep parallel sessions conflict-free)

Phase 00 creates these traits/registries in `kubyl_core` / `kubyl_ui`. Feature crates register
into them from their own `init(cx)` function, and `crates/kubyl/src/main.rs` calls each `init` in
one list. That list is the only shared line, and it is append-only.

- `ViewRegistry`: open a tab/pane for a `ViewRequest { kind: ViewKind, target: Option<ResourceRef> }`
  (Table, Details, Yaml, Logs, Terminal, Files, Overview, Operators, Updates, Settings…). Views
  implement `TabView`; dispatch `kubyl_core::actions::OpenView(request)` to open one. Kinds without
  a factory show a placeholder tab.
- `ActionRegistry`: named actions with keybindings, availability predicate
  (`fn(&ResourceRef, &ClusterCaps) -> bool`), and palette metadata. k9s hints read from here.
- `ResourceColumns`: per-(group, kind) column providers. Falls back to the server-side `Table`
  (`application/json;as=Table`) so CRD printer columns work without special code.
- `ChromeRegistry`: `StatusBarItem`, `DockPanel` (right/bottom dock tabs) and `SidebarSection`
  trait objects. Until a crate registers a sidebar section, the sidebar shows sample data.
- `ActiveContext` (global): the cluster/namespace shown in the title and status bars.
- `NotificationCenter::push(cx, Notification::error(…))`: toasts from any crate, no window needed.
- `spawn_kube(cx, future) -> Task<T>`: runs on the shared Tokio runtime; dropping the task aborts it.
- App-wide actions (`ToggleCommandPalette`, `SwitchCluster`, `SwitchNamespace`, `AddKubeconfig`,
  `ShowNotifications`, `OpenSettings`, `OpenView`) live in `kubyl_core::actions`; the chrome
  dispatches them and the owning crate handles them. A global `cx.on_action` handler runs
  inside the dispatching window's update: `cx.defer` before `window.update` on it.
- Command palette (phase 03): dispatch `kubyl_palette::Open { mode, query }`; register actions
  in the `ActionRegistry` to make them searchable in `>` mode.
- YAML (phase 04): open `ViewKind::Yaml` with an object ref (edit) or a list ref / no target
  (new resource); `kubyl_yaml::{parse, schema, validate, diff, apply, render}` are usable without
  the view (see plans/04-yaml-editor.md, "API for later phases").
- Confirmations (phase 02/04): `kubyl_explorer::dialogs::confirm(ConfirmSpec { typed, lines, .. })`.

### UX principles (from the mockups)

- Zed look: One Dark tokens, 13px IBM Plex Sans UI, IBM Plex Mono for data, focus = 1px accent
  outline, no gradients, keyboard first.
- Multi-root sidebar like Zed's project panel: every cluster is a root. **Favorites** (namespaces
  from any cluster or kubeconfig file) are pinned at the top.
- Everything live: lists are watch-driven. Nothing needs a manual refresh.
- Safety: production clusters get a red title-bar badge. Destructive actions need typed
  confirmation there. Clusters can be set read-only.
- k9s parity: single-key actions in list views (`l` logs, `s` shell, `e` edit, `d` describe,
  `⌃d` delete, `⇧f` port-forward, `/` filter, `:` command), shown in a key-hint bar.

## Global definition of done (every phase)

- `cargo fmt`, `cargo clippy --workspace -- -D warnings` and `cargo test --workspace` pass on the
  three CI OSes.
- New UI matches its mockup board closely. Deviations are noted in the Handoff log.
- Works against a local `kind` cluster (`script/dev-cluster.sh`, created in phase 00). Features that
  need extras (Prometheus, OLM, cert-manager) come with a setup script in `script/`.
- No blocking calls on the UI thread. Lists stay smooth at 5,000 rows.
- No secrets in logs, crash reports or `state.json`.
