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
trap 'rm -f "$KUBECONFIG_FILE"' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
export KUBECONFIG="$KUBECONFIG_FILE"

if kind get clusters | grep -Fxq "$CLUSTER_NAME"; then
  kind export kubeconfig --name "$CLUSTER_NAME" --kubeconfig "$KUBECONFIG_FILE"
else
  kind create cluster --name "$CLUSTER_NAME" --kubeconfig "$KUBECONFIG_FILE"
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
kubectl --context "kind-$CLUSTER_NAME" apply -f - <<EOF
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: local-development-user
  namespace: $NAMESPACE
spec:
  email: $USER_EMAIL
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
export OIDC_COOKIE_SECURE=false

echo "Running Pinglow locally against Kind context kind-$CLUSTER_NAME."
echo "Start the frontend separately with: cd $ROOT_DIR/../pinglow-frontend/client && npm run dev"
cargo run -p pinglow
