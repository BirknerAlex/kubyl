#!/usr/bin/env bash
# Creates a namespace `load` with 5,000 pods for the list smoothness check (phase 02), plus one
# crashlooping pod to time status updates.
#
# The 5,000 pods come from a Deployment whose node selector matches no node, so they stay
# Pending: the API server holds 5,000 objects without running containers (kind nodes allow
# ~110 pods each). The crashlooping pod does get scheduled.
#
# Usage:
#   script/load-pods.sh              create (or resize) the load
#   script/load-pods.sh 20000        another pod count
#   script/load-pods.sh --churn      also restart 50 pods every 2 s (live update load)
#   script/load-pods.sh --delete     delete the namespace
#
# Needs: kubectl. Uses the current context of $KUBECONFIG (point it at the kind cluster from
# script/dev-cluster.sh, not a real cluster).
set -euo pipefail

NAMESPACE="load"
COUNT=5000
CHURN=false

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }

for arg in "$@"; do
  case "$arg" in
    --delete)
      log "Deleting namespace $NAMESPACE"
      kubectl delete namespace "$NAMESPACE" --wait=false
      exit 0
      ;;
    --churn) CHURN=true ;;
    ''|*[!0-9]*) echo "unknown argument: $arg" >&2; exit 1 ;;
    *) COUNT="$arg" ;;
  esac
done

log "Context: $(kubectl config current-context)"
kubectl apply -f - <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: ${NAMESPACE}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: bulk
  namespace: ${NAMESPACE}
  labels: { app: bulk }
spec:
  replicas: ${COUNT}
  selector:
    matchLabels: { app: bulk }
  template:
    metadata:
      labels: { app: bulk, tier: load }
    spec:
      # Matches no node: pods stay Pending and cost the cluster nothing but etcd space.
      nodeSelector: { kubyl.dev/never: "true" }
      containers:
        - name: pause
          image: registry.k8s.io/pause:3.10
          resources: { requests: { cpu: 1m, memory: 1Mi } }
---
apiVersion: v1
kind: Pod
metadata:
  name: crashloop
  namespace: ${NAMESPACE}
  labels: { app: crashloop }
spec:
  containers:
    - name: crash
      image: busybox:1.37
      command: ["sh", "-c", "echo starting; sleep 5; exit 1"]
EOF

log "Waiting for ${COUNT} pods (this takes a minute)"
for _ in $(seq 1 120); do
  have=$(kubectl -n "$NAMESPACE" get pods -l app=bulk --no-headers 2>/dev/null | wc -l | tr -d ' ')
  printf '\r    %s / %s' "$have" "$COUNT"
  [ "$have" -ge "$COUNT" ] && break
  sleep 2
done
echo

if $CHURN; then
  log "Churning: deleting 50 pods every 2 s (Ctrl-C to stop)"
  while true; do
    kubectl -n "$NAMESPACE" get pods -l app=bulk -o name | shuf -n 50 \
      | xargs kubectl -n "$NAMESPACE" delete --wait=false >/dev/null
    sleep 2
  done
fi
