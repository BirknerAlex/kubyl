# Phase 01: Cluster connectivity (kubeconfigs, auth, discovery)

**Status:** done (2026-09-24; EKS/GKE exec verified only with a stand-in plugin, see Handoff log)
**Depends on:** 00
**Owns:** `crates/kubyl_kube` (plus the Clusters settings view inside it)
**Mockups:** board 5 · Kubeconfigs, contexts, OIDC sign-in

## Goal

Load any number of kubeconfig files, merge their contexts, authenticate with every common method
(including OIDC in the browser), and keep a live, discovered connection per cluster. Several
clusters can be connected at once.

## Tasks

### Kubeconfig sources
- [x] Source types: default `~/.kube/config`, every file in `$KUBECONFIG` (merged), and user-added files and folders. Stored in `settings.json` as paths; kubeconfig files are never copied or modified
- [x] Add a source via file picker, drag and drop of files onto the window, or "Paste YAML" (saved to `config_dir/kubyl/kubeconfigs/<name>.yaml` with permissions 0600)
- [x] Hot reload with `notify`: context list updates without a restart. Parse errors show on the source row
- [x] Merge rules: contexts from all sources in one list. A name collision gets a `@<file-stem>` suffix. Each context remembers its source file (shown in the UI and in Favorites)
- [x] Per-context overrides stored in Kubyl settings, never in the kubeconfig: display name, color tag, default namespace, **production** flag, **read-only** flag, hidden

### Authentication
- [x] Client certificates, bearer tokens, token files, basic auth (legacy)
- [x] Exec plugins (`aws eks get-token`, `gke-gcloud-auth-plugin`, `kubelogin`, `az`…): run off the UI thread, cache `ExecCredential` until expiry, surface stderr on failure. Interactive exec plugins (`interactiveMode: Always/IfAvailable`) get a terminal prompt modal
- [x] Inherit the user's login-shell `PATH` on macOS and Linux, so exec plugins are found when the app is launched from Finder or a desktop entry
- [x] **OIDC** (`auth-provider: oidc` and kubelogin-style exec configs): auth code + PKCE with a loopback redirect (`127.0.0.1:<port>/callback`), device-code fallback, refresh-token rotation. Tokens live in the OS keychain via `keyring`
- [x] Sign-in modal as in the mockup: issuer, client id, scopes, a "waiting for browser" state, reopen browser, device code with copy button, cancel
- [x] Proxy support: `proxy-url`, `HTTPS_PROXY`/`NO_PROXY`. Custom CA, `tls-server-name`, `insecure-skip-tls-verify` (with a warning badge)

### Connection manager
- [x] `ConnectionManager` entity: per-context state machine `Disconnected → Connecting → Connected{latency, version} | AuthRequired | Unreachable(err) | Forbidden`
- [x] Lazy connect when a cluster root is expanded or a favorite is opened. Background health ping (`/version`, `/readyz`) with backoff
- [x] Client pool: one `kube::Client` per context, rebuilt when the credential or kubeconfig changes
- [x] Namespace list per cluster (watch). If listing namespaces is forbidden, fall back to the kubeconfig namespace or user-entered names
- [x] `SelfSubjectAccessReview` / `SelfSubjectRulesReview` helper (`can_i(verb, gvr, ns)`), cached, so UI actions can hide or disable themselves (RBAC-aware UI)

### Discovery
- [x] Aggregated discovery (`/apis` with `apidiscovery.k8s.io/v2`), falling back to legacy discovery. Result: every GVK/GVR, scope, verbs, short names, categories, preferred version
- [x] CRD awareness: watch `CustomResourceDefinitions` and re-run discovery when they change (new CRDs appear in the sidebar live)
- [x] OpenAPI v3 fetcher (`/openapi/v3`) with an on-disk cache keyed by the server's ETag. Used by phases 04 and 08
- [x] Cluster capabilities probe (`ClusterCaps`): version, distribution guess (EKS/GKE/AKS/OpenShift/k3s/kind), metrics-server present, Prometheus candidates (phase 07 finishes this), OLM present, Gateway API present

### Clusters and kubeconfigs view
- [x] Left: list of sources (path, number of contexts, watched, status icon), drop zone, add/paste/browse
- [x] Right: contexts table (color, name, API server, auth method, status plus latency), safety settings and connection details card
- [x] Title-bar cluster switcher lists contexts grouped by source, with status dots

## Acceptance criteria

- Two kubeconfig files with overlapping context names both load. Editing one file on disk updates the UI.
- Connects to kind (client cert), EKS (exec), GKE (exec) and an OIDC-protected cluster (Dex or
  Keycloak in `script/oidc-dev.sh`), including refresh after the token expires.
- Discovery lists built-in kinds and CRDs. Installing a CRD makes it appear without a restart.
- No token or refresh token is in logs, `settings.json` or `state.json`.

## Risks and open questions

- The `kube` crate's `oidc`/`oauth` features cover refresh but not the interactive browser flow. We build that part ourselves.
- On Windows, exec plugins may need `.cmd` shims. Test `aws` and `gcloud` there.
- Keychain prompts on macOS for unsigned dev builds are a nuisance. Allow an env override to use a file store in dev.

## Handoff log

### 2026-09-25: managing sources (branch `phase/01-kubeconfig-sources`)

Gaps found in use: the Explorer's "+" (and its search button) said "not available in this build"
because phase 00's `dispatch_or_explain` only saw element action handlers, not the global ones
`kubyl_kube` and `kubyl_explorer` register; and sources could hardly be removed.
- The Explorer "+" is a menu: add a kubeconfig file or folder, paste YAML, manage kubeconfigs.
  Both header buttons dispatch directly; `dispatch_or_explain` is gone (every action exists now).
- Sources list: every row has a visible button. User-added files and folders are removed from
  Kubyl (the file stays); `~/.kube/config` and `$KUBECONFIG` stop loading; each pasted
  kubeconfig is its own row and can be deleted (the only files Kubyl deletes, after a
  confirmation). Toggles "Load ~/.kube/config" and "Load $KUBECONFIG" turn them back on.
- `ConnectionManager::set_load_default_kubeconfig`, `set_load_kubeconfig_env` and
  `delete_pasted` (refuses paths outside the pasted folder), with a test.
- Editing kubeconfigs is planned as phase 13 (`plans/13-kubeconfig-editor.md`).

### 2026-09-24: phase 01 implemented

**Auth methods verified.**
- Client certificates: live against kind (`script/dev-cluster.sh`), in the app and in
  `crates/kubyl_kube/tests/live.rs` (`kind_*`, ignored by default; see its header for how to run).
- OIDC: live against Dex + a kind cluster with `--oidc-*` flags (`script/oidc-dev.sh`), kubelogin
  exec config: browser sign-in (auth code + PKCE, loopback on `localhost:8000`), then the
  2-minute ID token expires and the refresh token renews it (`oidc_signs_in_and_refreshes`).
  The browser leg was driven by `curl` against Dex's login form (the built-in browser rejects the
  dev CA); the loopback server, code exchange, ID token verification and refresh are the real
  code paths.
- Exec plugins: unit-tested with shell-script plugins (token, client certificate, failure with
  stderr, not found + `installHint`, caching across contexts). **Not verified against real EKS,
  GKE or AKS** (no accounts in this session); do that before release.
- Bearer token, token file, basic auth, `auth-provider: gcp`: left to kube itself (we only strip
  exec/OIDC auth), detected and labelled, not tested live.
- Device code: implemented (RFC 8628, Dex supports it), unit-tested only.

**Decisions.**
- Kubyl runs exec plugins and OIDC itself instead of kube's built-in support: stderr capture,
  caching shared by identical exec configs, login-shell `PATH`, sign-in UI, keychain storage. A
  tower `AsyncFilterLayer` (`auth::AuthLayer`) adds `Authorization` per request; kube's boxed
  service isn't `Clone`, so a `tower::buffer` sits in between. Exec plugins that return client
  certificates are run before building the client and the client is rebuilt before they expire.
- OIDC redirect URI is `http://localhost:<port>` (8000, then 18000; kubelogin's
  `--listen-address` overrides), not `127.0.0.1:<port>/callback` as the plan said: this way IdP
  clients already registered for kubelogin work unchanged. The loopback server listens on
  127.0.0.1. Tokens are keyed by issuer + client id (contexts sharing an IdP client share a
  session). `offline_access` is requested only when the IdP advertises it (Google rejects it).
- `ClusterId` is `<context>@<kubeconfig path>` (stable across runs, unique across files).
  Per-context overrides live in settings.json under `kubernetes.contexts.<id>`; the active
  cluster is remembered in state.json (`kube.active`), never credentials.
- Health: `GET /version` every `kubernetes.health_check_interval` s (default 30). It also
  exercises credentials (an invalid token gets 401 even on public endpoints). Failures back off
  5 s → 60 s and reconnect. Connect = build client, `/version` (latency) + `SelfSubjectReview`
  (auth check and user name; 404 tolerated on old servers).
- kube needs a crypto backend for rustls: `ring` (builds everywhere without cmake/nasm). kube 4.2
  pins serde-saphyr 0.0.29, which needs `smallvec < 1.16`; the workspace pin moved to 1.15.2.
- `cargo deny` ignores RUSTSEC-2023-0071 (`rsa` via openidconnect, only public-key verification).
- Credential store: OS keychain via `keyring` 4 (v1 API). `KUBYL_CREDENTIAL_STORE=file` uses
  `<config dir>/dev-credentials.json` (0600, plain text, dev only; logs a warning),
  `=memory` keeps secrets for the run (tests, screenshots).

**API for phase 02** (details in `crates/kubyl_kube/src/lib.rs` docs):
`ConnectionManager::global(cx)` → `contexts()` (visible roots with `id`, `name`, `file`,
`source`, `server`, `auth`), `display_name/color/context_settings(id)`, `state(id)`,
`ensure_connected(id)` (expand a root / open a favorite), `activate(id)` (title bar),
`client(id)`, `namespaces(id)` (`listed: false` = forbidden, fallbacks from kubeconfig + the
`namespaces` setting), `discovery(id)` (`Arc<Discovery>`: `preferred()`, `resolve("po")`,
`by_gvk`, verbs, short names, categories), `caps(id)`, `can_i(id, AccessQuery, cx)` /
`cached_can_i`, `openapi_spec(id, OpenApiIndex::key(group, version), cx)`. Subscribe to
`ConnectionEvent` (`ContextsChanged`, `StateChanged`, `NamespacesChanged`, `DiscoveryChanged` —
also after CRDs change — `ActiveChanged`, `SignInRequested`). `ActiveContext` is set by
`activate`; phase 02's namespace switcher may overwrite `namespace` (the manager preserves it).
The Clusters tab is `ViewKind::Custom("clusters")` (`kubyl_kube::ui::clusters_view_kind()`).

**Stubs / not done.**
- Drag and drop works on the Clusters tab (whole tab is a drop target), not the entire window:
  that needs a drop handler in the workspace root (`kubyl` crate). Small follow-up for whoever
  touches the shell next.
- The title-bar cluster icon uses the accent color, not the cluster's color tag (title bar is
  `kubyl_ui`); `ClusterBadge.color` is filled, so it's a one-line change there.
- Interactive exec plugins (`interactiveMode: Always`) get a prompt modal with their stderr and a
  stdin line input, not a real PTY; plugins that check `isatty` behave as non-interactive.
  `IfAvailable` runs non-interactively.
- `NO_PROXY` supports hosts, domain suffixes, `*` and IPv4 CIDRs (no IPv6 CIDRs).
- Prometheus candidates are only named (`monitoring.coreos.com`, OpenShift thanos-querier);
  phase 07 probes them. Gateway API presence is in `ClusterInfo.gateway_api` (not in
  `ClusterCaps`, which lives in `kubyl_core`).

**Gotchas.**
- Windows: exec plugins installed as `.cmd`/`.bat` shims (gcloud, az) are resolved from `PATH`
  and run through `cmd /C`; plugins run with `CREATE_NO_WINDOW`. Only compiled and unit-tested
  in CI; not run against a real cluster on Windows. Credential Manager caps entries at ~2.5 KB:
  large ID tokens aren't persisted there (the refresh token is; the next start refreshes).
- Linux: the keychain is the Secret Service over D-Bus (zbus); without a running secret service
  the store fails and a warning is logged — sign-in still works for the session.
- macOS: unsigned dev builds trigger keychain prompts on every rebuild; use
  `KUBYL_CREDENTIAL_STORE=file` (or `memory`) in development. Apps started from Finder get the
  login shell's `PATH` (`$SHELL -l -c`, 5 s timeout, run once in the background).
- `script/oidc-dev.sh` uses `dex.127.0.0.1.nip.io` (public wildcard DNS → 127.0.0.1) as the
  issuer host; set `DEX_ISSUER_HOST` if your DNS blocks answers pointing at 127.0.0.1. Inside the
  kind node an /etc/hosts entry points it at the Dex container and the API server is restarted
  once so its pod copies the entry.
- GPUI tests: the manager's file watcher is off in tests (`ConnectionManager::install(.., false)`),
  and tests never connect (network would wake GPUI tasks from Tokio threads).

**Screenshot.** `KUBYL_SCREENSHOT=out.png` with a sandbox `HOME`/`KUBYL_CONFIG_DIR`
(state.json opening `{"kind":{"custom":"clusters"}}` and `kube.active` set) renders the tab; the
PR shows it connected to kind.
