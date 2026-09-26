# Phase 14: Alerts (Alertmanager), silences, alerting rules

**Status:** in progress (branch `phase/14-15-alerts-polish`, together with phase 15)
**Depends on:** 02 (explorer, details sections, `ResourceStores`), 05 (temporary port-forwards), 07 (Prometheus discovery and client, OpenShift Route auth); 08 optional (Alertmanager and Prometheus UIs in a web view)
**Owns:** `crates/kubyl_alerts` (new), `script/alertmanager-dev.sh`
**Mockups:** board 16 · Alerts, to be added to `design/mockups/generate.py` before the UI work: the Alerts tab with the details pane, the all-clear and "no Alertmanager" states, Silences with the silence editor and its PROD confirmation, the Rules tab, the sidebar row badge and the status bar item.

## Goal

Every cluster gets an **Alerts** entry: what is firing right now, since when, how severe, what it
is about, which object it concerns and who was notified, and a clear "all clear" when nothing is.
Silence and acknowledge alerts without leaving Kubyl. Charts stay in phase 07; this phase is about
lists, states and times.

## What phase 07 already does (reuse it)

- Prometheus is found by ranking Services (`kubyl_metrics::discover`) and probing them through the
  API server's service proxy with the cluster's own client
  (`/api/v1/namespaces/{ns}/services/{scheme}:{svc}:{port}/proxy/…`). This works with every kind
  of cluster auth (token, client certificate, exec, OIDC, OpenShift OAuth): the API server
  authenticates the user and forwards the request. It needs `get services/proxy`.
- The API server strips credentials from proxied requests. OpenShift's monitoring Services sit
  behind kube-rbac-proxy, so the proxy gets a 401. `kubyl_metrics::openshift::through_route` then
  calls the Service's admitted Route with the user's own token (`ConnectionManager::bearer_token`).
  Client-certificate users have no token; for them it mints a short-lived TokenRequest token of
  `openshift-monitoring/prometheus-k8s`. TLS is checked against the OS roots, then against the
  ingress CA (`openshift-config-managed/default-ingress-cert`). The Route fallback only runs for
  `openshift-monitoring` or a Service named in settings.
- External URLs: `url` in settings, the Authorization header in the keychain
  (`metrics-auth:<cluster id>`), credentials only over HTTPS or to loopback.
- `MetricsService` is a demand-driven cache: reads mark data as wanted, and a 1 s tick refreshes
  whatever is stale. The alerts service works the same way.
- Prometheus discovery already skips `alertmanager` Services (`NOT_AN_API`), so it needs no change.

## Decisions to make first

Each has a recommendation. Record the outcomes in the README's decision table.

1. **Crate.** Recommended: a new `kubyl_alerts` crate owned by this phase, built on
   `kubyl_metrics`' transport (a small refactor there, see "Shared-crate commits"). Putting it
   inside `kubyl_metrics` would blur crate ownership.
2. **Sources.** Recommended: Alertmanager is the main source (firing, silenced and inhibited
   alerts, receivers, silences). The Prometheus rules API (`/api/v1/alerts`, `/api/v1/rules`, on
   the Prometheus phase 07 found) adds pending alerts, rule definitions and rule health. Either one
   alone still works and shows what it can: Prometheus only means firing and pending alerts
   without silences; Alertmanager only means no pending alerts and no rules.
3. **Writes.** Recommended: silences are in scope (create, edit, extend, expire, acknowledge),
   because silencing is the one thing people do with an alert. They are hidden on read-only
   clusters and confirmed on PROD. The alternative is a read-only first version.
4. **Where tokens may go.** Recommended: phase 07's rule, extended. The user's kube token, or a
   service-account token Kubyl minted, goes only to Services in `openshift-monitoring` or
   `openshift-user-workload-monitoring` (only cluster admins can create `openshift-*` namespaces)
   or to Services named in settings. It only goes over HTTPS with verified TLS, and never to a
   Service Kubyl merely discovered somewhere else.
5. **Polling.** Alertmanager has no watch API. Recommended: poll a cluster only while something
   shows its alerts: every 15 s for open views, every 60 s for badges, the status bar and
   notifications. Only connected clusters are polled, and polling slows to the background interval
   while no Kubyl window is active.

## Sources and auth

### Finding Alertmanager

In this order. Every candidate is probed with `GET <prefix>/api/v2/status`.

1. Settings: `alerts.clusters.<cluster id or context>.alertmanagers` (Services or URLs, see
   Settings). With entries there, nothing is discovered.
2. prometheus-operator objects: the `spec.alerting.alertmanagers` of the `Prometheus` CR behind
   the Prometheus phase 07 found (namespace, Service, port, `pathPrefix`, scheme), and
   `monitoring.coreos.com/v1` `Alertmanager` objects. For those, use the Service that selects
   `alertmanager: <name>`, else `alertmanager-operated` in the same namespace. The API prefix is
   `spec.routePrefix`, else the path of `spec.externalUrl`.
3. Prometheus's own list: `/api/v1/alertmanagers` (Prometheus only, Thanos doesn't serve it)
   returns pod URLs like `http://10.244.1.7:9093/api/v2/alerts`. Map the IP and port to a Service
   through EndpointSlices, and take the path prefix from the URL.
4. Service names and labels, from the Service list phase 07 already fetches (cluster-wide, or
   the well-known namespaces):
   - `alertmanager-main`: kube-prometheus; on OpenShift, port `web` 9094 over HTTPS.
   - `alertmanager-user-workload`: OpenShift user workload monitoring, port `web` 9095 over HTTPS.
   - `*-kube-prometheus-stack-alertmanager` and `*-kube-prom-alertmanager`: port `http-web` 9093.
   - `alertmanager-operated`: headless, port `web` 9093.
   - The community chart's `<release>-alertmanager`, and a plain `alertmanager`.
   - VictoriaMetrics `vmalertmanager-*`, and GKE Managed Prometheus `gmp-system/alertmanager`.
   - Services labelled `app.kubernetes.io/name=alertmanager` or `app=alertmanager`.
- Services that select the same pods count as one Alertmanager (prefer the ClusterIP Service over
  `-operated`).
- From `/api/v2/status`, keep only `versionInfo`, `uptime` and `cluster` (status and peers).
  Discard `config.original` right away, since it can contain receiver URLs and credentials.
- A cluster can have several Alertmanagers (the OpenShift platform one plus the user workload one,
  or a team's own). Show all of them merged, with a SOURCE column when there is more than one.
- Supported: Alertmanager API v2, 0.22 or newer (for `isEqual` matchers; v1 was removed in 0.27).
  Mimir and Cortex (`/alertmanager/api/v2`, which needs `X-Scope-OrgID`) only through settings
  (`path`, `tenant`).
- Look again when phase 07 would: after 3 consecutive failures, every 5 min while nothing was
  found, on `DiscoveryChanged`, and from the palette.

### Prometheus side (optional)

- New accessor `MetricsService::prometheus(cluster)` returns the client phase 07 found (service
  proxy, URL or Route), with the same auth. Nothing new to configure.
- `/api/v1/alerts`: firing and pending alerts, with `activeAt` (pending since) and `value`.
- `/api/v1/rules?type=alert&exclude_alerts=true` (older servers ignore the parameter): rule groups,
  `query`, `duration` (the `for`), `keepFiringFor`, labels, annotations, `health`, `lastError`,
  `lastEvaluation`, `evaluationTime` and state. OpenShift's thanos-querier serves both on its `web`
  port (`cluster-monitoring-view`). vmselect proxies them to vmalert when it is configured to.
- Range queries over `ALERTS` and `ALERTS_FOR_STATE`: the 24 h timeline in the details.

### Transports and auth

| Setup | How Kubyl reaches it | What authenticates |
|---|---|---|
| Plain Alertmanager Service (kube-prometheus-stack, community chart, GMP, VictoriaMetrics, Rancher monitoring) | The API server's service proxy with the cluster client. Reading needs `get services/proxy` in its namespace; silences need `create` (POST) and `delete` on `services/proxy` | The kubeconfig's credentials, checked by the API server (token, client certificate, exec, OIDC). Nothing reaches Alertmanager |
| OpenShift platform: `openshift-monitoring/alertmanager-main`. kube-rbac-proxy on `web` 9094 runs a SubjectAccessReview on `monitoring.coreos.com` `alertmanagers/api`, name `main`, with the verb taken from the HTTP method. Roles: `monitoring-alertmanager-view` (read) and `monitoring-alertmanager-edit` in `openshift-monitoring` | The proxy answers 401, so Kubyl uses the admitted Route `alertmanager-main` (path `/api`, reencrypt) through phase 07's `through_route`. TLS against the OS roots, then the ingress CA | The user's token (`oc login`, OIDC, exec). Client-certificate users (such as the installer's `system:admin` kubeconfig) get a TokenRequest token of `openshift-monitoring/prometheus-k8s`, which is bound to `monitoring-alertmanager-edit` (RoleBinding `alertmanager-prometheusk8s` in CMO's assets) |
| OpenShift user workload: `openshift-user-workload-monitoring/alertmanager-user-workload` on `web` 9095. Roles: `monitoring-alertmanager-api-reader` and `-api-writer` in that namespace | It has no Route by default, so Kubyl opens a temporary loopback port-forward (`ForwardSpec::ephemeral`) to the `web` port. TLS is verified against the service CA (`openshift-service-ca.crt` ConfigMap) with the server name `alertmanager-user-workload.openshift-user-workload-monitoring.svc` (`kube::Config::tls_server_name`) | The user's token, else a TokenRequest token of the service account named in settings (no default until one is verified on a real cluster) |
| Any Alertmanager behind an auth proxy, named in settings | The same fallbacks in order: service proxy, then the Route (OpenShift), then a temporary forward | The user's token, or the settings' `service_account` |
| External URL (a central Alertmanager) | Direct HTTPS, with an optional CA file and client certificate and key files (mTLS) | The Authorization header from the keychain (`alerts-auth:<cluster id>/<url>`), plus `X-Scope-OrgID` when `tenant` is set. The header isn't sent when `insecure_skip_tls_verify` is on (use `ca_file`) |

- On a 403, the message names what is missing: "needs `get services/proxy` in `monitoring`",
  "needs the `monitoring-alertmanager-view` role in `openshift-monitoring` (`-edit` for
  silences)", "needs `create pods/portforward`".
- Reads may fall back to the service-account token without asking, as in phase 07. **Writes** with
  a service-account token need explicit consent in the silence dialog ("You sign in with a client
  certificate, so this silence is created as service account openshift-monitoring/prometheus-k8s").
  `createdBy` stays the user's name either way.
- Expired tokens: a 401 on the Route or forward retries once with a refreshed token. Otherwise the
  view shows "Sign in again", which starts phase 01's sign-in. Polling never opens a dialog by
  itself.
- OpenShift users with project-level access only: the per-project tenancy ports (Alertmanager
  9092, thanos-querier `tenancy-rules` 9093) are reachable only inside the cluster (the console
  calls them from its backend). Kubyl could reach them only through a port-forward into
  `openshift-monitoring`, which these users can't create. They see "Your account can't read this
  cluster's alerts" and the roles to ask for.
- HA Alertmanagers: through the proxy, consecutive polls can hit different replicas, and silences
  take a few seconds to spread by gossip. After a write, show the new state right away and poll
  again after 2 s.

## Tasks

### Shared-crate commits (each lands on its own, first)
- [x] `kubyl_metrics`: move the transport out of `PromClient` into a public module: service-proxy base path, external URL with header, direct client with bearer, roots and `tls_server_name`, plus `get`/`post`/`delete` with the existing error mapping and `credentials_allowed`. Make `openshift::through_route` take the probe path, and add `PromClient::api(path, params)` and `MetricsService::prometheus(cluster)`. Phase 07's tests keep passing unchanged
- [x] `kubyl_explorer`: top-level rows that other crates add to the cluster tree (`catalog::register_view_row(cx, ViewRow { id, after, label, icon, kind, visible, badge })`), with a count badge colored by tone, following `explorer.group_order` and `hidden_groups`. Also a marker on a cluster's root row (a red dot while something critical fires, visible when the cluster is collapsed) and, optionally, a badge on favorite namespace rows
- [x] `kubyl_core` + `kubyl_overview`: an `OverviewSection` extension point (`ChromeRegistry::add_overview_section`, built like `DetailsSection`), rendered below the KPI tiles in the cluster and namespace variants
- [x] Stub crate `kubyl_alerts` (workspace member and its `init` line in `crates/kubyl/src/main.rs`), then mockup board 16

### Sources (`kubyl_alerts::discover`, `alertmanager`, `rules`)
- [x] Discovery as described above, with ranking tests on real Service and CR fixtures: kube-prometheus-stack, OpenShift platform and user workload, the community chart, VictoriaMetrics, GMP, a `routePrefix`
- [x] Alertmanager v2 client: `GET /api/v2/alerts` (active, silenced, inhibited and unprocessed alerts; `filter` for the cluster's matchers), `/api/v2/silences`, `/api/v2/status`, `/api/v2/receivers`; `POST /api/v2/silences`; `DELETE /api/v2/silence/{id}`. Parsers accept unknown fields and states
- [x] Parsers for Prometheus `/api/v1/alerts`, `/api/v1/rules` and `/api/v1/alertmanagers`, with Prometheus 3.x and Thanos fixtures
- [x] Transport fallbacks and the token rule, with a test that a look-alike Service outside the trusted namespaces never receives a token (like phase 07's)
- [x] A clear message for every failure (see the 403 examples above)

### Model and service (`kubyl_alerts::model`, `service`)
- [x] `Alert`: fingerprint, alert name, labels, annotations, severity, state (firing, pending, silenced by, inhibited by, unprocessed), firing since (Alertmanager's `startsAt`), pending since (Prometheus's `activeAt`), last received (`updatedAt`), receivers, generator URL, value, source, rule and target object
- [x] Merge: a Prometheus alert matches the Alertmanager alert with the same name whose labels include all of the Prometheus alert's labels (Prometheus adds external labels when it sends). Without Alertmanager, "firing since" is `activeAt` plus the rule's `for`, marked as approximate
- [x] Severity from `alerts.severity_label` (default `severity`), mapped to critical, warning, info or none. `alerts.severities` maps custom values such as `page`, `P1` or `high`; unknown values sort after info
- [x] Target object from labels, most specific first: `exported_namespace`/`exported_pod` when present, then `pod` (with `container`), `deployment`, `statefulset`, `daemonset`, `replicaset`, `job_name` (not `job`, which is the scrape job), `cronjob`, `horizontalpodautoscaler`, `persistentvolumeclaim`, `node`, `instance` when it matches a node name or `<InternalIP>:<port>`, the OpenShift ClusterOperator (`name` on `ClusterOperator*` alerts), `service` only for alerts not derived from kube-state-metrics, and finally `namespace`. Tested against the kube-prometheus-stack and OpenShift rule sets
- [x] Heartbeat: `Watchdog` (`alerts.heartbeat_alerts`) doesn't count as a problem. It is the pipeline check instead: received within the last 5 min means OK. If Prometheus has the rule but Alertmanager doesn't have the alert: "Prometheus isn't delivering alerts to this Alertmanager" (and when `/api/v1/alertmanagers` is empty: "Prometheus has no Alertmanager configured"). `InfoInhibitor` is hidden by default (`alerts.hidden_alerts`)
- [x] `AlertsService` (a global entity), per cluster: sources, alerts, silences, rules, status and errors. Demand-driven like `MetricsService`: demand from views refreshes every `refresh_interval`, background demand every `background_refresh_interval`, rules every 2 min. It drops clusters nobody asked about, resets on disconnect and settings changes, and uses a generation counter to drop stale results
- [x] Transitions (started firing, resolved) are kept in memory per cluster while Kubyl watches it. Resolved alerts show as "resolved 4 min ago" for a while (Alertmanager's API doesn't return resolved alerts), and notifications know what is new. The first fetch is the baseline
- [x] Performance: 5,000 alerts are parsed off the UI thread; the view diffs by fingerprint and keeps the selection

### Alerts view (`ViewKind::Custom("alerts")`, one tab per cluster, cluster color as the tab dot)
- [x] Header: cluster, PROD badge, source chips (`Alertmanager monitoring/alertmanager-operated · v0.28 · 2/2 peers`, `Rules: Prometheus`) and "Open Alertmanager UI" (a phase 08 web view on the Service; for URL and Route sources the URL opens in the browser instead)
- [x] Summary line: `3 critical · 7 warning · 2 info firing · 4 pending · 5 silenced`, the heartbeat state, and rule health (`2 rules fail to evaluate`)
- [x] All clear: "No alerts firing", heartbeat OK with its time, the number of rules, the time of the last check. Every other state explains itself and never shows an empty list without a reason: not connected, loading, no Alertmanager found (what was tried, the best candidate's error, "Set Alertmanager…" and the settings key), sign-in needed, 403 with the missing role, stale data (time of the last success)
- [x] Filters: a text and matcher input (`alertname=~"Kube.*", namespace="payments"`, same parser as silences), severity chips with counts, state chips (Firing, Pending, Silenced, Inhibited), namespace scope (all namespaces, or the active one; alerts without a namespace go under "Cluster"), receiver. Silenced and inhibited alerts are hidden by default behind a "5 silenced" row
- [x] Group by alert name (default), namespace, severity, receiver, target, or not at all. Collapsible group rows: `KubePodCrashLooping ×4 · critical · oldest 2h 14m`
- [x] Columns: severity pill, alert, state, since (relative and live; local time and UTC on hover), summary (`summary`, else `message`, else `description`), target (a link), namespace, receivers, and the remaining labels as muted chips. Sorted by severity, then age
- [x] Keys, registered in the `ActionRegistry` and shown in the key-hint bar: `enter` details, `s` silence, `a` acknowledge, `o` go to the target, `l` logs of the target pod, `r` runbook, `y` copy the labels as matchers, `/` filter
- [x] View options (grouping, filters, "show silenced") are kept in `state.json`, alert data never is

### Alert details (a pane in the Alerts view)
- [x] Name, severity, state, firing or pending since, resolved at, duration, last received
- [x] Summary and description as plain text (no HTML or Markdown rendering), long text folded
- [x] Labels as chips (click adds a filter, alt-click excludes) and an annotations table
- [x] Target: a link to the object (details, logs for pods), checked against the live object; shows "not found" once the object is gone
- [x] Runbook (`runbook_url`) and generator URL: `http`/`https` only, full URL shown, opened in the system browser. "Open in Prometheus" opens a phase 08 web view on the Prometheus Service with the generator URL's path and query, when Prometheus is a Service
- [x] Routing: receivers; silenced by (a link to the silence, with its comment, creator and end); inhibited by (a link to the inhibiting alert)
- [x] Rule (with a rules source): group, expression (monospace, copyable), `for`, `keep_firing_for`, health, last error, last evaluation. "Edit rule" opens the `PrometheusRule` that defines it in the YAML editor (matched by group and alert name over a `ResourceStores` list of `monitoring.coreos.com/v1` `PrometheusRule`)
- [x] Timeline (with Prometheus): the last 24 h of `ALERTS{…}` as firing and pending segments; flapping alerts say "fired 6 times in 24 h"
- [x] Actions: Silence…, Acknowledge, Copy labels, Copy as an `amtool` filter

### Silences
- [x] Silences tab: active, pending and expired silences (expired collapsed, last 24 h by default). Columns: state, matchers (chips), comment, created by, start, end (`ends in 3h 12m`), alerts it matches now. Filter by matcher text
- [x] Editor (dialog): matchers with name and value completion from current alerts, operators `=` `!=` `=~` `!~`. From an alert, it starts with all of the alert's labels, with the alert name, namespace and target labels ticked and the rest unticked. Durations 1h, 2h, 4h, 1d, 1w or a custom end; a comment is required; "created by" is the user from `SelfSubjectReview` (else the kubeconfig user) and can be edited. Live preview: "matches 4 alerts: 1 critical, 3 warning", computed locally with regexes anchored like Alertmanager's (RE2 syntax through `regex`). Validation: at least one matcher that doesn't match the empty string (Alertmanager rejects anything else), valid regexes, end after start
- [x] Central Alertmanager: the cluster's `matchers` from settings are always added and can't be removed in the editor, so a silence never covers another cluster's alerts
- [x] Acknowledge: a silence of exactly this alert's labels for `alerts.ack_duration` (default 1 h), with the comment "Acknowledged in Kubyl by <user>"
- [x] Edit (POST with the id; say that Alertmanager replaces the silence with a new id when its matchers or start change), extend (+1h, +4h), expire (confirmed), recreate an expired silence, copy as `amtool silence add …`
- [x] Guards: no silence actions on read-only clusters. Every create and edit shows a summary first (matchers, matched alerts by severity, duration). On PROD the summary needs the typed cluster name when the silence matches a critical alert, matches more than 10 alerts, or has no `alertname` matcher. Writes with a service-account token ask for consent (see "Transports and auth")
- [x] After a write: the list updates right away and refetches; a toast offers "Undo" (expire the new silence, or recreate the expired one)

### Rules (tab, with a Prometheus rules source)
- [x] Alerting rules by group and file: state (inactive shown as OK, pending, firing), health, last error, last evaluation, evaluation time, `for`, expression. Filter "only firing, pending or failing". Link to the `PrometheusRule` object
- [x] Header line: `312 rules · 4 firing · 2 pending · 1 failing`

### Across Kubyl
- [x] Sidebar: an "Alerts" row under each cluster, after Overview (the new explorer hook). The badge is the number of firing alerts in the color of the most severe one (red for critical, yellow for warning, dim for info), a check when all is clear, nothing while unknown. The row shows only when the cluster has an alert source (`alerts.sidebar`: `auto`, `always`, `never`). A red dot marks the cluster's root row while a critical alert fires
- [x] Status bar: a bell with the count for the active cluster, in the same colors; the tooltip lists the top three alerts; clicking opens the Alerts view
- [x] Details section "Alerts" (`DetailsSection`) for pods, workloads, nodes, namespaces, PVCs and Services: the object's firing and pending alerts (severity, name, since, summary); clicking one opens it in the Alerts view. Workloads include the alerts of their pods. Renders nothing when there are none
- [x] Action "Show Alerts for Selection" in resource lists: the Alerts view filtered to that object
- [x] Overview card (`OverviewSection`): firing alerts by severity, the five most severe, the heartbeat, and the all-clear state. The namespace variant shows that namespace's alerts
- [x] Notifications (opt-in, `alerts.notify`): a toast per cluster when alerts of at least `min_severity` start firing, for the chosen clusters (active, favorites' clusters, production, all), batched per 60 s, with "Show". Optionally when they resolve. Never for alerts that were already firing when Kubyl first looked
- [x] All clusters: "Alerts: Show Alerts in All Clusters" opens the same table with a CLUSTER column over every connected cluster that has a source. Clusters without one, or with errors, are listed at the bottom
- [x] Palette: "Alerts: Show Alerts", "Alerts: Show Alerts in All Clusters", "Alerts: Show Silences", "Alerts: New Silence…", "Alerts: Look for Alertmanager Again", "Alerts: Set Alertmanager Authorization Header…", "Alerts: Clear Alertmanager Authorization Header"

### Settings (`"alerts"` section of settings.json)

```jsonc
"alerts": {
  "enabled": true,
  "discover": true,
  "refresh_interval": 15,             // seconds, while a view shows alerts
  "background_refresh_interval": 60,  // badges, status bar, notifications
  "sidebar": "auto",                  // auto | always | never
  "severity_label": "severity",
  "severities": { "page": "critical", "high": "warning" },
  "heartbeat_alerts": ["Watchdog"],
  "hidden_alerts": ["InfoInhibitor"],
  "ack_duration": "1h",
  "notify": { "enabled": false, "min_severity": "critical", "clusters": "production", "resolved": false },
  "clusters": {
    "<cluster id or context>": {
      "disabled": false,
      "discover": true,
      "alertmanagers": [
        { "namespace": "monitoring", "service": "alertmanager-operated", "port": "9093",
          "scheme": "http", "path": "", "service_account": "monitoring/am-reader", "tenant": "" },
        { "url": "https://alertmanager.example.com", "ca_file": "", "client_certificate": "",
          "client_key": "", "insecure_skip_tls_verify": false, "tenant": "" }
      ],
      "matchers": ["cluster=\"prod-eu-1\""],
      "rules": true
    }
  }
}
```

- Keyed like `metrics.prometheus`: cluster id first, then context name.
- Services named here count as trusted: they may receive the user's token (see decision 4).
- `client_key` is a path, so the key stays in its file. Authorization headers only go to the
  keychain, never into this file.

### Dev setup and tests
- [x] `script/alertmanager-dev.sh` (run after `prometheus-dev.sh`). It turns Alertmanager on in kube-prometheus-stack (`prometheus-dev.sh` sets `alertmanager.enabled=false` today) and adds a `PrometheusRule` in `payments` with: an always-firing critical alert on a pod, a pending alert (`for: 1h`), a flapping alert, a rule with a broken expression (for rule health), and an inhibition. It also deploys a second Alertmanager behind kube-rbac-proxy (the auth-proxy path, named in settings) and one with `routePrefix: /am`. `memory-hog` already triggers `KubePodCrashLooping`. `--many` adds a rule that fires once per pod, for the 5,000-pod check with `load-pods.sh`
- [x] Unit tests: discovery ranking, v2 and rules parsing, merging, severity and target mapping, the matcher parser and anchored matching, silence validation, heartbeat logic, the token rule, and that no keychain value ever reaches `Debug` output or logs
- [x] Live tests (`crates/kubyl_alerts/tests/live.rs`, ignored by default): discovery finds both Alertmanagers; firing and pending alerts show with the right times; a silence round trip (create, alert silenced, expire); rules parse; the auth-proxy Alertmanager works with the token and the look-alike Service never receives it
- [ ] Screenshots of every board 16 frame

## Acceptance criteria

- On kind with `prometheus-dev.sh` and `alertmanager-dev.sh`: the Alerts row shows with its badge. The test critical alert's "firing since" equals Alertmanager's `startsAt`, and the pending alert shows "pending since". The details show labels, annotations, runbook, receivers, the rule and a working link to the pod. Watchdog shows as heartbeat OK.
- Silencing an alert moves it to Silenced within 2 s. The silence shows in the Silences tab and in Alertmanager's own UI, and expiring it brings the alert back. A read-only cluster offers no silence actions. On PROD the summary confirmation appears, with the typed name when a critical alert matches.
- With Prometheus but no Alertmanager, firing and pending alerts show and silences are marked unavailable. With neither, there's no sidebar row and the view (from the palette) explains what was tried.
- OpenShift (the user's test cluster): platform alerts come through the `alertmanager-main` Route with an `oc login` token and with a client-certificate kubeconfig (service-account token). Rules come from thanos-querier. A user without `monitoring-alertmanager-edit` gets a clear 403 for silences. When enabled, the user workload Alertmanager works through the forward.
- With 5,000 alerts, the view scrolls and filters smoothly.
- No token, Authorization header or Alertmanager config ever appears in logs, settings.json or state.json.

## Risks

- Auth proxies vary (kube-rbac-proxy, oauth-proxy, oauth2-proxy with SSO cookies). Only bearer tokens and a stored header are supported. Setups that need SSO cookies can use the Alertmanager UI in a web view.
- Mapping labels to objects is a heuristic, and a wrong link misleads. Keep the mapping table under test, and show "not found" rather than guess.
- Silences can hide a real outage: previews, guards, a visible end time, and never a silence without a comment.
- Polling many clusters loads Alertmanager and the API servers. Polling is demand-driven, uses the background interval, and backs off after errors.
- Clock skew: "since" uses the local clock. A `startsAt` in the future shows as "just now", and tooltips show the server's timestamp.

## Later (not in this phase)

- A routing tree viewer and "which receivers would get these labels" (like `amtool config routes test`), built only from the route part of the status config.
- Helpers for `AlertmanagerConfig` (namespaced routing) and a form for `PrometheusRule` rules.
- A chart of the rule's expression in the details (phase 07 charts).
- OS notifications instead of toasts.
- Project-scoped access on OpenShift for users without cluster-wide monitoring roles (tenancy ports).
- Grafana-managed alerts, Amazon Managed Prometheus (SigV4), Azure Monitor alerts.
- An Alerts panel in the right dock.
- The same CA and client-certificate options for phase 07's external Prometheus URL.

## Handoff log

### 2026-09-26 (branch `phase/14-15-alerts-polish`, one PR with phase 15)

**Shipped.** Shared-crate commits first: `kubyl_metrics` transport refactor (`transport::Transport`,
`through_route(.., probe)`, `PromClient::api`, `MetricsService::prometheus`; phase 07's tests
unchanged, its live tests pass), explorer view rows and root markers (reusing phase 15's status
slot), `OverviewSection`, toast buttons (`Notification::action`), the siren, bell-off and
list-checks icons, then the `kubyl_alerts` stub and the crate:

- `discover`: settings, prometheus-operator objects (`spec.alerting`, `Alertmanager` objects with
  `routePrefix`/`externalUrl`), Prometheus' `/api/v1/alertmanagers` (pod URLs through
  EndpointSlices), Service names and labels; Services selecting the same pods count once
  (ClusterIP over `-operated`); an Alertmanager seen twice (same gossip cluster name) too.
- `client`: service proxy first; after a 401/403, and only for `token_allowed` targets
  (`openshift-monitoring`, `openshift-user-workload-monitoring`, named in settings), the Route
  (`through_route`) or, without a Route, a temporary forward (`ForwardSpec::ephemeral`, shown in
  Active Sessions) verified against the `openshift-service-ca.crt` CA with
  `<svc>.<ns>.svc` as TLS server name, with the user's token else the settings'
  `service_account`. External URLs take the keychain header `alerts-auth:<cluster id>/<url>`
  (not sent with `insecure_skip_tls_verify`), a CA file, client certificate and key, and
  `X-Scope-OrgID`. Every failure says what's missing (403 → the role or `services/proxy`).
- `model`, `matchers`, `merge`: Alertmanager v2 and Prometheus parsers (3.x, Thanos), anchored
  matchers, the merge (a Prometheus alert joins the Alertmanager alert whose labels include its
  own; without Alertmanager "firing since" = `activeAt` + `for`, marked `~`), severities with
  `alerts.severities`, targets from labels, the heartbeat (`Watchdog`; "Prometheus isn't
  delivering…", "…has no Alertmanager configured").
- `service::AlertsService`: demand-driven polling (15 s while a view shows a cluster, 60 s for
  everything else and while no window is active, rules every 2 min), parsing and merging on
  Tokio, generation counters, transitions in memory (the first read is the baseline, resolved
  alerts listed 15 min), notifications batched per cluster and minute with "Show", optimistic
  writes plus a re-read after 2 s, reset on disconnect/settings, re-keyed with phase 15's
  `Rekeyed`, rediscovery after 3 failures / every 5 min while nothing was found / on
  `DiscoveryChanged` / from the palette.
- The Alerts view (one tab per cluster, cluster color as tab dot; "Alerts: Show Alerts in All
  Clusters" adds a CLUSTER column and lists clusters without a source at the bottom): header
  with source chips and "Open Alertmanager UI" (web view on the Service, browser for Routes and
  URLs), summary line with heartbeat and rule health, filters (matchers or text, severity and
  state chips with counts, namespace incl. "Cluster" and "the active namespace", receiver),
  grouping (name, namespace, severity, receiver, target, none; info groups start collapsed),
  suppressed alerts behind "5 silenced · Show", resolved ones below, the all-clear and every
  empty state (not connected, loading, off, no Alertmanager with what was tried and a settings
  snippet, sign in again / failures). The details pane: times (local and UTC), summary and
  description (plain text, folded), target checked against the live object ("not found"),
  labels (click filters, alt-click excludes), annotations, runbook and generator URL (http(s)
  only; "Open in Prometheus" as a web view), routing (silenced by, inhibited by), the rule
  (expression, `for`, health, last error, the defining `PrometheusRule` → YAML editor) and the
  last 24 h from `ALERTS` ("fired 6 times in 24 h"). Silences tab (active, pending, expired of
  the last 24 h collapsed; matches now; edit, +1h/+4h, expire, recreate, copy as `amtool`) and
  Rules tab (groups with their `PrometheusRule`, only problems, details). Keys through the
  `ActionRegistry` (`enter s a o l r y /`, `enter e ctrl-d n c`, `enter e a y`), bound on the
  lists only, so typing in the filters never triggers them.
- Silences (`silence.rs`): editor with label/value completion from current alerts, ticked
  labels from an alert, durations or a custom end, required comment, creator from
  `SelfSubjectReview`, live preview; the cluster's `matchers` are fixed rows. Every create and
  edit (also acknowledge and extend) shows the summary; on PROD the typed cluster name when a
  critical alert matches, more than 10 match or there's no `alertname` matcher; consent for
  service-account writes; Undo toasts (expire the new one, recreate the expired one). Nothing on
  read-only clusters or without an Alertmanager.
- Chrome: the "Alerts" sidebar row after Overview (`alerts.sidebar` auto/always/never) with the
  count in the worst severity's tone or a check, the red siren on root rows while something
  critical fires, the status bar siren with the top three in its tooltip, the "Alerts" details
  section (pods, workloads incl. their pods, nodes, namespaces, PVCs, Services, HPAs), the
  overview card (cluster and namespace variants), "Alerts: Show Alerts for Selection" in lists,
  and the palette actions (show, all clusters, silences, new silence, look again,
  set/clear the Authorization header).
- `script/alertmanager-dev.sh` (see its header) and `crates/kubyl_alerts/tests/live.rs`.

**Verified.** 42 unit/GPUI tests (discovery ranking fixtures, parsers, merge, targets, anchored
matchers, silence validation and the PROD rule, heartbeat, the token rule with a fake API server
that records every request, no secret in `Debug`, transitions/notification baseline, optimistic
writes, 5,000 alerts keeping the selection). Live on kind (`--ignored`, 5/5): discovery finds
`monitoring/kube-prometheus-stack-alertmanager` and `team-am/prefixed-alertmanager` (`/am`), the
look-alike `monitoring-evil/alertmanager-main` is tried and its access log shows no
`Authorization`; `KubylDevCritical`'s "firing since" equals `startsAt`, `KubylDevPending` is
pending with `activeAt`, `KubylDevInhibited` inhibited, Watchdog is the heartbeat; the broken
rule fails; a silence round trip (silenced within 10 s, expired brings it back); the
kube-rbac-proxy Alertmanager answers through a forward with `team-secure/am-reader`'s token and
its CA from `openshift-service-ca.crt`, never for an unnamed Service. In the app on kind: sidebar
badge, root siren, status bar, header chips, details (target Running, rule, runbook, timeline),
Rules and Silences tabs.

**Deferred / notes.**
- OpenShift acceptance (platform alerts through `alertmanager-main`'s Route with an `oc login`
  token and with a client-certificate kubeconfig, rules from thanos-querier, a 403 for silences
  without `monitoring-alertmanager-edit`, user workload through the forward): the user tests it
  on their cluster. The forward path itself is covered by the kube-rbac-proxy fixture.
- No default service account for user workload monitoring (as the plan says: only once one is
  verified on a real cluster); name one in `alerts.clusters.<cluster>.alertmanagers`.
- Optional explorer badge on favorite namespace rows: not done.
- A 401 on a Route or forward isn't retried with a refreshed token right away: the read fails,
  three failures rediscover (a fresh service-account token), and the view offers "Sign in
  again".
- The published mockup artifact (claude.ai) doesn't have boards 11–17 yet: republish it from
  `design/mockups/generate.py`.
