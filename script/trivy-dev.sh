#!/usr/bin/env bash
# Trivy Operator on the kind cluster from script/dev-cluster.sh, with sample workloads for
# Kubyl's Security Center (phase 25): VulnerabilityReports, ConfigAuditReports,
# ExposedSecretReports and RBAC assessment reports.
#
# In namespace `kubyl-trivy` (everything carries kubyl.dev/installed-by=trivy-dev.sh):
#   - `old-nginx`   nginx:1.19 (many known vulnerabilities, also run as root)
#   - `privileged`  a privileged pod without limits (ConfigAuditReports with failing checks)
#   - `reader`      a Role and ServiceAccount with wildcard rights (RbacAssessmentReport)
# The operator scans images through their registry, so it can't scan an image loaded into kind: the
# ExposedSecretReports it creates are empty (nothing secret in nginx or busybox). `--fixtures`
# adds a report with findings (a fake AWS key in an image `kubyl-trivy-leaky:1`; Trivy censors the
# matched text, and Kubyl never shows or exports it anyway).
# and the operator itself (Helm release `trivy-operator` in `trivy-system`, chart
# aquasecurity.github.io/helm-charts, installed with --repo: no Helm repository is added).
#
# The vulnerability scans need Trivy's database (ghcr.io/aquasecurity/trivy-db, and
# trivy-java-db). GitHub's registry rate-limits anonymous pulls and corporate networks block it:
# when the scan jobs fail with "TOOMANYREQUESTS" or time out, either set
# TRIVY_USERNAME/TRIVY_PASSWORD (a GitHub user and token with read:packages; they go into a
# Secret `trivy-operator-trivy-config` of the release, nowhere else), or use `--fixtures`:
# it applies the recorded reports of crates/kubyl_security_core/tests/fixtures/ (`*report-*.json`) as objects
# (no scans, no operator needed besides the CRDs) so the views have something to show.
#
# Usage:
#   script/trivy-dev.sh             install the operator and the samples
#   script/trivy-dev.sh --fixtures  CRDs from the chart plus recorded reports, no operator
#   script/trivy-dev.sh --delete    remove what the script installed (the CRDs stay when the
#                                   operator was there before)
#   script/trivy-dev.sh --force     allow a context that isn't kind-*
#
# Live tests: crates/kubyl_security_core/tests/live.rs. Needs kubectl and helm 3.13+ (docker
# for `leaky`). Respects $KUBECONFIG and $KUBYL_TRIVY_CONTEXT (default kind-kubyl-dev).
set -euo pipefail

CONTEXT="${KUBYL_TRIVY_CONTEXT:-kind-kubyl-dev}"
CHART_REPO="https://aquasecurity.github.io/helm-charts/"
CHART_VERSION="${TRIVY_OPERATOR_CHART_VERSION:-}"
NS="kubyl-trivy"
OP_NS="trivy-system"
MARK="kubyl.dev/installed-by"
ME="trivy-dev.sh"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FIXTURES="$HERE/../crates/kubyl_security_core/tests/fixtures"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

MODE=install
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --delete) MODE=delete ;;
    --fixtures) MODE=fixtures ;;
    --force) FORCE=1 ;;
    *) die "unknown option $arg" ;;
  esac
done
command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
case "$CONTEXT" in kind-*) ;; *) [ "$FORCE" = 1 ] || die "refusing context $CONTEXT (not kind-*); --force allows it" ;; esac
k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }
k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

marked() { k get ns "$1" -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null | grep -q "$ME"; }

delete() {
  if marked "$NS"; then
    log "deleting namespace $NS"
    k delete ns "$NS" --wait=true --timeout=180s || true
  fi
  if marked "$OP_NS"; then
    log "uninstalling trivy-operator"
    h -n "$OP_NS" uninstall trivy-operator --wait >/dev/null 2>&1 || true
    k delete ns "$OP_NS" --wait=true --timeout=180s || true
    # Reports and CRDs belong to the operator; the chart keeps its CRDs on uninstall.
    for crd in $(k get crd -o name | grep aquasecurity.github.io || true); do
      k delete "$crd" --wait=false >/dev/null 2>&1 || true
    done
  fi
  if [ "$(k get crd vulnerabilityreports.aquasecurity.github.io -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null || true)" = "$ME-fixtures" ]; then
    log "deleting the fixture CRDs"
    for crd in $(k get crd -o name | grep aquasecurity.github.io || true); do k delete "$crd" --wait=false >/dev/null 2>&1 || true; done
  fi
  log "done"
}

samples() {
  log "applying the sample workloads to $NS"
  k apply -f - <<YAML
apiVersion: v1
kind: Namespace
metadata:
  name: $NS
  annotations: {$MARK: $ME}
---
apiVersion: apps/v1
kind: Deployment
metadata: {name: old-nginx, namespace: $NS, labels: {$MARK: $ME}}
spec:
  replicas: 1
  selector: {matchLabels: {app: old-nginx}}
  template:
    metadata: {labels: {app: old-nginx}}
    spec:
      containers: [{name: nginx, image: "nginx:1.19", ports: [{containerPort: 80}]}]
---
apiVersion: v1
kind: Pod
metadata: {name: privileged, namespace: $NS, labels: {$MARK: $ME}}
spec:
  containers:
    - name: shell
      image: busybox:1.37
      command: ["sleep", "86400"]
      securityContext: {privileged: true, runAsUser: 0, allowPrivilegeEscalation: true}
  hostNetwork: true
---
apiVersion: v1
kind: ServiceAccount
metadata: {name: reader, namespace: $NS, labels: {$MARK: $ME}}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata: {name: everything, namespace: $NS, labels: {$MARK: $ME}}
rules:
  - {apiGroups: ["*"], resources: ["*"], verbs: ["*"]}
  - {apiGroups: [""], resources: ["secrets"], verbs: ["get", "list"]}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata: {name: reader-everything, namespace: $NS, labels: {$MARK: $ME}}
subjects: [{kind: ServiceAccount, name: reader, namespace: $NS}]
roleRef: {apiGroup: rbac.authorization.k8s.io, kind: Role, name: everything}
YAML
}

case "$MODE" in
  delete) delete; exit 0 ;;
  fixtures)
    command -v helm >/dev/null 2>&1 || die "helm is not installed"
    log "installing the CRDs of the trivy-operator chart"
    version_args=()
    [ -n "$CHART_VERSION" ] && version_args=(--version "$CHART_VERSION")
    h show crds trivy-operator --repo "$CHART_REPO" ${version_args[@]+"${version_args[@]}"} | k apply --server-side -f - >/dev/null
    for crd in $(k get crd -o name | grep aquasecurity.github.io); do
      k annotate "$crd" "$MARK=$ME-fixtures" --overwrite >/dev/null
    done
    k apply -f - <<YAML >/dev/null
apiVersion: v1
kind: Namespace
metadata:
  name: $NS
  annotations: {$MARK: $ME}
YAML
    [ -d "$FIXTURES" ] || die "no fixtures in $FIXTURES"
    for f in "$FIXTURES"/*report-*.json; do
      [ -e "$f" ] || continue
      log "applying $(basename "$f")"
      k apply -f "$f" >/dev/null
    done
    log "done: the Security Center reads these reports without an operator"
    exit 0
    ;;
esac

command -v helm >/dev/null 2>&1 || die "helm is not installed"
samples
log "installing trivy-operator into $OP_NS"
k create namespace "$OP_NS" --dry-run=client -o yaml | k apply -f - >/dev/null
k annotate namespace "$OP_NS" "$MARK=$ME" --overwrite >/dev/null
extra=()
[ -n "$CHART_VERSION" ] && extra+=(--version "$CHART_VERSION")
if [ -n "${TRIVY_USERNAME:-}" ] && [ -n "${TRIVY_PASSWORD:-}" ]; then
  extra+=(--set "trivy.dbRegistryInsecure=false")
  k -n "$OP_NS" create secret generic trivy-operator-trivy-config \
    --from-literal=trivy.githubToken="$TRIVY_PASSWORD" --dry-run=client -o yaml | k apply -f - >/dev/null
fi
h upgrade --install trivy-operator trivy-operator --repo "$CHART_REPO" -n "$OP_NS" \
  --set operator.scanJobsConcurrentLimit=2 \
  --set trivy.ignoreUnfixed=false \
  --set operator.scanJobTimeout=10m \
  ${extra[@]+"${extra[@]}"} --wait --timeout 5m >/dev/null
log "the operator runs; the first reports take a few minutes (it downloads Trivy's database first)"
log "watch: kubectl --context $CONTEXT -n $NS get vulnerabilityreports,configauditreports,exposedsecretreports,rbacassessmentreports"
