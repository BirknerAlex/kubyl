#!/usr/bin/env bash
# Adds metrics to the kind cluster from script/dev-cluster.sh: metrics-server (with
# --kubelet-insecure-tls, kind's kubelets use self-signed certificates) and a small
# kube-prometheus-stack (Prometheus, the operator, kube-state-metrics and node-exporter; no
# Grafana or Alertmanager). Also deploys `memory-hog` in `payments`, a pod that is OOMKilled
# every few seconds, for the events stream.
#
# Usage:
#   script/prometheus-dev.sh                      metrics-server + kube-prometheus-stack
#   script/prometheus-dev.sh --metrics-server-only metrics-server only (removes Prometheus), to
#                                                  test the fallback
#   script/prometheus-dev.sh --delete             remove both and memory-hog
#
# Kubyl finds Prometheus by itself (service monitoring/kube-prometheus-stack-prometheus, through
# the API server's service proxy). Needs: kubectl, helm. Respects $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
MONITORING="monitoring"
RELEASE="kube-prometheus-stack"
KPS_VERSION="${KPS_VERSION:-}"
METRICS_SERVER_VERSION="${METRICS_SERVER_VERSION:-}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in kubectl helm; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }
version_flag() { [[ -n "$1" ]] && printf -- '--version=%s' "$1" || true; }

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

remove_prometheus() {
  if h status "$RELEASE" -n "$MONITORING" >/dev/null 2>&1; then
    log "Removing $RELEASE"
    h uninstall "$RELEASE" -n "$MONITORING" --wait >/dev/null
  fi
  # The chart leaves its CRDs behind; they're harmless, but Kubyl shouldn't find a Prometheus.
  k -n "$MONITORING" delete service prometheus-operated --ignore-not-found >/dev/null
}

install_metrics_server() {
  log "Installing metrics-server"
  h repo add metrics-server https://kubernetes-sigs.github.io/metrics-server/ --force-update >/dev/null
  # shellcheck disable=SC2046
  h upgrade --install metrics-server metrics-server/metrics-server -n kube-system \
    $(version_flag "$METRICS_SERVER_VERSION") \
    --set 'args={--kubelet-insecure-tls}' --wait --timeout 5m >/dev/null
}

memory_hog() {
  log "Deploying payments/memory-hog (OOMKilled every few seconds)"
  k create namespace payments --dry-run=client -o yaml | k apply -f - >/dev/null
  k apply -f - >/dev/null <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: memory-hog
  namespace: payments
  labels: { app: memory-hog, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: memory-hog }
  template:
    metadata:
      labels: { app: memory-hog, team: payments }
    spec:
      containers:
        - name: hog
          image: python:3.14-alpine
          # Grows by 8 MiB every 200 ms until the 48 MiB limit kills it.
          command: ["python3", "-c", "import time\nb=[]\nwhile True:\n  b.append(bytearray(8<<20)); time.sleep(0.2)"]
          resources:
            requests: { cpu: 10m, memory: 32Mi }
            limits: { memory: 48Mi }
EOF
}

case "${1:-}" in
  --delete)
    remove_prometheus
    if h status metrics-server -n kube-system >/dev/null 2>&1; then
      log "Removing metrics-server"
      h uninstall metrics-server -n kube-system --wait >/dev/null
    fi
    k -n payments delete deployment memory-hog --ignore-not-found >/dev/null
    log "Done"
    exit 0
    ;;
  --metrics-server-only)
    remove_prometheus
    install_metrics_server
    memory_hog
    log "Done: metrics-server only. Context: $CONTEXT"
    exit 0
    ;;
  "") ;;
  *) die "unknown argument: $1 (use --metrics-server-only or --delete)" ;;
esac

install_metrics_server

log "Installing kube-prometheus-stack into $MONITORING (a few minutes on first run)"
h repo add prometheus-community https://prometheus-community.github.io/helm-charts --force-update >/dev/null
# kind binds etcd, the controller manager, the scheduler and kube-proxy to localhost, so they
# can't be scraped; turn those targets off instead of leaving them down.
# shellcheck disable=SC2046
h upgrade --install "$RELEASE" prometheus-community/kube-prometheus-stack -n "$MONITORING" \
  --create-namespace $(version_flag "$KPS_VERSION") \
  --set grafana.enabled=false \
  --set alertmanager.enabled=false \
  --set kubeEtcd.enabled=false \
  --set kubeControllerManager.enabled=false \
  --set kubeScheduler.enabled=false \
  --set kubeProxy.enabled=false \
  --set prometheus.prometheusSpec.retention=2d \
  --set prometheus.prometheusSpec.scrapeInterval=15s \
  --set prometheus.prometheusSpec.resources.requests.memory=256Mi \
  --wait --timeout 10m >/dev/null

memory_hog

log "Waiting for Prometheus"
k -n "$MONITORING" rollout status statefulset/"prometheus-${RELEASE}-prometheus" --timeout=300s >/dev/null
log "Done. Prometheus: $MONITORING/${RELEASE}-prometheus:9090 (context $CONTEXT)"
k -n "$MONITORING" get pods
