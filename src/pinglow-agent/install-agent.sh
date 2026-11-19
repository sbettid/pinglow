#!/usr/bin/env bash
set -euo pipefail
umask 077

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
CONFIG_PATH=/etc/pinglow-agent/agent.yaml
KEY_PATH=/etc/pinglow-agent/api-key
SERVICE_PATH=/etc/systemd/system/pinglow-agent.service
INSTALL_BIN_PATH=/usr/local/bin

die() {
  echo "Error: $*" >&2
  exit 1
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "required command not found: $1"
}

for command in cut getent install mktemp setfacl sudo systemctl useradd; do
  require_command "$command"
done

for asset in pinglow-agent pinglow-check-launcher pinglow-agent.service agent.example.yaml; do
  [[ -f "$SCRIPT_DIR/$asset" ]] || die "release archive is missing $asset"
done

[[ "$EUID" -ne 0 ]] || die "run this script as your regular user, not as root"
[[ -t 0 ]] || die "run this installer interactively to enter configuration and the API key securely"

first_install=false
if [[ -e "$CONFIG_PATH" || -L "$CONFIG_PATH" ]]; then
  [[ -f "$CONFIG_PATH" && ! -L "$CONFIG_PATH" ]] \
    || die "existing agent configuration is not a regular file: $CONFIG_PATH"
  echo "Existing agent configuration found; it will be preserved."
else
  first_install=true
  read -r -p "Agent ID [nas]: " agent_id
  agent_id="${agent_id:-nas}"

  read -r -p "Controller HTTPS URL: " controller_url
  [[ "$controller_url" == https://?* && "$controller_url" != *[$' \t\n']* ]] \
    || die "controller URL must be an HTTPS URL without whitespace"

  read -r -p "Allowed Check names (comma-separated): " check_list
  [[ -n "$check_list" ]] || die "at least one allowed Check is required"
  IFS=',' read -r -a checks <<< "$check_list"
  for index in "${!checks[@]}"; do
    check="${checks[$index]}"
    check="${check#"${check%%[![:space:]]*}"}"
    check="${check%"${check##*[![:space:]]}"}"
    [[ -n "$check" ]] || die "allowed Check names must not be empty"
    checks[$index]="$check"
  done
fi

tmp_key="$(mktemp)"
tmp_config=""
cleanup() {
  rm -f -- "$tmp_key"
  if [[ -n "$tmp_config" ]]; then
    rm -f -- "$tmp_config"
  fi
}
trap cleanup EXIT

read -r -s -p "Agent API key: " api_key || die "could not read the API key"
printf '\n'
[[ -n "$api_key" ]] || die "API key must not be empty"
printf '%s' "$api_key" > "$tmp_key"
unset api_key
[[ -s "$tmp_key" ]] || die "API key input was empty"

sudo -A -v

yaml_quote() {
  local value="$1"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

if [[ "$first_install" == true ]]; then
  tmp_config="$(mktemp)"
  {
    printf 'agentId: %s\n' "$(yaml_quote "$agent_id")"
    printf 'controllerUrl: %s\n' "$(yaml_quote "$controller_url")"
    printf 'apiKeyFile: %s\n' "$KEY_PATH"
    printf 'allowedChecks:\n'
    for check in "${checks[@]}"; do
      printf '  - %s\n' "$(yaml_quote "$check")"
    done
    printf 'checksBasePath: /var/lib/pinglow-agent/checks\n'
    printf 'executionTimeoutSeconds: 120\n'
  } > "$tmp_config"
fi

nologin=/usr/sbin/nologin
if [[ ! -x "$nologin" ]]; then
  nologin=/sbin/nologin
fi
[[ -x "$nologin" ]] || die "could not find nologin shell at /usr/sbin/nologin or /sbin/nologin"

if ! getent passwd pinglow-agent >/dev/null; then
  sudo -A useradd -r -U -d /var/lib/pinglow-agent -s "$nologin" -m pinglow-agent
fi
if ! getent passwd pinglow-check >/dev/null; then
  sudo -A useradd -r -U -M -d /nonexistent -s "$nologin" pinglow-check
fi

agent_uid="$(getent passwd pinglow-agent | cut -d: -f3)"
agent_gid="$(getent passwd pinglow-agent | cut -d: -f4)"
check_uid="$(getent passwd pinglow-check | cut -d: -f3)"
check_gid="$(getent passwd pinglow-check | cut -d: -f4)"
getent group pinglow-agent >/dev/null || die "agent account is missing its private group"
getent group pinglow-check >/dev/null || die "Check account is missing its private group"
agent_group_gid="$(getent group pinglow-agent | cut -d: -f3)"
check_group_gid="$(getent group pinglow-check | cut -d: -f3)"
[[ "$agent_uid" != 0 && "$check_uid" != 0 ]] || die "agent and Check accounts must be non-root"
[[ "$agent_uid" != "$check_uid" && "$agent_gid" != "$check_gid" ]] \
  || die "agent and Check accounts must have distinct UIDs and GIDs"
[[ "$agent_gid" == "$agent_group_gid" && "$check_gid" == "$check_group_gid" ]] \
  || die "agent and Check accounts must use their matching private groups"

sudo -A install -d -o root -g pinglow-agent -m 0750 /etc/pinglow-agent
sudo -A install -d -o pinglow-agent -g pinglow-agent -m 0750 \
  /var/lib/pinglow-agent/checks
sudo -A install -o root -g root -m 0755 "$SCRIPT_DIR/pinglow-agent" \
  "$INSTALL_BIN_PATH/pinglow-agent"
sudo -A install -o root -g pinglow-agent -m 0750 \
  "$SCRIPT_DIR/pinglow-check-launcher" "$INSTALL_BIN_PATH/pinglow-check-launcher"
if [[ "$first_install" == true ]]; then
  sudo -A install -o root -g root -m 0644 "$tmp_config" "$CONFIG_PATH"
else
  sudo -A chown root:root "$CONFIG_PATH"
  sudo -A chmod 0644 "$CONFIG_PATH"
fi
sudo -A install -o root -g root -m 0600 "$tmp_key" "$KEY_PATH"
sudo -A setfacl -m u:pinglow-agent:r "$KEY_PATH"
sudo -A install -o root -g root -m 0644 "$SCRIPT_DIR/pinglow-agent.service" \
  "$SERVICE_PATH"

sudo -A systemctl daemon-reload
sudo -A systemctl enable --now pinglow-agent

echo "Pinglow agent installed and started."
echo "Configuration: $CONFIG_PATH"
echo "Review allowedChecks and controllerUrl before relying on this agent."
