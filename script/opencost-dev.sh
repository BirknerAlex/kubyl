#!/usr/bin/env bash
# OpenCost on the kind cluster, on top of the Prometheus script/prometheus-dev.sh installs
# (kube-prometheus-stack in `monitoring`), for Kubyl's Cost view (phase 25).
#
# OpenCost (chart opencost.github.io/opencost-helm-chart, installed with --repo: no Helm
# repository is added) goes into namespace `opencost` as release `opencost` and reads the
# Prometheus Service `monitoring/kube-prometheus-stack-prometheus:9090`. Its API is the Service
# `opencost` port 9003 (`/allocation/compute`, `/model/...`), its UI port 9090.
#
# kind has no cloud provider, so OpenCost has no prices ("No pricing found for key=default",
# every cost 0): the script turns on its custom pricing with made-up on-prem rates (CPU $0.031611
# per core-hour, RAM $0.004237 per GiB-hour, storage $0.00005479 per GiB-hour). The costs are
# estimates, enough to see the shape. The allocation API needs Prometheus data for the window; on
# a fresh cluster the first minutes return empty or tiny allocations (a few even negative).
# node-exporter and kube-state-metrics (part of the stack) must run.
#
# Usage:
#   script/opencost-dev.sh            install or update
#   script/opencost-dev.sh --delete   uninstall and remove the namespace (Prometheus stays)
#   script/opencost-dev.sh --force    allow a context that isn't kind-*
#
# Live test: crates/kubyl_cost_core/tests/live.rs. Needs kubectl and helm 3.13+. Respects
# $KUBECONFIG and $KUBYL_COST_CONTEXT (default kind-kubyl-dev), $OPENCOST_CHART_VERSION.
set -euo pipefail

CONTEXT="${KUBYL_COST_CONTEXT:-kind-kubyl-dev}"
CHART_REPO="https://opencost.github.io/opencost-helm-chart"
CHART_VERSION="${OPENCOST_CHART_VERSION:-}"
NS="opencost"
PROM_NS="monitoring"
PROM_SVC="kube-prometheus-stack-prometheus"
MARK="kubyl.dev/installed-by"
ME="opencost-dev.sh"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

DELETE=0
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --delete) DELETE=1 ;;
    --force) FORCE=1 ;;
    *) die "unknown option $arg" ;;
  esac
done
for tool in kubectl helm; do command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"; done
case "$CONTEXT" in kind-*) ;; *) [ "$FORCE" = 1 ] || die "refusing context $CONTEXT (not kind-*); --force allows it" ;; esac
k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }
k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

marked() { k get ns "$NS" -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null | grep -q "$ME"; }

if [ "$DELETE" = 1 ]; then
  if h status opencost -n "$NS" >/dev/null 2>&1; then
    log "uninstalling opencost"
    h uninstall opencost -n "$NS" --wait >/dev/null
  fi
  if marked; then
    k delete ns "$NS" --wait=true --timeout=120s >/dev/null || true
  fi
  log "done"
  exit 0
fi

k -n "$PROM_NS" get svc "$PROM_SVC" >/dev/null 2>&1 \
  || die "no Prometheus service $PROM_NS/$PROM_SVC: run script/prometheus-dev.sh first"
if k get ns "$NS" >/dev/null 2>&1 && ! marked; then
  die "namespace $NS exists and isn't ours"
fi
k create namespace "$NS" --dry-run=client -o yaml | k apply -f - >/dev/null
k annotate namespace "$NS" "$MARK=$ME" --overwrite >/dev/null

version_args=()
[ -n "$CHART_VERSION" ] && version_args=(--version "$CHART_VERSION")
log "installing opencost into $NS (reads $PROM_NS/$PROM_SVC)"
h upgrade --install opencost opencost --repo "$CHART_REPO" -n "$NS" \
  --set opencost.prometheus.internal.enabled=true \
  --set opencost.prometheus.internal.serviceName="$PROM_SVC" \
  --set opencost.prometheus.internal.namespaceName="$PROM_NS" \
  --set opencost.prometheus.internal.port=9090 \
  --set opencost.exporter.defaultClusterId="$CONTEXT" \
  --set opencost.ui.enabled=true \
  --set opencost.metrics.serviceMonitor.enabled=false \
  --set opencost.customPricing.enabled=true \
  --set opencost.customPricing.provider=custom \
  --set opencost.customPricing.costModel.description="kubyl dev rates" \
  --set-string opencost.customPricing.costModel.CPU=0.031611 \
  --set-string opencost.customPricing.costModel.spotCPU=0.006655 \
  --set-string opencost.customPricing.costModel.RAM=0.004237 \
  --set-string opencost.customPricing.costModel.spotRAM=0.000892 \
  --set-string opencost.customPricing.costModel.GPU=0.95 \
  --set-string opencost.customPricing.costModel.storage=0.00005479 \
  --set-string opencost.customPricing.costModel.zoneNetworkEgress=0.01 \
  --set-string opencost.customPricing.costModel.regionNetworkEgress=0.01 \
  --set-string opencost.customPricing.costModel.internetNetworkEgress=0.12 \
  ${version_args[@]+"${version_args[@]}"} --wait --timeout 5m >/dev/null
log "OpenCost runs; its allocations fill in as Prometheus collects data (a few minutes)"
log "try: kubectl --context $CONTEXT get --raw '/api/v1/namespaces/$NS/services/opencost:9003/proxy/allocation/compute?window=1h&aggregate=namespace'"
