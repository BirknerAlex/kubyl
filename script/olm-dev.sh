#!/usr/bin/env bash
# Operator Lifecycle Manager for the kind cluster (phase 12). Run after script/dev-cluster.sh.
#
# - OLM v0 from the operator-framework release manifests (namespaces `olm` and `operators`,
#   the `operators/global-operators` OperatorGroup for all-namespaces installs) with the
#   operatorhub.io CatalogSource `olm/operatorhubio-catalog` the release ships.
# - A manual-approval Subscription pinned to an older CSV (`startingCSV`), so the Installed
#   Operators view shows "Upgrade available" and Approve can be tried:
#   `kubyl-manual/<package>` in its own namespace with an OwnNamespace OperatorGroup.
# - With --v1: OLM v1 as well (operator-controller and catalogd from the operator-controller
#   release, the operatorhub.io ClusterCatalog, and a ClusterExtension (grafana-operator; the
#   dev cluster's Argo CD owns the CRDs argocd-operator would install).
#   operator-controller needs cert-manager: the one running in the cluster
#   is used (install cert-manager from OperatorHub in Kubyl first, that's the phase 12 test),
#   else cert-manager's release manifests are applied.
#
# The script marks the namespaces of what it installs (`olm`, `olmv1-system`, `cert-manager`:
# annotation kubyl.dev/installed-by=olm-dev.sh); --delete removes only what's marked, so an OLM
# or cert-manager that was there before stays.
#
# Usage:
#   script/olm-dev.sh                 OLM v0, the catalog and the manual-approval subscription
#   script/olm-dev.sh --v1            also OLM v1 (operator-controller, a ClusterCatalog, a
#                                     ClusterExtension)
#   script/olm-dev.sh --reset-manual  the manual subscription again (after its upgrade was
#                                     approved: a new one waits for approval)
#   script/olm-dev.sh --delete        remove what the script installed (operators installed
#                                     through OLM stay: uninstall them in Kubyl first)
#
# The catalog image is large (quay.io/operatorhubio/catalog, several hundred MB unpacked): its
# pod takes a few minutes to become READY the first time.
#
# Needs: kubectl. Respects $KUBECONFIG and $CONTEXT, $OLM_VERSION and $OPERATOR_CONTROLLER_VERSION.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
OLM_VERSION="${OLM_VERSION:-v0.46.0}"
OPERATOR_CONTROLLER_VERSION="${OPERATOR_CONTROLLER_VERSION:-v1.12.0}"
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-v1.21.2}"
OLM_URL="https://github.com/operator-framework/operator-lifecycle-manager/releases/download/${OLM_VERSION}"
OC_URL="https://github.com/operator-framework/operator-controller/releases/download/${OPERATOR_CONTROLLER_VERSION}"

# The manual-approval subscription: a package of the operatorhub.io catalog with a short
# upgrade chain in one channel, pinned to the version before the channel head.
MANUAL_NS="kubyl-manual"
MANUAL_PACKAGE="${MANUAL_PACKAGE:-}"
MANUAL_CHANNEL="${MANUAL_CHANNEL:-}"
MANUAL_CSV="${MANUAL_CSV:-}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"

k() { kubectl --context "$CONTEXT" "$@"; }

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

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

# The namespace cert-manager runs in, empty without one (OperatorHub installs it into
# `operators`, its release manifests into `cert-manager`).
cert_manager_namespace() {
  k get deployment -A -o jsonpath='{range .items[*]}{.metadata.namespace} {.metadata.name}{"\n"}{end}' |
    awk '$2 == "cert-manager-webhook" && !found { print $1; found = 1 }'
}

# operator-controller's release manifest. It puts its CA (a Certificate and a self-signed
# Issuer) into cert-manager's namespace, assumed to be `cert-manager`: point them at $1.
operator_controller_manifest() {
  curl -fsSL "$OC_URL/operator-controller.yaml" |
    sed -e "s#namespace: cert-manager\$#namespace: $1#" \
      -e "s#inject-ca-from-secret: cert-manager/#inject-ca-from-secret: $1/#"
}

operator_controller_apply() {
  operator_controller_manifest "$1" | k apply --server-side --force-conflicts -f -
}

# Marks a namespace as installed by this script (extra annotations after the name).
INSTALLED_BY="kubyl.dev/installed-by"
CERT_MANAGER_VERSION_KEY="kubyl.dev/cert-manager-version"
mark() {
  local namespace="$1"
  shift
  k annotate namespace "$namespace" --overwrite "$INSTALLED_BY=olm-dev.sh" "$@" >/dev/null
}
marked() {
  [[ "$(k get namespace "$1" -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null || true)" == olm-dev.sh ]]
}

V1=false
RESET=false
for arg in "$@"; do
  case "$arg" in
    --delete)
      log "Removing the ClusterExtension kubyl-v1-sample"
      k delete clusterextension kubyl-v1-sample --ignore-not-found --wait=false >/dev/null 2>&1 || true
      k delete namespace kubyl-v1-sample --ignore-not-found --wait=false >/dev/null
      if marked olmv1-system; then
        log "Removing OLM v1"
        k delete clustercatalog operatorhubio --ignore-not-found --wait=false >/dev/null 2>&1 || true
        if command -v curl >/dev/null 2>&1; then
          cm_namespace="$(cert_manager_namespace)"
          operator_controller_manifest "${cm_namespace:-cert-manager}" |
            k delete --ignore-not-found --wait=false -f - >/dev/null 2>&1 || true
        else
          k delete -f "$OC_URL/operator-controller.yaml" --ignore-not-found --wait=false >/dev/null 2>&1 || true
        fi
      elif k get namespace olmv1-system >/dev/null 2>&1; then
        log "OLM v1 wasn't installed by this script: it stays"
      fi
      if marked cert-manager; then
        cm_version="$(k get namespace cert-manager -o jsonpath="{.metadata.annotations.kubyl\.dev/cert-manager-version}")"
        log "Removing cert-manager ${cm_version:-$CERT_MANAGER_VERSION}"
        k delete -f "https://github.com/cert-manager/cert-manager/releases/download/${cm_version:-$CERT_MANAGER_VERSION}/cert-manager.yaml" \
          --ignore-not-found --wait=false >/dev/null 2>&1 || true
      fi
      log "Removing the manual-approval subscription"
      k delete namespace "$MANUAL_NS" --ignore-not-found --wait=false >/dev/null
      if marked olm; then
        log "Removing OLM v0"
        k delete -f "$OLM_URL/olm.yaml" --ignore-not-found --wait=false >/dev/null 2>&1 || true
        k delete -f "$OLM_URL/crds.yaml" --ignore-not-found --wait=false >/dev/null 2>&1 || true
      elif k get namespace olm >/dev/null 2>&1; then
        log "OLM v0 wasn't installed by this script: it stays"
      fi
      log "Done"
      exit 0
      ;;
    --v1) V1=true ;;
    --reset-manual) RESET=true ;;
    *) die "unknown argument: $arg (use --v1, --reset-manual or --delete)" ;;
  esac
done

# ----- OLM v0 -----

if k get deployment olm-operator -n openshift-operator-lifecycle-manager >/dev/null 2>&1; then
  die "this cluster runs OpenShift's OLM; nothing to install"
fi
if k get deployment olm-operator -n olm >/dev/null 2>&1; then
  log "OLM v0 is installed"
else
  log "Installing OLM $OLM_VERSION"
  k create -f "$OLM_URL/crds.yaml" >/dev/null
  k wait --for=condition=Established -f "$OLM_URL/crds.yaml" --timeout=120s >/dev/null
  k create -f "$OLM_URL/olm.yaml" >/dev/null
  mark olm
fi
k rollout status -w deployment/olm-operator -n olm --timeout=300s >/dev/null
k rollout status -w deployment/catalog-operator -n olm --timeout=300s >/dev/null
log "Waiting for the packageserver CSV"
wait_for 600 "the packageserver CSV" \
  sh -c "kubectl --context '$CONTEXT' -n olm get csv packageserver -o jsonpath='{.status.phase}' | grep -qx Succeeded"
k rollout status -w deployment/packageserver -n olm --timeout=300s >/dev/null

log "Waiting for the operatorhub.io catalog (the image is large; the first pull takes a while)"
wait_for 900 "CatalogSource olm/operatorhubio-catalog to be READY" \
  sh -c "kubectl --context '$CONTEXT' -n olm get catalogsource operatorhubio-catalog -o jsonpath='{.status.connectionState.lastObservedState}' | grep -qx READY"
wait_for 300 "the catalog's PackageManifests" \
  sh -c "kubectl --context '$CONTEXT' get packagemanifests -n olm -l catalog=operatorhubio-catalog -o name | grep -q ."

# ----- The manual-approval subscription -----

if [[ -z "$MANUAL_PACKAGE" ]]; then
  MANUAL_PACKAGE="cloudnative-pg"
fi
if $RESET; then
  log "Removing $MANUAL_NS (its upgrade was approved; a new subscription waits again)"
  k delete namespace "$MANUAL_NS" --ignore-not-found --wait=true --timeout=300s >/dev/null
fi
manual_subscription=true
if k -n "$MANUAL_NS" get subscription "$MANUAL_PACKAGE" >/dev/null 2>&1; then
  state="$(k -n "$MANUAL_NS" get subscription "$MANUAL_PACKAGE" -o jsonpath='{.status.state} {.status.installedCSV}')"
  log "Subscription $MANUAL_NS/$MANUAL_PACKAGE exists ($state); --reset-manual makes a new one wait for approval"
  manual_subscription=false
fi
if $manual_subscription; then
if [[ -z "$MANUAL_CHANNEL" ]]; then
  MANUAL_CHANNEL="$(k get packagemanifest -n olm "$MANUAL_PACKAGE" -o jsonpath='{.status.defaultChannel}')"
fi
if [[ -z "$MANUAL_CSV" ]]; then
  # The entry before the channel head (entries are listed head first).
  MANUAL_CSV="$(k get packagemanifest -n olm "$MANUAL_PACKAGE" \
    -o jsonpath="{.status.channels[?(@.name=='$MANUAL_CHANNEL')].entries[1].name}")"
fi
[[ -n "$MANUAL_CSV" ]] || die "no older CSV in $MANUAL_PACKAGE/$MANUAL_CHANNEL (set MANUAL_PACKAGE, MANUAL_CHANNEL and MANUAL_CSV)"

log "Subscription $MANUAL_NS/$MANUAL_PACKAGE: channel $MANUAL_CHANNEL, Manual approval, starting at $MANUAL_CSV"
k apply -f - >/dev/null <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: $MANUAL_NS
---
apiVersion: operators.coreos.com/v1
kind: OperatorGroup
metadata:
  name: $MANUAL_NS
  namespace: $MANUAL_NS
spec:
  targetNamespaces: [$MANUAL_NS]
---
apiVersion: operators.coreos.com/v1alpha1
kind: Subscription
metadata:
  name: $MANUAL_PACKAGE
  namespace: $MANUAL_NS
spec:
  name: $MANUAL_PACKAGE
  channel: $MANUAL_CHANNEL
  source: operatorhubio-catalog
  sourceNamespace: olm
  installPlanApproval: Manual
  startingCSV: $MANUAL_CSV
EOF

# A manual subscription's first InstallPlan needs an approval too: approve the one that installs
# the pinned CSV, so the next one (the upgrade) is what Kubyl shows as pending.
log "Approving the first InstallPlan (installs $MANUAL_CSV)"
wait_for 300 "the first InstallPlan in $MANUAL_NS" \
  sh -c "kubectl --context '$CONTEXT' -n '$MANUAL_NS' get installplan -o name | grep -q ."
for plan in $(k -n "$MANUAL_NS" get installplan -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.spec.clusterServiceVersionNames[*]}{"\n"}{end}' | awk -v csv="$MANUAL_CSV" '$0 ~ csv {print $1}'); do
  k -n "$MANUAL_NS" patch installplan "$plan" --type merge -p '{"spec":{"approved":true}}' >/dev/null
done
wait_for 600 "$MANUAL_CSV to succeed" \
  sh -c "kubectl --context '$CONTEXT' -n '$MANUAL_NS' get csv '$MANUAL_CSV' -o jsonpath='{.status.phase}' | grep -qx Succeeded"
log "Waiting for the upgrade's InstallPlan (RequiresApproval)"
wait_for 300 "an InstallPlan that requires approval" \
  sh -c "kubectl --context '$CONTEXT' -n '$MANUAL_NS' get installplan -o jsonpath='{.items[*].status.phase}' | grep -q RequiresApproval"
fi

# ----- OLM v1 -----

if $V1; then
  command -v curl >/dev/null 2>&1 || die "curl is not installed"
  cm_namespace="$(cert_manager_namespace)"
  if [[ -n "$cm_namespace" ]]; then
    log "cert-manager runs in $cm_namespace; OLM v1 uses it"
    k -n "$cm_namespace" rollout status deployment/cert-manager-webhook --timeout=300s >/dev/null
  else
    log "Installing cert-manager $CERT_MANAGER_VERSION (operator-controller needs it)"
    k apply --server-side --force-conflicts -f "https://github.com/cert-manager/cert-manager/releases/download/${CERT_MANAGER_VERSION}/cert-manager.yaml" >/dev/null
    mark cert-manager "$CERT_MANAGER_VERSION_KEY=$CERT_MANAGER_VERSION"
    k -n cert-manager rollout status deployment/cert-manager-webhook --timeout=300s >/dev/null
    cm_namespace="cert-manager"
  fi
  if k get namespace olmv1-system >/dev/null 2>&1 && ! marked olmv1-system; then
    log "OLM v1 is installed (not by this script): using it"
  else
    log "Installing OLM v1 (operator-controller $OPERATOR_CONTROLLER_VERSION)"
    # cert-manager's webhook may still start: its Certificate and Issuer are retried.
    wait_for 180 "operator-controller's manifests to apply" operator_controller_apply "$cm_namespace"
    mark olmv1-system
  fi
  k -n olmv1-system rollout status deployment/catalogd-controller-manager --timeout=300s >/dev/null
  k -n olmv1-system rollout status deployment/operator-controller-controller-manager --timeout=300s >/dev/null
  k apply -f "$OC_URL/default-catalogs.yaml" >/dev/null
  wait_for 900 "ClusterCatalog operatorhubio to serve" \
    sh -c "kubectl --context '$CONTEXT' get clustercatalog operatorhubio -o jsonpath='{.status.conditions[?(@.type==\"Serving\")].status}' | grep -qx True"
  # operator-controller 1.12+ installs with its own service account (spec.serviceAccount is
  # deprecated and ignored).
  log "ClusterExtension kubyl-v1-sample (grafana-operator)"
  k apply -f - >/dev/null <<'EOF'
apiVersion: v1
kind: Namespace
metadata:
  name: kubyl-v1-sample
---
apiVersion: olm.operatorframework.io/v1
kind: ClusterExtension
metadata:
  name: kubyl-v1-sample
spec:
  namespace: kubyl-v1-sample
  source:
    sourceType: Catalog
    catalog:
      packageName: grafana-operator
EOF
fi

log "Done. Installed Operators: kubectl --context $CONTEXT get csv -A -l '!olm.copiedFrom'"
