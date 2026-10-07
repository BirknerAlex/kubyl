#!/usr/bin/env bash
# Objects for the resource views of phase 24 on the kind cluster (context kind-kubyl-dev, run
# script/dev-cluster.sh first): device resources (DRA), admission policies, the Gateway API,
# VerticalPodAutoscalers, plus a RuntimeClass.
#
# - DRA: the dra-example-driver Helm chart (registry.k8s.io/dra-example-driver, "gpu" profile:
#   8 fake GPUs per node, DeviceClass `gpu.example.com`) in namespace `kubyl-dra`, and in
#   `kubyl-views` a ResourceClaimTemplate `single-gpu`, a ResourceClaim `shared-gpu` and two
#   pods using them (`gpu-job-0` through the template, `gpu-shared` through the claim).
#   With --fake-dra (or when the driver doesn't come up), no chart: a DeviceClass
#   `fake-gpu.kubyl.dev` and a hand-made ResourceSlice per node for the made-up driver
#   `fake-gpu.kubyl.dev` instead. The scheduler allocates claims from it all the same; the pods
#   then wait in ContainerCreating (no kubelet plugin prepares the devices).
# - Admission policies: ValidatingAdmissionPolicy `kubyl-replica-limit` (a param ConfigMap,
#   variables, a match condition, an audit annotation) with the binding
#   `kubyl-replica-limit-views` (Deny, Audit) for namespaces labelled kubyl.dev/views=true, and
#   where served (Kubernetes 1.34+ beta, 1.36+ v1) MutatingAdmissionPolicy `kubyl-default-labels`
#   with a binding. The Gateway API install adds its own `safe-upgrades.gateway.networking.k8s.io`.
# - Gateway API v1.6 (experimental channel CRDs), GatewayClass `kubyl-fake`, Gateway
#   `kubyl-views/web-gateway` (http and https listeners), HTTPRoute `shop` (two rules to the
#   Services `web` and `web-canary`), GRPCRoute `checkout` and a ReferenceGrant. No controller
#   runs: the script writes the status one would (accepted, programmed, attached routes).
# - The VPA CRDs (autoscaler vertical-pod-autoscaler-1.8.0) and VPA `web` for the Deployment
#   `web` with a recommendation written by the script (no recommender runs).
# - RuntimeClass `kubyl-gvisor` (handler runsc, pod overhead).
#
# Everything the script creates carries kubyl.dev/installed-by=views-dev.sh (a label, and an
# annotation on CRDs and namespaces); --delete removes only what's marked. CRDs that were there
# before stay.
#
# Usage:
#   script/views-dev.sh             install or update
#   script/views-dev.sh --fake-dra  a hand-made ResourceSlice instead of the example driver
#   script/views-dev.sh --delete    remove what the script installed (waits for the namespaces)
#   script/views-dev.sh --force     allow a context that isn't a kind cluster (kind-*)
#
# Switching between the example driver and --fake-dra needs a --delete first (a claim's
# device class can't change). The namespaces must be new or the script's own: it refuses to
# take over an existing kubyl-views or kubyl-dra.
#
# Needs: kubectl, helm, curl. Respects $KUBECONFIG and $KUBYL_VIEWS_CONTEXT (default kind-kubyl-dev;
# it installs cluster-scoped CRDs and policies, so only kind-* contexts without --force).
set -euo pipefail

CONTEXT="${KUBYL_VIEWS_CONTEXT:-kind-kubyl-dev}"
GATEWAY_API_VERSION="${GATEWAY_API_VERSION:-v1.6.3}"
VPA_VERSION="${VPA_VERSION:-vertical-pod-autoscaler-1.8.0}"
DRA_CHART="oci://registry.k8s.io/dra-example-driver/charts/dra-example-driver"
DRA_CHART_VERSION="${DRA_CHART_VERSION:-0.5.0}"
NS="kubyl-views"
DRA_NS="kubyl-dra"
MARK="kubyl.dev/installed-by"
ME="views-dev.sh"

log() { printf '\033[1;34m==>\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }
die() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }

command -v kubectl >/dev/null 2>&1 || die "kubectl is not installed"
command -v helm >/dev/null 2>&1 || die "helm is not installed"
command -v curl >/dev/null 2>&1 || die "curl is not installed"

k() { kubectl --context "$CONTEXT" "$@"; }
h() { helm --kube-context "$CONTEXT" "$@"; }


served() {
  # served <plural> <group>
  local names
  names="$(k api-resources --api-group="$2" -o name 2>/dev/null || true)"
  grep -qx "$1.$2" <<<"$names"
}

# The preferred version of a group (`v1`, `v1beta1`).
version_of() {
  local versions v
  versions="$(k api-versions | awk -F/ -v g="$1" '$1 == g { print $2 }')"
  for v in v1 v1beta2 v1beta1 v1alpha3 v1alpha2 v1alpha1; do
    if grep -qx "$v" <<<"$versions"; then
      echo "$v"
      return
    fi
  done
}

marked() {
  # marked <kind> <name> [namespace]: whether the object carries the mark (label or annotation).
  local out
  out="$(k get "$1" "$2" ${3:+-n "$3"} -o jsonpath="{.metadata.labels.kubyl\.dev/installed-by}{.metadata.annotations.kubyl\.dev/installed-by}" 2>/dev/null || true)"
  [[ "$out" == *"$ME"* ]]
}

FAKE_DRA=false
DELETE=false
FORCE=false
for arg in "$@"; do
  case "$arg" in
    --fake-dra) FAKE_DRA=true ;;
    --delete) DELETE=true ;;
    --force) FORCE=true ;;
    -h | --help) sed -n '2,42p' "$0"; exit 0 ;;
    *) die "unknown argument: $arg (use --fake-dra, --delete or --force)" ;;
  esac
done

# A dev cluster only: the script adds CRDs and admission policies to the whole cluster.
if [[ "$CONTEXT" != kind-* ]] && ! $FORCE; then
  die "context $CONTEXT isn't a kind cluster; pass --force to use it anyway"
fi

k get --raw /version >/dev/null 2>&1 || die "context $CONTEXT isn't reachable (run script/dev-cluster.sh first)"

# own_namespace <name>: create the namespace and mark it, or use it when it's already marked;
# never take over a namespace the script didn't create.
own_namespace() {
  if k get namespace "$1" >/dev/null 2>&1; then
    marked namespace "$1" || die "namespace $1 exists and wasn't created by $ME; not touching it"
  else
    k create namespace "$1" >/dev/null
  fi
  k annotate namespace "$1" "$MARK=$ME" --overwrite >/dev/null
}

delete_all() {
  log "removing what $ME installed"
  # The pods first, while the driver still runs to unprepare their devices; pods of the
  # fake driver (or of a driver that is gone) never get unprepared: force them.
  if ! k -n "$NS" delete pods -l "$MARK=$ME" --ignore-not-found --timeout=90s >/dev/null 2>&1; then
    k -n "$NS" delete pods -l "$MARK=$ME" --ignore-not-found --force --grace-period=0 >/dev/null 2>&1 || true
  fi
  if marked namespace "$NS"; then
    k delete namespace "$NS" --ignore-not-found --timeout=300s >/dev/null || true
  fi
  if marked namespace "$DRA_NS"; then
    h uninstall dra-example-driver -n "$DRA_NS" >/dev/null 2>&1 || true
    k delete namespace "$DRA_NS" --ignore-not-found --timeout=300s >/dev/null || true
  fi
  for kind in deviceclasses.resource.k8s.io resourceslices.resource.k8s.io gatewayclasses.gateway.networking.k8s.io \
    validatingadmissionpolicybindings validatingadmissionpolicies mutatingadmissionpolicybindings \
    mutatingadmissionpolicies runtimeclasses; do
    k delete "$kind" -l "$MARK=$ME" --ignore-not-found --wait=false >/dev/null 2>&1 || true
  done
  # The Gateway API's own policy, when the CRDs came from here.
  if marked crd gateways.gateway.networking.k8s.io; then
    k delete validatingadmissionpolicybinding,validatingadmissionpolicy \
      safe-upgrades.gateway.networking.k8s.io --ignore-not-found >/dev/null 2>&1 || true
  fi
  for crd in $(k get crd -o name | grep -E 'gateway\.networking\.(x-)?k8s\.io|autoscaling\.k8s\.io$' || true); do
    if marked crd "${crd#*/}"; then
      k delete "$crd" --ignore-not-found --wait=false >/dev/null
    fi
  done
  log "done"
}

if $DELETE; then
  delete_all
  exit 0
fi

# A --delete just before: let its namespaces and CRDs go first.
for ns in "$NS" "$DRA_NS"; do
  if [[ "$(k get namespace "$ns" -o jsonpath='{.status.phase}' 2>/dev/null)" == Terminating ]]; then
    log "waiting for namespace $ns to be deleted"
    k wait --for=delete "namespace/$ns" --timeout=300s >/dev/null 2>&1 || true
  fi
done
for crd in $(k get crd -o name | grep -E 'gateway\.networking\.(x-)?k8s\.io|autoscaling\.k8s\.io$' || true); do
  if [[ -n "$(k get "$crd" -o jsonpath='{.metadata.deletionTimestamp}' 2>/dev/null)" ]]; then
    k wait --for=delete "$crd" --timeout=120s >/dev/null 2>&1 || true
  fi
done

served resourceclaims resource.k8s.io || die "the cluster doesn't serve resource.k8s.io (Kubernetes 1.34+ serves v1)"
DRA_VERSION="$(version_of resource.k8s.io)"
log "resource.k8s.io/$DRA_VERSION"

log "namespace $NS"
own_namespace "$NS"
k label namespace "$NS" "$MARK=$ME" kubyl.dev/views=true --overwrite >/dev/null

# ----- Workloads: two web Deployments behind Services -----
log "sample Deployments and Services"
k apply -n "$NS" -f - >/dev/null <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  labels: {app: web, $MARK: $ME}
spec:
  replicas: 2
  selector: {matchLabels: {app: web}}
  template:
    metadata: {labels: {app: web}}
    spec:
      containers:
        - name: nginx
          image: nginx:1.27-alpine
          ports: [{containerPort: 80, name: http}]
          resources:
            requests: {cpu: 50m, memory: 64Mi}
            limits: {memory: 128Mi}
        - name: metrics
          image: nginx:1.27-alpine
          command: [sh, -c, "sed -i 's/listen  *80;/listen 9113;/' /etc/nginx/conf.d/default.conf && exec nginx -g 'daemon off;'"]
          ports: [{containerPort: 9113, name: metrics}]
          resources:
            requests: {cpu: 10m, memory: 16Mi}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web-canary
  labels: {app: web-canary, $MARK: $ME}
spec:
  replicas: 1
  selector: {matchLabels: {app: web-canary}}
  template:
    metadata: {labels: {app: web-canary}}
    spec:
      containers:
        - name: nginx
          image: nginx:1.27-alpine
          ports: [{containerPort: 80, name: http}]
---
apiVersion: v1
kind: Service
metadata:
  name: web
  labels: {$MARK: $ME}
spec:
  selector: {app: web}
  ports: [{name: http, port: 80, targetPort: http}]
---
apiVersion: v1
kind: Service
metadata:
  name: web-canary
  labels: {$MARK: $ME}
spec:
  selector: {app: web-canary}
  ports: [{name: http, port: 80, targetPort: http}]
---
apiVersion: v1
kind: Service
metadata:
  name: checkout-grpc
  labels: {$MARK: $ME}
spec:
  selector: {app: web}
  ports: [{name: grpc, port: 9090, targetPort: 80, appProtocol: kubernetes.io/h2c}]
EOF

# ----- DRA -----
DEVICE_CLASS="gpu.example.com"
if ! $FAKE_DRA; then
  log "dra-example-driver $DRA_CHART_VERSION in $DRA_NS"
  own_namespace "$DRA_NS"
  if ! h upgrade --install dra-example-driver "$DRA_CHART" --version "$DRA_CHART_VERSION" \
    --set gpuDeviceStatus=true -n "$DRA_NS" --wait --timeout 4m >/dev/null; then
    warn "the example driver didn't come up; using a hand-made ResourceSlice (--fake-dra)"
    FAKE_DRA=true
  fi
fi
if $FAKE_DRA; then
  DEVICE_CLASS="fake-gpu.kubyl.dev"
  log "fake driver $DEVICE_CLASS: DeviceClass and a ResourceSlice per node"
  k apply -f - >/dev/null <<EOF
apiVersion: resource.k8s.io/$DRA_VERSION
kind: DeviceClass
metadata:
  name: $DEVICE_CLASS
  labels: {$MARK: $ME}
spec:
  selectors:
    - cel:
        expression: device.driver == "$DEVICE_CLASS"
EOF
  for node in $(k get nodes -o jsonpath='{.items[*].metadata.name}'); do
    devices=""
    for i in 0 1 2 3; do
      devices+="
    - name: gpu-$i
      attributes:
        model: {string: KB-100}
        index: {int: $i}
        driverVersion: {version: 1.4.2}
      capacity:
        memory: {value: 16Gi}"
    done
    k apply -f - >/dev/null <<EOF
apiVersion: resource.k8s.io/$DRA_VERSION
kind: ResourceSlice
metadata:
  name: $node-fake-gpu
  labels: {$MARK: $ME}
spec:
  driver: $DEVICE_CLASS
  nodeName: $node
  pool: {name: $node, generation: 1, resourceSliceCount: 1}
  devices:$devices
EOF
  done
fi

log "ResourceClaimTemplate, ResourceClaim and pods using them"
k apply -n "$NS" -f - >/dev/null <<EOF
apiVersion: resource.k8s.io/$DRA_VERSION
kind: ResourceClaimTemplate
metadata:
  name: single-gpu
  labels: {$MARK: $ME}
spec:
  spec:
    devices:
      requests:
        - name: gpu
          exactly:
            deviceClassName: $DEVICE_CLASS
---
apiVersion: resource.k8s.io/$DRA_VERSION
kind: ResourceClaim
metadata:
  name: shared-gpu
  labels: {$MARK: $ME}
spec:
  devices:
    requests:
      - name: gpus
        exactly:
          deviceClassName: $DEVICE_CLASS
          allocationMode: ExactCount
          count: 2
---
apiVersion: v1
kind: Pod
metadata:
  name: gpu-job-0
  labels: {app: gpu-job, $MARK: $ME}
spec:
  resourceClaims:
    - name: gpu
      resourceClaimTemplateName: single-gpu
  containers:
    - name: ctr
      image: busybox:1.37
      command: [sh, -c, "env | grep -i gpu; sleep 1000000"]
      resources:
        claims: [{name: gpu}]
---
apiVersion: v1
kind: Pod
metadata:
  name: gpu-shared
  labels: {app: gpu-shared, $MARK: $ME}
spec:
  resourceClaims:
    - name: gpus
      resourceClaimName: shared-gpu
  containers:
    - name: ctr
      image: busybox:1.37
      command: [sh, -c, "env | grep -i gpu; sleep 1000000"]
      resources:
        claims: [{name: gpus}]
EOF

# ----- Admission policies -----
AR_VERSION="$(version_of admissionregistration.k8s.io)"
log "ValidatingAdmissionPolicy (admissionregistration.k8s.io/$AR_VERSION)"
k apply -n "$NS" -f - >/dev/null <<EOF
apiVersion: v1
kind: ConfigMap
metadata:
  name: replica-limit
  labels: {$MARK: $ME}
data:
  maxReplicas: "5"
EOF
k apply -f - >/dev/null <<EOF
apiVersion: admissionregistration.k8s.io/$AR_VERSION
kind: ValidatingAdmissionPolicy
metadata:
  name: kubyl-replica-limit
  labels: {$MARK: $ME}
spec:
  failurePolicy: Fail
  paramKind:
    apiVersion: v1
    kind: ConfigMap
  matchConstraints:
    resourceRules:
      - apiGroups: [apps]
        apiVersions: [v1]
        operations: [CREATE, UPDATE]
        resources: [deployments, statefulsets]
  matchConditions:
    - name: not-a-system-user
      expression: "!request.userInfo.username.startsWith('system:')"
  variables:
    - name: replicas
      expression: "has(object.spec.replicas) ? object.spec.replicas : 1"
    - name: limit
      expression: "int(params.data.maxReplicas)"
  validations:
    - expression: "variables.replicas <= variables.limit"
      messageExpression: "'replicas must be at most ' + string(variables.limit)"
      reason: Invalid
    - expression: "object.metadata.name.size() <= 40"
      message: "names are at most 40 characters"
  auditAnnotations:
    - key: replicas
      valueExpression: "string(variables.replicas)"
---
apiVersion: admissionregistration.k8s.io/$AR_VERSION
kind: ValidatingAdmissionPolicyBinding
metadata:
  name: kubyl-replica-limit-views
  labels: {$MARK: $ME}
spec:
  policyName: kubyl-replica-limit
  validationActions: [Deny, Audit]
  paramRef:
    name: replica-limit
    namespace: $NS
    parameterNotFoundAction: Deny
  matchResources:
    namespaceSelector:
      matchLabels:
        kubyl.dev/views: "true"
EOF
if served mutatingadmissionpolicies admissionregistration.k8s.io; then
  # The policy may be served in an older version than the group's preferred one.
  MAP_VERSION="$(k api-resources --api-group=admissionregistration.k8s.io --no-headers 2>/dev/null |
    awk '$NF == "MutatingAdmissionPolicy" { n = split($(NF-2), a, "/"); print a[n] }')"
  MAP_VERSION="${MAP_VERSION:-$AR_VERSION}"
  log "MutatingAdmissionPolicy (admissionregistration.k8s.io/$MAP_VERSION)"
  k apply -f - >/dev/null <<EOF
apiVersion: admissionregistration.k8s.io/$MAP_VERSION
kind: MutatingAdmissionPolicy
metadata:
  name: kubyl-default-labels
  labels: {$MARK: $ME}
spec:
  failurePolicy: Ignore
  reinvocationPolicy: IfNeeded
  matchConstraints:
    resourceRules:
      - apiGroups: [""]
        apiVersions: [v1]
        operations: [CREATE]
        resources: [configmaps]
  matchConditions:
    - name: unlabelled
      expression: "!has(object.metadata.labels) || !('team' in object.metadata.labels)"
  mutations:
    - patchType: ApplyConfiguration
      applyConfiguration:
        expression: >
          Object{
            metadata: Object.metadata{
              labels: {"team": "platform"}
            }
          }
---
apiVersion: admissionregistration.k8s.io/$MAP_VERSION
kind: MutatingAdmissionPolicyBinding
metadata:
  name: kubyl-default-labels-views
  labels: {$MARK: $ME}
spec:
  policyName: kubyl-default-labels
  matchResources:
    namespaceSelector:
      matchLabels:
        kubyl.dev/views: "true"
EOF
else
  log "MutatingAdmissionPolicy isn't served: skipped"
fi

# ----- Gateway API -----
if ! served gateways gateway.networking.k8s.io; then
  log "Gateway API $GATEWAY_API_VERSION CRDs (experimental channel)"
  manifest="$(mktemp)"
  curl -sSfL -o "$manifest" \
    "https://github.com/kubernetes-sigs/gateway-api/releases/download/$GATEWAY_API_VERSION/experimental-install.yaml"
  k apply --server-side -f "$manifest" >/dev/null
  rm -f "$manifest"
  for crd in $(k get crd -o name | grep -E 'gateway\.networking\.(x-)?k8s\.io'); do
    k annotate "$crd" "$MARK=$ME" --overwrite >/dev/null
  done
  k wait --for=condition=Established crd/gateways.gateway.networking.k8s.io \
    crd/httproutes.gateway.networking.k8s.io --timeout=60s >/dev/null
else
  log "Gateway API CRDs already served: kept"
fi
GW_VERSION="$(version_of gateway.networking.k8s.io)"
log "GatewayClass, Gateway, HTTPRoute, GRPCRoute, ReferenceGrant (gateway.networking.k8s.io/$GW_VERSION)"
k apply -f - >/dev/null <<EOF
apiVersion: gateway.networking.k8s.io/$GW_VERSION
kind: GatewayClass
metadata:
  name: kubyl-fake
  labels: {$MARK: $ME}
spec:
  controllerName: kubyl.dev/fake-gateway-controller
  description: Written by script/views-dev.sh; no controller runs.
EOF
k apply -n "$NS" -f - >/dev/null <<EOF
apiVersion: gateway.networking.k8s.io/$GW_VERSION
kind: Gateway
metadata:
  name: web-gateway
  labels: {$MARK: $ME}
spec:
  gatewayClassName: kubyl-fake
  listeners:
    - name: http
      protocol: HTTP
      port: 80
      allowedRoutes:
        namespaces: {from: Same}
    - name: https
      protocol: HTTPS
      port: 443
      hostname: "*.shop.example.com"
      tls:
        mode: Terminate
        certificateRefs: [{kind: Secret, name: shop-tls}]
      allowedRoutes:
        namespaces: {from: All}
---
apiVersion: gateway.networking.k8s.io/$GW_VERSION
kind: HTTPRoute
metadata:
  name: shop
  labels: {$MARK: $ME}
spec:
  parentRefs:
    - name: web-gateway
      sectionName: http
    - name: web-gateway
      sectionName: https
  hostnames: [shop.example.com, www.shop.example.com]
  rules:
    - matches:
        - path: {type: PathPrefix, value: /api}
          headers: [{name: x-canary, value: "true"}]
      backendRefs:
        - {name: web-canary, port: 80}
    - matches:
        - path: {type: PathPrefix, value: /}
      backendRefs:
        - {name: web, port: 80, weight: 90}
        - {name: web-canary, port: 80, weight: 10}
---
apiVersion: gateway.networking.k8s.io/$GW_VERSION
kind: GRPCRoute
metadata:
  name: checkout
  labels: {$MARK: $ME}
spec:
  parentRefs:
    - {name: web-gateway, sectionName: https}
  hostnames: [grpc.shop.example.com]
  rules:
    - matches:
        - method: {service: shop.Checkout, method: Pay}
      backendRefs:
        - {name: checkout-grpc, port: 9090}
---
apiVersion: gateway.networking.k8s.io/$GW_VERSION
kind: ReferenceGrant
metadata:
  name: allow-default-routes
  labels: {$MARK: $ME}
spec:
  from:
    - {group: gateway.networking.k8s.io, kind: HTTPRoute, namespace: default}
  to:
    - {group: "", kind: Service}
EOF

# What a controller would report.
now="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
cond() {
  # cond <type> <status> <reason> <generation>
  printf '{"type":"%s","status":"%s","reason":"%s","message":"","lastTransitionTime":"%s","observedGeneration":%s}' \
    "$1" "$2" "$3" "$now" "$4"
}
gen() { k get "$@" -o jsonpath='{.metadata.generation}'; }
g="$(gen gatewayclass kubyl-fake)"
k patch gatewayclass kubyl-fake --subresource=status --type=merge \
  -p "{\"status\":{\"conditions\":[$(cond Accepted True Accepted "$g")]}}" >/dev/null
g="$(gen -n "$NS" gateway web-gateway)"
listener() {
  # listener <name> <attached routes> <kinds>
  printf '{"name":"%s","attachedRoutes":%s,"supportedKinds":[%s],"conditions":[%s,%s,%s]}' "$1" "$2" "$3" \
    "$(cond Accepted True Accepted "$g")" "$(cond Programmed True Programmed "$g")" "$(cond ResolvedRefs True ResolvedRefs "$g")"
}
k -n "$NS" patch gateway web-gateway --subresource=status --type=merge -p "{\"status\":{
  \"addresses\":[{\"type\":\"IPAddress\",\"value\":\"172.18.0.240\"}],
  \"conditions\":[$(cond Accepted True Accepted "$g"),$(cond Programmed True Programmed "$g")],
  \"listeners\":[$(listener http 1 '{"group":"gateway.networking.k8s.io","kind":"HTTPRoute"}'),
                 $(listener https 2 '{"group":"gateway.networking.k8s.io","kind":"HTTPRoute"},{"group":"gateway.networking.k8s.io","kind":"GRPCRoute"}')]}}" >/dev/null
parent() {
  # parent <section> <accepted status> <reason> <generation>
  printf '{"parentRef":{"group":"gateway.networking.k8s.io","kind":"Gateway","name":"web-gateway","sectionName":"%s"},"controllerName":"kubyl.dev/fake-gateway-controller","conditions":[%s,%s]}' \
    "$1" "$(cond Accepted "$2" "$3" "$4")" "$(cond ResolvedRefs True ResolvedRefs "$4")"
}
g="$(gen -n "$NS" httproute shop)"
k -n "$NS" patch httproute shop --subresource=status --type=merge \
  -p "{\"status\":{\"parents\":[$(parent http True Accepted "$g"),$(parent https True Accepted "$g")]}}" >/dev/null
g="$(gen -n "$NS" grpcroute checkout)"
k -n "$NS" patch grpcroute checkout --subresource=status --type=merge \
  -p "{\"status\":{\"parents\":[$(parent https True Accepted "$g")]}}" >/dev/null

# ----- VPA -----
if ! served verticalpodautoscalers autoscaling.k8s.io; then
  log "VPA CRDs ($VPA_VERSION)"
  k apply -f "https://raw.githubusercontent.com/kubernetes/autoscaler/$VPA_VERSION/vertical-pod-autoscaler/deploy/vpa-v1-crd-gen.yaml" >/dev/null
  for crd in verticalpodautoscalers.autoscaling.k8s.io verticalpodautoscalercheckpoints.autoscaling.k8s.io; do
    k annotate crd "$crd" "$MARK=$ME" --overwrite >/dev/null
  done
  k wait --for=condition=Established crd/verticalpodautoscalers.autoscaling.k8s.io --timeout=60s >/dev/null
else
  log "VPA CRDs already served: kept"
fi
log "VPA web (recommendation written by the script)"
k apply -n "$NS" -f - >/dev/null <<EOF
apiVersion: autoscaling.k8s.io/v1
kind: VerticalPodAutoscaler
metadata:
  name: web
  labels: {$MARK: $ME}
spec:
  targetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: web
  updatePolicy:
    updateMode: "Off"
  resourcePolicy:
    containerPolicies:
      - containerName: metrics
        mode: "Off"
EOF
k -n "$NS" patch verticalpodautoscaler web --subresource=status --type=merge -p "{\"status\":{
  \"conditions\":[{\"type\":\"RecommendationProvided\",\"status\":\"True\",\"lastTransitionTime\":\"$now\"}],
  \"recommendation\":{\"containerRecommendations\":[
    {\"containerName\":\"nginx\",\"lowerBound\":{\"cpu\":\"25m\",\"memory\":\"48Mi\"},\"target\":{\"cpu\":\"80m\",\"memory\":\"96Mi\"},
     \"uncappedTarget\":{\"cpu\":\"80m\",\"memory\":\"96Mi\"},\"upperBound\":{\"cpu\":\"320m\",\"memory\":\"256Mi\"}}]}}}" >/dev/null

# ----- RuntimeClass -----
log "RuntimeClass kubyl-gvisor"
k apply -f - >/dev/null <<EOF
apiVersion: node.k8s.io/v1
kind: RuntimeClass
metadata:
  name: kubyl-gvisor
  labels: {$MARK: $ME}
handler: runsc
overhead:
  podFixed: {cpu: 250m, memory: 120Mi}
scheduling:
  nodeSelector: {runtime: gvisor}
EOF

log "waiting for the claims to be allocated"
for _ in $(seq 1 24); do
  state="$(k -n "$NS" get resourceclaim shared-gpu -o jsonpath='{.status.allocation.devices.results[*].device}' 2>/dev/null || true)"
  [[ -n "$state" ]] && break
  sleep 5
done
k -n "$NS" get resourceclaims,pods -o wide || true
log "done: namespace $NS (kubyl.dev/views=true), DeviceClass $DEVICE_CLASS"
