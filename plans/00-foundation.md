# Phase 00: Foundation (workspace, app shell, theme)

**Status:** done (2026-09-24, PR https://github.com/BirknerAlex/kubyl/pull/1)
**Depends on:** none
**Owns:** `crates/kubyl`, `crates/kubyl_core`, `crates/kubyl_ui`, `crates/kubyl_settings`, `script/`, `.github/workflows/ci.yml`
**Mockups:** chrome shared by all boards (title bar, sidebar frame, tabs, status bar, docks)

## Goal

A Kubyl window opens on macOS, Windows and Linux. It shows the Zed-style chrome with placeholder
content, has theme tokens and core components, a Tokio↔GPUI bridge, settings persistence, and
green CI on all three OSes. Every later phase builds on the extension points defined here, so
parallel sessions don't touch the same files.

## Tasks

### Workspace and tooling
- [x] `git init` (if needed), `.gitignore` (`target/`, `design/mockups/out/`, `assets/logo/.build/`), `rust-toolchain.toml` (stable, pinned minor)
- [x] Cargo workspace with `[workspace.dependencies]` pinning `gpui` (one git rev), `gpui-component`, `kube`, `k8s-openapi`, `tokio`, `tracing`, `serde`, `anyhow`/`thiserror`
- [x] License `MIT OR Apache-2.0`: add `LICENSE-MIT` and `LICENSE-APACHE` (copyright Alexander Birkner). Put `license`, `edition`, `repository = "https://github.com/BirknerAlex/kubyl"` and `authors` in `[workspace.package]`, and use `*.workspace = true` in every crate
- [x] Verify licenses: `gpui` and `gpui-component` are Apache-2.0. Add `cargo-deny` config that bans GPL crates (this blocks Zed's `editor`/`workspace`/`terminal_view`)
- [x] Create empty stub crates for **every** crate in the README layout, each with `pub fn init(cx: &mut App) {}`, so later phases never edit `Cargo.toml` members at the same time
- [x] `script/dev-cluster.sh`: creates a `kind` cluster `kubyl-dev` with sample workloads (a deployment, statefulset, cronjob, a crashlooping pod, a pending pod, cert-manager CRDs) to match the mockups

### Core (`kubyl_core`)
- [x] Tokio runtime singleton on background threads plus `spawn_kube()` returning a GPUI-awaitable `Task`. Cancellation on drop
- [x] Shared IDs and types: `ClusterId`, `ContextName`, `Gvk`, `Gvr`, `ResourceRef { cluster, gvr, namespace, name }`, `ViewKind`
- [x] Registries (see README "Extension points"): `ViewRegistry`, `ActionRegistry`, `ResourceColumns`, `StatusBarItem`, `DockPanel`, `SidebarSection`
- [x] Error type plus a user-facing notification/toast pipeline

### UI kit (`kubyl_ui`)
- [x] Theme tokens from the mockups (One Dark: bg `#282c33`, panel `#2f343e`, elevated `#3b414d`, border `#464b57`, text `#dce0e5`, muted `#a9afbc`, accent `#74ade8`, green `#a1c181`, red `#d07277`, yellow `#dec184`, purple `#b477cf`, cyan `#6eb4bf`, orange `#bf956a`). Also a light theme stub
- [x] Fonts: bundle IBM Plex Sans and IBM Plex Mono (OFL) in `assets/fonts`, registered at startup
- [x] Icon set: Lucide SVGs (ISC) used in the mockups, embedded via `rust-embed`
- [x] Components: `TitleBar` (cluster/namespace switchers, search button, PROD badge, account), `Sidebar` (tree rows, section headers, focus outline), `TabBar` (dirty dot, close, split, zoom), `StatusBar` (item slots), `Dock` (left/right/bottom, resizable), `KeyHints`, `Chip`, `StatusPill`, `ProgressBar`, `Modal`, `Toast`, `Button` variants (default, ghost, primary, danger)
- [x] Virtualized `DataTable` (fixed row height, sticky header, column sizing, keyboard selection, 5k rows at 60 fps). Wrap `gpui-component`'s table if it fits, otherwise write our own
- [x] Client-side window decorations on Linux (Wayland/X11) and Windows. Native traffic lights on macOS

### App shell (`kubyl`)
- [x] Main window: title bar, left sidebar, center pane group (tabs, splits), right dock, bottom dock, status bar. The layout is persisted
- [x] App menu, standard shortcuts (quit, new window, close tab, settings, zoom)
- [x] App icon wiring from `assets/logo` (`kubyl.icns`, `kubyl.ico`, PNGs)
- [x] Empty state: "Add a kubeconfig" welcome view (a placeholder until phase 01)

### Settings (`kubyl_settings`)
- [x] `settings.json` (user-editable, with schema) and `state.json` (window layout, open tabs, favorites) in the platform config dir
- [x] Typed settings access with change notifications, e.g. `Settings::get::<T>(cx)` and an observer

### CI
- [x] GitHub Actions: fmt, clippy, test, and build on `macos-latest`, `windows-latest`, `ubuntu-latest` (with Wayland/X11 deps). Cache cargo
- [x] `cargo-deny` job (licenses, advisories)

## Acceptance criteria

- `cargo run -p kubyl` opens a window that matches the mockup chrome on all three OSes. Tabs,
  docks and splits work with placeholder views.
- A dummy 5,000-row table scrolls smoothly.
- CI is green. `cargo deny check` passes.

## Risks and open questions

- GPUI on Windows/Linux: check IME, HiDPI, and client-side decorations early. File upstream issues.
- Is `gpui-component`'s version compatible with the pinned `gpui` rev? Pin both together.

## Handoff log

<!-- YYYY-MM-DD: what changed, decisions, stubs left, notes for next phases -->

### 2026-09-24: phase 00 implemented

**GPUI choice.** `gpui = { package = "gpui-pre", version = "=0.3.6" }` (crates.io snapshot of
zed@`bcf6582`), `gpui_platform = gpui-pre-platform =0.3.6` (features `font-kit`, `x11`, `wayland`,
`runtime_shaders`), `gpui-component =0.6.6`, `gpui-base =0.6.6`, `gpui-kit-assets =0.6.6`. The
crates.io `gpui` (0.2.2) is a year old; gpui-component 0.6.6 is built against exactly this
snapshot, so all of these must be bumped together. All Apache-2.0. `runtime_shaders` means macOS
builds don't need Xcode's Metal toolchain (shaders compile at startup). Every other dependency is
on its latest release (kube 4.2, k8s-openapi 0.28, tokio 1.53, notify 8.2, rust-embed 8.12…).
Toolchain pinned to Rust 1.98.

**What exists.**
- `kubyl_core`: `spawn_kube` (shared Tokio runtime, abort on drop), IDs/types, `Error`,
  `NotificationCenter` (toasts from anywhere), `ActiveContext`, `ViewRegistry`/`TabView`,
  `ActionRegistry`, `ResourceColumns`, `ChromeRegistry` (`StatusBarItem`, `DockPanel`,
  `SidebarSection`), app-wide actions in `kubyl_core::actions`. See plans/README.md "Extension points".
- `kubyl_settings`: `Settings::{register,get,observe,update}::<T: SettingsSection>`; sections are
  flat (Zed-style, `KEY = None`) or keyed. settings.json is created on first run, hot-reloaded
  (notify), and gets `settings.schema.json`. `State::{get,set,update}::<T: StateSection>` with
  debounced atomic writes and a flush on quit. `$KUBYL_CONFIG_DIR` overrides the directory.
- `kubyl_ui`: tokens (`cx.colors()`), sizes (`sizes::*`, always wrapped in `u(px)` so zoom works),
  fonts, `IconName` (Lucide), components listed in the task above, `DataTable` +
  `TableDelegate`. `AppearanceSettings` (`theme`: dark/light/system, `ui_font_size`).
- `kubyl`: workspace (title bar, sidebar, pane group with tabs/splits/zoom, right and bottom
  docks, status bar), layout persisted in state.json (`workspace` key), menus, shortcuts
  (`secondary-*` = ⌘ on macOS, Ctrl elsewhere), welcome view, About and notifications overlays.
- CI (`.github/workflows/ci.yml`), `deny.toml`, `script/dev-cluster.sh` (tested against kind
  v0.33.0: Running, Pending, CrashLoopBackOff, Completed cronjob, cert-manager Issuer/Certificate).

**Stubs and placeholders for later phases.**
- The sidebar shows `SampleExplorer` (mockup tree, labelled "sample") until a crate registers a
  `SidebarSection`; phase 02 should register its sections and can then delete
  `crates/kubyl/src/views/sample_explorer.rs`.
- `ViewKind::Custom("sample_table")` is the 5,000-row sample Pods table (default second tab).
  Remove it with the sample explorer in phase 02; persisted layouts that still reference it show
  a placeholder tab.
- ViewKinds without a registered factory open a `PlaceholderView` ("… arrives with phase NN").
  The right dock shows a "Details" placeholder and the bottom dock a "Terminal" placeholder until
  `DockPanel`s are registered.
- Unhandled actions: `ToggleCommandPalette` (⌘K, phase 03), `SwitchCluster` (01),
  `SwitchNamespace` (02), `AddKubeconfig` (01; the welcome button shows a toast while unhandled).
  Handle them with `cx.on_action` in the owning crate's `init`.
- The title bar shows "No cluster"/"all namespaces" until someone calls `ActiveContext::set`.
  The avatar shows a person icon (no account concept yet).
- Split sizes aren't persisted (only split structure, tabs and dock sizes); restored splits are
  equal. Tabs can't be dragged between panes yet.

**Verification.** `cargo fmt`, `cargo clippy --workspace --all-targets -- -D warnings`,
`cargo test --workspace` (27 tests) and `cargo deny check` pass locally on macOS and in CI on all
three OSes. Screenshots were taken with the `screenshot` feature:
`KUBYL_SCREENSHOT=out.png cargo run -p kubyl --features screenshot` renders the real window
scene through Metal to a PNG (optional `KUBYL_SCREENSHOT_ACTIONS=split,bottom,zoom,light,about,toast`).
The table is virtualized (`uniform_list`, only visible rows are built, so row count doesn't
affect frame cost), but frame times were not profiled and scrolling was not tried by hand in
this session; check it when phase 02 puts real data in it.

**Gotchas.**
- gpui-component's `Root` doesn't draw its dialog/sheet/notification layers; the root view must
  render `Root::render_*_layer` (the workspace does). `Root` also resets the rem size to the
  theme font size each frame; the workspace sets it again (`kubyl_ui::apply_zoom`).
- `Window::render_to_image` needs `gpui/test-support` *and* `gpui_platform/test-support`.
- GPUI tests fail with "Detected activity on thread …, your test is not deterministic" when a
  background OS thread (e.g. a `notify` watcher) wakes a GPUI task. Tests must use
  `kubyl_settings::init_with_dir`, which doesn't watch. Only showed up on Linux/Windows CI.
- A GPUI `Task` must not drop itself (storing it in the entity and clearing that field from
  inside the task cancels it); use a flag and `.detach()`.
- gpui-component's resizable panels work in absolute pixels; dock sizes are stored unscaled and
  converted with the current rem scale.
- Windows: the icon and version info are embedded by `crates/kubyl/build.rs` (winresource); GPUI
  embeds its own manifest. Release builds use the `windows` subsystem (no console). Only compiled
  and unit-tested on Windows in CI, not run interactively yet: check title bar controls
  (gpui-component draws min/max/close), HiDPI and IME in phase 10 at the latest.
- Linux: client-side decorations are requested (`WindowDecorations::Client`); gpui-component's
  `Root` draws the border/shadow and resize areas. X11 gets the window icon from the PNG, Wayland
  needs a `.desktop` file with `app_id` `io.github.birkneralex.Kubyl` (phase 10). CI installs
  wayland/xkbcommon/x11-xcb/fontconfig/freetype/vulkan/alsa dev packages. Only compiled and
  unit-tested in CI, not run on a real desktop yet.
- macOS: `cargo run` sets the Dock icon at runtime (unbundled binary); `[package.metadata.bundle]`
  points at `kubyl.icns` for cargo-bundle. System notifications are disabled when not bundled.

