#!/usr/bin/env bash
# Installs Flux on the kind cluster from script/dev-cluster.sh, with sample objects for Kubyl's
# Flux views (phase 23). Flux comes from its release manifests (install.yaml); the `flux` CLI
# isn't needed, and Kubyl doesn't need it either.
#
# Everything lives in namespaces this script marks (label kubyl.dev/flux-dev):
#
#   flux-demo (sources and Flux objects):
#   - GitRepository `podinfo` (github.com/stefanprodan/podinfo, branch master) and
#     Kustomization `podinfo` (./kustomize into namespace flux-podinfo, prune on).
#   - HelmRepository `podinfo` (stefanprodan.github.io/podinfo) and HelmRelease `podinfo-helm`
#     (chart podinfo 6.x, values from a ConfigMap and a Secret, remediation settings).
#   - OCIRepository `podinfo-manifests` (ghcr.io/stefanprodan/manifests/podinfo).
#   - HelmRepository `podinfo-oci` of type oci (ghcr.io/stefanprodan/charts): static since
#     Flux 2.3, source-controller leaves its status empty.
#   - Kustomization `broken`: a path that doesn't exist (fails, shows under "Needs attention").
#   - Kustomization `paused`: reconciled once, then suspended.
#   - A dependency chain: `infra` → `apps` (dependsOn infra), and `apps-late` (dependsOn
#     broken: waits for it forever).
#   - ImageRepository/ImagePolicy `podinfo` (ghcr.io/stefanprodan/podinfo, semver 6.x) and an
#     ImageUpdateAutomation (suspended: it would push to the public repo).
#   - Provider `webhook` (generic, to an address that doesn't resolve), Alert `all-errors`,
#     Receiver `github` (its token in a Secret).
#
# Usage:
#   script/flux-dev.sh [VERSION]   install (default: $FLUX_VERSION or v2.9.6) and add the samples
#   script/flux-dev.sh --delete    remove the samples, and Flux with its CRDs if this script
#                                  installed it (a Flux that was there before stays; Flux
#                                  objects outside flux-demo stop it unless FORCE=1)
#
# Needs Flux 2.7 or newer (image.toolkit.fluxcd.io/v1) when Flux is already installed.
#
# Needs: kubectl, curl, network access to GitHub, ghcr.io and stefanprodan.github.io from the
# cluster. Respects $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
FLUX_NS="flux-system"
DEMO_NS="flux-demo"
TARGET_NS=(flux-podinfo flux-paused flux-chain)
MARK="kubyl.dev/flux-dev"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
command -v curl >/dev/null 2>&1 || die "curl is not installed"
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

# Waits until a Flux object is Ready (or gives up quietly after $2 seconds).
wait_ready() {
  local object="$1" timeout="${2:-180}"
  k -n "$DEMO_NS" wait --for=condition=Ready --timeout="${timeout}s" "$object" >/dev/null 2>&1 \
    || log "  $object isn't ready yet (it keeps trying)"
}

# The mark of a namespace ("installed" for flux-system, "demo" for the samples' namespaces).
mark_of() { k get namespace "$1" -o jsonpath="{.metadata.labels.kubyl\.dev/flux-dev}" --ignore-not-found 2>/dev/null; }
marked() { [[ -n "$(mark_of "$1")" ]]; }
flux_marked() { [[ "$(mark_of "$FLUX_NS")" == installed ]]; }

# The Flux kinds installed on the cluster (plural.group).
flux_kinds() {
  k get crd -o name 2>/dev/null | grep 'toolkit.fluxcd.io\|source.extensions.fluxcd.io' | sed 's|.*/||'
}

uninstall() {
  local demo=false
  if marked "$DEMO_NS"; then
    demo=true
    log "Deleting the samples in $DEMO_NS"
    # Through the controllers while they run: their finalizers prune what the objects applied.
    for kind in $(flux_kinds); do
      k -n "$DEMO_NS" delete "$kind" --all --timeout=90s >/dev/null 2>&1 || true
    done
    k delete namespace "$DEMO_NS" --ignore-not-found --wait=false >/dev/null
  fi
  local gone=()
  [[ "$demo" == true ]] && gone+=("$DEMO_NS")
  for ns in "${TARGET_NS[@]}"; do
    if marked "$ns"; then
      k delete namespace "$ns" --ignore-not-found --wait=false >/dev/null
      gone+=("$ns")
    fi
  done
  if flux_marked; then
    # Only Flux objects of the samples: anything else would lose its finalizers below.
    local foreign
    foreign="$(for kind in $(flux_kinds); do
      k get "$kind" -A -o jsonpath='{range .items[*]}{.metadata.namespace}/{.metadata.name}{"\n"}{end}' 2>/dev/null
    done | grep -v "^$DEMO_NS/" | grep -v '^$' || true)"
    if [[ -n "$foreign" && "${FORCE:-}" != 1 ]]; then
      die "Flux objects outside $DEMO_NS would be removed with Flux:"$'\n'"$foreign"$'\n'"(FORCE=1 deletes them too)"
    fi
    log "Deleting Flux (this script installed it)"
    # Stop the controllers first: a running controller puts its finalizer back.
    k -n "$FLUX_NS" delete deployment -l app.kubernetes.io/part-of=flux --wait=true --timeout=120s >/dev/null 2>&1 || true
    # Objects left anywhere would block the CRDs on their finalizers.
    for kind in $(flux_kinds); do
      for object in $(k get "$kind" -A -o jsonpath='{range .items[*]}{.metadata.namespace}/{.metadata.name}{"\n"}{end}' 2>/dev/null); do
        k -n "${object%%/*}" patch "$kind" "${object##*/}" --type merge \
          -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
      done
    done
    k delete namespace "$FLUX_NS" --ignore-not-found --wait=false >/dev/null
    k delete clusterrole,clusterrolebinding -l app.kubernetes.io/part-of=flux --ignore-not-found >/dev/null
    k delete crd -l app.kubernetes.io/part-of=flux --ignore-not-found >/dev/null
    wait_gone namespace "$FLUX_NS"
  else
    log "Flux wasn't installed by this script: it stays"
  fi
  [[ ${#gone[@]} -gt 0 ]] && wait_gone namespace "${gone[@]}"
  log "Done"
}

case "${1:-}" in
  --delete)
    uninstall
    exit 0
    ;;
  -*) die "unknown argument: $1 (use a version like v2.9.6, or --delete)" ;;
esac

VERSION="${1:-${FLUX_VERSION:-v2.9.6}}"
[[ "$VERSION" == v* ]] || VERSION="v$VERSION"

if [[ "$(k get namespace "$FLUX_NS" -o jsonpath='{.status.phase}' --ignore-not-found)" == Terminating ]]; then
  wait_gone namespace "$FLUX_NS"
fi
# Any Flux CRD, or a flux-system namespace this script didn't make, is someone else's Flux
# (helm-only installs have no Kustomization CRD; Flux Operator creates the namespace first).
if ! flux_marked && { [[ -n "$(flux_kinds)" ]] || k get namespace "$FLUX_NS" >/dev/null 2>&1; }; then
  log "Flux is already installed (not by this script): adding the samples only"
else
  log "Installing Flux $VERSION in $FLUX_NS (release manifests, no CLI)"
  manifests="$(mktemp)"
  trap 'rm -f "$manifests"' EXIT
  curl -fsSL -o "$manifests" "https://github.com/fluxcd/flux2/releases/download/${VERSION}/install.yaml" \
    || die "couldn't download Flux $VERSION's install.yaml"
  # Marked before anything is applied: an apply that fails halfway stays this script's to
  # remove with --delete.
  k create namespace "$FLUX_NS" --dry-run=client -o yaml | k apply -f - >/dev/null
  k label namespace "$FLUX_NS" "$MARK=installed" --overwrite >/dev/null
  k apply --server-side --force-conflicts -f "$manifests" >/dev/null
  k wait --for condition=Established --timeout=60s crd -l app.kubernetes.io/part-of=flux >/dev/null
  log "Waiting for the controllers"
  for d in source-controller kustomize-controller helm-controller notification-controller \
    image-reflector-controller image-automation-controller; do
    k -n "$FLUX_NS" rollout status "deployment/$d" --timeout=300s >/dev/null
  done
fi

# The samples use image.toolkit.fluxcd.io/v1 (Flux 2.7+) and OCIRepository v1 (Flux 2.6+):
# check first rather than apply half of them.
for gv in source.toolkit.fluxcd.io/v1 kustomize.toolkit.fluxcd.io/v1 helm.toolkit.fluxcd.io/v2 \
  image.toolkit.fluxcd.io/v1 notification.toolkit.fluxcd.io/v1beta3 notification.toolkit.fluxcd.io/v1; do
  k get --raw "/apis/$gv" >/dev/null 2>&1 \
    || die "the cluster doesn't serve $gv: the samples need Flux 2.7 or newer with image automation"
done

log "Adding the samples in $DEMO_NS"
for ns in "$DEMO_NS" "${TARGET_NS[@]}"; do
  if k get namespace "$ns" >/dev/null 2>&1; then
    # Never take over a namespace this script didn't create: --delete removes marked ones.
    # (flux-chain loses the mark once the sample `infra` applies it as its own Namespace.)
    marked "$ns" || [[ "$(k get namespace "$ns" -o jsonpath='{.metadata.labels.kustomize\.toolkit\.fluxcd\.io/namespace}')" == "$DEMO_NS" ]] \
      || die "namespace $ns exists and wasn't created by this script; remove it or use another cluster"
  else
    k create namespace "$ns" >/dev/null
    k label namespace "$ns" "$MARK=demo" --overwrite >/dev/null
  fi
done

k apply -f - >/dev/null <<EOF
apiVersion: source.toolkit.fluxcd.io/v1
kind: GitRepository
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  interval: 5m
  url: https://github.com/stefanprodan/podinfo
  ref:
    branch: master
---
apiVersion: source.toolkit.fluxcd.io/v1
kind: HelmRepository
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  interval: 30m
  url: https://stefanprodan.github.io/podinfo
---
apiVersion: source.toolkit.fluxcd.io/v1
kind: HelmRepository
metadata:
  name: podinfo-oci
  namespace: $DEMO_NS
spec:
  type: oci
  interval: 30m
  url: oci://ghcr.io/stefanprodan/charts
---
apiVersion: source.toolkit.fluxcd.io/v1
kind: OCIRepository
metadata:
  name: podinfo-manifests
  namespace: $DEMO_NS
spec:
  interval: 30m
  url: oci://ghcr.io/stefanprodan/manifests/podinfo
  ref:
    tag: latest
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: podinfo-values
  namespace: $DEMO_NS
data:
  values.yaml: |
    replicaCount: 1
    ui:
      message: Deployed by Flux
---
apiVersion: v1
kind: Secret
metadata:
  name: podinfo-secret-values
  namespace: $DEMO_NS
stringData:
  values.yaml: |
    ui:
      color: "#34577c"
---
apiVersion: v1
kind: Secret
metadata:
  name: podinfo-substitutions
  namespace: $DEMO_NS
stringData:
  db_password: not-a-real-password
---
apiVersion: helm.toolkit.fluxcd.io/v2
kind: HelmRelease
metadata:
  name: podinfo-helm
  namespace: $DEMO_NS
spec:
  interval: 10m
  chart:
    spec:
      chart: podinfo
      version: "6.x"
      sourceRef:
        kind: HelmRepository
        name: podinfo
  install:
    remediation:
      retries: 3
  upgrade:
    remediation:
      retries: 3
      remediateLastFailure: true
  valuesFrom:
    - kind: ConfigMap
      name: podinfo-values
    - kind: Secret
      name: podinfo-secret-values
  values:
    resources:
      requests:
        cpu: 10m
        memory: 32Mi
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  interval: 10m
  path: ./kustomize
  prune: true
  targetNamespace: flux-podinfo
  sourceRef:
    kind: GitRepository
    name: podinfo
  postBuild:
    substitute:
      cluster_env: dev
    substituteFrom:
      - kind: Secret
        name: podinfo-substitutions
        optional: true
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: broken
  namespace: $DEMO_NS
spec:
  interval: 5m
  retryInterval: 1m
  path: ./does-not-exist
  prune: true
  sourceRef:
    kind: GitRepository
    name: podinfo
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: paused
  namespace: $DEMO_NS
spec:
  interval: 10m
  path: ./kustomize
  prune: true
  targetNamespace: flux-paused
  sourceRef:
    kind: GitRepository
    name: podinfo
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: infra
  namespace: $DEMO_NS
spec:
  interval: 10m
  path: ./deploy/webapp/common
  prune: true
  targetNamespace: flux-chain
  sourceRef:
    kind: GitRepository
    name: podinfo
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: apps
  namespace: $DEMO_NS
spec:
  interval: 10m
  # The live tests suspend and resume it: a re-run resumes it if a test stopped halfway.
  suspend: false
  dependsOn:
    - name: infra
  path: ./deploy/webapp/backend
  prune: true
  targetNamespace: flux-chain
  sourceRef:
    kind: GitRepository
    name: podinfo
---
apiVersion: kustomize.toolkit.fluxcd.io/v1
kind: Kustomization
metadata:
  name: apps-late
  namespace: $DEMO_NS
spec:
  interval: 10m
  retryInterval: 1m
  dependsOn:
    - name: broken
  path: ./kustomize
  prune: true
  targetNamespace: flux-chain
  sourceRef:
    kind: GitRepository
    name: podinfo
---
apiVersion: image.toolkit.fluxcd.io/v1
kind: ImageRepository
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  image: ghcr.io/stefanprodan/podinfo
  interval: 1h
---
apiVersion: image.toolkit.fluxcd.io/v1
kind: ImagePolicy
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  imageRepositoryRef:
    name: podinfo
  policy:
    semver:
      range: 6.x
---
apiVersion: image.toolkit.fluxcd.io/v1
kind: ImageUpdateAutomation
metadata:
  name: podinfo
  namespace: $DEMO_NS
spec:
  suspend: true
  interval: 30m
  sourceRef:
    kind: GitRepository
    name: podinfo
  git:
    checkout:
      ref:
        branch: master
    commit:
      author:
        name: fluxcdbot
        email: fluxcdbot@users.noreply.github.com
      messageTemplate: "Update images"
    push:
      branch: master
  update:
    path: ./kustomize
    strategy: Setters
---
apiVersion: v1
kind: Secret
metadata:
  name: webhook-token
  namespace: $DEMO_NS
stringData:
  token: kubyl-dev-not-a-real-token
---
apiVersion: notification.toolkit.fluxcd.io/v1beta3
kind: Provider
metadata:
  name: webhook
  namespace: $DEMO_NS
spec:
  type: generic
  address: http://alerts.flux-demo.invalid/hook
---
apiVersion: notification.toolkit.fluxcd.io/v1beta3
kind: Alert
metadata:
  name: all-errors
  namespace: $DEMO_NS
spec:
  providerRef:
    name: webhook
  eventSeverity: error
  eventSources:
    - kind: Kustomization
      name: "*"
    - kind: HelmRelease
      name: "*"
---
apiVersion: notification.toolkit.fluxcd.io/v1
kind: Receiver
metadata:
  name: github
  namespace: $DEMO_NS
spec:
  type: github
  events: ["ping", "push"]
  secretRef:
    name: webhook-token
  resources:
    - kind: GitRepository
      name: podinfo
EOF

log "Waiting for the sources and objects (GitHub, ghcr.io)"
wait_ready gitrepository/podinfo
wait_ready helmrepository/podinfo
wait_ready kustomization/podinfo
wait_ready kustomization/paused
wait_ready kustomization/infra
wait_ready kustomization/apps
wait_ready helmrelease/podinfo-helm 240

log "Suspending Kustomization paused"
k -n "$DEMO_NS" patch kustomization paused --type merge -p '{"spec":{"suspend":true}}' >/dev/null

log "Flux is ready:"
k get kustomizations.kustomize.toolkit.fluxcd.io,helmreleases.helm.toolkit.fluxcd.io -n "$DEMO_NS"
