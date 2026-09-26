#!/usr/bin/env bash
# Clusters for cluster updates and OpenShift Routes (phase 13).
#
# 1. The kind cluster from script/dev-cluster.sh (context kind-kubyl-dev) gets a namespace
#    `kubyl-updates` with
#    - Deployment `ledger-writer` (2 replicas) and PodDisruptionBudget `ledger-writer-pdb`
#      (minAvailable 2, so 0 allowed disruptions: it blocks node drains);
#    - a Helm release `legacy-app` (revision 1, deployed): only its release Secret, nothing of it
#      runs. Its manifest uses removed APIs (extensions/v1beta1 Ingress, policy/v1beta1
#      PodDisruptionBudget, batch/v1beta1 CronJob, autoscaling/v2beta2 HorizontalPodAutoscaler,
#      flowcontrol.apiserver.k8s.io/v1beta3 FlowSchema) next to an apps/v1 Deployment.
#    It also requests the deprecated resources Kubernetes v1.37 still serves (v1 ComponentStatus
#    and v1 Endpoints), so `apiserver_requested_deprecated_apis` (the API server's /metrics, and
#    Prometheus from prometheus-dev.sh) has series for them, with an empty removed_release.
#    v1.37 serves no deprecated group-version (beta APIs are off by default, and all the ones in
#    the removal list are gone), so series with a removed_release can't be produced on kind: test
#    those through the APIRequestCounts of the fake OpenShift cluster.
# 2. A second kind cluster `kubyl-ocp` (1 control plane, 2 workers) that looks like OpenShift
#    4.17 to Kubyl: the real openshift/api CRDs (ClusterVersion, ClusterOperator, Route,
#    MachineConfigPool, APIRequestCount; the Default feature set of release-4.17) and
#    - ClusterVersion `version` at 4.17.8 on stable-4.17 with 4 available and 2 conditional
#      updates (4.17.13, 4.18.2; one risk each), five history entries and an admin gate
#      (Upgradeable=False, AdminAckRequired; `openshift-config-managed/admin-gates`,
#      `openshift-config/admin-acks`);
#    - 33 ClusterOperators, MachineConfigPools `master` and `worker` (the nodes carry the
#      node-role labels and MCO's annotations), APIRequestCounts (deprecated flowcontrol v1beta3
#      and autoscaling v2beta2 with users, and two current APIs);
#    - Routes in `shop` with real backends (nginx): `shop` (edge, Redirect, 80/20 with
#      shop-canary), `shop-api` (a numeric target port), `shop-secure` (reencrypt with an inline
#      example certificate and key, generated at run time), `shop-passthrough`, `shop-wildcard`
#      (Subdomain), `shop-conflict` (not admitted: HostAlreadyClaimed) and `shop-ingress-x7k2p`,
#      generated from the Ingress `shop-ingress` like OpenShift's ingress-to-route controller.
#    Nothing of OpenShift runs: script/updates-dev/fake_ocp.py writes the status an OpenShift
#    cluster would report. The ClusterVersion CRD gets one addition,
#    spec.desiredUpdate.acceptRisks (newer OpenShift, TechPreview gate ClusterUpdateAcceptRisks;
#    4.17's schema would prune it), so the fake CVO can take accepted risks. Real 4.17 CVOs only
#    warn about a not-recommended target (`oc adm upgrade --allow-not-recommended` is the gate).
# 3. With --k3s: a k3d cluster `kubyl-k3s` (1 server, 1 agent, no load balancer, $K3S_IMAGE:
#    one patch behind its minor's latest) with system-upgrade-controller and two Plans in
#    `system-upgrade`, both pinned to the running version: `server-plan` (control-plane nodes,
#    cordon) and `agent-plan` (the other nodes; its prepare step waits for server-plan, then it
#    drains). Nodes carry the opt-in label k3s-upgrade=true of system-upgrade-controller's k3s
#    example. The plans' first run restarts k3s once per node: the image's /bin/k3s isn't
#    byte-identical to the release binary in rancher/k3s-upgrade, so the job swaps in the same
#    version. After that nothing happens until someone edits spec.version (both plans): the
#    controller cordons the server, runs rancher/k3s-upgrade:<version> in a privileged job that
#    replaces /bin/k3s in the k3d node container and restarts k3s (the container restarts),
#    uncordons it; the agent-plan job's prepare step waits for server-plan, then it drains the
#    agent and does the same there (`kubectl -n system-upgrade get plans,jobs`).
#
# Usage:
#   script/updates-dev.sh                     1 and 2 (updates what exists)
#   script/updates-dev.sh --dev-only          only 1
#   script/updates-dev.sh --ocp-only          only 2
#   script/updates-dev.sh --k3s               only 3 (--all: 1, 2 and 3)
#   script/updates-dev.sh --ocp-stage <stage> a fixed update state on kubyl-ocp: idle (4.17.8),
#                                             started (12%), operators (61%, three operators
#                                             progressing), nodes (the worker pool at 1 of 2, a
#                                             worker cordoned and "draining"), done (4.17.12)
#   script/updates-dev.sh --ocp-reset         the same as --ocp-stage idle
#   script/updates-dev.sh --ocp-update        idle, then the stages to 4.17.12, $STAGE_SECONDS
#                                             (15) apart, for recordings
#   script/updates-dev.sh --fake-cvo          a fake cluster-version operator in the foreground
#                                             (Ctrl-C stops): follows spec.channel (stable-4.18
#                                             recommends 4.18.2, fast-4.17 4.17.13, unknown
#                                             channels show VersionNotFound), the admin acks, and
#                                             plays an update when spec.desiredUpdate asks for
#                                             a recommended version, a conditional one whose
#                                             risks are all in acceptRisks, or force is set
#   script/updates-dev.sh --delete            remove what the script created (with --dev-only,
#                                             --ocp-only or --k3s only that part)
#
# Point Kubyl at the scratch kubeconfigs (never ~/.kube/config):
#   KUBECONFIG=/tmp/kubyl-dev/ocp-kubeconfig cargo run -p kubyl
#   KUBECONFIG=/tmp/kubyl-dev/kubeconfig:/tmp/kubyl-dev/ocp-kubeconfig:/tmp/kubyl-dev/k3s-kubeconfig cargo run -p kubyl
# and compare with `oc --kubeconfig /tmp/kubyl-dev/ocp-kubeconfig adm upgrade`. With --fake-cvo
# running, `oc --kubeconfig /tmp/kubyl-dev/ocp-kubeconfig adm upgrade --to=4.17.10` updates it
# (then --ocp-reset).
#
# Everything is marked with kubyl.dev/installed-by=updates-dev.sh, and --delete removes only
# what carries it: namespaces on kind-kubyl-dev and kube-system of kubyl-ocp (annotation), the
# node containers of kubyl-k3s (Docker label).
#
# Needs: docker (running), kind, kubectl, python3, curl, openssl; k3d for --k3s. Respects
# $KUBECONFIG (a single file with context kind-kubyl-dev; default /tmp/kubyl-dev/kubeconfig),
# $OCP_KUBECONFIG, $K3S_KUBECONFIG, $K3S_IMAGE, $SUC_VERSION, $OPENSHIFT_API_COMMIT and
# $STAGE_SECONDS.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FAKE_OCP="$SCRIPT_DIR/updates-dev/fake_ocp.py"

DEV_CONTEXT="kind-kubyl-dev"
DEV_KUBECONFIG="${KUBECONFIG:-/tmp/kubyl-dev/kubeconfig}"
DEV_NAMESPACE="kubyl-updates"

OCP_CLUSTER="kubyl-ocp"
OCP_CONTEXT="kind-${OCP_CLUSTER}"
OCP_KUBECONFIG="${OCP_KUBECONFIG:-/tmp/kubyl-dev/ocp-kubeconfig}"
# openshift/api, release-4.17 as of 2026-09-26.
OPENSHIFT_API_COMMIT="${OPENSHIFT_API_COMMIT:-92c55bf0f372c222dc02f17685c383d18e2c2e8b}"
ROUTER_HOST="router-default.apps.ocp-dev.example.com"
SHOP_IMAGE="nginxinc/nginx-unprivileged:1.29-alpine"

K3S_CLUSTER="kubyl-k3s"
K3S_CONTEXT="k3d-${K3S_CLUSTER}"
K3S_KUBECONFIG="${K3S_KUBECONFIG:-/tmp/kubyl-dev/k3s-kubeconfig}"
K3S_IMAGE="${K3S_IMAGE:-rancher/k3s:v1.36.3-k3s1}"
SUC_VERSION="${SUC_VERSION:-v0.20.2}"
SUC_URL="https://github.com/rancher/system-upgrade-controller/releases/download/${SUC_VERSION}"

STAGE_SECONDS="${STAGE_SECONDS:-15}"
INSTALLED_BY="kubyl.dev/installed-by"
MARK="updates-dev.sh"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in kubectl python3; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done
[[ "$DEV_KUBECONFIG" != *:* ]] || die "KUBECONFIG lists several files; set it to the one with context $DEV_CONTEXT"

# Every kubectl call names its kubeconfig and context; the current context is never used.
kd() { kubectl --kubeconfig "$DEV_KUBECONFIG" --context "$DEV_CONTEXT" "$@"; }
ko() { kubectl --kubeconfig "$OCP_KUBECONFIG" --context "$OCP_CONTEXT" "$@"; }
kk() { kubectl --kubeconfig "$K3S_KUBECONFIG" --context "$K3S_CONTEXT" "$@"; }
fake_ocp() { python3 "$FAKE_OCP" --kubeconfig "$OCP_KUBECONFIG" --context "$OCP_CONTEXT" "$@"; }

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

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

# marked <kubectl function> <namespace>: whether the namespace carries this script's mark.
marked() {
  [[ "$("$1" get namespace "$2" -o jsonpath='{.metadata.annotations.kubyl\.dev/installed-by}' 2>/dev/null || true)" == "$MARK" ]]
}

# ensure_namespace <kubectl function> <namespace>: creates it with the mark. One that exists
# without the mark isn't adopted (--delete would remove it).
ensure_namespace() {
  if "$1" get namespace "$2" >/dev/null 2>&1; then
    marked "$1" "$2" || die "namespace $2 exists but wasn't created by this script; remove it first"
    return
  fi
  "$1" create -f - >/dev/null <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: $2
  annotations:
    $INSTALLED_BY: $MARK
EOF
}

# The kube-system mark of a cluster, read through a kubeconfig the cluster tool prints
# (so it works without the scratch kubeconfig).
cluster_marked() {
  local config="$tmp/check-kubeconfig" context="$1"
  shift
  "$@" >"$config" 2>/dev/null || return 1
  [[ "$(kubectl --kubeconfig "$config" --context "$context" get namespace kube-system \
    -o jsonpath='{.metadata.annotations.kubyl\.dev/installed-by}' 2>/dev/null || true)" == "$MARK" ]]
}

ocp_exists() { kind get clusters 2>/dev/null | grep -qx "$OCP_CLUSTER"; }
k3s_exists() { k3d cluster get "$K3S_CLUSTER" >/dev/null 2>&1; }
# The k3d cluster's mark is a Docker label on its node containers (readable while k3s is down).
k3s_labelled() {
  [[ "$(docker inspect -f "{{ index .Config.Labels \"$INSTALLED_BY\" }}" "k3d-${K3S_CLUSTER}-server-0" 2>/dev/null || true)" == "$MARK" ]]
}

# ----- Arguments -----

DEV=false OCP=false K3S=false PARTS_GIVEN=false MODE=setup STAGE=""
while (($#)); do
  case "$1" in
    --dev-only) DEV=true PARTS_GIVEN=true ;;
    --ocp-only) OCP=true PARTS_GIVEN=true ;;
    --k3s) K3S=true PARTS_GIVEN=true ;;
    --all) DEV=true OCP=true K3S=true PARTS_GIVEN=true ;;
    --ocp-stage)
      MODE=stage STAGE="${2:-}"
      shift || true
      case "$STAGE" in
        idle | started | operators | nodes | done) ;;
        *) die "--ocp-stage needs one of idle, started, operators, nodes, done" ;;
      esac
      ;;
    --ocp-reset) MODE=stage STAGE=idle ;;
    --ocp-update) MODE=update ;;
    --fake-cvo) MODE=cvo ;;
    --delete) MODE=delete ;;
    -h | --help)
      sed -n '2,/^set -euo/p' "${BASH_SOURCE[0]}" | sed -e '$d' -e 's/^# \{0,1\}//'
      exit 0
      ;;
    *) die "unknown argument: $1 (see --help)" ;;
  esac
  shift
done
if ! $PARTS_GIVEN; then
  DEV=true OCP=true
  [[ "$MODE" == delete ]] && K3S=true
fi

# ----- --delete -----

delete_dev() {
  if ! kd get --raw /version >/dev/null 2>&1; then
    warn "context $DEV_CONTEXT isn't reachable: nothing removed there"
    return
  fi
  local ns found=false
  for ns in $(kd get namespaces -o jsonpath='{range .items[*]}{.metadata.name}{"\t"}{.metadata.annotations.kubyl\.dev/installed-by}{"\n"}{end}' |
    awk -F'\t' -v mark="$MARK" '$2 == mark { print $1 }'); do
    marked kd "$ns" || continue
    log "Deleting namespace $ns on $DEV_CONTEXT"
    kd delete namespace "$ns" --wait=false >/dev/null
    found=true
  done
  $found || log "No namespace of this script on $DEV_CONTEXT"
}

delete_ocp() {
  command -v kind >/dev/null 2>&1 || { warn "kind is not installed: $OCP_CLUSTER not checked"; return; }
  if ! ocp_exists; then
    log "kind cluster $OCP_CLUSTER doesn't exist"
    return
  fi
  if ! cluster_marked "$OCP_CONTEXT" kind get kubeconfig --name "$OCP_CLUSTER"; then
    warn "kind cluster $OCP_CLUSTER isn't reachable or wasn't created by this script (no $INSTALLED_BY=$MARK on kube-system): not deleting it"
    return
  fi
  log "Deleting kind cluster $OCP_CLUSTER"
  KUBECONFIG="$OCP_KUBECONFIG" kind delete cluster --name "$OCP_CLUSTER" --kubeconfig "$OCP_KUBECONFIG"
}

delete_k3s() {
  command -v k3d >/dev/null 2>&1 || { warn "k3d is not installed: $K3S_CLUSTER not checked"; return; }
  if ! k3s_exists; then
    log "k3d cluster $K3S_CLUSTER doesn't exist"
    return
  fi
  if ! k3s_labelled; then
    warn "k3d cluster $K3S_CLUSTER wasn't created by this script (its nodes lack the label $INSTALLED_BY=$MARK): not deleting it"
    return
  fi
  log "Deleting k3d cluster $K3S_CLUSTER"
  # k3d removes the cluster from the kubeconfig in $KUBECONFIG: point it at the scratch file.
  mkdir -p "$(dirname "$K3S_KUBECONFIG")"
  touch "$K3S_KUBECONFIG"
  KUBECONFIG="$K3S_KUBECONFIG" k3d cluster delete "$K3S_CLUSTER"
}

case "$MODE" in
  delete)
    ! $DEV || delete_dev
    ! $OCP || delete_ocp
    ! $K3S || delete_k3s
    log "Done"
    exit 0
    ;;
  stage)
    fake_ocp stage "$STAGE"
    exit 0
    ;;
  update)
    fake_ocp update --stage-seconds "$STAGE_SECONDS"
    exit 0
    ;;
  cvo)
    exec python3 "$FAKE_OCP" --kubeconfig "$OCP_KUBECONFIG" --context "$OCP_CONTEXT" cvo --stage-seconds "$STAGE_SECONDS"
    ;;
esac

# ----- 1. The dev cluster -----

# The Secret Helm 3 stores a release in: data.release is base64(gzip(release JSON)), and the API
# adds its own base64 on top (so the Secret's data holds it base64-encoded twice).
helm_release_secret() {
  python3 - "$DEV_NAMESPACE" <<'PY'
import base64, gzip, json, sys

namespace = sys.argv[1]
labels = """  labels:
    app.kubernetes.io/name: legacy-app
    app.kubernetes.io/instance: legacy-app
    app.kubernetes.io/version: "1.4.0"
    app.kubernetes.io/managed-by: Helm
    helm.sh/chart: legacy-app-0.3.1"""
selector = """    matchLabels:
      app.kubernetes.io/name: legacy-app
      app.kubernetes.io/instance: legacy-app"""
templates = [
    ("legacy-app/templates/pdb.yaml", f"""apiVersion: policy/v1beta1
kind: PodDisruptionBudget
metadata:
  name: legacy-app
{labels}
spec:
  minAvailable: 1
  selector:
{selector}"""),
    ("legacy-app/templates/deployment.yaml", f"""apiVersion: apps/v1
kind: Deployment
metadata:
  name: legacy-app
{labels}
spec:
  replicas: 1
  selector:
{selector}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: legacy-app
        app.kubernetes.io/instance: legacy-app
    spec:
      containers:
        - name: legacy-app
          image: "registry.example.com/legacy-app:1.4.0"
          ports:
            - name: http
              containerPort: 8080"""),
    ("legacy-app/templates/hpa.yaml", f"""apiVersion: autoscaling/v2beta2
kind: HorizontalPodAutoscaler
metadata:
  name: legacy-app
{labels}
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: legacy-app
  minReplicas: 1
  maxReplicas: 4
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: 80"""),
    ("legacy-app/templates/cronjob.yaml", f"""apiVersion: batch/v1beta1
kind: CronJob
metadata:
  name: legacy-app-cleanup
{labels}
spec:
  schedule: "0 3 * * *"
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: OnFailure
          containers:
            - name: cleanup
              image: "registry.example.com/legacy-app:1.4.0"
              args: ["cleanup", "--older-than=30d"]"""),
    ("legacy-app/templates/ingress.yaml", f"""apiVersion: extensions/v1beta1
kind: Ingress
metadata:
  name: legacy-app
{labels}
spec:
  rules:
    - host: legacy-app.example.com
      http:
        paths:
          - path: /
            backend:
              serviceName: legacy-app
              servicePort: 80"""),
    ("legacy-app/templates/flowschema.yaml", f"""apiVersion: flowcontrol.apiserver.k8s.io/v1beta3
kind: FlowSchema
metadata:
  name: legacy-app
{labels}
spec:
  priorityLevelConfiguration:
    name: workload-low
  matchingPrecedence: 1000
  distinguisherMethod:
    type: ByUser
  rules:
    - subjects:
        - kind: ServiceAccount
          serviceAccount:
            name: legacy-app
            namespace: {namespace}
      resourceRules:
        - verbs: ["*"]
          apiGroups: ["*"]
          resources: ["*"]
          namespaces: ["*"]"""),
]
# Helm writes each rendered template as "---\n# Source: <path>\n<content>\n".
manifest = "".join(f"---\n# Source: {name}\n{content}\n" for name, content in templates)
deployed = "2024-03-14T10:21:07.482913+01:00"
release = {
    "name": "legacy-app",
    "info": {
        "first_deployed": deployed,
        "last_deployed": deployed,
        "deleted": "",
        "description": "Install complete",
        "status": "deployed",
        "notes": "legacy-app 1.4.0 is running at http://legacy-app.example.com/\n",
    },
    "chart": {
        "metadata": {
            "name": "legacy-app",
            "version": "0.3.1",
            "description": "A legacy app whose chart still uses APIs removed from Kubernetes",
            "apiVersion": "v2",
            "appVersion": "1.4.0",
            "type": "application",
        },
        "lock": None,
        "templates": [
            {"name": name.split("/", 1)[1], "data": base64.b64encode(content.encode()).decode()}
            for name, content in templates
        ],
        "values": {"replicaCount": 1, "image": {"repository": "registry.example.com/legacy-app",
                                                "tag": "1.4.0"}},
        "schema": None,
        "files": [],
    },
    "config": {},
    "manifest": manifest,
    "version": 1,
    "namespace": namespace,
}
stored = base64.b64encode(gzip.compress(json.dumps(release).encode(), mtime=0))
secret = {
    "apiVersion": "v1",
    "kind": "Secret",
    "type": "helm.sh/release.v1",
    "metadata": {
        "name": "sh.helm.release.v1.legacy-app.v1",
        "namespace": namespace,
        "labels": {"owner": "helm", "name": "legacy-app", "status": "deployed", "version": "1",
                   "modifiedAt": "1710408067"},
    },
    "data": {"release": base64.b64encode(stored).decode()},
}
print(json.dumps(secret))
PY
}

pdb_healthy() {
  [[ "$(kd -n "$DEV_NAMESPACE" get pdb ledger-writer-pdb -o jsonpath='{.status.currentHealthy}')" == 2 ]]
}

setup_dev() {
  kd get --raw /version >/dev/null 2>&1 \
    || die "context $DEV_CONTEXT isn't reachable through $DEV_KUBECONFIG (run script/dev-cluster.sh first)"
  log "Namespace $DEV_NAMESPACE on $DEV_CONTEXT: ledger-writer with a blocking PDB"
  ensure_namespace kd "$DEV_NAMESPACE"
  kd apply -f - >/dev/null <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: ledger-writer
  namespace: $DEV_NAMESPACE
  labels: { app: ledger-writer }
spec:
  replicas: 2
  selector:
    matchLabels: { app: ledger-writer }
  template:
    metadata:
      labels: { app: ledger-writer }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: writer
          image: busybox:1.37
          command: ["sleep", "infinity"]
          resources:
            requests: { cpu: 5m, memory: 8Mi }
---
# minAvailable equals the replicas: no pod may be evicted, so a drain blocks.
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: ledger-writer-pdb
  namespace: $DEV_NAMESPACE
spec:
  minAvailable: 2
  selector:
    matchLabels: { app: ledger-writer }
EOF

  log "Helm release $DEV_NAMESPACE/legacy-app (only its release Secret, with removed APIs)"
  helm_release_secret | kd apply --server-side --force-conflicts --field-manager="$MARK" -f - >/dev/null

  kd -n "$DEV_NAMESPACE" rollout status deployment/ledger-writer --timeout=180s >/dev/null
  wait_for 60 "ledger-writer-pdb to see 2 healthy pods" pdb_healthy

  log "Requesting the deprecated APIs v1.37 serves (v1 ComponentStatus, v1 Endpoints)"
  kd get --raw /api/v1/componentstatuses >/dev/null 2>&1 || true
  kd get --raw "/api/v1/namespaces/$DEV_NAMESPACE/endpoints" >/dev/null 2>&1 || true
  kd get --raw /metrics 2>/dev/null | grep '^apiserver_requested_deprecated_apis' || warn "no apiserver_requested_deprecated_apis series (is /metrics readable?)"
  kd -n "$DEV_NAMESPACE" get pdb ledger-writer-pdb
}

# ----- 2. The fake OpenShift cluster -----

OCP_CRDS="
config/v1/zz_generated.crd-manifests/0000_00_cluster-version-operator_01_clusterversions-Default.crd.yaml
config/v1/zz_generated.crd-manifests/0000_00_cluster-version-operator_01_clusteroperators.crd.yaml
route/v1/zz_generated.crd-manifests/routes-Default.crd.yaml
machineconfiguration/v1/zz_generated.crd-manifests/0000_80_machine-config_01_machineconfigpools-Default.crd.yaml
apiserver/v1/zz_generated.crd-manifests/kube-apiserver_apirequestcounts.crd.yaml
"

# spec.desiredUpdate.acceptRisks as in openshift/api's newer ClusterVersion (feature gate
# ClusterUpdateAcceptRisks); release-4.17's schema doesn't have it and would prune it.
ACCEPT_RISKS_PATCH='[{"op":"add","path":"/spec/versions/0/schema/openAPIV3Schema/properties/spec/properties/desiredUpdate/properties/acceptRisks","value":{"description":"acceptRisks is an optional set of names of conditional update risks that are considered acceptable. (Added by script/updates-dev.sh: TechPreview in newer OpenShift.)","type":"array","minItems":1,"maxItems":1000,"x-kubernetes-list-type":"map","x-kubernetes-list-map-keys":["name"],"items":{"type":"object","required":["name"],"properties":{"name":{"type":"string","minLength":1,"maxLength":256}}}}}]'

# Indents a PEM file for a YAML block scalar at the Route's spec.tls level.
pem() { sed 's/^/      /' "$1"; }

# route_status <name> <host> <wildcard policy> [HostAlreadyClaimed message]: what the default
# router would write.
route_status() {
  local condition now
  now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  if [[ -n "${4:-}" ]]; then
    condition="{\"type\":\"Admitted\",\"status\":\"False\",\"reason\":\"HostAlreadyClaimed\",\"message\":\"$4\",\"lastTransitionTime\":\"$now\"}"
  else
    condition="{\"type\":\"Admitted\",\"status\":\"True\",\"lastTransitionTime\":\"$now\"}"
  fi
  ko -n shop patch route "$1" --subresource=status --type=merge \
    -p "{\"status\":{\"ingress\":[{\"host\":\"$2\",\"routerName\":\"default\",\"routerCanonicalHostname\":\"$ROUTER_HOST\",\"wildcardPolicy\":\"$3\",\"conditions\":[$condition]}]}}" >/dev/null
}

setup_ocp() {
  for tool in docker kind curl openssl; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
  done
  docker info >/dev/null 2>&1 || die "docker is not running"
  mkdir -p "$(dirname "$OCP_KUBECONFIG")"

  if ocp_exists; then
    log "kind cluster $OCP_CLUSTER exists; updating it"
    ko get --raw /version >/dev/null 2>&1 \
      || KUBECONFIG="$OCP_KUBECONFIG" kind export kubeconfig --name "$OCP_CLUSTER" --kubeconfig "$OCP_KUBECONFIG" >/dev/null 2>&1
    marked ko kube-system || die "kind cluster $OCP_CLUSTER wasn't created by this script (no $INSTALLED_BY on kube-system)"
  else
    log "Creating kind cluster $OCP_CLUSTER (kubeconfig $OCP_KUBECONFIG)"
    KUBECONFIG="$OCP_KUBECONFIG" kind create cluster --name "$OCP_CLUSTER" --kubeconfig "$OCP_KUBECONFIG" \
      --wait 120s --config - <<'EOF'
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
  - role: worker
  - role: worker
EOF
    # The whole cluster is this script's: --delete checks this mark.
    ko annotate namespace kube-system --overwrite "$INSTALLED_BY=$MARK" >/dev/null
  fi

  log "Installing the openshift/api CRDs (${OPENSHIFT_API_COMMIT:0:12}, Default feature set)"
  mkdir -p "$tmp/crds"
  local path
  for path in $OCP_CRDS; do
    curl -fsSL "https://raw.githubusercontent.com/openshift/api/${OPENSHIFT_API_COMMIT}/${path}" -o "$tmp/crds/$(basename "$path")" \
      || die "couldn't download $path from openshift/api"
  done
  ko apply --server-side --force-conflicts --field-manager="$MARK" -f "$tmp/crds" >/dev/null
  ko patch crd clusterversions.config.openshift.io --type=json -p "$ACCEPT_RISKS_PATCH" >/dev/null
  ko wait --for condition=Established --timeout=60s \
    crd/clusterversions.config.openshift.io crd/clusteroperators.config.openshift.io \
    crd/routes.route.openshift.io crd/machineconfigpools.machineconfiguration.openshift.io \
    crd/apirequestcounts.apiserver.openshift.io >/dev/null

  log "Labelling the nodes like OpenShift's (master, worker)"
  local node
  for node in $(ko get nodes -l node-role.kubernetes.io/control-plane -o name); do
    ko label "$node" --overwrite node-role.kubernetes.io/master= >/dev/null
  done
  for node in $(ko get nodes -l '!node-role.kubernetes.io/control-plane' -o name); do
    ko label "$node" --overwrite node-role.kubernetes.io/worker= >/dev/null
  done

  log "ClusterVersion, ClusterOperators, MachineConfigPools, APIRequestCounts, admin gates"
  fake_ocp setup

  setup_shop
}

setup_shop() {
  log "Namespace shop: backends (nginx) and Routes"
  ensure_namespace ko shop

  # The backend certificate of shop-tls (self-signed), kept across runs.
  if ko -n shop get secret shop-tls >/dev/null 2>&1; then
    ko -n shop get secret shop-tls -o jsonpath='{.data.tls\.crt}' | base64 --decode >"$tmp/backend.crt"
  else
    openssl req -x509 -newkey rsa:2048 -nodes -days 825 -keyout "$tmp/backend.key" -out "$tmp/backend.crt" \
      -subj "/O=Kubyl dev/CN=shop-tls.shop.svc" \
      -addext "subjectAltName=DNS:shop-tls,DNS:shop-tls.shop.svc,DNS:shop-tls.shop.svc.cluster.local" >/dev/null 2>&1
    ko -n shop create secret tls shop-tls --cert="$tmp/backend.crt" --key="$tmp/backend.key" >/dev/null
  fi

  # The example certificate, key and CA inline in Route shop-secure, kept across runs.
  if [[ "$(ko -n shop get route shop-secure -o jsonpath='{.spec.tls.termination}' 2>/dev/null || true)" == reencrypt ]]; then
    ko -n shop get route shop-secure -o jsonpath='{.spec.tls.certificate}' >"$tmp/route.crt"
    ko -n shop get route shop-secure -o jsonpath='{.spec.tls.key}' >"$tmp/route.key"
    ko -n shop get route shop-secure -o jsonpath='{.spec.tls.caCertificate}' >"$tmp/route-ca.crt"
  else
    openssl req -x509 -newkey rsa:2048 -nodes -days 825 -keyout "$tmp/route-ca.key" -out "$tmp/route-ca.crt" \
      -subj "/O=Kubyl dev/CN=Kubyl example route CA" >/dev/null 2>&1
    openssl req -newkey rsa:2048 -nodes -keyout "$tmp/route.key" -out "$tmp/route.csr" \
      -subj "/O=Kubyl dev/CN=secure.apps.ocp-dev.example.com" >/dev/null 2>&1
    printf 'subjectAltName=DNS:secure.apps.ocp-dev.example.com\n' >"$tmp/route.ext"
    openssl x509 -req -in "$tmp/route.csr" -CA "$tmp/route-ca.crt" -CAkey "$tmp/route-ca.key" -CAcreateserial \
      -days 825 -extfile "$tmp/route.ext" -out "$tmp/route.crt" >/dev/null 2>&1
  fi

  ko apply -f - >/dev/null <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: shop-nginx
  namespace: shop
data:
  web.conf: |
    server {
      listen 8080;
      location / {
        default_type text/html;
        return 200 '<!doctype html><title>shop</title><h1>shop-web</h1><p>Stable backend of Route shop (weight 80), served by $hostname</p>';
      }
    }
  canary.conf: |
    server {
      listen 8080;
      location / {
        default_type text/html;
        return 200 '<!doctype html><title>shop canary</title><h1>shop-canary</h1><p>Canary backend of Route shop (weight 20), served by $hostname</p>';
      }
    }
  api.conf: |
    server {
      listen 8080;
      location / {
        default_type application/json;
        return 200 '{"service":"shop-api","path":"$request_uri","pod":"$hostname"}\n';
      }
    }
    server {
      listen 9100;
      location / {
        default_type text/plain;
        return 200 '# HELP shop_api_requests_total Requests served.\n# TYPE shop_api_requests_total counter\nshop_api_requests_total 42\n';
      }
    }
  tls.conf: |
    server {
      listen 8443 ssl;
      ssl_certificate /tls/tls.crt;
      ssl_certificate_key /tls/tls.key;
      location / {
        default_type text/html;
        return 200 '<!doctype html><title>shop tls</title><h1>shop-tls</h1><p>HTTPS backend of Routes shop-secure (reencrypt) and shop-passthrough, served by $hostname</p>';
      }
    }
EOF
  local name conf port replicas tls_mount tls_volume
  for name in shop-web shop-canary shop-api shop-tls; do
    tls_mount="" tls_volume=""
    case "$name" in
      shop-web) conf=web.conf port="{ name: http, containerPort: 8080 }" replicas=2 ;;
      shop-canary) conf=canary.conf port="{ name: http, containerPort: 8080 }" replicas=1 ;;
      shop-api) conf=api.conf port="{ name: http, containerPort: 8080 }, { name: metrics, containerPort: 9100 }" replicas=1 ;;
      shop-tls)
        conf=tls.conf port="{ name: https, containerPort: 8443 }" replicas=1
        tls_mount="- { name: tls, mountPath: /tls, readOnly: true }"
        tls_volume="- { name: tls, secret: { secretName: shop-tls } }"
        ;;
    esac
    ko apply -f - >/dev/null <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: $name
  namespace: shop
  labels: { app: $name }
spec:
  replicas: $replicas
  selector:
    matchLabels: { app: $name }
  template:
    metadata:
      labels: { app: $name }
    spec:
      containers:
        - name: nginx
          image: $SHOP_IMAGE
          ports: [$port]
          resources:
            requests: { cpu: 5m, memory: 16Mi }
          volumeMounts:
            - { name: conf, mountPath: /etc/nginx/conf.d }
            $tls_mount
      volumes:
        - name: conf
          configMap:
            name: shop-nginx
            items: [{ key: $conf, path: default.conf }]
        $tls_volume
EOF
  done
  ko apply -f - >/dev/null <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: shop-web
  namespace: shop
spec:
  selector: { app: shop-web }
  ports: [{ name: http, port: 80, targetPort: http }]
---
apiVersion: v1
kind: Service
metadata:
  name: shop-canary
  namespace: shop
spec:
  selector: { app: shop-canary }
  ports: [{ name: http, port: 80, targetPort: http }]
---
# Port 80 targets the container port named http (8080): Route shop-api names it as 8080.
apiVersion: v1
kind: Service
metadata:
  name: shop-api
  namespace: shop
spec:
  selector: { app: shop-api }
  ports:
    - { name: http, port: 80, targetPort: http }
    - { name: metrics, port: 9090, targetPort: 9100 }
---
apiVersion: v1
kind: Service
metadata:
  name: shop-tls
  namespace: shop
spec:
  selector: { app: shop-tls }
  ports: [{ name: https, port: 443, targetPort: https }]
---
# Like OpenShift's: Ingresses of this class become Routes (ingress-to-route controller).
apiVersion: networking.k8s.io/v1
kind: IngressClass
metadata:
  name: openshift-default
spec:
  controller: openshift.io/ingress-to-route
  parameters:
    apiGroup: operator.openshift.io
    kind: IngressController
    name: default
    namespace: openshift-ingress-operator
    scope: Namespace
---
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: shop-ingress
  namespace: shop
  labels: { app: shop-web }
spec:
  ingressClassName: openshift-default
  rules:
    - host: ingress.apps.ocp-dev.example.com
      http:
        paths:
          - path: /
            pathType: Prefix
            backend:
              service:
                name: shop-web
                port: { number: 80 }
EOF
  ko -n shop patch ingress shop-ingress --subresource=status --type=merge \
    -p "{\"status\":{\"loadBalancer\":{\"ingress\":[{\"hostname\":\"$ROUTER_HOST\"}]}}}" >/dev/null
  local ingress_uid
  ingress_uid="$(ko -n shop get ingress shop-ingress -o jsonpath='{.metadata.uid}')"

  # Server-side apply: a client-side apply would copy the inline key of shop-secure into the
  # last-applied-configuration annotation.
  ko apply --server-side --force-conflicts --field-manager="$MARK" -f - >/dev/null <<EOF
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop
  namespace: shop
  labels: { app: shop-web }
spec:
  host: shop.apps.ocp-dev.example.com
  path: /
  to: { kind: Service, name: shop-web, weight: 80 }
  alternateBackends:
    - { kind: Service, name: shop-canary, weight: 20 }
  port: { targetPort: http }
  tls:
    termination: edge
    insecureEdgeTerminationPolicy: Redirect
  wildcardPolicy: None
---
# A numeric target port: the Service's targetPort (container port 8080), not its port 80.
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-api
  namespace: shop
  labels: { app: shop-api }
spec:
  host: api.apps.ocp-dev.example.com
  path: /api
  to: { kind: Service, name: shop-api, weight: 100 }
  port: { targetPort: 8080 }
  wildcardPolicy: None
---
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-secure
  namespace: shop
  labels: { app: shop-tls }
spec:
  host: secure.apps.ocp-dev.example.com
  to: { kind: Service, name: shop-tls, weight: 100 }
  port: { targetPort: 8443 }
  tls:
    termination: reencrypt
    insecureEdgeTerminationPolicy: Allow
    certificate: |
$(pem "$tmp/route.crt")
    key: |
$(pem "$tmp/route.key")
    caCertificate: |
$(pem "$tmp/route-ca.crt")
    destinationCACertificate: |
$(pem "$tmp/backend.crt")
  wildcardPolicy: None
---
# No spec.port: the router uses the Service's first port.
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-passthrough
  namespace: shop
  labels: { app: shop-tls }
spec:
  host: passthrough.apps.ocp-dev.example.com
  to: { kind: Service, name: shop-tls, weight: 100 }
  tls:
    termination: passthrough
    insecureEdgeTerminationPolicy: None
  wildcardPolicy: None
---
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-wildcard
  namespace: shop
  labels: { app: shop-web }
spec:
  host: wildcard.apps.ocp-dev.example.com
  to: { kind: Service, name: shop-web, weight: 100 }
  wildcardPolicy: Subdomain
---
# The same host as Route shop, which is older: the router doesn't admit it.
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-conflict
  namespace: shop
  labels: { app: shop-canary }
spec:
  host: shop.apps.ocp-dev.example.com
  path: /
  to: { kind: Service, name: shop-canary, weight: 100 }
  wildcardPolicy: None
---
# What OpenShift's ingress-to-route controller makes of Ingress shop-ingress: the Ingress's
# labels, owned by it, the Service port's name as target port.
apiVersion: route.openshift.io/v1
kind: Route
metadata:
  name: shop-ingress-x7k2p
  namespace: shop
  labels: { app: shop-web }
  ownerReferences:
    - apiVersion: networking.k8s.io/v1
      kind: Ingress
      name: shop-ingress
      uid: $ingress_uid
      controller: true
spec:
  host: ingress.apps.ocp-dev.example.com
  path: /
  to: { kind: Service, name: shop-web, weight: 100 }
  port: { targetPort: http }
  wildcardPolicy: None
EOF
  route_status shop shop.apps.ocp-dev.example.com None
  route_status shop-api api.apps.ocp-dev.example.com None
  route_status shop-secure secure.apps.ocp-dev.example.com None
  route_status shop-passthrough passthrough.apps.ocp-dev.example.com None
  route_status shop-wildcard wildcard.apps.ocp-dev.example.com Subdomain
  route_status shop-conflict shop.apps.ocp-dev.example.com None \
    "route shop already exposes shop.apps.ocp-dev.example.com and is older"
  route_status shop-ingress-x7k2p ingress.apps.ocp-dev.example.com None

  log "Waiting for the shop backends"
  for name in shop-web shop-canary shop-api shop-tls; do
    ko -n shop rollout status "deployment/$name" --timeout=300s >/dev/null
  done
  ko -n shop get routes
}

# ----- 3. k3s with system-upgrade-controller -----

setup_k3s() {
  for tool in docker k3d curl; do
    command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
  done
  docker info >/dev/null 2>&1 || die "docker is not running"
  mkdir -p "$(dirname "$K3S_KUBECONFIG")"

  if k3s_exists; then
    log "k3d cluster $K3S_CLUSTER exists; updating it"
    k3s_labelled || die "k3d cluster $K3S_CLUSTER wasn't created by this script (its nodes lack the label $INSTALLED_BY=$MARK)"
  else
    log "Creating k3d cluster $K3S_CLUSTER ($K3S_IMAGE, 1 server, 1 agent)"
    # KUBECONFIG points at the scratch file, and k3d doesn't touch a default kubeconfig. The
    # node containers carry the mark as a Docker label (--delete checks it). No load balancer:
    # its name would be a --tls-san of the server, see k3s_upgrade_safe.
    KUBECONFIG="$K3S_KUBECONFIG" k3d cluster create "$K3S_CLUSTER" --image "$K3S_IMAGE" \
      --servers 1 --agents 1 --no-lb --k3s-arg '--disable=traefik@server:*' \
      --runtime-label "$INSTALLED_BY=$MARK@server:*;agent:*" \
      --kubeconfig-update-default=false --kubeconfig-switch-context=false --wait --timeout 300s
  fi
  KUBECONFIG="$K3S_KUBECONFIG" k3d kubeconfig get "$K3S_CLUSTER" >"$tmp/k3s-kubeconfig"
  mv "$tmp/k3s-kubeconfig" "$K3S_KUBECONFIG"
  chmod 600 "$K3S_KUBECONFIG"
  wait_for 120 "the k3s API server" kk get --raw /readyz
  kk annotate namespace kube-system --overwrite "$INSTALLED_BY=$MARK" >/dev/null
  k3s_upgrade_safe

  log "Installing system-upgrade-controller $SUC_VERSION"
  kk apply --server-side --force-conflicts --field-manager="$MARK" \
    -f "$SUC_URL/crd.yaml" -f "$SUC_URL/system-upgrade-controller.yaml" >/dev/null
  kk wait --for condition=Established --timeout=60s crd/plans.upgrade.cattle.io >/dev/null
  kk -n system-upgrade rollout status deployment/system-upgrade-controller --timeout=300s >/dev/null

  # Opt-in label of the controller's k3s example; servers already carry
  # node-role.kubernetes.io/control-plane=true, which the plans of the k3s docs select on.
  kk label nodes --all --overwrite k3s-upgrade=true >/dev/null
  local version
  version="$(kk get nodes -l node-role.kubernetes.io/control-plane=true -o jsonpath='{.items[0].status.nodeInfo.kubeletVersion}')"
  [[ -n "$version" ]] || die "no control-plane node found on $K3S_CONTEXT"

  log "Plans server-plan and agent-plan, pinned to the running $version"
  kk apply -f - >/dev/null <<EOF
apiVersion: upgrade.cattle.io/v1
kind: Plan
metadata:
  name: server-plan
  namespace: system-upgrade
  labels: { k3s-upgrade: server }
spec:
  concurrency: 1
  cordon: true
  nodeSelector:
    matchExpressions:
      - { key: k3s-upgrade, operator: In, values: ["true"] }
      - { key: node-role.kubernetes.io/control-plane, operator: In, values: ["true"] }
  serviceAccountName: system-upgrade
  upgrade:
    image: rancher/k3s-upgrade
  version: $version
---
apiVersion: upgrade.cattle.io/v1
kind: Plan
metadata:
  name: agent-plan
  namespace: system-upgrade
  labels: { k3s-upgrade: agent }
spec:
  concurrency: 1
  drain:
    force: true
    skipWaitForDeleteTimeout: 60
  nodeSelector:
    matchExpressions:
      - { key: k3s-upgrade, operator: In, values: ["true"] }
      - { key: node-role.kubernetes.io/control-plane, operator: DoesNotExist }
  # Waits until server-plan is complete (logic of the k3s-upgrade image).
  prepare:
    image: rancher/k3s-upgrade
    args: ["prepare", "server-plan"]
  serviceAccountName: system-upgrade
  upgrade:
    image: rancher/k3s-upgrade
  version: $version
EOF
  log "Waiting for both plans to complete at $version (the first run restarts k3s once per node)"
  wait_for 600 "server-plan to complete" plan_complete server-plan
  wait_for 600 "agent-plan to complete" plan_complete agent-plan
  kk -n system-upgrade get plans
}

# k3s-upgrade takes the parent of `k3s server|agent` for the k3s process when the parent's command
# line contains "k3s". In k3d that parent is `/bin/sh /bin/k3d-entrypoint.sh <args>`: an argument
# with "k3s" in it (the load balancer's --tls-san k3d-<cluster>-serverlb, with a cluster named
# kubyl-k3s) makes the job overwrite /bin/sh (busybox) with k3s, and the node never starts again.
k3s_upgrade_safe() {
  local node
  for node in $(docker ps --filter "label=k3d.cluster=$K3S_CLUSTER" --filter "label=k3d.role=server" --format '{{.Names}}') \
    $(docker ps --filter "label=k3d.cluster=$K3S_CLUSTER" --filter "label=k3d.role=agent" --format '{{.Names}}'); do
    if docker inspect -f '{{json .Args}}' "$node" | grep -q k3s; then
      die "the k3s arguments of $node contain \"k3s\": rancher/k3s-upgrade would overwrite /bin/sh there (recreate the cluster without a load balancer)"
    fi
  done
}

plan_complete() {
  [[ "$(kk -n system-upgrade get plan "$1" -o jsonpath='{.status.conditions[?(@.type=="Complete")].status}')" == True ]]
}

! $DEV || setup_dev
! $OCP || setup_ocp
! $K3S || setup_k3s

log "Done."
! $DEV || log "  dev:  kubectl --kubeconfig $DEV_KUBECONFIG --context $DEV_CONTEXT -n $DEV_NAMESPACE get pdb,secret"
! $OCP || log "  ocp:  oc --kubeconfig $OCP_KUBECONFIG --context $OCP_CONTEXT adm upgrade"
! $K3S || log "  k3s:  kubectl --kubeconfig $K3S_KUBECONFIG --context $K3S_CONTEXT -n system-upgrade get plans"
