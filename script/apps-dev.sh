#!/usr/bin/env bash
# Sample applications for the Applications view of phase 25 on the kind cluster (context
# kind-kubyl-dev, run script/dev-cluster.sh first). In namespace `kubyl-apps`:
#
# - `shop`     a healthy app: Deployments `shop-web` and `shop-api` (version labels 2.3.1 and
#              2.3.0), a Service and a ConfigMap; managed-by label `kustomize`.
# - `ledger`   a StatefulSet whose second container's image doesn't exist (no replica ever
#              becomes ready: Degraded); Helm's release annotations on it make "Managed by"
#              read `Helm · ledger` without a Helm release.
# - `report`   a suspended CronJob (Suspended), the shape of a batch app without a Service.
# - `idle`     a Deployment scaled to 0 (Suspended).
#
# Everything carries kubyl.dev/installed-by=apps-dev.sh; --delete removes the namespace.
#
# Usage:
#   script/apps-dev.sh            install or update
#   script/apps-dev.sh --delete   remove the namespace
#
# Live test: crates/kubyl_apps_core/tests/live.rs. Respects $KUBECONFIG and $KUBYL_APPS_CONTEXT
# (default kind-kubyl-dev; it refuses other contexts without --force).
set -euo pipefail

CONTEXT="${KUBYL_APPS_CONTEXT:-kind-kubyl-dev}"
NS="kubyl-apps"
MARK="kubyl.dev/installed-by"
ME="apps-dev.sh"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
DELETE=0
FORCE=0
for arg in "$@"; do
  case "$arg" in
    --delete) DELETE=1 ;;
    --force) FORCE=1 ;;
    *) die "unknown option $arg" ;;
  esac
done
case "$CONTEXT" in kind-*) ;; *) [ "$FORCE" = 1 ] || die "refusing context $CONTEXT (not kind-*); --force allows it" ;; esac
k() { kubectl --context "$CONTEXT" "$@"; }

if [ "$DELETE" = 1 ]; then
  if k get ns "$NS" -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null | grep -q "$ME"; then
    log "deleting namespace $NS"
    k delete ns "$NS" --wait=true
  else
    log "namespace $NS isn't ours (or is gone): nothing to delete"
  fi
  exit 0
fi

if k get ns "$NS" >/dev/null 2>&1 && ! k get ns "$NS" -o jsonpath="{.metadata.annotations.kubyl\.dev/installed-by}" | grep -q "$ME"; then
  die "namespace $NS exists and isn't ours"
fi

log "applying the sample applications to $CONTEXT"
k apply -f - <<YAML
apiVersion: v1
kind: Namespace
metadata:
  name: $NS
  annotations: {$MARK: $ME}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: shop-web
  namespace: $NS
  labels: &shop-web
    app.kubernetes.io/instance: shop
    app.kubernetes.io/name: web
    app.kubernetes.io/component: frontend
    app.kubernetes.io/part-of: webshop
    app.kubernetes.io/version: "2.3.1"
    app.kubernetes.io/managed-by: kustomize
    $MARK: $ME
spec:
  replicas: 2
  selector: {matchLabels: {app.kubernetes.io/instance: shop, app.kubernetes.io/name: web}}
  template:
    metadata: {labels: {app.kubernetes.io/instance: shop, app.kubernetes.io/name: web}}
    spec:
      containers:
        - name: web
          image: busybox:1.37
          command: ["sh", "-c", "while true; do echo shop-web \$(date) request handled; sleep 5; done"]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: shop-api
  namespace: $NS
  labels:
    app.kubernetes.io/instance: shop
    app.kubernetes.io/name: api
    app.kubernetes.io/component: backend
    app.kubernetes.io/part-of: webshop
    app.kubernetes.io/version: "2.3.0"
    app.kubernetes.io/managed-by: kustomize
    $MARK: $ME
spec:
  replicas: 1
  selector: {matchLabels: {app.kubernetes.io/instance: shop, app.kubernetes.io/name: api}}
  template:
    metadata: {labels: {app.kubernetes.io/instance: shop, app.kubernetes.io/name: api}}
    spec:
      containers:
        - name: api
          image: busybox:1.37
          command: ["sh", "-c", "while true; do echo shop-api \$(date) query ok; sleep 7; done"]
---
apiVersion: v1
kind: Service
metadata:
  name: shop-web
  namespace: $NS
  labels:
    app.kubernetes.io/instance: shop
    app.kubernetes.io/name: web
    app.kubernetes.io/managed-by: kustomize
    $MARK: $ME
spec:
  selector: {app.kubernetes.io/instance: shop, app.kubernetes.io/name: web}
  ports: [{port: 80, targetPort: 8080}]
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: shop-config
  namespace: $NS
  labels:
    app.kubernetes.io/instance: shop
    app.kubernetes.io/managed-by: kustomize
    $MARK: $ME
data: {theme: dark}
---
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: ledger-db
  namespace: $NS
  labels:
    app.kubernetes.io/instance: ledger
    app.kubernetes.io/name: postgres
    app.kubernetes.io/version: "16.4"
    app.kubernetes.io/managed-by: Helm
    $MARK: $ME
  annotations: {meta.helm.sh/release-name: ledger, meta.helm.sh/release-namespace: $NS}
spec:
  serviceName: ledger-db
  replicas: 2
  podManagementPolicy: Parallel
  selector: {matchLabels: {app.kubernetes.io/instance: ledger}}
  template:
    metadata: {labels: {app.kubernetes.io/instance: ledger}}
    spec:
      containers:
        - name: db
          image: busybox:1.37
          command: ["sh", "-c", "sleep 3600"]
        - name: broken-sidecar
          image: registry.invalid/kubyl/does-not-exist:1
---
apiVersion: batch/v1
kind: CronJob
metadata:
  name: report
  namespace: $NS
  labels:
    app.kubernetes.io/instance: report
    app.kubernetes.io/name: report
    app.kubernetes.io/version: "0.9.0"
    $MARK: $ME
spec:
  schedule: "0 6 * * *"
  suspend: true
  jobTemplate:
    spec:
      template:
        spec:
          restartPolicy: Never
          containers: [{name: report, image: busybox:1.37, command: ["true"]}]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: idle
  namespace: $NS
  labels:
    app.kubernetes.io/instance: idle
    app.kubernetes.io/name: idle
    $MARK: $ME
spec:
  replicas: 0
  selector: {matchLabels: {app.kubernetes.io/instance: idle}}
  template:
    metadata: {labels: {app.kubernetes.io/instance: idle}}
    spec:
      containers: [{name: idle, image: "busybox:1.37", command: ["sleep", "3600"]}]
YAML
log "done: open Applications in Kubyl (namespace $NS)"
