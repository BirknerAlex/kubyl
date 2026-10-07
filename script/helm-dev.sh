#!/usr/bin/env bash
# Helm fixtures for the kind cluster (phase 22). Run after script/dev-cluster.sh.
#
# - A local HTTP chart repository `kubyl-dev` (http://127.0.0.1:8879, a python3 http.server in
#   the background) with:
#     kubyl-demo 0.1.0     a Deployment (pause image), Service, ConfigMap and a Secret from values
#     kubyl-demo 0.2.0     the same plus values.schema.json, a post-install/post-upgrade hook
#                          Job, a CRD in crds/ and a PVC kept on uninstall
#                          (helm.sh/resource-policy: keep)
#     kubyl-hookfail 0.1.0 a post-install hook that fails
# - An OCI registry `kubyl-helm-registry` (registry:2 on 127.0.0.1:5022, joined to kind's
#   `kind` network like kind's local-registry setup) with oci://localhost:5022/charts/kubyl-demo
#   0.2.0. Plain HTTP: Helm needs `--plain-http` for it (Kubyl adds it for OCI references to
#   localhost and 127.0.0.1).
# - In namespace `kubyl-helm`: the release `kubyl-demo` (0.1.0, two revisions) and the release
#   `kubyl-stuck`, left in `pending-upgrade` (an upgrade with --wait whose helm was killed).
#
# Helm's own files are used, like Kubyl uses them: the repository is added to the
# repositories.yaml of `helm env` (HELM_REPOSITORY_CONFIG). Set HELM_REPOSITORY_CONFIG,
# HELM_REPOSITORY_CACHE and HELM_REGISTRY_CONFIG to scratch paths (and run Kubyl with the same
# variables) to keep the fixtures out of your Helm setup.
#
# Everything is marked: the namespace has the label kubyl.dev/owner=helm-dev, the registry
# container the label kubyl.dev/owner=helm-dev; --delete removes only what's marked.
#
# Usage:
#   script/helm-dev.sh           repository, registry, releases
#   script/helm-dev.sh --stuck   only (re)create the stuck release
#   script/helm-dev.sh --delete  remove the releases, namespace, registry, repository server and
#                                the kubyl-dev repository entry
#
# Needs: helm (3.13+), kubectl, docker, python3, curl. Respects $KUBECONFIG and $CONTEXT.
set -euo pipefail

CONTEXT="${CONTEXT:-kind-kubyl-dev}"
NS="kubyl-helm"
WORK="${KUBYL_HELM_DEV_DIR:-/tmp/kubyl-dev/helm}"
REPO_PORT="${REPO_PORT:-8879}"
REGISTRY_PORT="${REGISTRY_PORT:-5022}"
REGISTRY="kubyl-helm-registry"
OWNER="kubyl.dev/owner=helm-dev"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

for tool in helm kubectl python3 curl; do
  command -v "$tool" >/dev/null 2>&1 || die "$tool is not installed"
done

k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

stop_repo() {
  if [ -f "$WORK/repo.pid" ]; then
    kill "$(cat "$WORK/repo.pid")" >/dev/null 2>&1 || true
    rm -f "$WORK/repo.pid"
  fi
}

delete_all() {
  log "Removing the releases in $NS"
  if [ "$(k get namespace "$NS" -o jsonpath='{.metadata.labels.kubyl\.dev/owner}' 2>/dev/null)" = "helm-dev" ]; then
    for release in $( (h list -n "$NS" -a -q 2>/dev/null || h list -n "$NS" -q) 2>/dev/null); do
      h uninstall "$release" -n "$NS" --no-hooks >/dev/null 2>&1 || true
    done
    k delete crd widgets.demo.kubyl.dev --ignore-not-found >/dev/null 2>&1 || true
    k delete namespace "$NS" --ignore-not-found --wait=false >/dev/null
  fi
  if command -v docker >/dev/null 2>&1 \
    && [ "$(docker inspect -f '{{index .Config.Labels "kubyl.dev/owner"}}' "$REGISTRY" 2>/dev/null)" = "helm-dev" ]; then
    log "Removing the registry $REGISTRY"
    docker rm -f "$REGISTRY" >/dev/null
  fi
  stop_repo
  helm repo remove kubyl-dev >/dev/null 2>&1 || true
  log "Done"
}

write_charts() {
  rm -rf "$WORK/src" "$WORK/repo"
  mkdir -p "$WORK/src" "$WORK/repo"
  for version in 0.1.0 0.2.0; do
    local dir="$WORK/src/$version/kubyl-demo"
    mkdir -p "$dir/templates"
    cat >"$dir/Chart.yaml" <<EOF
apiVersion: v2
name: kubyl-demo
description: A small chart for Kubyl's Helm views (phase 22).
type: application
version: $version
appVersion: "1.$( [ "$version" = 0.1.0 ] && echo 0 || echo 1 ).0"
keywords: [kubyl, demo]
maintainers:
  - name: Kubyl
home: https://kubyl.dev
EOF
    cat >"$dir/values.yaml" <<'EOF'
# How many pods run.
replicaCount: 1
image:
  repository: registry.k8s.io/pause
  tag: "3.10"
service:
  port: 80
# Shown in the ConfigMap.
greeting: hello
auth:
  # Rendered into a Secret.
  password: change-me
EOF
    cat >"$dir/README.md" <<EOF
# kubyl-demo $version

A small chart to try Kubyl's Helm views: install, upgrade, rollback and uninstall.

| Value | Default | What it does |
|---|---|---|
| \`replicaCount\` | \`1\` | How many pods run |
| \`greeting\` | \`hello\` | Shown in the ConfigMap |
| \`auth.password\` | \`change-me\` | Rendered into a Secret |
EOF
    cat >"$dir/templates/deployment.yaml" <<'EOF'
apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ .Release.Name }}
  labels:
    app.kubernetes.io/name: kubyl-demo
    app.kubernetes.io/instance: {{ .Release.Name }}
spec:
  replicas: {{ .Values.replicaCount }}
  selector:
    matchLabels:
      app.kubernetes.io/instance: {{ .Release.Name }}
  template:
    metadata:
      labels:
        app.kubernetes.io/name: kubyl-demo
        app.kubernetes.io/instance: {{ .Release.Name }}
    spec:
      containers:
        - name: app
          image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"
          {{- if .Values.storage }}
          volumeMounts:
            - name: data
              mountPath: /data
          {{- end }}
      {{- if .Values.storage }}
      # Mounted so the PVC binds (kind's local-path class waits for a consumer).
      volumes:
        - name: data
          persistentVolumeClaim:
            claimName: {{ .Release.Name }}-data
      {{- end }}
EOF
    cat >"$dir/templates/service.yaml" <<'EOF'
apiVersion: v1
kind: Service
metadata:
  name: {{ .Release.Name }}
spec:
  selector:
    app.kubernetes.io/instance: {{ .Release.Name }}
  ports:
    - port: {{ .Values.service.port }}
EOF
    cat >"$dir/templates/configmap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}-settings
data:
  greeting: {{ .Values.greeting | quote }}
EOF
    cat >"$dir/templates/secret.yaml" <<'EOF'
apiVersion: v1
kind: Secret
metadata:
  name: {{ .Release.Name }}-auth
type: Opaque
stringData:
  password: {{ .Values.auth.password | quote }}
EOF
    cat >"$dir/templates/NOTES.txt" <<'EOF'
kubyl-demo {{ .Chart.Version }} is installed as {{ .Release.Name }} in {{ .Release.Namespace }}.
EOF
    if [ "$version" = 0.2.0 ]; then
      mkdir -p "$dir/crds"
      cat >"$dir/values.schema.json" <<'EOF'
{
  "$schema": "https://json-schema.org/draft-07/schema#",
  "type": "object",
  "required": ["replicaCount"],
  "properties": {
    "replicaCount": { "type": "integer", "minimum": 0, "maximum": 10 },
    "greeting": { "type": "string" },
    "image": {
      "type": "object",
      "properties": { "repository": { "type": "string" }, "tag": { "type": "string" } }
    },
    "service": { "type": "object", "properties": { "port": { "type": "integer" } } },
    "auth": { "type": "object", "properties": { "password": { "type": "string" } } },
    "storage": { "type": "string", "enum": ["1Mi", "2Mi"] }
  }
}
EOF
      printf 'storage: 1Mi\n' >>"$dir/values.yaml"
      cat >"$dir/templates/hook.yaml" <<'EOF'
apiVersion: batch/v1
kind: Job
metadata:
  name: {{ .Release.Name }}-migrate
  annotations:
    helm.sh/hook: post-install,post-upgrade
    helm.sh/hook-delete-policy: before-hook-creation,hook-succeeded
spec:
  backoffLimit: 0
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: migrate
          image: busybox:1.36
          command: ["sh", "-c", "echo migrating; sleep 2"]
EOF
      cat >"$dir/templates/pvc.yaml" <<'EOF'
apiVersion: v1
kind: PersistentVolumeClaim
metadata:
  name: {{ .Release.Name }}-data
  annotations:
    helm.sh/resource-policy: keep
spec:
  accessModes: [ReadWriteOnce]
  resources:
    requests:
      storage: {{ .Values.storage }}
EOF
      cat >"$dir/crds/widgets.yaml" <<'EOF'
apiVersion: apiextensions.k8s.io/v1
kind: CustomResourceDefinition
metadata:
  name: widgets.demo.kubyl.dev
spec:
  group: demo.kubyl.dev
  scope: Namespaced
  names: { plural: widgets, singular: widget, kind: Widget }
  versions:
    - name: v1
      served: true
      storage: true
      schema:
        openAPIV3Schema:
          type: object
          x-kubernetes-preserve-unknown-fields: true
EOF
    fi
    helm package "$dir" -d "$WORK/repo" >/dev/null
  done
  local fail="$WORK/src/hookfail/kubyl-hookfail"
  mkdir -p "$fail/templates"
  cat >"$fail/Chart.yaml" <<'EOF'
apiVersion: v2
name: kubyl-hookfail
description: A chart whose post-install hook fails (phase 22).
version: 0.1.0
appVersion: "0.1.0"
EOF
  printf 'message: this hook fails\n' >"$fail/values.yaml"
  cat >"$fail/templates/configmap.yaml" <<'EOF'
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ .Release.Name }}
data:
  message: {{ .Values.message | quote }}
EOF
  cat >"$fail/templates/hook.yaml" <<'EOF'
apiVersion: batch/v1
kind: Job
metadata:
  name: {{ .Release.Name }}-check
  annotations:
    helm.sh/hook: post-install
spec:
  backoffLimit: 0
  template:
    spec:
      restartPolicy: Never
      containers:
        - name: check
          image: busybox:1.36
          command: ["sh", "-c", "echo {{ .Values.message }}; exit 1"]
EOF
  helm package "$fail" -d "$WORK/repo" >/dev/null
  helm repo index "$WORK/repo" --url "http://127.0.0.1:$REPO_PORT" >/dev/null
}

start_repo() {
  stop_repo
  log "Serving the chart repository on http://127.0.0.1:$REPO_PORT"
  nohup python3 -m http.server "$REPO_PORT" --bind 127.0.0.1 --directory "$WORK/repo" \
    </dev/null >"$WORK/repo.log" 2>&1 &
  echo $! >"$WORK/repo.pid"
  for _ in $(seq 1 20); do
    curl -fsS "http://127.0.0.1:$REPO_PORT/index.yaml" >/dev/null 2>&1 && break
    sleep 0.25
  done
  helm repo add kubyl-dev "http://127.0.0.1:$REPO_PORT" --force-update >/dev/null
  helm repo update kubyl-dev >/dev/null
}

start_registry() {
  command -v docker >/dev/null 2>&1 || { log "docker isn't installed: skipping the OCI registry"; return; }
  if ! docker inspect "$REGISTRY" >/dev/null 2>&1; then
    log "Starting the OCI registry $REGISTRY on 127.0.0.1:$REGISTRY_PORT"
    docker run -d --restart=always --name "$REGISTRY" --label "$OWNER" \
      -p "127.0.0.1:$REGISTRY_PORT:5000" registry:2 >/dev/null
    docker network connect kind "$REGISTRY" >/dev/null 2>&1 || true
  fi
  for _ in $(seq 1 20); do
    curl -fsS "http://127.0.0.1:$REGISTRY_PORT/v2/" >/dev/null 2>&1 && break
    sleep 0.5
  done
  log "Pushing kubyl-demo 0.2.0 to oci://localhost:$REGISTRY_PORT/charts"
  helm push "$WORK/repo/kubyl-demo-0.2.0.tgz" "oci://localhost:$REGISTRY_PORT/charts" --plain-http >/dev/null 2>&1 \
    || helm push "$WORK/repo/kubyl-demo-0.2.0.tgz" "oci://localhost:$REGISTRY_PORT/charts" >/dev/null
}

ensure_namespace() {
  if ! k get namespace "$NS" >/dev/null 2>&1; then
    k create namespace "$NS" >/dev/null
    k label namespace "$NS" "$OWNER" >/dev/null
  fi
  # Never adopt a namespace someone else made (--delete would remove it).
  [ "$(k get namespace "$NS" -o jsonpath='{.metadata.labels.kubyl\.dev/owner}')" = "helm-dev" ] \
    || die "namespace $NS exists and isn't marked $OWNER: not touching it"
}

releases() {
  ensure_namespace
  if ! h status kubyl-demo -n "$NS" >/dev/null 2>&1; then
    log "Installing kubyl-demo 0.1.0 (two revisions) in $NS"
    h install kubyl-demo kubyl-dev/kubyl-demo --version 0.1.0 -n "$NS" --wait --timeout 3m >/dev/null
    h upgrade kubyl-demo kubyl-dev/kubyl-demo --version 0.1.0 -n "$NS" --set greeting=hi --wait --timeout 3m >/dev/null
  fi
  stuck
}

stuck() {
  ensure_namespace
  h uninstall kubyl-stuck -n "$NS" --no-hooks >/dev/null 2>&1 || true
  log "Leaving kubyl-stuck in pending-upgrade"
  h install kubyl-stuck kubyl-dev/kubyl-demo --version 0.1.0 -n "$NS" --wait --timeout 3m >/dev/null
  # An upgrade that waits for an image that never comes, interrupted hard: Helm can't record
  # the failure and the release stays pending-upgrade.
  helm --kube-context "$CONTEXT" upgrade kubyl-stuck kubyl-dev/kubyl-demo --version 0.1.0 -n "$NS" \
    --set image.tag=does-not-exist --wait --timeout 10m >/dev/null 2>&1 &
  local pid=$!
  for _ in $(seq 1 60); do
    if [ "$(h history kubyl-stuck -n "$NS" -o json 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)[-1]["status"])' 2>/dev/null)" = "pending-upgrade" ]; then
      break
    fi
    sleep 0.5
  done
  sleep 1
  kill -9 "$pid" >/dev/null 2>&1 || true
  wait "$pid" 2>/dev/null || true
}

case "${1:-}" in
  --delete) delete_all; exit 0 ;;
  --stuck) stuck; exit 0 ;;
  "") ;;
  *) die "unknown argument: $1 (use --stuck or --delete)" ;;
esac

mkdir -p "$WORK"
log "Packaging the charts into $WORK/repo"
write_charts
start_repo
start_registry
releases
log "Ready: helm search repo kubyl-dev; oci://localhost:$REGISTRY_PORT/charts/kubyl-demo; releases in $NS"
h list -n "$NS" -a 2>/dev/null || h list -n "$NS"
