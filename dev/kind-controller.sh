#!/usr/bin/env bash
set -euo pipefail
umask 077

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CLUSTER_NAME="${KIND_CLUSTER_NAME:-pinglow-dev}"
NAMESPACE="${NAMESPACE:-pinglow}"
USER_EMAIL="${1:-}"

if [[ ! "$CLUSTER_NAME" =~ ^[a-z0-9][a-z0-9.-]*$ ]]; then
  echo "Invalid KIND_CLUSTER_NAME: $CLUSTER_NAME" >&2
  exit 2
fi

if [[ ! "$NAMESPACE" =~ ^[a-z0-9]([a-z0-9-]*[a-z0-9])?$ ]]; then
  echo "Invalid NAMESPACE: $NAMESPACE" >&2
  exit 2
fi

if [[ ! "$USER_EMAIL" =~ ^[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}$ ]]; then
  echo "Usage: $0 user-email@example.com" >&2
  exit 2
fi

for command in kind kubectl helm cargo base64; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 2
  fi
done

KUBECONFIG_FILE="$(mktemp)"
RUNNER_PID=""
CONTROLLER_PID=""
AGENT_PID=""
PORT_FORWARD_PIDS=()
AGENT_TMP_DIR="$(mktemp -d)"
AGENT_KEY_FILE="$AGENT_TMP_DIR/api-key"
AGENT_CONFIG_FILE="$AGENT_TMP_DIR/agent.yaml"

cleanup() {
  for pid in "${PORT_FORWARD_PIDS[@]}" "$AGENT_PID" "$RUNNER_PID" "$CONTROLLER_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid"
    fi
  done

  for pid in "${PORT_FORWARD_PIDS[@]}" "$AGENT_PID" "$RUNNER_PID" "$CONTROLLER_PID"; do
    if [[ -n "$pid" ]]; then
      wait "$pid" 2>/dev/null || true
    fi
  done

  rm -f "$KUBECONFIG_FILE" "$AGENT_KEY_FILE" "$AGENT_CONFIG_FILE"
  rmdir "$AGENT_TMP_DIR" 2>/dev/null || true
}

trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export KUBECONFIG="$KUBECONFIG_FILE"

create_cluster() {
  kind create cluster --name "$CLUSTER_NAME" --kubeconfig "$KUBECONFIG_FILE"
  kubectl --context "kind-$CLUSTER_NAME" wait \
    --for=condition=Ready \
    nodes \
    --all \
    --timeout=120s
}

cluster_is_healthy() {
  kind export kubeconfig --name "$CLUSTER_NAME" --kubeconfig "$KUBECONFIG_FILE" \
    && kubectl --context "kind-$CLUSTER_NAME" get nodes --request-timeout=10s >/dev/null 2>&1 \
    && kubectl --context "kind-$CLUSTER_NAME" wait \
      --for=condition=Ready \
      nodes \
      --all \
      --timeout=90s >/dev/null 2>&1
}

if kind get clusters | grep -Fxq "$CLUSTER_NAME"; then
  if ! cluster_is_healthy; then
    echo "Kind cluster '$CLUSTER_NAME' is unavailable or unhealthy; recreating this disposable dev cluster."
    kind delete cluster --name "$CLUSTER_NAME"
    create_cluster
  fi
else
  create_cluster
fi

kubectl --context "kind-$CLUSTER_NAME" apply -f "$ROOT_DIR/charts/pinglow/crds/"
kubectl --context "kind-$CLUSTER_NAME" wait \
  --for=condition=Established \
  crd/checks.pinglow.io \
  crd/scripts.pinglow.io \
  crd/pinglowuserbindings.pinglow.io \
  crd/apikeybindings.pinglow.io \
  crd/telegramchannels.pinglow.io
kubectl --context "kind-$CLUSTER_NAME" create namespace "$NAMESPACE" \
  --dry-run=client -o yaml | kubectl --context "kind-$CLUSTER_NAME" apply -f -

export DB_USER="${DB_USER:-pinglow}"
export DB_USER_PASSWORD="${DB_USER_PASSWORD:-pinglow}"
export DB="${DB:-pinglow}"
export REDIS_PASSWORD="${REDIS_PASSWORD:-pinglow}"
export KC_BOOTSTRAP_ADMIN_USERNAME="${KC_BOOTSTRAP_ADMIN_USERNAME:-admin}"
export KC_BOOTSTRAP_ADMIN_PASSWORD="${KC_BOOTSTRAP_ADMIN_PASSWORD:-admin}"
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  create secret generic pinglow-dev-dependencies \
  --from-literal=DB_USER="$DB_USER" \
  --from-literal=DB_USER_PASSWORD="$DB_USER_PASSWORD" \
  --from-literal=POSTGRES_DB="$DB" \
  --from-literal=POSTGRES_USER="$DB_USER" \
  --from-literal=POSTGRES_PASSWORD="$DB_USER_PASSWORD" \
  --from-literal=REDIS_PASSWORD="$REDIS_PASSWORD" \
  --from-literal=KC_BOOTSTRAP_ADMIN_USERNAME="$KC_BOOTSTRAP_ADMIN_USERNAME" \
  --from-literal=KC_BOOTSTRAP_ADMIN_PASSWORD="$KC_BOOTSTRAP_ADMIN_PASSWORD" \
  --dry-run=client -o yaml | \
  kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" apply -f -

for release in pinglow-dev-dependencies pinglow-dev; do
  if helm --kube-context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    status "$release" >/dev/null 2>&1; then
    echo "Removing the previous Helm release '$release'."
    helm --kube-context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
      uninstall "$release" --wait
  fi
done

recreate_statefulset_with_pvc_template() {
  local name="$1"
  local claim_name

  claim_name="$(kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    get statefulset "$name" --ignore-not-found \
    -o jsonpath='{.spec.volumeClaimTemplates[0].metadata.name}')"
  if [[ -n "$claim_name" ]]; then
    echo "Recreating StatefulSet '$name' to switch its data volume to ephemeral storage."
    echo "Leaving existing PVC '${claim_name}-${name}-0' untouched."
    kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
      delete statefulset "$name" --wait
  fi
}

recreate_statefulset_with_pvc_template redis
recreate_statefulset_with_pvc_template pinglow-dev-dependencies-timescaledb

helm template pinglow-dev-dependencies "$ROOT_DIR/charts/pinglow" \
  --namespace "$NAMESPACE" \
  --show-only templates/redis-statefulset.yaml \
  --show-only templates/timescaledb-statefulset.yaml \
  --show-only templates/timescaledb-service.yaml \
  --set-string redis.secretName=pinglow-dev-dependencies \
  --set redis.persistence.enabled=false \
  --set timescaledb.enabled=true \
  --set timescaledb.persistence.enabled=false \
  --set-string timescaledb.secretName=pinglow-dev-dependencies \
  --set-string timescaledb.username="$DB_USER" \
  --set-string timescaledb.database="$DB" | \
  kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" apply -f -
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  apply -k "$ROOT_DIR/dev"
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  rollout status statefulset/redis --timeout=5m
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  rollout status statefulset/pinglow-dev-dependencies-timescaledb --timeout=5m
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  rollout status deployment/keycloak --timeout=180s
if kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  get checks.pinglow.io pinglow-dev-host-cpu --ignore-not-found -o name | grep -q .; then
  # This legacy demo Check may be terminating while its local controller is stopped.
  kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    patch checks.pinglow.io pinglow-dev-host-cpu \
    --type=merge -p '{"metadata":{"finalizers":[]}}'
  kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    delete checks.pinglow.io pinglow-dev-host-cpu --ignore-not-found --wait=false
fi
kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
  apply -f "$ROOT_DIR/dev/demo-checks.yaml"
kubectl --context "kind-$CLUSTER_NAME" apply -f - <<EOF
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: local-development-user
  namespace: $NAMESPACE
spec:
  email: $USER_EMAIL
  role: viewer
---
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: local-development-admin-group
  namespace: $NAMESPACE
spec:
  groups:
    - pinglow-admins
  role: admin
EOF

export DB_HOST="${DB_HOST:-localhost}"
export REDIS_HOST="${REDIS_HOST:-localhost}"
export NAMESPACE
export OIDC_ISSUER_URL="${OIDC_ISSUER_URL:-http://localhost:8081/realms/pinglow}"
export OIDC_CLIENT_ID="${OIDC_CLIENT_ID:-pinglow}"
export OIDC_CLIENT_SECRET="${OIDC_CLIENT_SECRET:-pinglow-dev-secret}"
export OIDC_REDIRECT_URL="${OIDC_REDIRECT_URL:-http://localhost:5173/api/auth/callback}"
export OIDC_GROUPS_CLAIM="${OIDC_GROUPS_CLAIM:-groups}"
export OIDC_COOKIE_SECURE=false
export RUNNER_NAME="${RUNNER_NAME:-pinglow-dev-runner}"
export RUNNER_DROP_PRIVILEGES="${RUNNER_DROP_PRIVILEGES:-false}"
export CHECKS_BASE_PATH="${CHECKS_BASE_PATH:-${TMPDIR:-/tmp}/pinglow-dev-checks}"

start_port_forward() {
  local service="$1"
  local local_port="$2"
  local remote_port="$3"
  local pid

  kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    port-forward "service/$service" "$local_port:$remote_port" &
  pid=$!
  PORT_FORWARD_PIDS+=("$pid")

  for _ in {1..60}; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "Port-forward for service '$service' exited unexpectedly." >&2
      return 1
    fi
    if (echo >"/dev/tcp/127.0.0.1/$local_port") >/dev/null 2>&1; then
      return
    fi
    sleep 1
  done

  echo "Timed out waiting for port-forward to service '$service' on port $local_port." >&2
  return 1
}

start_port_forward redis 6379 6379
start_port_forward pinglow-dev-dependencies-timescaledb 5432 5432
start_port_forward keycloak 8081 8080

echo "Running Pinglow and its check runner locally against Kind context kind-$CLUSTER_NAME."
echo "Local runner privilege dropping is $RUNNER_DROP_PRIVILEGES."
echo "Starting the local dev-host agent for the script and built-in collector examples."
echo "Start the frontend separately with: cd $ROOT_DIR/../pinglow-frontend/client && npm run dev"

cargo run -p pinglow-runner &
RUNNER_PID=$!
cargo run -p pinglow &
CONTROLLER_PID=$!

for _ in {1..120}; do
  if ! kill -0 "$CONTROLLER_PID" 2>/dev/null; then
    echo "The Pinglow controller exited before its API became ready." >&2
    exit 1
  fi
  if (echo >/dev/tcp/127.0.0.1/8000) >/dev/null 2>&1; then
    break
  fi
  sleep 1
done

if ! (echo >/dev/tcp/127.0.0.1/8000) >/dev/null 2>&1; then
  echo "Timed out waiting for the local controller API on port 8000." >&2
  exit 1
fi

api_key_data=""
for _ in {1..120}; do
  api_key_data="$(kubectl --context "kind-$CLUSTER_NAME" --namespace "$NAMESPACE" \
    get secret pinglow-agent-dev-host-api-key \
    -o jsonpath='{.data.API_KEY}' 2>/dev/null || true)"
  if [[ -n "$api_key_data" ]]; then
    break
  fi
  sleep 1
done

if [[ -z "$api_key_data" ]]; then
  echo "Timed out waiting for the dev-host agent API key Secret." >&2
  exit 1
fi
printf '%s' "$api_key_data" | base64 --decode >"$AGENT_KEY_FILE"

AGENT_CHECKS_BASE_PATH="${TMPDIR:-/tmp}/pinglow-dev-agent-checks"
mkdir -p "$AGENT_CHECKS_BASE_PATH"
cat >"$AGENT_CONFIG_FILE" <<EOF
agentId: dev-host
controllerUrl: http://127.0.0.1:8000
apiKeyFile: "$AGENT_KEY_FILE"
allowedChecks:
  - pinglow-dev-agent-heartbeat
  - pinglow-dev-host-disk
  - pinglow-dev-host-memory
  - pinglow-dev-host-load
checksBasePath: "$AGENT_CHECKS_BASE_PATH"
executionTimeoutSeconds: 120
dropPrivileges: false
EOF
cargo run -p pinglow-agent -- "$AGENT_CONFIG_FILE" &
AGENT_PID=$!

set +e
wait -n "$RUNNER_PID" "$CONTROLLER_PID" "$AGENT_PID"
status=$?
set -e
exit "$status"
