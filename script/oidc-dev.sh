#!/usr/bin/env bash
# Creates a kind cluster `kubyl-oidc` whose API server authenticates users with a local Dex
# (OIDC), plus a kubelogin-style kubeconfig for it. Used to test Kubyl's OIDC sign-in
# (browser + PKCE, device code) and token refresh.
#
# Usage:
#   script/oidc-dev.sh            create Dex and the cluster (or recreate Dex if it exists)
#   script/oidc-dev.sh --delete   delete both
#
# Then add the printed kubeconfig in Kubyl (Clusters → Add) and sign in as
#   admin@kubyl.dev / password
#
# The issuer is https://dex.127.0.0.1.nip.io:5556/dex: on your machine the name resolves to
# 127.0.0.1 (public wildcard DNS), where Dex's port is published; inside the kind node an
# /etc/hosts entry points it at the Dex container. ID tokens expire after 2 minutes so refresh
# is exercised quickly. Override with DEX_ISSUER_HOST if your DNS blocks 127.0.0.1 answers.
#
# Needs: docker (running), kind, kubectl, openssl.
set -euo pipefail

CLUSTER="kubyl-oidc"
CONTEXT="kind-${CLUSTER}"
DEX_CONTAINER="kubyl-dex"
DEX_IMAGE="${DEX_IMAGE:-ghcr.io/dexidp/dex:v2.45.1}"
DEX_PORT=5556
ISSUER_HOST="${DEX_ISSUER_HOST:-dex.127.0.0.1.nip.io}"
ISSUER="https://${ISSUER_HOST}:${DEX_PORT}/dex"
CLIENT_ID="kubyl"
STATE_DIR="${KUBYL_OIDC_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/kubyl-oidc-dev}"
KUBECONFIG_OUT="${STATE_DIR}/kubeconfig.yaml"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in docker kind kubectl openssl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done
docker info >/dev/null 2>&1 || die "docker is not running"

if [[ "${1:-}" == "--delete" ]]; then
  log "Deleting $CLUSTER and $DEX_CONTAINER"
  kind delete cluster --name "$CLUSTER" || true
  docker rm -f "$DEX_CONTAINER" >/dev/null 2>&1 || true
  rm -rf "$STATE_DIR"
  exit 0
fi
[[ -z "${1:-}" ]] || die "unknown argument: $1 (use --delete)"

mkdir -p "$STATE_DIR/pki"

# --- TLS for Dex -----------------------------------------------------------------------------
if [[ ! -f "$STATE_DIR/pki/ca.crt" ]]; then
  log "Creating a CA and a certificate for $ISSUER_HOST"
  openssl req -x509 -newkey rsa:2048 -nodes -days 3650 -subj "/CN=kubyl-oidc-dev-ca" \
    -keyout "$STATE_DIR/pki/ca.key" -out "$STATE_DIR/pki/ca.crt" 2>/dev/null
  cat > "$STATE_DIR/pki/server.ext" <<EOF
basicConstraints=CA:FALSE
keyUsage=digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:${ISSUER_HOST},DNS:${DEX_CONTAINER},IP:127.0.0.1
EOF
  openssl req -newkey rsa:2048 -nodes -subj "/CN=${ISSUER_HOST}" \
    -keyout "$STATE_DIR/pki/server.key" -out "$STATE_DIR/pki/server.csr" 2>/dev/null
  openssl x509 -req -in "$STATE_DIR/pki/server.csr" -days 3650 \
    -CA "$STATE_DIR/pki/ca.crt" -CAkey "$STATE_DIR/pki/ca.key" -CAcreateserial \
    -extfile "$STATE_DIR/pki/server.ext" -out "$STATE_DIR/pki/server.crt" 2>/dev/null
  chmod 644 "$STATE_DIR/pki/server.key"
fi

# --- Dex -------------------------------------------------------------------------------------
cat > "$STATE_DIR/dex.yaml" <<EOF
issuer: ${ISSUER}
storage:
  type: memory
web:
  https: 0.0.0.0:${DEX_PORT}
  tlsCert: /pki/server.crt
  tlsKey: /pki/server.key
oauth2:
  skipApprovalScreen: true
expiry:
  idTokens: 2m
  refreshTokens:
    validIfNotUsedFor: 24h
enablePasswordDB: true
staticPasswords:
  - email: admin@kubyl.dev
    # "password"
    hash: "\$2a\$10\$2b2cU8CPhOTaGrs1HRQuAueS7JTT5ZHsHSzYiFPm1leZck7Mc8T4W"
    username: admin
    userID: 08a8684b-db88-4b73-90a9-3cd1661f5466
staticClients:
  - id: ${CLIENT_ID}
    name: Kubyl
    public: true
    # kubelogin-compatible loopback redirects, plus Dex's device flow callback.
    redirectURIs:
      - http://localhost:8000
      - http://localhost:18000
      - /device/callback
EOF

docker network inspect kind >/dev/null 2>&1 || docker network create kind >/dev/null
log "Starting Dex ($DEX_IMAGE) as $DEX_CONTAINER"
docker rm -f "$DEX_CONTAINER" >/dev/null 2>&1 || true
docker run -d --name "$DEX_CONTAINER" --network kind \
  -p "127.0.0.1:${DEX_PORT}:${DEX_PORT}" \
  -v "$STATE_DIR/dex.yaml:/etc/dex/config.yaml:ro" \
  -v "$STATE_DIR/pki:/pki:ro" \
  "$DEX_IMAGE" dex serve /etc/dex/config.yaml >/dev/null
for _ in $(seq 1 30); do
  curl -sf --cacert "$STATE_DIR/pki/ca.crt" "${ISSUER}/.well-known/openid-configuration" >/dev/null && break
  sleep 1
done
curl -sf --cacert "$STATE_DIR/pki/ca.crt" "${ISSUER}/.well-known/openid-configuration" >/dev/null \
  || die "Dex is not reachable at $ISSUER (does $ISSUER_HOST resolve to 127.0.0.1?)"
DEX_IP="$(docker inspect -f '{{(index .NetworkSettings.Networks "kind").IPAddress}}' "$DEX_CONTAINER")"

# --- kind cluster ----------------------------------------------------------------------------
if ! kind get clusters 2>/dev/null | grep -qx "$CLUSTER"; then
  log "Creating kind cluster $CLUSTER with OIDC authentication"
  kind create cluster --name "$CLUSTER" --kubeconfig "$STATE_DIR/admin.kubeconfig" --wait 120s --config - <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
nodes:
  - role: control-plane
    extraMounts:
      - hostPath: ${STATE_DIR}/pki/ca.crt
        containerPath: /etc/kubernetes/pki/oidc-ca.crt
        readOnly: true
kubeadmConfigPatches:
  - |
    kind: ClusterConfiguration
    apiServer:
      extraArgs:
        - name: oidc-issuer-url
          value: ${ISSUER}
        - name: oidc-client-id
          value: ${CLIENT_ID}
        - name: oidc-username-claim
          value: email
        - name: oidc-username-prefix
          value: "oidc:"
        - name: oidc-ca-file
          value: /etc/kubernetes/pki/oidc-ca.crt
EOF
fi
NODE="${CLUSTER}-control-plane"

# The API server (host network) resolves the issuer through the node's /etc/hosts, copied when
# its pod starts: add the entry, then restart the static pod.
if ! docker exec "$NODE" grep -q " ${ISSUER_HOST}\$" /etc/hosts; then
  log "Pointing $ISSUER_HOST at Dex ($DEX_IP) inside the node"
  docker exec "$NODE" sh -c "echo '${DEX_IP} ${ISSUER_HOST}' >> /etc/hosts"
  docker exec "$NODE" sh -c 'mv /etc/kubernetes/manifests/kube-apiserver.yaml /tmp/ && sleep 8 && mv /tmp/kube-apiserver.yaml /etc/kubernetes/manifests/'
else
  docker exec "$NODE" sh -c "sed -i 's/^.* ${ISSUER_HOST}\$/${DEX_IP} ${ISSUER_HOST}/' /etc/hosts" || true
fi
kind get kubeconfig --name "$CLUSTER" > "$STATE_DIR/admin.kubeconfig"
for _ in $(seq 1 60); do
  kubectl --kubeconfig "$STATE_DIR/admin.kubeconfig" get --raw /readyz >/dev/null 2>&1 && break
  sleep 2
done

log "Granting oidc:admin@kubyl.dev the view role"
kubectl --kubeconfig "$STATE_DIR/admin.kubeconfig" apply -f - >/dev/null <<EOF
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: kubyl-oidc-admin-view
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: view
subjects:
  - apiGroup: rbac.authorization.k8s.io
    kind: User
    name: oidc:admin@kubyl.dev
EOF

# --- kubeconfig for Kubyl ----------------------------------------------------------------------
SERVER="$(kubectl --kubeconfig "$STATE_DIR/admin.kubeconfig" config view --minify -o jsonpath='{.clusters[0].cluster.server}')"
CA_DATA="$(kubectl --kubeconfig "$STATE_DIR/admin.kubeconfig" config view --minify --raw -o jsonpath='{.clusters[0].cluster.certificate-authority-data}')"
cat > "$KUBECONFIG_OUT" <<EOF
apiVersion: v1
kind: Config
current-context: ${CLUSTER}
clusters:
  - name: ${CLUSTER}
    cluster:
      server: ${SERVER}
      certificate-authority-data: ${CA_DATA}
contexts:
  - name: ${CLUSTER}
    context:
      cluster: ${CLUSTER}
      user: oidc
users:
  - name: oidc
    user:
      exec:
        apiVersion: client.authentication.k8s.io/v1
        command: kubectl
        args:
          - oidc-login
          - get-token
          - --oidc-issuer-url=${ISSUER}
          - --oidc-client-id=${CLIENT_ID}
          - --oidc-extra-scope=email
          - --oidc-extra-scope=offline_access
          - --certificate-authority=${STATE_DIR}/pki/ca.crt
        interactiveMode: IfAvailable
EOF
chmod 600 "$KUBECONFIG_OUT"

log "Done."
cat <<EOF

  Issuer      ${ISSUER}
  Client ID   ${CLIENT_ID} (public, PKCE)
  User        admin@kubyl.dev / password   (bound to the "view" ClusterRole)
  Kubeconfig  ${KUBECONFIG_OUT}

Add the kubeconfig in Kubyl (Clusters & kubeconfigs → Add) and connect to "${CLUSTER}".
ID tokens expire after 2 minutes; Kubyl refreshes them with the refresh token.
EOF
