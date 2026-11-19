---
sidebar_position: 5
---

import useBaseUrl from '@docusaurus/useBaseUrl';

# Pinglow agent

The agent is a Linux service that connects outbound to Pinglow's controller using HTTPS long-polling. It does not connect to Redis or TimescaleDB. To reach a remote host, expose the controller API through a private VPN or an operator-managed HTTPS reverse proxy.

## Create an agent identity

Create a separate `ApiKeyBinding` for each installed agent. Pinglow creates the API-key Secret (by default `<binding-name>-api-key`, with an `API_KEY` key). Agent-scoped keys can only use the agent endpoints and cannot be used as operator API keys.

```yaml
apiVersion: pinglow.io/v1alpha1
kind: ApiKeyBinding
metadata:
  name: nas-agent
  namespace: pinglow
spec:
  role: operator
  agent_targets:
    - nas
    - linux-hosts
```

`agent_targets` lists every queue the agent may consume. The binding above authorizes this agent for both `agent/nas` and `agent/linux-hosts`. A Check's `runOn: agent/<target>` selects its queue; this agent can run Checks for either target as long as their names are in `allowedChecks`. To share a target such as `linux-hosts` across machines, give each agent its own binding and key with that target. The agents then compete for jobs in the shared queue, with at-least-once delivery.

The `agentId` in the agent YAML is only a local logging label, not a queue target or limit. For example, the agent can keep `agentId: nas` while consuming both `agent/nas` and `agent/linux-hosts`; `runOn` and the API-key binding determine the target.

## Install on Linux

Before installing, create the agent's `ApiKeyBinding` and make sure the host can reach the controller over HTTPS. Have the binding's API key available to enter during installation. You will need `curl`, `tar`, `sha256sum`, `sudo`, and `systemd`. Install your distribution's `acl` package if `setfacl` is not already available.

### 1. Download and verify the release

Set `VERSION` to the release tag you want to install. The commands below detect `amd64` or `arm64`, download the matching archive and checksum file, verify the archive, and extract it:

```bash
VERSION=vX.Y.Z # replace with the release tag, for example v0.17.0

case "$(uname -m)" in
  x86_64) ARCH=amd64 ;;
  aarch64|arm64) ARCH=arm64 ;;
  *) echo "Unsupported architecture: $(uname -m)" >&2; exit 1 ;;
esac

ARCHIVE="pinglow-agent-${VERSION}-linux-${ARCH}.tar.gz"
RELEASE_URL="https://github.com/sbettid/pinglow/releases/download/${VERSION}"
mkdir -p "$HOME/pinglow-agent-install"
cd "$HOME/pinglow-agent-install"
curl --fail --location --remote-name "$RELEASE_URL/$ARCHIVE"
curl --fail --location --remote-name "$RELEASE_URL/SHA256SUMS"
sha256sum --check --ignore-missing SHA256SUMS
tar -xzf "$ARCHIVE"
```

The archive contains the agent binary, Check sandbox launcher, example configuration, interactive installer, systemd unit, and a copy of these installation instructions.

### 2. Install with the bundled script

From the extracted archive directory, run:

```bash
bash ./install-agent.sh
```

The script prompts for the API key without echoing it, then uses `sudo` to create the service accounts, install the binaries and systemd unit, write the key with restrictive permissions and an ACL for `pinglow-agent`, and enable and start the service. On a first install, it also prompts for the agent ID, controller URL, and allowed Check names. Review the prompts and ensure the allowed names match Checks authorized for that agent.

On subsequent runs the script preserves the existing agent configuration, prompts for the API key again, and updates the binaries and service. It does not modify existing service accounts. To change the agent configuration, edit `/etc/pinglow-agent/agent.yaml` yourself.

### 3. Manual installation

If you prefer not to use the installer, or need to adapt the process for your distribution, follow these steps from the extracted archive directory.

#### Create accounts and install files

Create the service and Check accounts, then install the binaries, example configuration, and systemd unit:

```bash
sudo -A bash -eu <<'EOF'
useradd -r -U -d /var/lib/pinglow-agent -s /usr/sbin/nologin -m pinglow-agent
useradd -r -U -M -d /nonexistent -s /usr/sbin/nologin pinglow-check
install -d -o root -g pinglow-agent -m 0750 /etc/pinglow-agent
install -d -o pinglow-agent -g pinglow-agent -m 0750 \
  /var/lib/pinglow-agent/checks
install -o root -g root -m 0755 pinglow-agent /usr/local/bin/pinglow-agent
install -o root -g pinglow-agent -m 0750 pinglow-check-launcher \
  /usr/local/bin/pinglow-check-launcher
install -o root -g root -m 0644 agent.example.yaml /etc/pinglow-agent/agent.yaml
install -o root -g root -m 0644 pinglow-agent.service \
  /etc/systemd/system/pinglow-agent.service
EOF
```

#### Configure the API key and agent

Enter the API key without echoing it, write it to `/etc/pinglow-agent/api-key` with owner `root:root` and mode `0600`, then grant read access only to the agent service account with an ACL:

```bash
read -r -s -p "Agent API key: " API_KEY
printf '\n'
printf '%s' "$API_KEY" | sudo -A install -o root -g root -m 0600 \
  /dev/stdin /etc/pinglow-agent/api-key
unset API_KEY
sudo -A setfacl -m u:pinglow-agent:r /etc/pinglow-agent/api-key
```

Do not put the key in the YAML file or command-line arguments. The ACL keeps the key unavailable to Check subprocesses.

Example `/etc/pinglow-agent/agent.yaml`:

```yaml
agentId: nas
controllerUrl: https://pinglow.example.net
apiKeyFile: /etc/pinglow-agent/api-key
allowedChecks:
  - nas-disk
  - nas-memory
checksBasePath: /var/lib/pinglow-agent/checks
executionTimeoutSeconds: 120
```

#### Start and check the service

Use the systemd unit from the release archive, review it for your distribution, then enable it:

```bash
sudo -A systemctl daemon-reload
sudo -A systemctl enable --now pinglow-agent
sudo -A journalctl -u pinglow-agent -f
```

### 4. Runtime and security requirements (both methods)

For either installation method, use the HTTPS controller URL reachable from this host, and allow only the exact Check names approved for it. Each Check must also use `runOn: agent/<target>`, where `<target>` is listed in this agent's `ApiKeyBinding`.

Keep `dropPrivileges` enabled (the default) in service deployments. Each script Check runs as the dedicated `pinglow-check` account, distinct from `pinglow-agent`; all supplementary groups and capabilities are cleared. The service unit needs only `CAP_SETUID` and `CAP_SETGID` to switch to that account. The same account is reused for every Check, while each execution receives its own Landlock domain and private writable workspace.

Landlock ABI v6 is required; the agent fails closed at startup if the host kernel or service restrictions do not support it. This requires a Linux kernel with Landlock ABI v6 (available starting with Linux 6.12). The sandbox grants read-only access to the Python/system runtime, selected certificate and resolver files, the Check's own `/proc` entry and selected system metrics, and `/sys`; it grants write access only to that Check's `work-sandboxed` directory.A seccomp filter blocks cross-process inspection syscalls so Checks sharing the worker account cannot inspect one another's process memory. Outbound networking remains available for monitoring and Python package installation. Check execution also requires POSIX ACL support on the filesystem containing `checksBasePath`.

The agent's API key is read only by the agent process. Script subprocesses receive only the secrets explicitly configured for their Check.

The <a href={useBaseUrl('/downloads/pinglow-agent.service')} download>systemd unit is available to download</a>. It is also included in each agent release archive. The service runs as `pinglow-agent`, not root. Its unit grants only `CAP_SETUID` and `CAP_SETGID`; each script subprocess uses `pinglow-check` with no capabilities, supplementary groups, or inherited non-standard file descriptors. Workspaces use POSIX ACLs and Landlock; built-in collectors run in the agent process. Grant additional host data access narrowly with ACLs; do not run the agent as root to make a check pass.

The allowlist is an explicit host-owner approval boundary, not a sandbox. An allowlisted script can execute code as its Check's unprivileged UID and may receive that Check's referenced secrets. Review changes to allowed Checks and rotate the API key by updating the binding and the host's key file.
