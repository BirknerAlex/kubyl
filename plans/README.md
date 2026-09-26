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
| 08 | [Service web views over temporary port-forwards](08-service-webview.md) | 02, 05 | `kubyl_webview` (new) | 10 · Web view |
| 09 | [Packaging, release, auto-update, hardening](09-packaging-release.md) | 00 (CI), then all | `script/`, `.github/`, `crates/kubyl` bundling | none |
| 10 | [Argo CD: applications, sync, history, rollback](10-argocd.md) | 02, 04, 05 (08 optional) | `kubyl_argocd` (new) | 12–15 · Argo CD |
| 11 | [Kubeconfig editor: clusters, credentials, contexts, connection test](11-kubeconfig-editor.md) | 01, 04 | `kubyl_kubeconfig` (new) | 11 · Kubeconfig editor |
| 12 | [Operators (OLM) and Helm releases](12-operators-olm.md) | 02, 04 | `kubyl_operators` | 7 · Operators |
| 13 | [Cluster updates](13-cluster-updates.md) | 02, 07, 12 | `kubyl_updates` | 8 · Updates |
| 14 | [Alerts: Alertmanager, silences, alerting rules](14-alerts.md) | 02, 05, 07 (08 optional) | `kubyl_alerts` (new) | 16 · Alerts |
| 15 | [Polish: connection dots, one entry per cluster and user, ConfigMap data](15-sidebar-details-polish.md) | 01, 02, 03 (part 3 after 11) | none (small commits in `kubyl_explorer`, `kubyl_kube`, `kubyl_palette`) | 17 · Cluster status and ConfigMap data |

```
00 ─▶ 01 ─▶ 02 ─┬─▶ 03
                ├─▶ 04 ──────┐
                ├─▶ 05 ─▶ 06 │
                ├─▶ 07 ──────┼─▶ 13 (also needs 12)
                └────────────┴─▶ 12 (needs 04 for install YAML/diff)
05 ─▶ 08 (web views)        02 + 04 + 05 ─▶ 10 (Argo CD; uses 08 for "Open Argo CD UI" if present)
01 + 04 ─▶ 11 (kubeconfig editor)
02 + 05 + 07 ─▶ 14 (alerts; uses 08 for the Alertmanager/Prometheus UIs if present)
01 + 02 + 03 ─▶ 15 (polish; context grouping after 11)
09: CI part runs from 00 onward; packaging and release after the feature phases
```

After phase 02, phases 03, 04, 05 and 07 can run in parallel sessions. Phase 08 can start once 05 is done, phase 10 once 04 and 05 are done, phase 11 once 04 is done, and phase 14 once 07 is done.
Phases 08, 10, 11 and 14 add crates that phase 00 didn't stub (`kubyl_webview`, `kubyl_argocd`, `kubyl_kubeconfig`, `kubyl_alerts`): their first commit adds the stub crate (workspace member plus the `init` line in `crates/kubyl/src/main.rs`) in a tiny PR that lands on `main` before the feature work, so parallel sessions don't conflict (phases 10 and 11 skipped it on request: their crates landed with the feature PR).
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
  kubyl_webview/            # embedded web views over temporary port-forwards (phase 08)
  kubyl_argocd/             # Argo CD applications, sync, history, rollback (phase 10)
  kubyl_kubeconfig/         # kubeconfig editor, connection test, creation wizard (phase 11)
  kubyl_alerts/             # Alertmanager alerts, silences, alerting rules (phase 14)
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
| Secrets | `keyring` 4 (macOS Keychain, Windows Credential Manager, Secret Service) | OIDC refresh/ID tokens (keyed by issuer + client id), OpenShift OAuth tokens (keyed by API server + kubeconfig user), Argo CD session tokens (keyed by API server + context + namespace + Service, phase 10). Exec credentials are cached in memory only. Never written to disk in plain text, except the opt-in dev store `KUBYL_CREDENTIAL_STORE=file` (`memory` for tests). |
| OIDC | `openidconnect` crate: auth-code + PKCE with loopback redirect, device-code fallback | Compatible with kubelogin-style kubeconfigs: redirect `http://localhost:8000` (then `18000`), like kubelogin (decided in phase 01). `script/oidc-dev.sh` runs Dex + an OIDC kind cluster. |
| OpenShift OAuth | `kubyl_kube::auth::openshift` on the `openidconnect` re-export of `reqwest` (phase 07) | Contexts with an `oc login` token (`sha256~…`) sign in like `oc login`: browser (`openshift-cli-client`, PKCE, loopback), username/password (challenging client) or a pasted token. Endpoints from `/.well-known/oauth-authorization-server`. |
| YAML | `granit-parser` 1.3 (decided in phase 04): the saphyr-parser fork that `serde-saphyr` 1.3 uses, events with byte spans and comments. `kubyl_yaml::parse` builds a spanned tree on it; output goes through `serde-saphyr` (keys sorted like `kubectl get -o yaml`) | The editor never re-serializes the user's buffer: diagnostics, hover, completion, Secret masking and the managedFields toggle work on spans and text edits, so comments and key order survive. `serde_yaml` is unmaintained; `serde_yaml_ng` has no node spans. |
| Syntax | `tree-sitter-yaml` through gpui-component's `tree-sitter-yaml` feature (phase 04) | Highlighting and folding in gpui-component's code editor (`EditorState`); editor colors come from `kubyl_ui` (One Dark). |
| YAML editor | gpui-component `EditorState` (Apache-2.0), not Zed's GPL `editor` (phase 04) | Diagnostics, hover and completion use its LSP-style providers; gutter markers, code lenses and end-of-line messages are painted by `kubyl_yaml` over the editor. Diffs: `similar` 3 (line diff, three-way merge). |
| Apply | Server-side apply, field manager `kubyl`, strict field validation; `force` only after the user saw the conflicting managers; replace/create fallback on 415 (phase 04) | `kubyl_yaml::apply`. PROD clusters confirm with the change summary and the typed object name. Kubyl's own applies are kept in state.json (`yaml_history`, never Secrets). |
| Terminal | `alacritty_terminal` (Apache-2.0) with a custom GPUI canvas renderer (decided in phase 05) | Same approach as Zed, without Zed's GPL view code. Cell width = the advance of `m` from GPUI's text system; runs are shaped with that width forced. Control keys, tab and escape are bound to `terminal::SendKeystroke` in the `TerminalView` key context, because gpui-component's `Root` binds `ctrl-c`/`tab` and `secondary-*` is Ctrl on Linux/Windows. |
| Log view | `gpui::list` with `FollowMode::Tail`, spliced per batch (decided in phase 05) | Variable-height rows (wrap, pretty JSON). Timestamps are always requested from the API and split off each line; reconnects resume at `sinceTime`. Selector sources watch their pods. |
| File transfers | Exec only: `tar` streams, `cat`, `dd` chunks with resume, `sha256sum` verification (decided in phase 06) | Works without `kubectl` and without anything installed in the image; distroless containers go through an ephemeral busybox container reading `/proc/1/root`. Uploads extract with `tar xof` (files belong to the container's user). |
| Drag out to the OS | GPUI's `external_drag_payload` with files staged locally (decided in phase 06) | GPUI hands only existing local files to the OS (macOS, Wayland; no file promises, nothing on Windows/X11). Small pod files are downloaded when a drag starts and offered once complete; folders, large files and Secret mounts use "Download to…". |
| Charts | Own GPUI `canvas`/path renderer in `kubyl_charts` | No webviews. Eight series colors: the theme's accent/orange/purple/cyan hues plus magenta/mint/indigo/amber, re-stepped in lightness per theme (adjacent CVD separation, 3:1 on the card surface). A chart shows at most four series plus a dashed "other". `ColorRegistry` keeps an entity's color on every chart of a scope (`<cluster>/namespace`…), filling the safest slots first (phase 07). |
| Metrics | `kubyl_metrics::MetricsService`: one demand-driven cache per cluster (decided in phase 07). Prometheus through the API server's service proxy (discovered, or a settings override incl. an external URL whose Authorization header lives in the keychain), metrics-server as fallback | Reads mark data as wanted; a 1 s loop refreshes whatever is stale, so every view shares one fetch and unwatched clusters cost nothing. The PromQL library is versioned, prefers kube-prometheus recording rules when present, and is overridable (`metrics.queries`). Current usage: Prometheus instant queries (else metrics-server); history: Prometheus range queries only. Which exporters and rules exist comes from `/api/v1/label/__name__/values`; panels and charts whose metric is missing are left out. Node-exporter series are joined to node names through `node_uname_info`. |
| Events | `events.k8s.io/v1` with core `v1` fallback, over shared `ResourceStores` watches; `OOMKilled` warnings derived from pod status (decided in phase 07) | Kubernetes records no Event for an OOM kill (only `BackOff` after the restart), so the stream derives one from `lastState.terminated.reason`, marked "pod status". Repeats fold into `×N`. |
| Web views | `wry` 0.57 (MIT/Apache-2.0), decided per platform in phase 08: **macOS** WKWebView as an NSView child of GPUI's view; **Windows** WebView2 as an HWND child; **Linux X11** WebKitGTK as an X11 child window; **Linux Wayland** a GTK window of its own that the tab drives (Wayland can't embed another client's surface); the **system browser** (`webview.open_in = "browser"`, or when a view can't be created) with a session tab that keeps the forward | `kubyl_webview::host` places native views where their tab paints them and hides them when the tab isn't painted or GPUI draws over them (dialogs, palette, toasts, the tab's menu). Views are created from a GPUI task outside `App` updates (WebView2 pumps Win32 messages while creating). Kubyl's shortcuts reach GPUI while the page has focus (macOS: GPUI's `performKeyEquivalent:`; Windows: WebView2 `AcceleratorKeyPressed`; Linux: GTK `key-press-event`). One data store per (cluster, namespace, service): WKWebsiteDataStore identifier (macOS 14+, private stores before), a WebView2 profile, a WebKitGTK context. Self-signed HTTPS is accepted per (cluster, service, port) by certificate fingerprint through each engine's trust hook. Forwards stay TCP (no Host rewriting). Nothing is initialized until the first web view; the WebKit/WebKitGTK libraries are linked (Linux packages depend on WebKitGTK 4.1 and GTK 3). |
| Argo CD access | Two modes (decided in phase 10). **Kubernetes mode** (default): reads the Argo CD CRDs through `ResourceStores` and acts by patching the `Application` (`operation` like `argocd app sync`, refresh annotation, `spec.syncPolicy`, finalizers for cascading delete) under the user's kube RBAC. **API mode** (after signing in): `argocd-server`'s REST API for diffs, the full resource tree and actions under Argo CD's RBAC | Same `operation` format in v3.4 and v3.5 (the two tested minors). Per-resource health is computed from live objects in Kubernetes mode (Argo CD 3.x doesn't store it). `kubyl_argocd::run::run` picks the mode per action. |
| Argo CD server trust | Kubyl never signs in to an argocd-server it found by itself: the user confirms the install (cluster, namespace, Service, URL shown) when signing in, and `state.json` keeps that per context with the **Service UID**; a re-created Service asks again. Kube credentials never reach Argo CD | Through the API server's service proxy the Argo CD token travels as the `argocd.token` cookie (the proxy strips `Authorization`, verified); otherwise over a temporary loopback forward (`ForwardSpec::ephemeral`) with a bearer header. Sign-in: SSO in the system browser like `argocd login --sso` (Dex, or a `cliClientID`; PKCE, redirect `http://localhost:8085/auth/callback`, refresh token renews it), username/password, or a pasted token. Tokens only in the keychain. Detection reads ConfigMaps, Services and workloads, never Secrets. Argo CD's web UI in a web view gets the session as its `argocd.token` cookie (session only, HttpOnly, only that Service): the one exception to "no credentials in web content". |
| Kubeconfig writes | Kubyl writes kubeconfigs only from the kubeconfig editor (`kubyl_kubeconfig`, decided in phase 11; this replaces "Kubyl never modifies kubeconfig files"). **Kubyl-owned** files (`<config dir>/kubeconfigs/`: pasted or created in Kubyl) are edited freely. **Other files** (`~/.kube/config`, `$KUBECONFIG`, user-added) stay read-only until the user turns on editing for that file ("Edit this file", kept in settings.json `kubeconfig_editor.editable_files`; `kubeconfig_editor.allow_external_edits: false` removes the option). The alternative is "Save as a Kubyl copy": the copy replaces the original as a source, the original is untouched | Every save shows a diff preview, keeps a timestamped backup (`<config dir>/kubeconfig-backups/`, mode 0600, the last `kubeconfig_editor.backups_kept` per file, default 10), writes atomically (temp file in the same folder, fsync, rename; symlinks are followed) and refuses to overwrite a file whose SHA-256 changed since it was loaded (checked before the backup and again right before the rename). New files and files with inline credentials are 0600, others keep their mode. Phase 01 still deletes only pasted files. |
| Kubeconfig comments and key order | Kept (decided in phase 11). The editor never re-serializes a file it can edit in place: `kubyl_kubeconfig::yaml` compares the loaded document with the edited model on `kubyl_yaml::parse` spans and writes minimal text edits (scalars replaced in place with their quote style and trailing comment, keys and list items removed or appended at the right indent, flow collections rewritten in flow style). The result is parsed again and must equal the edited model; otherwise the file is rendered from scratch and the save preview lists every comment that would be lost | Spike (2026-09-26): `yamlpatch` 1.30.1 (MIT) dropped the other keys of a flow mapping when replacing one value, wrote `'yes'` unquoted (a boolean for kubectl's YAML 1.1 parser) and removed a trailing comment with the last list item; `yaml-edit` 0.3.2 (Apache-2.0) mis-indented appended nested entries. Both pass `cargo deny`, neither was solid enough for `~/.kube/config`. Scalars are quoted whenever YAML 1.1 (`sigs.k8s.io/yaml`, which kubectl uses) could read them as anything but a string. |
| Kubeconfig credentials | Inline in the file, as kubectl expects (decided in phase 11); a file with inline credentials is written 0600. Keeping a secret in the OS keychain behind a `kubyl credential <id>` exec plugin is a follow-up | Secrets (tokens, client keys, passwords, OIDC client secrets and refresh tokens) are masked in the form and the YAML tab until revealed; copying one is an explicit action with a toast; they never reach logs, settings.json or state.json. Exports with credentials warn first and are written 0600. |
| Exec-plugin consent and CA trust (kubeconfig editor) | An exec plugin from an import, a paste or unsaved edits runs only after the user saw its command, args, env, API version and interactive mode and agreed (decided in phase 11). `interactiveMode` isn't consent. Consent covers that exact config for the session (a hash in memory); "Test all contexts" asks once for every plugin that needs it. Plugins in saved, loaded kubeconfigs behave as in phase 01. Pasting a kubeconfig (phase 01's dialog) lists its exec plugins and adds the file only after "I checked these commands and trust them" | Fetching a CA is trust on first use: a TLS handshake that sends nothing, plus an anonymous read of `kube-public/cluster-info` (kubeadm clusters publish their CA there); a candidate must verify the server's certificate. The UI shows subject, validity and the SHA-256 fingerprint and needs an explicit confirmation. No credentials go to a server before its certificate verifies against a trusted CA; a TLS failure never falls back to insecure mode (`insecure-skip-tls-verify` only when the user sets it, with a red warning). |
| Cluster entries (context grouping) | One sidebar entry per cluster and user (decided in phase 15). Contexts of one kubeconfig file that differ only in their namespace (the same `cluster` and `user` entries, or entries with identical contents, and every other context field equal) are one entry; `oc` adds a context per `oc project`, so 37 contexts become 8 rows. Another user, token, exec plugin, impersonation, proxy, TLS setting or file stays separate. Credentials are compared in memory only, never logged or hashed to disk. `kubernetes.group_contexts` (default on) turns grouping off; "Show Contexts Separately" does it per group (`kubernetes.separate_groups`) | **Ids:** a group is `group:<cluster entry>,<user entry>@<file>/` (`%`, `,` and `@` in the names are percent-escaped). A context id `<context>@<file>` never ends in `/`, so the two forms can't collide; contexts without siblings keep their id. Ids don't change with `oc project` (new members, another current context). `ConnectionManager::resolve` maps member ids, old group ids and live aliases to the current entry: every accessor resolves, events are also emitted under member ids, and `ViewRegistry::build` resolves the target of restored tabs (`kubyl_core::resolve_cluster`). When a context gets its first sibling (or loses its last one) the live connection is re-keyed, never reconnected; a changed namespace or current context never reconnects. **Settings:** the group id first, then the members (the file's current context first, then by name); `production`/`read_only` on any member apply to the group, `hidden` only when every member is hidden, color/display name/default namespace from the first that sets them, `namespaces` is the union plus every member's namespace. Changes are written under the group id (turning a safety flag off also clears it on the members). `settings_keys(id)` lists the group id, member ids and member context names for crates that key settings by cluster (`metrics.prometheus`, `alerts.clusters`). **Label:** the display name, else for `oc`-style contexts (`<ns>/<cluster entry>/<user>`) the server host without `api.` and without `:443`/`:6443`, then ` · ` and the user (`ocp.eu1.example.com · jane.doe@example.com`), else `<cluster entry> · <user entry>`. **Namespace:** a group starts in its default namespace, else the file's current context's namespace when that context is a member, else the last namespace used in Kubyl, else the first member's; the members' namespaces are offered in the namespace picker ("from kubeconfig"). |
| Alerts | `kubyl_alerts` (decided in phase 14). Alertmanager API v2 (0.22 or newer; Mimir/Cortex through settings) is the main source: firing, silenced, inhibited alerts, receivers and silences. The Prometheus rules API of the Prometheus phase 07 found (`/api/v1/alerts`, `/api/v1/rules`) adds pending alerts, rules and rule health; either source alone shows what it can. Silences are in scope: create, edit, extend, expire, acknowledge | Built on `kubyl_metrics::transport` (service proxy, OpenShift Route, direct URL) and `MetricsService::prometheus`. Demand-driven like `MetricsService`: every 15 s while a view shows a cluster's alerts, every 60 s for badges, the status bar and notifications (also while no Kubyl window is active), rules every 2 min; only connected clusters. Alert data is kept in memory only; view options (grouping, filters) go to state.json. Silences are hidden on read-only clusters, always need a comment, and every create or edit shows a summary; on PROD it needs the typed cluster name when a critical alert matches, more than 10 alerts match or there's no `alertname` matcher. |
| Alertmanager credentials | The user's kube token, or a service-account token Kubyl minted (TokenRequest), only goes to Services in `openshift-monitoring` or `openshift-user-workload-monitoring` (only cluster admins can create `openshift-*` namespaces) or to Services named in settings, and only over HTTPS with verified TLS (decided in phase 14). Never to a Service Kubyl merely discovered elsewhere | Plain Alertmanagers go through the API server's service proxy: the API server authenticates the user and nothing reaches Alertmanager. OpenShift's platform Alertmanager: its admitted Route (phase 07's `through_route`); user workload monitoring: a temporary loopback forward verified against the service CA with the Service's DNS name as TLS server name. External URLs: an Authorization header from the keychain (`alerts-auth:<cluster id>/<url>`), not sent with `insecure_skip_tls_verify`. Writes with a service-account token need consent in the silence dialog; `createdBy` stays the user. `config.original` of `/api/v2/status` (receiver URLs, secrets) is dropped as soon as it arrives. |
| Operators (OLM v0) | `kubyl_operators` reads Subscriptions, ClusterServiceVersions (without OLM's copies: label selector `!olm.copiedFrom`), InstallPlans, CatalogSources and OperatorGroups through shared `ResourceStores` watches and joins them (decided in phase 12): a Subscription's `status.installedCSV` is the operator, its `installPlanRef` the pending plan; CSVs without a Subscription are listed too | Status: **Failed** (CSV `Failed`, `ResolutionFailed`, a failed InstallPlan), **Installing** (CSV not yet `Succeeded`, an approved plan still installing), **Upgrade available** (a `RequiresApproval` InstallPlan for another CSV), **Succeeded**. Writes (install, approve, uninstall, create instance) are hidden on read-only clusters, always show what gets created or deleted first, and need the typed cluster name on PROD. |
| Operator upgrade review | The CRDs of a pending upgrade come from the InstallPlan's `status.plan[]` steps: inline manifests, or (what OLM writes for bundle images) a reference to the bundle ConfigMap OLM unpacked in the catalog's namespace (`olm.contentEncoding: gzip+base64`), read the way OLM reads it. Each is compared with the **live** CRD (`spec` only, as YAML through `kubyl_yaml::diff`). Kubyl never pulls bundle images or talks to catalog pods (decided in phase 12) | **RBAC changes** compare the new CSV's `spec.install.spec.permissions`/`clusterPermissions` (from the same plan) with the installed CSV's, flattened to (scope, service account, API group, resource, verb; resource names and non-resource URLs kept apart): added and removed rules. **Compatibility**: `minKubeVersion` against the server, `olm.maxOpenShiftVersion` against OpenShift, CRD versions that stop being served but are still in `status.storedVersions` (OLM refuses those), install modes the new CSV drops. Approve is a merge patch `spec.approved: true`. Reading the bundle ConfigMap needs `get configmaps` in the catalog namespace; without it the review says so and shows the plan's steps. |
| OperatorHub | PackageManifests (`packages.operators.coreos.com/v1`, get/list only) are the only source; no catalog gRPC or FBC (decided in phase 12): they'd need pod access, bypass RBAC and duplicate the packageserver | Listed across namespaces on Tokio when the tab opens, every 10 min while it's shown and on reload, and parsed into compact records (the operatorhub.io catalog is ~11 MB, 449 packages). Channel heads' `currentCSVDesc` give description, install modes, CRDs, capability level, categories and `alm-examples`; `entries` give older versions (`startingCSV`). Icons come from the `packagemanifests/icon` subresource for the cards on screen (PNG/SVG, cached in memory), letter tiles otherwise. |
| Operator install and uninstall | Install creates a Subscription and, when needed, the Namespace and an OperatorGroup (decided in phase 12). All-namespaces installs go to the namespace of the global OperatorGroup (one without `targetNamespaces` or selector: `operators/global-operators`, OpenShift's `openshift-operators/global-operators`); single-namespace installs use the namespace's OperatorGroup if it targets exactly that namespace, else create one named after the namespace, and refuse namespaces with several OperatorGroups or one targeting others | Uninstall deletes the Subscription and the installed CSV. Optionally the instances of the operator's CRDs first (while the operator still runs, so their finalizers complete; up to 2 min), then the CRDs: that needs the typed operator name, and on PROD every uninstall needs it. OperatorGroups and namespaces stay (other operators may use them). |
| OLM v1 | Read-only lists of `ClusterExtension` and `ClusterCatalog` (`olm.operatorframework.io/v1`) with their conditions; install and upgrade through YAML templates (a ClusterExtension with its installer ServiceAccount; upgrades edit `spec.source.catalog.version`). A full UI is a follow-up (decided in phase 12) | Shown as the Operators tab's Extensions sub-tab when the cluster serves the group. |
| Helm releases | Read-only in v1: rollback, uninstall and upgrade are "Copy helm command" (decided in phase 12). Releases come from a **metadata-only** watch of Secrets labelled `owner=helm` (and ConfigMaps, for `HELM_DRIVER=configmap`; the SQL driver isn't supported). The latest revision of each release is fetched and decoded on Tokio: Secret `data.release` is base64 (the API) of base64 of gzip of JSON, a ConfigMap's is base64 of gzip; JSON without the gzip magic is taken as is | The list keeps only chart, versions, status, dates and description. Values, manifest and notes are decoded when a release tab opens, stay in that tab's memory, and never reach logs, settings.json, state.json, `Debug` output, toasts or the palette. **Values** show keys and booleans; strings and numbers are masked until "Reveal" (per tab, not remembered); copying them is an explicit action with a toast that says they may hold passwords. Secrets in the manifest keep `data`/`stringData` masked. A 403 says which verb on `secrets` is missing and falls back to the active namespace. |
| Operators views | `ViewKind::Operators` is one tab per cluster with sub-tabs Installed · Install plans · Subscriptions · Helm releases (· Extensions with OLM v1), each with a details pane; `ViewKind::Custom("operatorhub")` is its own tab (decided in phase 12) | The Administration rows "Installed Operators" and "OperatorHub" open them; a new row "Helm Releases" (not gated on OLM) opens the Operators tab on Helm releases (`ViewKind::Custom("helm_releases")`). A release opens in its own tab (`ViewKind::Custom("helm_release")`, target: the release's latest Secret or ConfigMap). Without OLM the Operators tab has Installed (which explains it) and Helm releases. The upgrade review is a dialog (board 7), "Create" opens the YAML editor with the kind's `alm-examples` entry. |
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

- `ViewRegistry`: open a tab/pane for a `ViewRequest { kind: ViewKind, target: Option<ResourceRef>,
  path: Option<PathBuf> }` (Table, Details, Yaml, Logs, Terminal, Files, Overview, Operators,
  Updates, Settings…; `ViewRequest::for_path` for views of a file, since phase 11). Views
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
  `kubyl_yaml::open_draft(list_ref, text, note, window, cx)` opens a new-resource editor with
  that text and an info banner instead of the kind's template (phase 12: operator examples).
- Confirmations (phase 02/04): `kubyl_explorer::dialogs::confirm(ConfirmSpec { typed, lines, .. })`,
  one-line input: `kubyl_explorer::dialogs::prompt_text`.
- Dock panels (phase 05): dispatch `kubyl_core::actions::ActivateDockPanel(id)` to show the dock
  holding a `DockPanel` with that id, activate it and focus it (the panel's `Focusable` decides
  which element). Dispatch it before focusing anything inside a hidden dock.
- Active sessions (phase 05): long-running work registers a row with
  `kubyl_logs::sessions::SessionRegistry::add(kind, title, subtitle, status, tone, on_stop)` and
  keeps it current with `set_status` / `set_title` / `set_buttons`; the right-dock "Active
  Sessions" panel and the status bar show them next to the resource watches
  (`kubyl_resources::ResourceStores::watches`, pausable with `ResourceStore::pause/resume`).
- Terminals (phase 05): `kubyl_terminal::open(TerminalSpec, in_tab, cx)` opens exec, attach,
  debug-container or node-shell sessions in the bottom-dock Terminal panel. `kubyl_terminal::exec`
  (`pod_info`, `run`, `create_debug_container`, `wait_running`) is the exec layer for other crates
  (the file browser in phase 06).
- Files (phase 06): open `ViewKind::Files` for a pod (`kubyl_files::view::open`, `f` in pod
  lists). `kubyl_files::remote::open` probes a container and gives a `RemoteTarget` (list, read,
  write, stat, mkdir, rename, delete, chmod over exec); `kubyl_files::queue::TransferQueue`
  runs verified transfers for any crate (`enqueue(TransferJob)`).
- Metrics (phase 07): `kubyl_resources::metrics::Metrics::provider(cx)` answers pod/node usage,
  pod history (sparklines) and `source_status` from a cache; asking keeps the data fresh.
  Observe the `Metrics` global (bumped by `Metrics::changed`) to re-render or re-sort. For
  charts and totals use `kubyl_metrics::MetricsService::global(cx)`: `source`, `nodes`, `pods`,
  `range(cluster, &RangeKey::new("namespace_cpu", TimeRange::H1).filter("namespace", ns))`
  (query ids in `kubyl_metrics::queries::LIBRARY`); observe the entity for new results.
- Charts (phase 07): `kubyl_charts::{LineChart, Sparkline, Meter, TimeRangePicker}`;
  `ColorRegistry::assign(cx, "<cluster>/namespace", &names)` for entity colors that match the
  other charts, `series_color(slot, colors)`, `data::{align, top_n}` for Prometheus data.
- Details sections (phase 07): `ChromeRegistry::add_details_section(cx, impl DetailsSection)`
  adds a view to the Summary of an object's details (built once per shown object). The Metrics
  section (`kubyl_metrics::details`) is one; its panels per kind are in `kubyl_metrics::panels`.
- Overview and events (phase 07): open `ViewKind::Overview` with
  `ResourceRef::list(cluster, Gvr::new("", "", ""), namespace)` (a namespace gives the namespace
  variant) and `ViewKind::Events` (namespace `None` follows the active one). The right-dock panel
  id is `events`. `kubyl_overview::events::EventsFeed` is the live, groupable, pausable stream for
  any scope.
- Secrets typed by the user (phase 07): `kubyl_explorer::dialogs::prompt_secret` (masked); store
  the value with `kubyl_kube::auth::store`, never in settings.
- Port-forwards from anywhere (phase 05): dispatch `kubyl_core::actions::ForwardPort { target,
  port }` (one click: same local port when free, `80` → `8080`) and `StopForward(id)`; read the
  running ones from the `kubyl_core::forwards::ActiveForwards` global (observe it to update).
  `kubyl_portforward` publishes it; the details pane shows forwards next to each port.
- Temporary forwards (phase 08): `ForwardSpec::ephemeral` (`kubyl_portforward::manager::Ephemeral`)
  makes a forward unsaveable, keeps it out of `ActiveForwards`, titles its Active Sessions row
  and calls back when the user stops it; `PortForwardManager::info(id)` gives its state, local
  port, pod, connections and bytes.
- Web views (phase 08): dispatch `kubyl_webview::OpenWebView { target, port, path, ask }` for a
  port of a Service or Pod (`ask`: pick the scheme and start path first). The tab shares the
  port's loopback forward (`kubyl_webview::forward::WebForwards`) with other tabs of the port.
- Table cells with buttons (phase 08): `CellValue::Buttons(Vec<CellButton>)`, each building
  its action for the row's object; `ResourceColumns::extend` adds columns (before `age`) to a
  kind that has a provider.
- Tabs (phase 08): `TabView::tab_dot` (a colored dot, e.g. the cluster color) and
  `TabView::wants_close` (the pane closes the tab when its view asks).
- Kind-specific views (phase 10): `ViewRegistry::register_list_view(cx, group, resource, kind)`
  and `register_object_view` make the explorer, the palette and `open_selected` open a kind's
  own view instead of the generic table/details (`ViewRegistry::list_view`/`object_view`
  resolve, with the generic view as fallback).
- Sidebar groups (phase 10): `kubyl_explorer::catalog::register_tree_group(cx, TreeGroup {
  id, parent, label, kinds, badge })` adds a group under a built-in one (e.g. Argo CD under
  Administration); only kinds the cluster serves show, and the group follows discovery.
  Call `catalog::tree_groups_changed(cx)` when the badge changes. The kinds stay under Custom
  Resources too.
- Edit notices (phase 10): `ChromeRegistry::add_edit_notice(cx, impl EditNotice)` puts a
  warning banner in the YAML editor for objects it matches (Argo CD: "managed by app X,
  self-heal may revert this").
- Diffs and filtered logs (phase 10): `kubyl_yaml::diff_view(&LineDiff, side_by_side, colors)`
  renders phase 04's diff anywhere; `kubyl_logs::open_filtered(target, query, regex)` opens a
  log view with a search applied (matches only).
- Web view sessions (phase 10): `kubyl_webview::add_session_provider(cx, |target, cx| cookies)`
  hands session cookies (HttpOnly, session only, set before the first load) to web views of a
  target the crate holds a session for. Only for a target the user confirmed.
- Argo CD (phase 10): `ClusterCaps::argocd` (`applications`, `application_sets`, `projects`,
  `any()`) says which Argo CD CRDs a cluster serves; `kubyl_argocd::dock::managed_by(object)`
  tells which Application tracks an object.
- Kubeconfig editor (phase 11): dispatch `kubyl_core::actions::EditKubeconfig { path, context }`
  to open a kubeconfig in the editor tab (and select a context), `NewKubeconfig` for the
  wizard. Without the UI: `kubyl_kubeconfig::conntest::run` (the step-by-step connection
  test), `tls::{check, fetch_ca}` (handshake and trust-on-first-use CA fetch), `certs` (PEM and
  X.509 details), `files::save` (atomic, backup, hash check), `yaml::write` (comment-preserving
  writes). `kubyl_kube::kubeconfig::context_info` builds a `ContextInfo` from any in-memory
  kubeconfig; `ConnectionManager::move_context_settings` moves a context's overrides.
- Cluster ids (phase 15): a grouped entry's id is `group:…`, its members keep theirs. Anything
  that reads a persisted cluster id goes through `ConnectionManager::resolve` (or
  `kubyl_core::ClusterIds::resolve` without the manager: `ViewRegistry::build` does it for
  restored tabs; observe `ClusterIds` for changes). Settings keyed by cluster look up
  `ConnectionManager::settings_keys(id)` (entry id, member ids, member context names), state that
  must survive `oc project` uses `kubyl_kube::kubeconfig::stable_key`, and
  `ConnectionEvent::Rekeyed { from, to }` says a live connection moved to a new id.
- Status dots and rows (phase 15): `kubyl_ui::StatusDot::pulsing(id)` (connecting),
  `TreeRow::tooltip`, `SectionHeader::end_child`.
- Sidebar rows (phase 14): `kubyl_explorer::catalog::register_view_row(cx, ViewRow { id, after,
  label, icon, kind, visible, badge })` adds a top-level row under every cluster (it follows
  `explorer.group_order` and `hidden_groups` under its id and opens `kind` for the cluster);
  `register_root_marker(cx, |cluster, cx| Option<RootMarker>)` puts an icon on cluster root
  rows (visible while collapsed). Call `catalog::view_rows_changed(cx)` when badges or markers
  change.
- Overview sections (phase 14): `ChromeRegistry::add_overview_section(cx, impl
  OverviewSection)` adds a view below the KPI tiles of the cluster and namespace overviews.
- Toast buttons (phase 14): `Notification::action(label, |window, cx| …)` ("Undo", "Show"); a
  toast with a button stays until closed.
- HTTP transports (phase 14): `kubyl_metrics::transport::Transport` (`service_proxy`, `direct`
  with a bearer, roots and `tls_server_name`, `external` with a keychain header, `with_header`;
  `get`/`post_json`/`delete` with Prometheus-style error mapping) for any in-cluster HTTP API.
  `kubyl_metrics::openshift::through_route(.., probe_path)` reaches a Service behind an OpenShift
  auth proxy through its Route (callers decide whether it may get a token);
  `MetricsService::prometheus(cluster)` hands out the Prometheus client phase 07 found.
- Alerts (phase 14): `kubyl_alerts::AlertsService::global(cx)`: `cluster(id, Pace)` (a view's
  read keeps the 15 s pace), `counts`, `phase`, `has_source`; `service::alerts_for(cluster,
  &ObjectFilter, cx)` for an object's alerts. `kubyl_alerts::view::open(cluster, tab, …)` and
  `open_with(cluster, Pending { tab, query, object, select }, …)` open the Alerts tab filtered
  or on one alert.

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
