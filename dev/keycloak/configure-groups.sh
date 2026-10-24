#!/usr/bin/env bash
set -euo pipefail

KCADM="/opt/keycloak/bin/kcadm.sh"
SERVER="http://keycloak:8080"
REALM="pinglow"
GROUP="pinglow-admins"
USERNAME="dev"
CLIENT_ID="pinglow"

for attempt in {1..60}; do
  if "$KCADM" config credentials \
    --server "$SERVER" \
    --realm master \
    --user "$KC_BOOTSTRAP_ADMIN_USERNAME" \
    --password "$KC_BOOTSTRAP_ADMIN_PASSWORD" >/dev/null 2>&1; then
    break
  fi
  if [[ "$attempt" -eq 60 ]]; then
    echo "Could not authenticate to Keycloak after waiting for startup." >&2
    exit 1
  fi
  sleep 2
done

first_id() {
  grep -m1 -oE '"id"[[:space:]]*:[[:space:]]*"[^"]+"' \
    | sed -E 's/^"id"[[:space:]]*:[[:space:]]*"([^"]+)"/\1/'
}

group_json=$("$KCADM" get groups -r "$REALM" -q "search=$GROUP" -q exact=true)
group_id=$(printf '%s\n' "$group_json" | first_id || true)
if [[ -z "$group_id" ]]; then
  "$KCADM" create groups -r "$REALM" -s "name=$GROUP"
  group_json=$("$KCADM" get groups -r "$REALM" -q "search=$GROUP" -q exact=true)
  group_id=$(printf '%s\n' "$group_json" | first_id || true)
fi
if [[ -z "$group_id" ]]; then
  echo "Could not find or create Keycloak group '$GROUP'." >&2
  exit 1
fi

user_json=$("$KCADM" get users -r "$REALM" -q "username=$USERNAME" -q exact=true)
user_id=$(printf '%s\n' "$user_json" | first_id || true)
if [[ -z "$user_id" ]]; then
  echo "Keycloak user '$USERNAME' was not found in realm '$REALM'." >&2
  exit 1
fi

"$KCADM" update "users/$user_id/groups/$group_id" \
  -r "$REALM" \
  -s "realm=$REALM" \
  -s "userId=$user_id" \
  -s "groupId=$group_id"

client_json=$("$KCADM" get clients -r "$REALM" -q "clientId=$CLIENT_ID" -q exact=true)
client_uuid=$(printf '%s\n' "$client_json" | first_id || true)
if [[ -z "$client_uuid" ]]; then
  echo "Keycloak client '$CLIENT_ID' was not found in realm '$REALM'." >&2
  exit 1
fi

mapper_json=$("$KCADM" get "clients/$client_uuid/protocol-mappers/models" -r "$REALM")
if ! grep -Eq '"name"[[:space:]]*:[[:space:]]*"groups"' <<<"$mapper_json"; then
  mapper_file=$(mktemp)
  trap 'rm -f "$mapper_file"' EXIT
  cat >"$mapper_file" <<'EOF'
{
  "name": "groups",
  "protocol": "openid-connect",
  "protocolMapper": "oidc-group-membership-mapper",
  "consentRequired": false,
  "config": {
    "full.path": "false",
    "id.token.claim": "true",
    "access.token.claim": "true",
    "userinfo.token.claim": "true",
    "claim.name": "groups"
  }
}
EOF
  "$KCADM" create "clients/$client_uuid/protocol-mappers/models" \
    -r "$REALM" \
    -f "$mapper_file"
fi

echo "Keycloak group '$GROUP' exists, user '$USERNAME' is a member, and client '$CLIENT_ID' exposes groups in its ID token."
