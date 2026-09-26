#!/usr/bin/env bash
# Alerts for the kind cluster (phase 14). Run after script/prometheus-dev.sh.
#
# - Turns Alertmanager on in kube-prometheus-stack (2 replicas, receivers pagerduty-payments
#   and slack-payments without integrations, an inhibition rule).
# - A PrometheusRule `payments/payments-slo` with: an always-firing critical alert on the
#   memory-hog pod, a pending alert (`for: 1h`), a flapping alert, a rule that fails to evaluate
#   ("vector contains metrics with the same labelset after applying alert labels"), and a
#   warning the critical one inhibits. memory-hog already triggers KubePodCrashLooping.
# - `team-am/prefixed`: an Alertmanager object with `routePrefix: /am` (discovery reads it).
# - `team-secure/secured-alertmanager`: an Alertmanager behind kube-rbac-proxy (HTTPS, a
#   serving certificate from a CA published as the `openshift-service-ca.crt` ConfigMap, like
#   OpenShift's service CA). The API server's service proxy gets a 401 there; Kubyl reaches it
#   through a temporary port-forward with the token of service account `team-secure/am-reader`
#   once it's named in settings (see below).
# - `monitoring-evil/alertmanager-main`: a look-alike that answers 401 and logs every
#   Authorization header it gets (`kubectl -n monitoring-evil logs deploy/alertmanager-main`).
#   It must never see one.
#
# Usage:
#   script/alertmanager-dev.sh           set up everything above
#   script/alertmanager-dev.sh --many    also a rule firing once per pod in namespace `load`
#                                        (run script/load-pods.sh first: 5,000 alerts)
#   script/alertmanager-dev.sh --delete  remove it (Alertmanager off again)
#
# Settings for the auth-proxy path (settings.json; with entries there Kubyl discovers nothing
# else for the cluster, so name the main one too):
#   "alerts": { "clusters": { "kind-kubyl-dev": { "alertmanagers": [
#     { "namespace": "monitoring", "service": "kube-prometheus-stack-alertmanager", "port": "9093" },
#     { "namespace": "team-secure", "service": "secured-alertmanager", "port": "https",
#       "scheme": "https", "service_account": "team-secure/am-reader" } ] } } }
#
# Needs: kubectl, helm, openssl. Respects $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
MONITORING="monitoring"
RELEASE="kube-prometheus-stack"
KPS_VERSION="${KPS_VERSION:-}"
RBAC_PROXY_IMAGE="${RBAC_PROXY_IMAGE:-quay.io/brancz/kube-rbac-proxy:v0.18.2}"
AM_IMAGE="${AM_IMAGE:-quay.io/prometheus/alertmanager:v0.28.1}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in kubectl helm openssl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }
version_flag() { [[ -n "$1" ]] && printf -- '--version=%s' "$1" || true; }

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"
h status "$RELEASE" -n "$MONITORING" >/dev/null 2>&1 || die "run script/prometheus-dev.sh first"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

MANY=false
case "${1:-}" in
  --delete)
    log "Removing the alert fixtures"
    k delete namespace team-am team-secure monitoring-evil --ignore-not-found --wait=false >/dev/null
    k -n payments delete prometheusrule payments-slo load-per-pod --ignore-not-found >/dev/null
    log "Turning Alertmanager off"
    # shellcheck disable=SC2046
    h upgrade "$RELEASE" prometheus-community/kube-prometheus-stack -n "$MONITORING" \
      $(version_flag "$KPS_VERSION") --reuse-values --set alertmanager.enabled=false \
      --wait --timeout 10m >/dev/null
    log "Done"
    exit 0
    ;;
  --many) MANY=true ;;
  "") ;;
  *) die "unknown argument: $1 (use --many or --delete)" ;;
esac

log "Turning Alertmanager on in $RELEASE"
cat >"$WORK/values.yaml" <<'EOF'
alertmanager:
  enabled: true
  alertmanagerSpec:
    replicas: 2
    resources:
      requests: { memory: 48Mi }
  config:
    global:
      resolve_timeout: 5m
    route:
      receiver: slack-payments
      group_by: [alertname, namespace]
      group_wait: 10s
      group_interval: 1m
      repeat_interval: 4h
      routes:
        - receiver: "null"
          matchers: ['alertname=~"Watchdog|InfoInhibitor"']
        - receiver: pagerduty-payments
          matchers: ['severity="critical"']
          continue: true
        - receiver: slack-payments
          matchers: ['namespace="payments"']
    inhibit_rules:
      - source_matchers: ['alertname="KubylDevCritical"']
        target_matchers: ['alertname="KubylDevInhibited"']
        equal: [namespace]
    receivers:
      - name: "null"
      - name: pagerduty-payments
      - name: slack-payments
EOF
# shellcheck disable=SC2046
h upgrade "$RELEASE" prometheus-community/kube-prometheus-stack -n "$MONITORING" \
  $(version_flag "$KPS_VERSION") --reuse-values -f "$WORK/values.yaml" \
  --wait --timeout 10m >/dev/null

log "PrometheusRule payments/payments-slo"
k create namespace payments --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<'EOF'
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: payments-slo
  namespace: payments
  labels: { release: kube-prometheus-stack }
spec:
  groups:
    - name: payments.rules
      rules:
        - alert: KubylDevCritical
          expr: max by (namespace, pod) (kube_pod_info{namespace="payments", pod=~"memory-hog-.*"})
          for: 1m
          labels: { severity: critical, team: payments }
          annotations:
            summary: memory-hog is out of memory again.
            description: >-
              Pod {{ $labels.namespace }}/{{ $labels.pod }} keeps getting OOMKilled. This alert
              always fires on the dev cluster so the Alerts view has something critical to show.
            runbook_url: https://runbooks.example.com/payments/memory-hog
        - alert: KubylDevPending
          expr: max by (namespace, deployment) (kube_deployment_created{namespace="payments"}) > 0
          for: 1h
          labels: { severity: warning, team: payments }
          annotations:
            summary: A payments Deployment exists (pending for an hour before it fires).
        - alert: KubylDevFlapping
          expr: vector(1) and on() (vector(time() % 240) < 120)
          labels: { severity: warning, namespace: payments, team: payments }
          annotations:
            summary: Fires for two minutes, then resolves for two.
        - alert: KubylDevInhibited
          expr: max by (namespace) (kube_pod_info{namespace="payments"})
          labels: { severity: warning, team: payments }
          annotations:
            summary: Inhibited by KubylDevCritical in the same namespace.
    - name: payments.broken
      rules:
        - alert: KubylDevBrokenRule
          # Every namespace collapses into the same labels: the rule fails to evaluate.
          expr: count by (namespace) (kube_pod_info) > 0
          labels: { severity: critical, namespace: payments }
          annotations:
            summary: This rule never evaluates.
EOF

if $MANY; then
  log "PrometheusRule payments/load-per-pod (one alert per pod in namespace load)"
  k apply -f - >/dev/null <<'EOF'
apiVersion: monitoring.coreos.com/v1
kind: PrometheusRule
metadata:
  name: load-per-pod
  namespace: payments
  labels: { release: kube-prometheus-stack }
spec:
  groups:
    - name: load.rules
      rules:
        - alert: KubylDevPerPod
          expr: max by (namespace, pod) (kube_pod_info{namespace="load"})
          labels: { severity: info }
          annotations:
            summary: One alert per pod in load.
EOF
fi

log "team-am/prefixed: an Alertmanager with routePrefix /am"
k create namespace team-am --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<'EOF'
apiVersion: monitoring.coreos.com/v1
kind: Alertmanager
metadata:
  name: prefixed
  namespace: team-am
spec:
  replicas: 1
  routePrefix: /am
  resources:
    requests: { memory: 32Mi }
---
apiVersion: v1
kind: Service
metadata:
  name: prefixed-alertmanager
  namespace: team-am
spec:
  selector: { alertmanager: prefixed }
  ports:
    - { name: web, port: 9093, targetPort: 9093 }
EOF

log "team-secure/secured-alertmanager: behind kube-rbac-proxy"
openssl req -x509 -newkey rsa:2048 -nodes -days 365 -subj "/CN=kubyl-dev-service-ca" \
  -keyout "$WORK/ca.key" -out "$WORK/ca.crt" >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -subj "/CN=secured-alertmanager.team-secure.svc" \
  -keyout "$WORK/tls.key" -out "$WORK/tls.csr" >/dev/null 2>&1
printf 'subjectAltName=DNS:secured-alertmanager.team-secure.svc,DNS:secured-alertmanager.team-secure.svc.cluster.local\n' >"$WORK/san.ext"
openssl x509 -req -in "$WORK/tls.csr" -CA "$WORK/ca.crt" -CAkey "$WORK/ca.key" -CAcreateserial \
  -days 365 -extfile "$WORK/san.ext" -out "$WORK/tls.crt" >/dev/null 2>&1
k create namespace team-secure --dry-run=client -o yaml | k apply -f - >/dev/null
k -n team-secure create secret tls secured-alertmanager-tls --cert="$WORK/tls.crt" --key="$WORK/tls.key" \
  --dry-run=client -o yaml | k apply -f - >/dev/null
# Where OpenShift publishes its service CA; Kubyl verifies forwards against it.
k -n team-secure create configmap openshift-service-ca.crt --from-file=service-ca.crt="$WORK/ca.crt" \
  --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<EOF
apiVersion: v1
kind: ServiceAccount
metadata: { name: secured-alertmanager, namespace: team-secure }
---
apiVersion: v1
kind: ServiceAccount
metadata: { name: am-reader, namespace: team-secure }
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata: { name: kubyl-dev-secured-alertmanager-auth-delegator }
roleRef: { apiGroup: rbac.authorization.k8s.io, kind: ClusterRole, name: system:auth-delegator }
subjects: [{ kind: ServiceAccount, name: secured-alertmanager, namespace: team-secure }]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata: { name: alertmanager-api, namespace: team-secure }
rules:
  - apiGroups: [monitoring.coreos.com]
    resources: [alertmanagers/api]
    resourceNames: [secured]
    verbs: [get, list, create, update, patch, delete]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata: { name: am-reader-alertmanager-api, namespace: team-secure }
roleRef: { apiGroup: rbac.authorization.k8s.io, kind: Role, name: alertmanager-api }
subjects: [{ kind: ServiceAccount, name: am-reader, namespace: team-secure }]
---
apiVersion: v1
kind: ConfigMap
metadata: { name: secured-alertmanager, namespace: team-secure }
data:
  alertmanager.yml: |
    route: { receiver: "null" }
    receivers: [{ name: "null" }]
  rbac-proxy.yaml: |
    authorization:
      resourceAttributes:
        namespace: team-secure
        apiGroup: monitoring.coreos.com
        resource: alertmanagers
        subresource: api
        name: secured
---
apiVersion: apps/v1
kind: Deployment
metadata: { name: secured-alertmanager, namespace: team-secure }
spec:
  replicas: 1
  selector: { matchLabels: { app: secured-alertmanager } }
  template:
    metadata: { labels: { app: secured-alertmanager } }
    spec:
      serviceAccountName: secured-alertmanager
      containers:
        - name: alertmanager
          image: $AM_IMAGE
          args: [--config.file=/etc/am/alertmanager.yml, --web.listen-address=127.0.0.1:9093, --cluster.listen-address=]
          resources: { requests: { memory: 32Mi } }
          volumeMounts: [{ name: config, mountPath: /etc/am }]
        - name: kube-rbac-proxy
          image: $RBAC_PROXY_IMAGE
          args:
            - --secure-listen-address=0.0.0.0:9443
            - --upstream=http://127.0.0.1:9093/
            - --config-file=/etc/am/rbac-proxy.yaml
            - --tls-cert-file=/tls/tls.crt
            - --tls-private-key-file=/tls/tls.key
          ports: [{ name: https, containerPort: 9443 }]
          resources: { requests: { memory: 16Mi } }
          volumeMounts:
            - { name: config, mountPath: /etc/am }
            - { name: tls, mountPath: /tls }
      volumes:
        - { name: config, configMap: { name: secured-alertmanager } }
        - { name: tls, secret: { secretName: secured-alertmanager-tls } }
---
apiVersion: v1
kind: Service
metadata: { name: secured-alertmanager, namespace: team-secure }
spec:
  selector: { app: secured-alertmanager }
  ports:
    - { name: https, port: 9443, targetPort: https }
EOF

log "monitoring-evil/alertmanager-main: a look-alike that logs Authorization headers"
k create namespace monitoring-evil --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata: { name: alertmanager-main, namespace: monitoring-evil }
data:
  default.conf: |
    log_format auth '$remote_addr "$request" authorization="$http_authorization"';
    server {
      listen 9094;
      access_log /dev/stdout auth;
      location / { return 401 "Unauthorized\n"; }
    }
---
apiVersion: apps/v1
kind: Deployment
metadata: { name: alertmanager-main, namespace: monitoring-evil }
spec:
  replicas: 1
  selector: { matchLabels: { app: alertmanager-main } }
  template:
    metadata: { labels: { app: alertmanager-main } }
    spec:
      containers:
        - name: nginx
          image: nginx:1.29-alpine
          ports: [{ name: web, containerPort: 9094 }]
          resources: { requests: { memory: 16Mi } }
          volumeMounts: [{ name: conf, mountPath: /etc/nginx/conf.d }]
      volumes: [{ name: conf, configMap: { name: alertmanager-main } }]
---
apiVersion: v1
kind: Service
metadata: { name: alertmanager-main, namespace: monitoring-evil }
spec:
  selector: { app: alertmanager-main }
  ports:
    - { name: web, port: 9094, targetPort: web }
EOF

log "Waiting for the Alertmanagers"
k -n "$MONITORING" rollout status statefulset/"alertmanager-${RELEASE}-alertmanager" --timeout=300s >/dev/null
k -n team-secure rollout status deployment/secured-alertmanager --timeout=300s >/dev/null
k -n monitoring-evil rollout status deployment/alertmanager-main --timeout=300s >/dev/null
for _ in $(seq 1 60); do
  k -n team-am get statefulset alertmanager-prefixed >/dev/null 2>&1 && break
  sleep 2
done
k -n team-am rollout status statefulset/alertmanager-prefixed --timeout=300s >/dev/null
log "Done (context $CONTEXT). Alerts need a minute or two to fire."
