# Phase 11: Service web views (embedded browser over temporary port-forwards)

**Status:** not started
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
- [ ] Embed [`wry`](https://github.com/tauri-apps/wry) (MIT/Apache-2.0; WKWebView on macOS, WebView2 on Windows, WebKitGTK on Linux) as a **child view of the GPUI window**, using `WebViewBuilder::build_as_child` with the window's `raw-window-handle` and GPUI-driven bounds (resize, scroll, hide when the tab is inactive or covered by a modal/palette)
- [ ] Check each platform: macOS (NSView child), Windows (HWND child, WebView2 runtime present or bootstrapper), Linux X11 (child works) and **Wayland** (wry child webviews need a GTK container; likely not possible inside a GPUI Wayland surface)
- [ ] Decide per platform and write the result in the Handoff log and README decision table:
  1. **Embedded child view** (preferred)
  2. **Separate native window** owned by Kubyl (wry + `tao` window, styled title, closes with its tab), e.g. for Linux/Wayland
  3. **System browser** fallback: a "web session" tab in Kubyl shows the URL, forward status and a Stop button. The forward lives as long as that tab
- [ ] Measure the binary size and startup cost. Load the webview lazily (only when the first web view opens)

### Temporary port-forwards
- [ ] `EphemeralForward`: built on phase 05's port-forward manager. Binds `127.0.0.1:<random free port>`, **not** listed as a saved forward, owned by a web-view handle (ref-counted: two tabs on the same service port share one forward)
- [ ] Lifecycle: starts on open, reconnects if the target pod is replaced (Service re-resolve from phase 05), **stops when the last web view using it closes**, when the cluster disconnects, or when the app quits. The Active sessions panel shows it as "web view · svc/grafana:3000" with a stop button (stopping closes the tab)
- [ ] Idle timeout option for background tabs (e.g. stop forwards of tabs hidden for more than 30 min, restart on focus)

### Where the button appears
- [ ] Service details and the Services table: one button per port that looks like HTTP. Detection order: `appProtocol` (`http`, `https`, `kubernetes.io/h2c`), port name (`http`, `https`, `web`, `ui`, `metrics`, `admin`, `http-*`), well-known port numbers (80, 443, 8080, 8443, 3000, 9090, 9093, 15672…), or "Open as web view…" on any port
- [ ] Pod details: container ports (same detection), for pods without a Service
- [ ] Ingress details: open the backend service through a forward (even if the ingress host isn't reachable from here)
- [ ] Palette action `> Open web view…` (pick service and port), and a key in list views (`w`, added to the k9s hint bar)
- [ ] Remember per service port: scheme, start path (e.g. `/graph`, `/grafana/`), last URL. Built-in presets for common apps (Grafana, Prometheus, Alertmanager, Argo CD, RabbitMQ, Kibana, Kafka UI, Jaeger)

### The web-view tab
- [ ] Tab title: page title plus service, and an icon showing the cluster color (PROD badge on production clusters)
- [ ] Toolbar: back, forward, reload, address bar (path editable, host locked to the forward), copy URL, open in the external browser (the forward is kept alive while that tab stays open), zoom, dev tools (debug builds, or a setting)
- [ ] HTTPS services with self-signed certificates: show an interstitial with the certificate details. "Proceed" is remembered per (cluster, service, port) only
- [ ] Storage isolation: a separate web data directory per (cluster, namespace, service), so cookies and logins for different clusters never mix. "Clear site data" action. Private mode option (nothing persisted)
- [ ] Rewrite `Host`/`Origin` only if an app needs it (per-preset option). Handle apps that redirect to their external URL (show "This app redirected to https://grafana.example.com — open externally / stay").
- [ ] Downloads go through Kubyl's download prompt. File uploads use the native picker
- [ ] Keyboard: web content gets focus inside the tab; Kubyl's global shortcuts (⌘K, ⌘W, tab switching) still work

### Safety
- [ ] Forwards only bind to loopback. Nothing is exposed on the network
- [ ] Read-only clusters: web views are still allowed (they're a read path), but show the read-only badge in the toolbar, because the app inside can change things
- [ ] No Kubyl credentials or tokens are ever injected into web content

## Acceptance criteria

- On kind with kube-prometheus-stack: Grafana and Prometheus open in tabs from the Services list with one click. Closing the tab stops the forward (it disappears from Active sessions, and the local port is free).
- Two tabs on the same service share one forward. Closing one keeps the other working.
- Killing the backing pod keeps the web view working after a short reconnect.
- Logins to Grafana on two different clusters don't share cookies.
- Works on macOS and Windows embedded. Linux works embedded on X11, and via the documented fallback on Wayland.

## Risks

- Embedding a native webview inside GPUI isn't officially supported by GPUI. Keep it isolated in `kubyl_webview` behind a trait, so the separate-window or system-browser fallbacks stay possible.
- Wayland will probably need the separate-window or system-browser fallback.
- WebView2 must be present on Windows 10 (bundle the bootstrapper in phase 10).

## Handoff log
