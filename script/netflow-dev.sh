#!/usr/bin/env bash
# Network flow backends (phase 16): one kind cluster per backend. Run one at a time, each needs
# about 3 GB of Docker memory (NetObserv with Loki and Prometheus about 4 GB).
#
#   --cilium     kind cluster kubyl-cilium (/tmp/kubyl-dev/cilium-kubeconfig) without kindnet
#                (disableDefaultCNI), Cilium $CILIUM_VERSION with Hubble Relay (plain gRPC: Service
#                kube-system/hubble-relay port 80, pod port 4245; no Hubble UI).
#   --netobserv  kind cluster kubyl-netobserv (/tmp/kubyl-dev/netobserv-kubeconfig) on kindnet,
#                the default CNI: cert-manager $CERT_MANAGER_VERSION and trust-manager (NetObserv's
#                certificates), NetObserv $NETOBSERV_VERSION with its Loki and Prometheus
#                subcharts, and a FlowCollector that samples every packet and tracks drops and DNS.
#   --calico     kind cluster kubyl-calico (/tmp/kubyl-dev/calico-kubeconfig) without kindnet,
#                Calico $CALICO_VERSION through tigera-operator with Goldmane and Whisker (Service
#                calico-system/whisker port 8081).
#
# Every cluster gets the payments workloads of script/dev-cluster.sh, plus traffic:
#   - payments/checkout-client calls checkout-api and ledger-api every second.
#   - payments/ledger-api (nginx) is isolated by the NetworkPolicy ledger-api-isolation: ingress
#     only from checkout-client. Other callers are dropped with no policy that denies them: a
#     plain NetworkPolicy has no deny rules, it isolates.
#   - storefront/web (nginx); storefront/shopper calls web, web/search?q=shoes&token=…, and
#     payments' checkout-api (allowed) and ledger-api (dropped: isolated); storefront/scraper
#     calls web every 2 s.
#   - web-guard, an explicit deny rule where the CNI has them: a CiliumNetworkPolicy (ingressDeny
#     from scraper, ingress from storefront with L7 HTTP visibility, so Hubble reports paths) or a
#     Calico NetworkPolicy (Deny from scraper, then Allow). kindnet has no deny rules: on
#     --netobserv the scraper's calls go through.
#
# Usage:
#   script/netflow-dev.sh --cilium|--netobserv|--calico   create the cluster (or update it)
#   script/netflow-dev.sh --busy <mode>                   also 12 shopper replicas (a few hundred
#                                                         flows a second, for list performance)
#   script/netflow-dev.sh --cilium --relay-tls            Hubble Relay with server TLS (Service port
#                                                         443, certificate *.hubble-relay.cilium.io)
#                                                         and its CA in ConfigMap
#                                                         kube-system/cilium-root-ca.crt
#   script/netflow-dev.sh --delete [<mode>]               delete the clusters this script created
#                                                         (only one mode with a mode flag)
#
# Point Kubyl at a scratch kubeconfig (never ~/.kube/config):
#   KUBECONFIG=/tmp/kubyl-dev/cilium-kubeconfig cargo run -p kubyl
# and compare with `hubble observe` through `kubectl port-forward -n kube-system svc/hubble-relay
# 4245:80`, Loki's /loki/api/v1/query_range, or Whisker's UI (`kubectl port-forward -n
# calico-system svc/whisker 8081`).
#
# Each cluster is this script's alone: kube-system carries the annotation
# kubyl.dev/installed-by=netflow-dev.sh and --delete removes only clusters with it. Never
# installs anything into kind-kubyl-dev.
#
# Needs: docker (running), kind, kubectl, helm. Respects $CILIUM_VERSION, $CALICO_VERSION,
# $NETOBSERV_VERSION, $CERT_MANAGER_VERSION, $TRUST_MANAGER_VERSION and $KUBECONFIG_DIR.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Tested versions (2026-09-27).
CILIUM_VERSION="${CILIUM_VERSION:-1.20.2}"
CALICO_VERSION="${CALICO_VERSION:-v3.32.2}"
NETOBSERV_VERSION="${NETOBSERV_VERSION:-2.0.0}"
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-v1.21.2}"
TRUST_MANAGER_VERSION="${TRUST_MANAGER_VERSION:-v0.25.0}"
KUBECONFIG_DIR="${KUBECONFIG_DIR:-/tmp/kubyl-dev}"

INSTALLED_BY="kubyl.dev/installed-by"
MARK="netflow-dev.sh"
MODES="cilium netobserv calico"
WEB_IMAGE="nginx:1.29-alpine"
CLIENT_IMAGE="busybox:1.37"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

cluster_of() { echo "kubyl-$1"; }
context_of() { echo "kind-kubyl-$1"; }
kubeconfig_of() { echo "$KUBECONFIG_DIR/$1-kubeconfig"; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# ----- Arguments -----

MODE="" ACTION=setup BUSY=false RELAY_TLS=false
while (($#)); do
  case "$1" in
    --cilium | --netobserv | --calico)
      [[ -z "$MODE" ]] || die "pick one of --cilium, --netobserv, --calico"
      MODE="${1#--}"
      ;;
    --busy) BUSY=true ;;
    --relay-tls) RELAY_TLS=true ;;
    --delete) ACTION=delete ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done

for tool in docker kind kubectl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done
docker info >/dev/null 2>&1 || die "docker is not running"

exists() { kind get clusters 2>/dev/null | grep -qx "$(cluster_of "$1")"; }

# The kube-system mark, read through the kubeconfig kind prints (works without the scratch file).
cluster_marked() {
  local config="$tmp/check-kubeconfig"
  kind get kubeconfig --name "$(cluster_of "$1")" >"$config" 2>/dev/null || return 1
  [[ "$(kubectl --kubeconfig "$config" --context "$(context_of "$1")" get namespace kube-system \
    -o jsonpath='{.metadata.annotations.kubyl\.dev/installed-by}' 2>/dev/null || true)" == "$MARK" ]]
}

# ----- --delete -----

if [[ "$ACTION" == delete ]]; then
  for mode in ${MODE:-$MODES}; do
    cluster="$(cluster_of "$mode")"
    if ! exists "$mode"; then
      log "kind cluster $cluster doesn't exist"
      continue
    fi
    if ! cluster_marked "$mode"; then
      warn "kind cluster $cluster isn't reachable or wasn't created by this script (no $INSTALLED_BY=$MARK on kube-system): not deleting it"
      continue
    fi
    log "Deleting kind cluster $cluster"
    mkdir -p "$KUBECONFIG_DIR"
    touch "$(kubeconfig_of "$mode")"
    kind delete cluster --name "$cluster" --kubeconfig "$(kubeconfig_of "$mode")"
    rm -f "$(kubeconfig_of "$mode")"
  done
  log "Done"
  exit 0
fi

[[ -n "$MODE" ]] || die "pick one of --cilium, --netobserv, --calico (see --help)"
command -v helm >/dev/null 2>&1 || [[ "$MODE" == calico ]] || die "helm is not installed"

CLUSTER="$(cluster_of "$MODE")"
CONTEXT="$(context_of "$MODE")"
KCFG="$(kubeconfig_of "$MODE")"

# Every call names its kubeconfig and context; the current context is never used.
k() { kubectl --kubeconfig "$KCFG" --context "$CONTEXT" "$@"; }
h() { helm --kubeconfig "$KCFG" --kube-context "$CONTEXT" "$@"; }

wait_for() {
  # wait_for <seconds> <description> <command…>: retries the command every 5 s.
  local timeout="$1" what="$2"
  shift 2
  local start
  start="$(date +%s)"
  until "$@" >/dev/null 2>&1; do
    if (($(date +%s) - start > timeout)); then
      die "timed out waiting for $what"
    fi
    sleep 5
  done
}

# ----- Cluster -----

create_cluster() {
  mkdir -p "$KUBECONFIG_DIR"
  if exists "$MODE"; then
    log "kind cluster $CLUSTER exists; updating it"
    k get --raw /version >/dev/null 2>&1 \
      || kind export kubeconfig --name "$CLUSTER" --kubeconfig "$KCFG" >/dev/null 2>&1
    cluster_marked "$MODE" || die "kind cluster $CLUSTER wasn't created by this script (no $INSTALLED_BY on kube-system)"
    return
  fi
  local networking=""
  case "$MODE" in
    cilium) networking=$'networking:\n  disableDefaultCNI: true' ;;
    # custom-resources.yaml's IP pool.
    calico) networking=$'networking:\n  disableDefaultCNI: true\n  podSubnet: 192.168.0.0/16' ;;
  esac
  log "Creating kind cluster $CLUSTER (kubeconfig $KCFG)"
  # No --wait: without a CNI the nodes only get ready once it's installed.
  kind create cluster --name "$CLUSTER" --kubeconfig "$KCFG" --config - <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
$networking
nodes:
  - role: control-plane
  - role: worker
EOF
  # The whole cluster is this script's: --delete checks this mark.
  k annotate namespace kube-system --overwrite "$INSTALLED_BY=$MARK" >/dev/null
}

# ----- Backends -----

install_cilium() {
  local tls=(--set hubble.relay.tls.server.enabled=false --set tls.caBundle.enabled=false)
  if $RELAY_TLS; then
    log "Installing Cilium $CILIUM_VERSION with Hubble Relay (server TLS, CA bundle ConfigMap)"
    tls=(--set hubble.relay.tls.server.enabled=true)
  else
    log "Installing Cilium $CILIUM_VERSION with Hubble Relay"
  fi
  h upgrade --install cilium cilium --repo https://helm.cilium.io --version "$CILIUM_VERSION" \
    --namespace kube-system \
    --set image.pullPolicy=IfNotPresent \
    --set ipam.mode=kubernetes \
    --set operator.replicas=1 \
    --set hubble.enabled=true \
    --set hubble.relay.enabled=true \
    --set hubble.ui.enabled=false \
    "${tls[@]}" >/dev/null
  if $RELAY_TLS; then
    # The chart generated its CA (Secret cilium-ca). Publish its certificate in the ConfigMap
    # cilium-root-ca.crt (tls.caBundle), which is what Kubyl verifies the Relay against: it
    # never reads Secrets. The script may, it set the cluster up.
    local ca
    ca="$(k -n kube-system get secret cilium-ca -o jsonpath='{.data.ca\.crt}' | base64 -d)"
    [[ -n "$ca" ]] || die "Secret kube-system/cilium-ca has no ca.crt"
    h upgrade cilium cilium --repo https://helm.cilium.io --version "$CILIUM_VERSION" \
      --namespace kube-system --reuse-values \
      --set tls.caBundle.enabled=true --set-string "tls.caBundle.content=$ca" >/dev/null
  fi
  k -n kube-system rollout status daemonset/cilium --timeout=300s
  k -n kube-system rollout status deployment/hubble-relay --timeout=300s
}

install_calico() {
  log "Installing Calico $CALICO_VERSION (tigera-operator, Goldmane, Whisker)"
  local base="https://raw.githubusercontent.com/projectcalico/calico/${CALICO_VERSION}/manifests"
  k apply --server-side --force-conflicts -f "$base/operator-crds.yaml" >/dev/null
  k apply --server-side --force-conflicts -f "$base/tigera-operator.yaml" >/dev/null
  k wait --for condition=Established --timeout=60s crd/installations.operator.tigera.io \
    crd/goldmanes.operator.tigera.io crd/whiskers.operator.tigera.io >/dev/null
  k apply --server-side --force-conflicts -f "$base/custom-resources.yaml" >/dev/null
  wait_for 600 "calico-node" k -n calico-system rollout status daemonset/calico-node --timeout=10s
  wait_for 600 "Goldmane" k -n calico-system rollout status deployment/goldmane --timeout=10s
  wait_for 600 "Whisker" k -n calico-system rollout status deployment/whisker --timeout=10s
  # Calico's projectcalico.org/v3 API (for web-guard) is served by its API server.
  wait_for 600 "the Calico API server" k get networkpolicies.projectcalico.org -A
}

install_netobserv() {
  log "Installing cert-manager $CERT_MANAGER_VERSION and trust-manager $TRUST_MANAGER_VERSION"
  # dev-cluster.sh applied cert-manager's CRDs already.
  h upgrade --install cert-manager cert-manager --repo https://charts.jetstack.io \
    --version "$CERT_MANAGER_VERSION" --namespace cert-manager --create-namespace \
    --set crds.enabled=false --wait --timeout 5m >/dev/null
  h upgrade --install trust-manager oci://quay.io/jetstack/charts/trust-manager \
    --version "$TRUST_MANAGER_VERSION" --namespace cert-manager --wait --timeout 5m >/dev/null

  # Upgrading the same version again conflicts with the operator's own edits of its RBAC (a
  # failed release of that version is from such a run; its objects are in place).
  if h list -n netobserv --filter '^netobserv$' --deployed --failed -o yaml 2>/dev/null |
    grep -q "chart: netobserv-operator-${NETOBSERV_VERSION}\$"; then
    log "NetObserv $NETOBSERV_VERSION is installed"
  else
    log "Installing NetObserv $NETOBSERV_VERSION with Loki and Prometheus"
    h upgrade --install netobserv netobserv-operator --repo https://netobserv.io/static/helm/ \
      --version "$NETOBSERV_VERSION" --namespace netobserv --create-namespace \
      --set install.loki=true --set install.prom-stack=true \
      --set standaloneConsole.enable=false \
      --wait --timeout 10m >/dev/null
  fi
  k wait --for condition=Established --timeout=60s crd/flowcollectors.flows.netobserv.io >/dev/null
  # The operator's webhook must answer before the FlowCollector can be created.
  wait_for 300 "the FlowCollector webhook" k apply --dry-run=server -f - <<'EOF'
apiVersion: flows.netobserv.io/v1beta2
kind: FlowCollector
metadata:
  name: cluster
spec: {}
EOF
  k apply -f - >/dev/null <<'EOF'
apiVersion: flows.netobserv.io/v1beta2
kind: FlowCollector
metadata:
  name: cluster
spec:
  namespace: netobserv
  agent:
    ebpf:
      # Every packet (the default samples 1 in 50), with drops and DNS.
      sampling: 1
      privileged: true
      features: [PacketDrop, DNSTracking, FlowRTT]
  processor:
    service:
      tlsType: Auto-mTLS
  loki:
    enable: true
    mode: Monolithic
    monolithic:
      url: 'http://netobserv-loki.netobserv.svc.cluster.local.:3100/'
  prometheus:
    querier:
      mode: Manual
      manual:
        url: http://netobserv-prom-stack-prometheus.netobserv.svc.cluster.local.:9090/
  consolePlugin:
    enable: false
EOF
  wait_for 600 "the NetObserv agents" k -n netobserv-privileged rollout status daemonset/netobserv-ebpf-agent --timeout=10s
  # A Deployment since NetObserv 2.0 (a DaemonSet before).
  wait_for 600 "flowlogs-pipeline" flowlogs_pipeline_ready
}

flowlogs_pipeline_ready() {
  if k -n netobserv get deployment/flowlogs-pipeline >/dev/null 2>&1; then
    k -n netobserv rollout status deployment/flowlogs-pipeline --timeout=10s
  else
    k -n netobserv rollout status daemonset/flowlogs-pipeline --timeout=10s
  fi
}

# ----- Traffic -----

apply_payments() {
  log "Applying the payments workloads of dev-cluster.sh"
  KUBECONFIG="$KCFG" KUBYL_DEV_CLUSTER="$CLUSTER" "$SCRIPT_DIR/dev-cluster.sh" >/dev/null
}

apply_traffic() {
  log "Applying the traffic workloads (payments, storefront)"
  local shoppers=1
  $BUSY && shoppers=12
  k apply -f - >/dev/null <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: storefront
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ledger-api
  namespace: payments
  labels: { app: ledger-api, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: ledger-api }
  template:
    metadata:
      labels: { app: ledger-api, team: payments }
    spec:
      containers:
        - name: api
          image: ${WEB_IMAGE}
          ports: [{ name: http, containerPort: 80 }]
          resources:
            requests: { cpu: 10m, memory: 16Mi }
---
apiVersion: v1
kind: Service
metadata:
  name: ledger-api
  namespace: payments
spec:
  selector: { app: ledger-api }
  ports: [{ name: http, port: 80, targetPort: http }]
---
# Isolates ledger-api: only checkout-client may call it. Nothing denies the other callers,
# nothing allows them either.
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: ledger-api-isolation
  namespace: payments
spec:
  podSelector:
    matchLabels: { app: ledger-api }
  policyTypes: [Ingress]
  ingress:
    - from:
        - podSelector:
            matchLabels: { app: checkout-client }
      ports: [{ port: 80 }]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkout-client
  namespace: payments
  labels: { app: checkout-client, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: checkout-client }
  template:
    metadata:
      labels: { app: checkout-client, team: payments }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: client
          image: ${CLIENT_IMAGE}
          resources:
            requests: { cpu: 5m, memory: 8Mi }
          command:
            - sh
            - -c
            - |
              while true; do
                wget -q -T 2 -O /dev/null http://checkout-api/ || true
                wget -q -T 2 -O /dev/null http://ledger-api/ || true
                sleep 1
              done
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: storefront
  labels: { app: web, team: storefront }
spec:
  replicas: 2
  selector:
    matchLabels: { app: web }
  template:
    metadata:
      labels: { app: web, team: storefront }
    spec:
      containers:
        - name: web
          image: ${WEB_IMAGE}
          ports: [{ name: http, containerPort: 80 }]
          resources:
            requests: { cpu: 10m, memory: 16Mi }
---
apiVersion: v1
kind: Service
metadata:
  name: web
  namespace: storefront
spec:
  selector: { app: web }
  ports: [{ name: http, port: 80, targetPort: http }]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: shopper
  namespace: storefront
  labels: { app: shopper, team: storefront }
spec:
  replicas: ${shoppers}
  selector:
    matchLabels: { app: shopper }
  template:
    metadata:
      labels: { app: shopper, team: storefront }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: shopper
          image: ${CLIENT_IMAGE}
          resources:
            requests: { cpu: 5m, memory: 8Mi }
          command:
            - sh
            - -c
            - |
              while true; do
                wget -q -T 2 -O /dev/null http://web/ || true
                # A query value Kubyl must not show (a dummy, never a real token).
                wget -q -T 2 -O /dev/null "http://web/search?q=shoes&token=not-a-real-token" || true
                wget -q -T 2 -O /dev/null http://checkout-api.payments/ || true
                wget -q -T 2 -O /dev/null http://ledger-api.payments/ || true
                sleep 1
              done
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: scraper
  namespace: storefront
  labels: { app: scraper, team: storefront }
spec:
  replicas: 1
  selector:
    matchLabels: { app: scraper }
  template:
    metadata:
      labels: { app: scraper, team: storefront }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: scraper
          image: ${CLIENT_IMAGE}
          resources:
            requests: { cpu: 5m, memory: 8Mi }
          command:
            - sh
            - -c
            - |
              while true; do
                wget -q -T 2 -O /dev/null http://web/ || true
                sleep 2
              done
EOF
}

apply_deny_rule() {
  case "$MODE" in
    cilium)
      log "Applying web-guard (CiliumNetworkPolicy: ingressDeny from scraper, L7 visibility)"
      k apply -f - >/dev/null <<'EOF'
apiVersion: cilium.io/v2
kind: CiliumNetworkPolicy
metadata:
  name: web-guard
  namespace: storefront
spec:
  endpointSelector:
    matchLabels: { app: web }
  ingress:
    # Everyone in storefront, through Envoy so Hubble sees HTTP (paths, not bodies).
    - fromEndpoints:
        - {}
      toPorts:
        - ports: [{ port: "80", protocol: TCP }]
          rules:
            http: [{}]
  ingressDeny:
    - fromEndpoints:
        - matchLabels: { app: scraper }
EOF
      ;;
    calico)
      log "Applying web-guard (Calico NetworkPolicy: Deny from scraper, then Allow)"
      k apply -f - >/dev/null <<'EOF'
apiVersion: projectcalico.org/v3
kind: NetworkPolicy
metadata:
  name: web-guard
  namespace: storefront
spec:
  selector: app == 'web'
  types: [Ingress]
  ingress:
    - action: Deny
      source:
        selector: app == 'scraper'
    - action: Allow
EOF
      ;;
    netobserv)
      log "kindnet has no deny rules: no web-guard on this cluster"
      ;;
  esac
}

# ----- Main -----

create_cluster
case "$MODE" in
  cilium) install_cilium ;;
  calico) install_calico ;;
  netobserv) ;;
esac
wait_for 300 "the nodes" k wait --for condition=Ready nodes --all --timeout=10s
apply_payments
[[ "$MODE" == netobserv ]] && install_netobserv
apply_traffic
apply_deny_rule
k -n storefront rollout status deployment/shopper --timeout=180s >/dev/null
k -n payments rollout status deployment/checkout-client --timeout=180s >/dev/null

log "Done. KUBECONFIG=$KCFG (context $CONTEXT)"
