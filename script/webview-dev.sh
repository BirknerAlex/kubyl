#!/usr/bin/env bash
# Web UIs for trying service web views (phase 08) on the kind cluster from script/dev-cluster.sh,
# after script/prometheus-dev.sh:
#
# - Grafana and Alertmanager (turned on in the kube-prometheus-stack release), plus an Ingress
#   `monitoring/grafana` (no ingress controller needed: Kubyl opens its backend Service).
# - `payments/tls-web`: nginx behind HTTPS with a self-signed certificate (port 443, name
#   https), for the certificate interstitial.
# - `payments/admin-console`: a page on a port that doesn't look like HTTP (7000, name
#   tcp-mgmt), for "Open as web view…", with a download and a file input.
#
# Usage:
#   script/webview-dev.sh           install
#   script/webview-dev.sh --delete  remove them again
#
# Grafana's login is admin / kubyl-dev (dev only). Needs: kubectl, helm, openssl. Respects
# $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
MONITORING="monitoring"
RELEASE="kube-prometheus-stack"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in kubectl helm openssl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

if [[ "${1:-}" == "--delete" ]]; then
  log "Removing tls-web, admin-console and the grafana Ingress"
  k -n payments delete deployment,service tls-web admin-console --ignore-not-found >/dev/null
  k -n payments delete secret tls-web --ignore-not-found >/dev/null
  k -n payments delete configmap tls-web admin-console --ignore-not-found >/dev/null
  k -n "$MONITORING" delete ingress grafana --ignore-not-found >/dev/null
  if h status "$RELEASE" -n "$MONITORING" >/dev/null 2>&1; then
    log "Turning Grafana and Alertmanager off again"
    h upgrade "$RELEASE" prometheus-community/kube-prometheus-stack -n "$MONITORING" \
      --reuse-values --set grafana.enabled=false --set alertmanager.enabled=false \
      --wait --timeout 10m >/dev/null
  fi
  log "Done"
  exit 0
fi
[[ -z "${1:-}" ]] || die "unknown argument: $1 (use --delete)"

h status "$RELEASE" -n "$MONITORING" >/dev/null 2>&1 \
  || die "$RELEASE isn't installed (run script/prometheus-dev.sh first)"

log "Turning on Grafana and Alertmanager in $RELEASE"
h repo add prometheus-community https://prometheus-community.github.io/helm-charts --force-update >/dev/null
h upgrade "$RELEASE" prometheus-community/kube-prometheus-stack -n "$MONITORING" \
  --reuse-values \
  --set grafana.enabled=true \
  --set grafana.adminPassword=kubyl-dev \
  --set grafana.service.portName=http-web \
  --set alertmanager.enabled=true \
  --wait --timeout 10m >/dev/null

log "Adding the Ingress $MONITORING/grafana"
k apply -f - >/dev/null <<EOF
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: grafana
  namespace: $MONITORING
spec:
  rules:
    - host: grafana.kubyl.local
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: $RELEASE-grafana
                port: { name: http-web }
EOF

log "Deploying payments/tls-web (self-signed HTTPS)"
k create namespace payments --dry-run=client -o yaml | k apply -f - >/dev/null
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 365 -keyout "$tmp/tls.key" -out "$tmp/tls.crt" \
  -subj "/O=Kubyl dev/CN=tls-web.payments.svc" \
  -addext "subjectAltName=DNS:tls-web,DNS:tls-web.payments.svc" >/dev/null 2>&1
k -n payments create secret tls tls-web --cert="$tmp/tls.crt" --key="$tmp/tls.key" \
  --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: tls-web
  namespace: payments
data:
  default.conf: |
    server {
      listen 8443 ssl;
      ssl_certificate /tls/tls.crt;
      ssl_certificate_key /tls/tls.key;
      location / {
        default_type text/html;
        return 200 '<!doctype html><title>tls-web</title><h1>Served over HTTPS with a self-signed certificate</h1>';
      }
    }
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: tls-web
  namespace: payments
  labels: { app: tls-web, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: tls-web }
  template:
    metadata:
      labels: { app: tls-web, team: payments }
    spec:
      containers:
        - name: nginx
          image: nginx:1.29-alpine
          ports: [{ name: https, containerPort: 8443 }]
          volumeMounts:
            - { name: tls, mountPath: /tls, readOnly: true }
            - { name: conf, mountPath: /etc/nginx/conf.d }
      volumes:
        - { name: tls, secret: { secretName: tls-web } }
        - { name: conf, configMap: { name: tls-web } }
---
apiVersion: v1
kind: Service
metadata:
  name: tls-web
  namespace: payments
spec:
  selector: { app: tls-web }
  ports:
    - { name: https, port: 443, targetPort: https }
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: admin-console
  namespace: payments
data:
  default.conf: |
    server {
      listen 7000;
      location = /report.csv {
        default_type text/csv;
        add_header Content-Disposition 'attachment; filename="report.csv"';
        return 200 'service,port\ngrafana,80\n';
      }
      location / {
        default_type text/html;
        return 200 '<!doctype html><title>admin console</title><h1>An HTTP page on a port that does not look like HTTP</h1><p><a href="/report.csv">Download report.csv</a></p><p><input type="file"></p>';
      }
    }
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: admin-console
  namespace: payments
  labels: { app: admin-console, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: admin-console }
  template:
    metadata:
      labels: { app: admin-console, team: payments }
    spec:
      containers:
        - name: nginx
          image: nginx:1.29-alpine
          ports: [{ name: tcp-mgmt, containerPort: 7000 }]
          volumeMounts:
            - { name: conf, mountPath: /etc/nginx/conf.d }
      volumes:
        - { name: conf, configMap: { name: admin-console } }
---
apiVersion: v1
kind: Service
metadata:
  name: admin-console
  namespace: payments
spec:
  selector: { app: admin-console }
  ports:
    - { name: tcp-mgmt, port: 7000, targetPort: tcp-mgmt }
EOF

log "Waiting for the pods"
k -n payments rollout status deployment/tls-web --timeout=180s >/dev/null
k -n payments rollout status deployment/admin-console --timeout=180s >/dev/null
k -n "$MONITORING" rollout status deployment/"$RELEASE"-grafana --timeout=300s >/dev/null
log "Done. Grafana: $MONITORING/$RELEASE-grafana:80 (admin / kubyl-dev), HTTPS: payments/tls-web:443"
