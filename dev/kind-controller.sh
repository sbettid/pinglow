#!/usr/bin/env bash
set -euo pipefail

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

for command in kind kubectl cargo; do
  if ! command -v "$command" >/dev/null 2>&1; then
    echo "Required command not found: $command" >&2
    exit 2
  fi
done

KUBECONFIG_FILE="$(mktemp)"
RUNNER_PID=""
CONTROLLER_PID=""

cleanup() {
  for pid in "$RUNNER_PID" "$CONTROLLER_PID"; do
    if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
      kill "$pid"
    fi
  done

  for pid in "$RUNNER_PID" "$CONTROLLER_PID"; do
    if [[ -n "$pid" ]]; then
      wait "$pid" 2>/dev/null || true
    fi
  done

  rm -f "$KUBECONFIG_FILE"
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

export DB_USER="${DB_USER:-pinglow}"
export DB_USER_PASSWORD="${DB_USER_PASSWORD:-pinglow}"
export DB_HOST="${DB_HOST:-localhost}"
export DB="${DB:-pinglow}"
export REDIS_HOST="${REDIS_HOST:-localhost}"
export REDIS_PASSWORD="${REDIS_PASSWORD:-pinglow}"
export NAMESPACE
export OIDC_ISSUER_URL="${OIDC_ISSUER_URL:-http://localhost:8081/realms/pinglow}"
export OIDC_CLIENT_ID="${OIDC_CLIENT_ID:-pinglow}"
export OIDC_CLIENT_SECRET="${OIDC_CLIENT_SECRET:-pinglow-dev-secret}"
export OIDC_REDIRECT_URL="${OIDC_REDIRECT_URL:-http://localhost:5173/api/auth/callback}"
export OIDC_GROUPS_CLAIM="${OIDC_GROUPS_CLAIM:-groups}"
export OIDC_COOKIE_SECURE=false
export RUNNER_NAME="${RUNNER_NAME:-pinglow-dev-runner}"
export CHECKS_BASE_PATH="${CHECKS_BASE_PATH:-${TMPDIR:-/tmp}/pinglow-dev-checks}"

echo "Running Pinglow and its check runner locally against Kind context kind-$CLUSTER_NAME."
echo "Start the frontend separately with: cd $ROOT_DIR/../pinglow-frontend/client && npm run dev"

cargo run -p pinglow-runner &
RUNNER_PID=$!
cargo run -p pinglow &
CONTROLLER_PID=$!

set +e
wait -n "$RUNNER_PID" "$CONTROLLER_PID"
status=$?
set -e
exit "$status"
