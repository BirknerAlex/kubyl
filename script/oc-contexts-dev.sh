#!/usr/bin/env bash
# Writes a scratch kubeconfig that looks like `oc` has been at work on the kind cluster from
# script/dev-cluster.sh, for phase 15's context grouping:
#   - one `oc`-style cluster entry (`127-0-0-1:<port>`) and user entry
#     (`kubernetes-admin/127-0-0-1:<port>`), with 30 contexts `<namespace>/<cluster>/<user>`,
#     like 30 `oc project` calls. Kubyl shows them as one row.
#   - a second user on the same server (ServiceAccount `payments/developer`, view role, a 24 h
#     token) with 2 contexts: a second, separate row.
#   - kind's own `kind-kubyl-dev` context: its cluster and user entries have other names but the
#     same contents (server, CA, client certificate), so it joins the first row.
#
# Only the scratch file is written, never ~/.kube/config. Point Kubyl at it with KUBECONFIG, or
# add it as a source.
#
# Usage:
#   script/oc-contexts-dev.sh [OUT]   default OUT: /tmp/kubyl-dev/oc-contexts.yaml
#   script/oc-contexts-dev.sh --delete  removes the ServiceAccount and its binding
#
# Needs: kubectl. Reads the kind kubeconfig from $KUBECONFIG (default /tmp/kubyl-dev/kubeconfig)
# and $CONTEXT (default kind-kubyl-dev).
set -euo pipefail

SOURCE="${KUBECONFIG:-/tmp/kubyl-dev/kubeconfig}"
CONTEXT="${CONTEXT:-kind-kubyl-dev}"
OUT="${1:-/tmp/kubyl-dev/oc-contexts.yaml}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
k() { kubectl --kubeconfig "$SOURCE" --context "$CONTEXT" "$@"; }

if [[ "${1:-}" == "--delete" ]]; then
  k -n payments delete rolebinding developer-view --ignore-not-found
  k -n payments delete serviceaccount developer --ignore-not-found
  exit 0
fi

# A path whose file may not exist yet: its folder's real path, then the name.
resolve() {
  local dir
  dir="$(dirname "$1")"
  if [[ -d "$dir" ]]; then
    printf '%s/%s\n' "$(cd "$dir" && pwd -P)" "$(basename "$1")"
  else
    printf '%s\n' "$1"
  fi
}
OUT_REAL="$(resolve "$OUT")"
KUBE_DIR="$(resolve "$HOME/.kube/config")"
KUBE_DIR="${KUBE_DIR%/config}"
# Checked before anything changes, also for files that don't exist yet.
if [[ "$OUT" -ef "$SOURCE" || "$OUT_REAL" == "$(resolve "$SOURCE")" ]]; then
  die "refusing to overwrite the source kubeconfig $SOURCE"
fi
if [[ "$OUT" -ef "$HOME/.kube/config" || "$OUT_REAL" == "$KUBE_DIR"/* ]]; then
  die "refusing to write into ~/.kube"
fi
k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

cfg() { kubectl config --kubeconfig "$OUT" "$@" >/dev/null; }
jsonpath() { kubectl config view --kubeconfig "$SOURCE" --raw --minify --context "$CONTEXT" -o jsonpath="$1"; }

SERVER="$(jsonpath '{.clusters[0].cluster.server}')"
# `oc login https://127.0.0.1:52341` names the cluster entry `127-0-0-1:52341`.
HOSTPORT="${SERVER#https://}"
CLUSTER="${HOSTPORT//./-}"
ADMIN="kubernetes-admin"
DEVELOPER="developer"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
umask 077
jsonpath '{.clusters[0].cluster.certificate-authority-data}' | base64 -d > "$WORK/ca.crt"
jsonpath '{.users[0].user.client-certificate-data}' | base64 -d > "$WORK/admin.crt"
jsonpath '{.users[0].user.client-key-data}' | base64 -d > "$WORK/admin.key"

log "Service account payments/$DEVELOPER (view) and a 24 h token"
k -n payments create serviceaccount "$DEVELOPER" --dry-run=client -o yaml | k apply -f - >/dev/null
k -n payments create rolebinding developer-view --clusterrole=view \
  --serviceaccount="payments:$DEVELOPER" --dry-run=client -o yaml | k apply -f - >/dev/null
TOKEN="$(k -n payments create token "$DEVELOPER" --duration=24h)"

mkdir -p "$(dirname "$OUT")"
rm -f "$OUT"
log "Writing $OUT"
cfg set-cluster "$CLUSTER" --server="$SERVER" --certificate-authority="$WORK/ca.crt" --embed-certs=true
cfg set-cluster "$CONTEXT" --server="$SERVER" --certificate-authority="$WORK/ca.crt" --embed-certs=true
cfg set-credentials "$ADMIN/$CLUSTER" --client-certificate="$WORK/admin.crt" \
  --client-key="$WORK/admin.key" --embed-certs=true
cfg set-credentials "$CONTEXT" --client-certificate="$WORK/admin.crt" \
  --client-key="$WORK/admin.key" --embed-certs=true
cfg set-credentials "$DEVELOPER/$CLUSTER" --token="$TOKEN"

# 30 `oc project` calls: the namespaces of the cluster first, then made-up ones (a context may
# name a namespace that doesn't exist).
# (No mapfile: macOS ships bash 3.2. Namespace names never contain spaces.)
# shellcheck disable=SC2207
NAMESPACES=($(k get namespaces -o jsonpath='{.items[*].metadata.name}'))
for n in $(seq -w 1 30); do NAMESPACES+=("team-$n"); done
for ns in "${NAMESPACES[@]:0:30}"; do
  cfg set-context "$ns/$CLUSTER/$ADMIN" --cluster="$CLUSTER" --user="$ADMIN/$CLUSTER" --namespace="$ns"
done
for ns in payments default; do
  cfg set-context "$ns/$CLUSTER/$DEVELOPER" --cluster="$CLUSTER" --user="$DEVELOPER/$CLUSTER" --namespace="$ns"
done
cfg set-context "$CONTEXT" --cluster="$CONTEXT" --user="$CONTEXT"
cfg use-context "payments/$CLUSTER/$ADMIN"
chmod 600 "$OUT"

log "$(kubectl config --kubeconfig "$OUT" get-contexts -o name | wc -l | tr -d ' ') contexts; Kubyl shows 2 rows:"
echo "  ${HOSTPORT} · $ADMIN   (31 contexts with $CONTEXT, starts in payments)"
echo "  ${HOSTPORT} · $DEVELOPER   (2 contexts)"
echo
echo "Try it:  KUBECONFIG=$OUT cargo run -p kubyl"
echo "Then run, while Kubyl is open:  kubectl config --kubeconfig $OUT set-context team-99/$CLUSTER/$ADMIN \\"
echo "           --cluster=$CLUSTER --user=$ADMIN/$CLUSTER --namespace=team-99 && \\"
echo "         kubectl config --kubeconfig $OUT use-context team-99/$CLUSTER/$ADMIN"
echo "(like \`oc project team-99\`: no new row, no reconnect)."
