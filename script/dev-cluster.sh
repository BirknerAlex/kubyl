#!/usr/bin/env bash
# Creates a local kind cluster `kubyl-dev` with the sample workloads the mockups show:
# a deployment, a statefulset, a cronjob, a crashlooping pod, a pending pod, a 3-replica
# deployment with JSON logs (log view) and the cert-manager CRDs (with an Issuer and a
# Certificate), all in the `payments` namespace.
#
# Usage:
#   script/dev-cluster.sh            create the cluster (or update the workloads if it exists)
#   script/dev-cluster.sh --recreate delete and create again
#   script/dev-cluster.sh --delete   delete the cluster
#
# Needs: docker (running), kind, kubectl. Respects $KUBECONFIG.
set -euo pipefail

CLUSTER="kubyl-dev"
CONTEXT="kind-${CLUSTER}"
NAMESPACE="payments"
CERT_MANAGER_VERSION="${CERT_MANAGER_VERSION:-v1.21.2}"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in docker kind kubectl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed (see https://kind.sigs.k8s.io/docs/user/quick-start/)"
done
docker info >/dev/null 2>&1 || die "docker is not running"

cluster_exists() { kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; }

case "${1:-}" in
  --delete)
    log "Deleting kind cluster $CLUSTER"
    kind delete cluster --name "$CLUSTER"
    exit 0
    ;;
  --recreate)
    cluster_exists && kind delete cluster --name "$CLUSTER"
    ;;
  "") ;;
  *) die "unknown argument: $1 (use --recreate or --delete)" ;;
esac

if cluster_exists; then
  log "Cluster $CLUSTER exists; updating workloads"
else
  log "Creating kind cluster $CLUSTER"
  kind create cluster --name "$CLUSTER" --wait 120s --config - <<'EOF'
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
  - role: worker
EOF
fi

k() { kubectl --context "$CONTEXT" "$@"; }

log "Installing cert-manager CRDs ($CERT_MANAGER_VERSION)"
k apply --server-side -f "https://github.com/cert-manager/cert-manager/releases/download/${CERT_MANAGER_VERSION}/cert-manager.crds.yaml" >/dev/null
k wait --for condition=Established --timeout=60s crd/certificates.cert-manager.io crd/issuers.cert-manager.io >/dev/null

log "Applying sample workloads to namespace $NAMESPACE"
k apply -f - <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: ${NAMESPACE}
---
# Healthy deployment with a service.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkout-api
  namespace: ${NAMESPACE}
  labels: { app: checkout-api, team: payments }
spec:
  replicas: 3
  selector:
    matchLabels: { app: checkout-api }
  template:
    metadata:
      labels: { app: checkout-api, team: payments, version: "2.14.1" }
    spec:
      containers:
        - name: api
          image: nginx:1.29-alpine
          ports: [{ name: http, containerPort: 80 }]
          resources:
            requests: { cpu: 25m, memory: 32Mi }
            limits: { cpu: 250m, memory: 128Mi }
          readinessProbe:
            httpGet: { path: /, port: http }
          volumeMounts:
            - { name: config, mountPath: /app/config/flags }
            - { name: db, mountPath: /app/config/secrets, readOnly: true }
            - { name: cache, mountPath: /var/cache/checkout }
      volumes:
        - { name: config, configMap: { name: checkout-config } }
        - { name: db, secret: { secretName: checkout-db } }
        - { name: cache, emptyDir: {} }
---
apiVersion: v1
kind: Service
metadata:
  name: checkout-api
  namespace: ${NAMESPACE}
spec:
  selector: { app: checkout-api }
  ports: [{ name: http, port: 80, targetPort: http }]
---
# StatefulSet with persistent volumes (kind ships a local-path storage class).
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: ledger-writer
  namespace: ${NAMESPACE}
  labels: { app: ledger-writer, team: payments }
spec:
  serviceName: ledger-writer
  replicas: 2
  selector:
    matchLabels: { app: ledger-writer }
  template:
    metadata:
      labels: { app: ledger-writer, team: payments }
    spec:
      containers:
        - name: writer
          image: busybox:1.37
          command: ["sh", "-c", "i=0; while true; do i=\$((i+1)); echo \"\$(date -Iseconds) INFO wrote ledger batch \$i\" | tee -a /data/ledger.log; sleep 5; done"]
          volumeMounts: [{ name: data, mountPath: /data }]
          resources:
            requests: { cpu: 10m, memory: 16Mi }
  volumeClaimTemplates:
    - metadata:
        name: data
      spec:
        accessModes: [ReadWriteOnce]
        resources:
          requests: { storage: 64Mi }
---
apiVersion: v1
kind: Service
metadata:
  name: ledger-writer
  namespace: ${NAMESPACE}
spec:
  clusterIP: None
  selector: { app: ledger-writer }
  ports: [{ port: 80 }]
---
# CronJob that runs every minute and completes.
apiVersion: batch/v1
kind: CronJob
metadata:
  name: settlement-batch
  namespace: ${NAMESPACE}
  labels: { app: settlement-batch, team: payments }
spec:
  schedule: "* * * * *"
  successfulJobsHistoryLimit: 3
  failedJobsHistoryLimit: 1
  jobTemplate:
    spec:
      template:
        metadata:
          labels: { app: settlement-batch }
        spec:
          restartPolicy: Never
          containers:
            - name: settle
              image: busybox:1.37
              command: ["sh", "-c", "echo settling payments; sleep 3; echo done"]
---
# Crashlooping deployment: the container exits after logging an error.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: payment-gateway
  namespace: ${NAMESPACE}
  labels: { app: payment-gateway, team: payments }
spec:
  replicas: 1
  selector:
    matchLabels: { app: payment-gateway }
  template:
    metadata:
      labels: { app: payment-gateway, team: payments }
    spec:
      containers:
        - name: gateway
          image: busybox:1.37
          command: ["sh", "-c", "echo 'INFO starting gateway'; sleep 2; echo 'ERROR upstream timeout calling bank-api:8443' >&2; exit 1"]
---
# Three replicas writing JSON and plain-text logs with levels (log view, board 2): an INFO line
# every 300 ms, WARN/ERROR with "timeout" now and then, DEBUG stats.
apiVersion: apps/v1
kind: Deployment
metadata:
  name: checkout-events
  namespace: ${NAMESPACE}
  labels: { app: checkout-events, team: payments }
spec:
  replicas: 3
  selector:
    matchLabels: { app: checkout-events }
  template:
    metadata:
      labels: { app: checkout-events, team: payments }
    spec:
      terminationGracePeriodSeconds: 1
      containers:
        - name: api
          image: busybox:1.37
          resources:
            requests: { cpu: 5m, memory: 8Mi }
          command:
            - sh
            - -c
            - |
              i=0
              while true; do
                i=\$((i+1))
                ms=\$((20 + i % 37))
                echo "{\"level\":\"info\",\"msg\":\"POST /v1/checkout 200\",\"order\":\"ord_\$i\",\"latency_ms\":\$ms,\"pod\":\"\$HOSTNAME\"}"
                if [ \$((i % 7)) -eq 0 ]; then echo "{\"level\":\"warn\",\"msg\":\"payment-gateway slow response\",\"attempt\":1,\"latency_ms\":1840}"; fi
                if [ \$((i % 13)) -eq 0 ]; then echo "ERROR upstream timeout after 2000ms calling payment-gateway:8443/authorize"; fi
                if [ \$((i % 5)) -eq 0 ]; then echo "DEBUG pool stats active=14 idle=6 waiting=0"; fi
                sleep 0.3
              done
---
# Pending pod: no node carries the requested label.
apiVersion: v1
kind: Pod
metadata:
  name: invoice-renderer-pending
  namespace: ${NAMESPACE}
  labels: { app: invoice-renderer, team: payments }
spec:
  nodeSelector:
    kubyl.dev/unschedulable: "true"
  containers:
    - name: renderer
      image: busybox:1.37
      command: ["sleep", "infinity"]
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: checkout-config
  namespace: ${NAMESPACE}
data:
  PAYMENT_TIMEOUT_MS: "2000"
  CURRENCY: EUR
---
# Dummy data only; never put real credentials here.
apiVersion: v1
kind: Secret
metadata:
  name: checkout-db
  namespace: ${NAMESPACE}
type: Opaque
stringData:
  username: checkout
  password: not-a-real-password
---
# cert-manager resources. Only the CRDs are installed, so these stay without status.
apiVersion: cert-manager.io/v1
kind: Issuer
metadata:
  name: selfsigned
  namespace: ${NAMESPACE}
spec:
  selfSigned: {}
---
apiVersion: cert-manager.io/v1
kind: Certificate
metadata:
  name: api-tls
  namespace: ${NAMESPACE}
spec:
  secretName: api-tls
  dnsNames: [checkout.payments.svc.cluster.local]
  issuerRef:
    name: selfsigned
    kind: Issuer
EOF

log "Waiting for checkout-api to become ready"
k -n "$NAMESPACE" rollout status deployment/checkout-api --timeout=180s

log "Done. Context: $CONTEXT"
k -n "$NAMESPACE" get pods -o wide
