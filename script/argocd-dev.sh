#!/usr/bin/env bash
# Installs Argo CD on the kind cluster from script/dev-cluster.sh, with sample apps from
# https://github.com/argoproj/argocd-example-apps for Kubyl's Argo CD views (phase 10):
#
#   - AppProject `kubyl-demo`: the example repo, any destination on this cluster, and
#     Applications from the `argocd-apps` namespace ("apps in any namespace").
#   - Application `argocd/guestbook`: the guestbook, manual sync (so it can be rolled back),
#     synced twice so its history has two entries (68657670 then the branch head).
#   - Application `argocd/guestbook-multi`: a multi-source app (guestbook + kustomize-guestbook).
#   - Application `argocd-apps/guestbook-team`: an app outside the Argo CD namespace.
#   - ApplicationSet `argocd/guestbook-envs`: a list generator making `guestbook-dev` and
#     `guestbook-staging` (helm-guestbook, auto-sync with prune and self-heal).
#
# Usage:
#   script/argocd-dev.sh [VERSION]   install or upgrade (default: $ARGOCD_VERSION or v3.5.3; the
#                                    two latest minors are v3.5 and v3.4, e.g. v3.4.9)
#   script/argocd-dev.sh --delete    uninstall everything, including the CRDs (Kubyl's Argo CD
#                                    views disappear without a restart)
#   script/argocd-dev.sh --sso       SSO through Argo CD's Dex with a mock connector (signs in as
#                                    kilgore@kilgore.trout without a password, read-only). Argo
#                                    CD's URL becomes http://localhost:8080: keep
#                                    `kubectl -n argocd port-forward svc/argocd-server 8080:80`
#                                    running so the browser (and Kubyl) reach its Dex there.
#   script/argocd-dev.sh --no-sso    back to no SSO (HTTPS, no URL)
#
# The admin password is printed at the end (from argocd-initial-admin-secret). Needs: kubectl,
# network access to GitHub from the cluster. Respects $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
NAMESPACE="argocd"
APPS_NAMESPACE="argocd-apps"
REPO="https://github.com/argoproj/argocd-example-apps.git"
# Two guestbook revisions with different images, for history and rollback.
OLD_REVISION="68657670d9131dc5bc5f538b14c1de3377d74591"
CRDS=(applications.argoproj.io applicationsets.argoproj.io appprojects.argoproj.io)

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
k() { kubectl --context "$CONTEXT" "$@"; }
k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

# Waits (3 min at most) until none of the named objects exist.
wait_gone() {
  local kind="$1"; shift
  for _ in $(seq 90); do
    [[ -z "$(k get "$kind" "$@" -o name --ignore-not-found 2>/dev/null)" ]] && return 0
    sleep 2
  done
  die "$kind $* still there after 3 minutes"
}

uninstall() {
  if k get crd applications.argoproj.io >/dev/null 2>&1; then
    log "Deleting Applications (cascading, 60 s at most)"
    k delete applicationsets.argoproj.io --all -A --wait=false >/dev/null 2>&1 || true
    k delete applications.argoproj.io --all -A --timeout=60s >/dev/null 2>&1 || true
    # Whatever is left (controller gone, stuck finalizers) goes without cascading.
    for app in $(k get applications.argoproj.io -A -o jsonpath='{range .items[*]}{.metadata.namespace}/{.metadata.name}{"\n"}{end}' 2>/dev/null); do
      k -n "${app%%/*}" patch applications.argoproj.io "${app##*/}" --type merge \
        -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
    done
  fi
  log "Deleting Argo CD"
  k delete namespace "$NAMESPACE" "$APPS_NAMESPACE" guestbook guestbook-multi guestbook-team \
    guestbook-dev guestbook-staging --ignore-not-found --wait=false >/dev/null
  k delete clusterrole,clusterrolebinding -l app.kubernetes.io/part-of=argocd --ignore-not-found >/dev/null
  k delete crd "${CRDS[@]}" --ignore-not-found >/dev/null
  log "Waiting for the namespaces and CRDs to go"
  wait_gone namespace "$NAMESPACE" "$APPS_NAMESPACE" guestbook guestbook-multi guestbook-team \
    guestbook-dev guestbook-staging
  wait_gone crd "${CRDS[@]}"
  log "Argo CD removed"
}

restart_server() {
  k -n "$NAMESPACE" rollout restart deployment/argocd-server deployment/argocd-dex-server >/dev/null
  k -n "$NAMESPACE" rollout status deployment/argocd-server --timeout=300s >/dev/null
  k -n "$NAMESPACE" rollout status deployment/argocd-dex-server --timeout=300s >/dev/null
}

# Dex SSO with the mock connector: argocd-server on plain HTTP (the issuer is
# http://localhost:8080/api/dex, through a port-forward), SSO users read-only.
sso_on() {
  k get crd applications.argoproj.io >/dev/null 2>&1 || die "Argo CD isn't installed"
  log "Enabling SSO (Dex, mock connector) at http://localhost:8080"
  k -n "$NAMESPACE" patch configmap argocd-cmd-params-cm --type merge \
    -p '{"data":{"server.insecure":"true"}}' >/dev/null
  k -n "$NAMESPACE" patch configmap argocd-cm --type merge -p '{"data":{
    "url":"http://localhost:8080",
    "dex.config":"connectors:\n- type: mockCallback\n  id: mock\n  name: Mock\n"}}' >/dev/null
  k -n "$NAMESPACE" patch configmap argocd-rbac-cm --type merge \
    -p '{"data":{"policy.default":"role:readonly"}}' >/dev/null
  restart_server
  log "SSO is on. Keep this running while you sign in:"
  echo "    kubectl --context $CONTEXT -n $NAMESPACE port-forward svc/argocd-server 8080:80"
}

sso_off() {
  log "Disabling SSO"
  k -n "$NAMESPACE" patch configmap argocd-cmd-params-cm --type json \
    -p '[{"op":"remove","path":"/data/server.insecure"}]' >/dev/null 2>&1 || true
  k -n "$NAMESPACE" patch configmap argocd-cm --type json \
    -p '[{"op":"remove","path":"/data/url"},{"op":"remove","path":"/data/dex.config"}]' >/dev/null 2>&1 || true
  k -n "$NAMESPACE" patch configmap argocd-rbac-cm --type json \
    -p '[{"op":"remove","path":"/data/policy.default"}]' >/dev/null 2>&1 || true
  restart_server
  log "SSO is off"
}

case "${1:-}" in
  --delete)
    uninstall
    exit 0
    ;;
  --sso)
    sso_on
    exit 0
    ;;
  --no-sso)
    sso_off
    exit 0
    ;;
  -*) die "unknown argument: $1 (use a version like v3.4.9, --delete, --sso or --no-sso)" ;;
esac

VERSION="${1:-${ARGOCD_VERSION:-v3.5.3}}"
[[ "$VERSION" == v* ]] || VERSION="v$VERSION"

log "Installing Argo CD $VERSION in namespace $NAMESPACE"
# Right after --delete the namespaces may still be terminating.
if [[ "$(k get namespace "$NAMESPACE" -o jsonpath='{.status.phase}' --ignore-not-found)" == Terminating ]]; then
  wait_gone namespace "$NAMESPACE"
fi
k create namespace "$NAMESPACE" --dry-run=client -o yaml | k apply -f - >/dev/null
# Server-side: the ApplicationSet CRD is too large for kubectl's last-applied annotation.
k apply -n "$NAMESPACE" --server-side --force-conflicts \
  -f "https://raw.githubusercontent.com/argoproj/argo-cd/${VERSION}/manifests/install.yaml" >/dev/null
k wait --for condition=Established --timeout=60s "${CRDS[@]/#/crd/}" >/dev/null

log "Allowing Applications in namespace $APPS_NAMESPACE"
k -n "$NAMESPACE" patch configmap argocd-cmd-params-cm --type merge \
  -p "{\"data\":{\"application.namespaces\":\"$APPS_NAMESPACE\"}}" >/dev/null
k -n "$NAMESPACE" rollout restart deployment/argocd-server statefulset/argocd-application-controller >/dev/null

log "Waiting for Argo CD to start"
k -n "$NAMESPACE" rollout status deployment/argocd-server --timeout=300s >/dev/null
k -n "$NAMESPACE" rollout status deployment/argocd-repo-server --timeout=300s >/dev/null
k -n "$NAMESPACE" rollout status deployment/argocd-applicationset-controller --timeout=300s >/dev/null
k -n "$NAMESPACE" rollout status statefulset/argocd-application-controller --timeout=300s >/dev/null

log "Adding the sample project, apps and ApplicationSet"
k create namespace "$APPS_NAMESPACE" --dry-run=client -o yaml | k apply -f - >/dev/null
k apply -f - >/dev/null <<EOF
apiVersion: argoproj.io/v1alpha1
kind: AppProject
metadata:
  name: kubyl-demo
  namespace: $NAMESPACE
spec:
  description: Sample apps for Kubyl's Argo CD views
  sourceRepos: ["$REPO"]
  sourceNamespaces: ["$APPS_NAMESPACE"]
  destinations:
    - server: https://kubernetes.default.svc
      namespace: "guestbook*"
  clusterResourceWhitelist:
    - group: ""
      kind: Namespace
  namespaceResourceBlacklist:
    - group: ""
      kind: ResourceQuota
  roles:
    - name: read-only
      description: Can see the demo apps
      policies: ["p, proj:kubyl-demo:read-only, applications, get, kubyl-demo/*, allow"]
  syncWindows:
    - kind: deny
      schedule: "0 22 * * *"
      duration: 8h
      applications: ["guestbook-staging"]
      manualSync: true
---
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: guestbook
  namespace: $NAMESPACE
  finalizers: [resources-finalizer.argocd.argoproj.io]
spec:
  project: kubyl-demo
  source:
    repoURL: $REPO
    targetRevision: HEAD
    path: guestbook
  destination:
    server: https://kubernetes.default.svc
    namespace: guestbook
  syncPolicy:
    syncOptions: [CreateNamespace=true]
---
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: guestbook-multi
  namespace: $NAMESPACE
  finalizers: [resources-finalizer.argocd.argoproj.io]
spec:
  project: kubyl-demo
  sources:
    - repoURL: $REPO
      targetRevision: HEAD
      path: guestbook
    - repoURL: $REPO
      targetRevision: HEAD
      path: kustomize-guestbook
  destination:
    server: https://kubernetes.default.svc
    namespace: guestbook-multi
  syncPolicy:
    automated: { prune: true }
    syncOptions: [CreateNamespace=true]
---
apiVersion: argoproj.io/v1alpha1
kind: Application
metadata:
  name: guestbook-team
  namespace: $APPS_NAMESPACE
spec:
  project: kubyl-demo
  source:
    repoURL: $REPO
    targetRevision: HEAD
    path: kustomize-guestbook
  destination:
    server: https://kubernetes.default.svc
    namespace: guestbook-team
  syncPolicy:
    automated: {}
    syncOptions: [CreateNamespace=true]
---
apiVersion: argoproj.io/v1alpha1
kind: ApplicationSet
metadata:
  name: guestbook-envs
  namespace: $NAMESPACE
spec:
  goTemplate: true
  generators:
    - list:
        elements:
          - env: dev
            replicas: "1"
          - env: staging
            replicas: "2"
  template:
    metadata:
      name: "guestbook-{{.env}}"
      finalizers: [resources-finalizer.argocd.argoproj.io]
    spec:
      project: kubyl-demo
      source:
        repoURL: $REPO
        targetRevision: HEAD
        path: helm-guestbook
        helm:
          parameters:
            - name: replicaCount
              value: "{{.replicas}}"
      destination:
        server: https://kubernetes.default.svc
        namespace: "guestbook-{{.env}}"
      syncPolicy:
        automated: { prune: true, selfHeal: true }
        syncOptions: [CreateNamespace=true]
EOF

# Syncs argocd/guestbook to a revision the way `argocd app sync --core` does (writes
# `operation`), then waits for the result.
sync_guestbook() {
  local revision="$1"
  k -n "$NAMESPACE" patch applications.argoproj.io guestbook --type merge -p "{\"operation\":{
    \"initiatedBy\":{\"username\":\"argocd-dev.sh\"},
    \"sync\":{\"revision\":\"$revision\",\"syncOptions\":[\"CreateNamespace=true\"]}}}" >/dev/null
  for _ in $(seq 1 90); do
    phase=$(k -n "$NAMESPACE" get applications.argoproj.io guestbook \
      -o jsonpath='{.status.operationState.phase}' 2>/dev/null || true)
    pending=$(k -n "$NAMESPACE" get applications.argoproj.io guestbook \
      -o jsonpath='{.operation.sync.revision}' 2>/dev/null || true)
    if [[ -z "$pending" && "$phase" =~ ^(Succeeded|Failed|Error)$ ]]; then
      [[ "$phase" == Succeeded ]] || die "sync to $revision ended with $phase"
      return
    fi
    sleep 2
  done
  die "sync to $revision didn't finish in 3 minutes"
}

history=$(k -n "$NAMESPACE" get applications.argoproj.io guestbook -o jsonpath='{.status.history[*].id}' 2>/dev/null || true)
if [[ -z "$history" ]]; then
  log "Syncing guestbook twice (history: ${OLD_REVISION:0:7}, then HEAD)"
  sync_guestbook "$OLD_REVISION"
  sync_guestbook HEAD
fi

password=$(k -n "$NAMESPACE" get secret argocd-initial-admin-secret \
  -o jsonpath='{.data.password}' 2>/dev/null | base64 -d || true)
log "Argo CD $VERSION is ready"
cat <<EOF
    Applications: kubectl --context $CONTEXT get applications.argoproj.io -A
    Web UI:       Kubyl → Administration → Argo CD → "Open Argo CD UI", or
                  kubectl --context $CONTEXT -n $NAMESPACE port-forward svc/argocd-server 8443:443
    Sign in:      admin / ${password:-<argocd-initial-admin-secret is gone>}
    Uninstall:    script/argocd-dev.sh --delete
EOF
