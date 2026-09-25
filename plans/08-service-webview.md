# Phase 08: Service web views (embedded browser over temporary port-forwards)

**Status:** done (branch `phase/08-service-webview`; see the handoff log)
**Depends on:** 05 (port-forward manager), 02 (Services/Pods/Ingress lists and details)
**Owns:** `crates/kubyl_webview`
**Mockups:** board 10 · Service web view over a temporary port-forward

## Goal

Every HTTP(S) port on a Service or Pod gets a **web view** button. Clicking it starts a hidden
port-forward on a random local port and opens the page in an embedded browser tab inside Kubyl.
Closing the tab stops the forward. This makes internal UIs (admin dashboards, Grafana,
Prometheus, RabbitMQ management, Argo CD, Kafka UIs…) reachable without exposing them or
running `kubectl port-forward` by hand.

## Tasks

### Spike first (decides the approach, 1–2 days)
- [x] Embed [`wry`](https://github.com/tauri-apps/wry) (MIT/Apache-2.0; WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux) as a **child view of the GPUI window**, using `WebViewBuilder::build_as_child` with the window's `raw-window-handle` and GPUI-driven bounds (resize, scroll, hide when the tab is inactive or covered by a modal/palette)
- [x] Check each platform: macOS (NSView child), Windows (HWND child, WebView2 runtime present or bootstrapper), Linux X11 (child works) and **Wayland** (wry child webviews need a GTK container; likely not possible inside a GPUI Wayland surface)
- [x] Decide per platform and write the result in the Handoff log and README decision table:
  1. **Embedded child view** (preferred)
  2. **Separate native window** owned by Kubyl (wry + `tao` window, styled title, closes with its tab), e.g. for Linux/Wayland
  3. **System browser** fallback: a "web session" tab in Kubyl shows the URL, forward status and a Stop button. The forward lives as long as that tab
- [x] Measure the binary size and startup cost. Load the webview lazily (only when the first web view opens)

### Temporary port-forwards
- [x] `EphemeralForward`: built on phase 05's port-forward manager. Binds `127.0.0.1:<random free port>`, **not** listed as a saved forward, owned by a web-view handle (ref-counted: two tabs on the same service port share one forward)
- [x] Lifecycle: starts on open, reconnects if the target pod is replaced (Service re-resolve from phase 05), **stops when the last web view using it closes**, when the cluster disconnects, or when the app quits. The Active sessions panel shows it as "web view · svc/grafana:3000" with a stop button (stopping closes the tab)
- [x] Idle timeout option for background tabs (e.g. stop forwards of tabs hidden for more than 30 min, restart on focus)

### Where the button appears
- [x] Service details and the Services table: one button per port that looks like HTTP. Detection order: `appProtocol` (`http`, `https`, `kubernetes.io/h2c`), port name (`http`, `https`, `web`, `ui`, `metrics`, `admin`, `http-*`), well-known port numbers (80, 443, 8080, 8443, 3000, 9090, 9093, 15672…), or "Open as web view…" on any port
- [x] Pod details: container ports (same detection), for pods without a Service
- [x] Ingress details: open the backend service through a forward (even if the ingress host isn't reachable from here)
- [x] Palette action `> Open web view…` (pick service and port), and a key in list views (`w`, added to the k9s hint bar)
- [x] Remember per service port: scheme, start path (e.g. `/graph`, `/grafana/`), last URL. Built-in presets for common apps (Grafana, Prometheus, Alertmanager, Argo CD, RabbitMQ, Kibana, Kafka UI, Jaeger)

### The web-view tab
- [x] Tab title: page title plus service, and an icon showing the cluster color (PROD badge on production clusters)
- [x] Toolbar: back, forward, reload, address bar (path editable, host locked to the forward), copy URL, open in the external browser (the forward is kept alive while that tab stays open), zoom, dev tools (debug builds, or a setting)
- [x] HTTPS services with self-signed certificates: show an interstitial with the certificate details. "Proceed" is remembered per (cluster, service, port) only
- [x] Storage isolation: a separate web data directory per (cluster, namespace, service), so cookies and logins for different clusters never mix. "Clear site data" action. Private mode option (nothing persisted)
- [x] Rewrite `Host`/`Origin` only if an app needs it (per-preset option). Handle apps that redirect to their external URL (show "This app redirected to https://grafana.example.com — open externally / stay"). *(Redirect banner done; Host/Origin rewriting deferred: no preset needs it, see the handoff log.)*
- [x] Downloads go through Kubyl's download prompt. File uploads use the native picker
- [x] Keyboard: web content gets focus inside the tab; Kubyl's global shortcuts (⌘K, ⌘W, tab switching) still work

### Safety
- [x] Forwards only bind to loopback. Nothing is exposed on the network
- [x] Read-only clusters: web views are still allowed (they're a read path), but show the read-only badge in the toolbar, because the app inside can change things
- [x] No Kubyl credentials or tokens are ever injected into web content

## Acceptance criteria

- On kind with kube-prometheus-stack: Grafana and Prometheus open in tabs from the Services list with one click. Closing the tab stops the forward (it disappears from Active sessions, and the local port is free).
- Two tabs on the same service share one forward. Closing one keeps the other working.
- Killing the backing pod keeps the web view working after a short reconnect.
- Logins to Grafana on two different clusters don't share cookies.
- Works on macOS and Windows embedded. Linux works embedded on X11, and via the documented fallback on Wayland.

## Risks

- Embedding a native webview inside GPUI isn't officially supported by GPUI. Keep it isolated in `kubyl_webview` behind a trait, so the separate-window or system-browser fallbacks stay possible.
- Wayland will probably need the separate-window or system-browser fallback.
- WebView2 must be present on Windows 10 (bundle the bootstrapper in phase 09).

## Handoff log

### 2026-09-25 (branch `phase/08-service-webview`)

Everything in the task list is done except Host/Origin rewriting (deferred, below). The stub
crate landed first as PR #4.

**Per-platform decision** (also in the README decision table).

| Platform | Approach | Verified |
|---|---|---|
| macOS | Embedded: WKWebView as an NSView subview of GPUI's view (`build_as_child`) | Real app on kind: Grafana, Prometheus, the self-signed HTTPS service, downloads, typing, ⌘A/⌘C/⌘V, ⌘K, ⌘W, ctrl-tab (real AppKit key events, see below) |
| Windows | Embedded: WebView2 as an HWND child | Cross-compiled (`x86_64-pc-windows-msvc`) and CI builds; CI runs the real web view test (`live_webview`) on `windows-latest`. Not run interactively: no Windows machine here |
| Linux, X11 | Embedded: WebKitGTK in a child X11 window of GPUI's window | Real app under Xvfb in Docker (Ubuntu 24.04): Grafana renders in the tab, real X11 typing and Ctrl+A in the page, Ctrl+K from the page opens the palette (screenshots `design/screenshots/phase-08-linux-x11.png`) |
| Linux, Wayland | A GTK window of its own, driven by the tab (toolbar, address bar, zoom…); closing either closes both | Real app under headless sway: the tab shows "The page is open in its own window", the GTK window renders Grafana (`phase-08-linux-wayland.png`). A Wayland client can't embed another client's surface, and GTK's `build_gtk` needs a GTK container |
| Any | System browser (`webview.open_in = "browser"`, or when a view can't be created): the tab shows the session and keeps the forward | Code path only |

**Cost.** Release binary on macOS: 58.1 MB → 59.8 MB (+1.7 MB, +3 %). App start is unchanged
(process start to Kubyl's first log line: 6.2 ms median on both; WebKit comes from the dyld
shared cache). Nothing web-related starts before the first web view: GTK is initialized then,
WebView2 environments and WebKit processes are created per view. The first view takes ~70 ms
on the UI thread (debug and release alike). Linux binaries link WebKitGTK 4.1 and GTK 3; the
.deb/.rpm/Arch packages depend on them and CI installs the -dev packages.

**How it fits together** (`crates/kubyl_webview`).
- `native`: the platform views through wry 0.57, plus per-platform hooks (`macos.rs`,
  `windows.rs`, `linux.rs`). Views are created from a GPUI task *outside* any `App` update,
  because WebView2 pumps the Win32 message loop while creating a controller. Events come back
  through a channel (`NativeEvent`), never by touching GPUI from a callback.
- `host`: a `WebContent` element places the native view where its tab paints it; at the end of
  every frame (a deferred element in the status bar item) views that weren't painted are
  hidden, as are views under a dialog, the palette, a sheet or a toast. The tab hides its view
  itself while it draws something in its place (starting, errors, the certificate
  interstitial, its "…" menu).
- `forward`: `WebForwards`, one forward per (cluster, namespace, object, port), held by its
  tabs (`hold`/`release`), on phase 05's manager with the new `ForwardSpec::ephemeral`
  (unsaveable, not in `ActiveForwards`, titled `web view · svc/grafana:80`, its stop button
  closes the tabs). Stops with the last tab, on idle, on disconnect; restarts on reconnect.
  The last local port is remembered and reused when free, so a page keeps its origin (and
  local storage) between sessions.
- `view`: the tab (board 10 toolbar, address bar with PROD/read-only badges, service chip and
  origin, zoom chip, open in browser, dev tools, "…" menu with copy URL, new tab, zoom, HTTPS
  toggle, "start here next time", private session, clear site data, stop), interstitial,
  redirect banner, downloads bar. `target`/`store`: detection, presets, per-port memory,
  accepted certificates. `details`: the "Web views" sections of Service, Pod and Ingress
  details (ports with `Web view` / `Open · N tabs` / `Open as web view…`, the temporary
  forward, session storage and idle stop, other web UIs in the namespace). `picker`: the
  palette picker, `w` with several ports, "Open as web view…" (scheme, start path).
- Shared-crate commits: `kubyl_portforward` (ephemeral forwards, `info`, connections report
  their pod, probes use the forward's own client), `kubyl_core` (`CellValue::Buttons`,
  `ResourceColumns::extend`, `TabView::tab_dot`/`wants_close`), `kubyl_explorer`/`kubyl_ui`
  (button cells, tab dots), `kubyl` (pane closes tabs that ask; harness pastes web view
  snapshots into screenshots), CI/packaging (WebKitGTK), `script/webview-dev.sh`.

**Decisions and why.**
- **Keys.** macOS: GPUI's view gets `performKeyEquivalent:` before the page, so ⌘/ctrl
  shortcuts reach GPUI while WKWebView is first responder, as long as GPUI's focus is on the
  tab. A local event monitor reports clicks into the page (`Focused`) and the tab takes GPUI's
  focus. Kubyl has no Edit menu, so ⌘C/⌘V/⌘X/⌘A/⌘Z are bound in the `WebView` context and sent
  *directly* to the WKWebView (a nil-targeted action fell through to GPUI's app delegate while
  it was dispatching the key and deadlocked). Windows: WebView2's `AcceleratorKeyPressed`;
  Linux: GTK `key-press-event`. Both check the keystroke against the ctrl/alt/cmd/F-key
  bindings Kubyl has in the page's key context (editing keys excluded), mark it handled and
  dispatch it to GPUI. Page shortcuts can't trigger anything Kubyl doesn't bind there.
- **Certificates.** Forwards stay plain TCP; the engine talks TLS to the pod, so Secure
  cookies and absolute https links keep working. Each engine's trust hook accepts a loopback
  certificate only if its SHA-256 was accepted for this (cluster, service, port); otherwise it
  cancels and the tab shows the interstitial (subject, issuer, validity, names, fingerprint).
  macOS: wry has no hook, so `webView:didReceiveAuthenticationChallenge:completionHandler:` is
  added to wry's navigation delegate class at runtime (the class name is mangled per wry
  version: it's taken from the live delegate) and the delegate is set again. Windows:
  `ServerCertificateErrorDetected` (runtime ≥ 1.0.1245). Linux: `load-failed-with-tls-errors`
  + `allow_tls_certificate_for_host`.
- **Storage.** macOS: `WKWebsiteDataStore` per identifier (macOS 14+; older systems get a
  non-persistent store per view, so nothing is ever shared); Windows: one user data folder,
  a profile per service; Linux: one WebKitGTK context (data directory) per service, shared by
  its views (two contexts on one directory don't see each other's cookies). The id is a hash
  of context name + API server + namespace + object (not the ClusterId, which contains the
  kubeconfig path). Remembered paths drop the origin, the fragment and credential-looking
  query parameters (`token`, `code`, `session…`).
- **Tabs aren't restored** at startup (forwards never start by themselves); reopening a port
  goes back to its last page. `window.open`/`target=_blank` on the forward opens another Kubyl
  tab sharing the forward; other URLs go to the system browser. A page that navigates off the
  forward (an app redirecting to its external URL) gets a banner: open in browser, back, stay.
- **Host/Origin rewriting: deferred.** It needs an HTTP-aware proxy instead of the TCP forward.
  None of the presets needs it (they accept any Host); the redirect banner covers apps that
  insist on their external URL.

**Verification.** fmt, `clippy --workspace --all-targets -D warnings`, `cargo test --workspace`
(383 tests) and `cargo deny check` pass; clippy also on Linux (Docker) and a Windows check
build. Unit tests: detection, presets, Ingress backends, remembered paths, per-port memory,
storage ids, accepted certificates, certificate parsing (fixture), download staging, key
names (Windows/Linux), forward sharing/idle/disconnect/failure/stop with a fake backend,
column extensions, `wants_close`.

Live tests, all passing:
- `kubyl_webview/tests/live.rs` (kind, `KUBYL_TEST_KUBECONFIG=/tmp/kubyl-dev/kubeconfig cargo
  test -p kubyl_webview --test live -- --ignored --test-threads=1`): open and close stops the
  forward (session row gone, port free); two tabs share one forward (closing one keeps it);
  killing the pod: the same local port answers again after ~2 s, via the new pod.
- `kubyl_webview/tests/live_webview.rs` (real web views, `KUBYL_TEST_WEBVIEW=1 cargo test -p
  kubyl_webview --test live_webview`; macOS here, Linux under Xvfb, and in CI on macOS,
  Windows and Linux): a login cookie on cluster A's store is seen by a second tab of the same
  service and not by cluster B's store.
- In the app (harness, `script/webview-dev.sh`): Grafana and Prometheus from the Services list
  (`w`, the Web column, details buttons, the palette picker), login and dashboards, the
  interstitial and Proceed, "Open as web view…" on a non-HTTP port, a download through the save
  dialog, the redirect banner, `window.open` → second tab, the "…" menu over the page, ⌘K/⌘W/
  ctrl-tab and copy/paste with the page focused, idle stop after 1 min (setting) and restart,
  PROD/read-only badges. Screenshots: `design/screenshots/phase-08-*.png`.

**Deviations from board 10 / limits.**
- The dock sections are the explorer's details with a contributed "Web views" section; "Open ·
  N tabs" opens another tab (nothing can focus a specific tab from outside the pane yet).
- The address bar drops the origin (then the session label) when the tab is narrow; long
  service names are shortened in the middle of the chip.
- While GPUI draws over a page (palette, dialogs, the menu, toasts), the page is hidden and the
  tab's background shows; snapshots are only used by the screenshot harness (macOS).
- macOS: `Content-Disposition: attachment` on a type WebKit can show (text/csv) opens in the
  page; links with `download` (and blob exports, like Grafana's) download. File inputs use the
  engine's picker (wry's NSOpenPanel on macOS); not automated here.
- The Web column shows up to three port buttons; phase 05's status-bar forward count includes
  web forwards too.
- The harness can't make its window key on macOS 14+ (cooperative activation) and GPUI sends
  no focus events to an inactive window; `webview::DebugInput` (debug builds) posts real
  AppKit events and emulates the key-window routing for tests.

**Gotchas for later sessions.**
- Anything GPUI draws over the center pane (menus, popovers, tooltips) is hidden under a native
  web view. Cover the page first (`Embedded::set_covered`) or keep overlays outside its bounds.
- Never send AppKit actions with a nil target from a GPUI action handler: if nothing in the
  responder chain takes it, GPUI's app delegate does, while GPUI is busy: deadlock.
- WebKitGTK X11 children: showing a view re-runs GTK's size negotiation, which foreign windows
  never finish: re-apply the bounds after showing (done in `NativeWebView::set_visible`).
- wry's `open_devtools` only exists in debug builds or with its `devtools` feature (enabled).
- Web view tabs need the kind port-forward to reach pods; `script/webview-dev.sh` adds Grafana,
  Alertmanager, an Ingress, a self-signed HTTPS service and a non-HTTP-looking port.
