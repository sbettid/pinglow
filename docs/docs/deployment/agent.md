---
sidebar_position: 5
---

import useBaseUrl from '@docusaurus/useBaseUrl';

# Pinglow agent

The agent is a Linux service that connects outbound to Pinglow's controller using HTTPS long-polling. It does not connect to Redis or TimescaleDB. The Helm chart provides an internal `ClusterIP` service only; it creates no Ingress or LoadBalancer. To reach a remote host, expose the controller API through a private VPN or an operator-managed HTTPS reverse proxy. Do not expose Redis.

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

`agent_targets` is the list of queues this agent is allowed to consume. Older bindings using the single `agent_target` field are still supported. To form a group, give each group member a separate binding and key that includes the same target, such as `linux-hosts`. Set `runOn: agent/linux-hosts` on a Check; the agents in that group share one Redis consumer group, so each queued run is assigned to one agent. Delivery is at-least-once, so a job may be retried after a worker failure.

An agent can be authorized for both its own target and shared groups. The `agentId` in the agent YAML is a local logging label; target authorization comes from the API-key binding.

## Install on Linux

Download the matching `pinglow-agent` archive for `linux-amd64` or `linux-arm64` from the GitHub release and verify its SHA-256 checksum. The archive contains the binary, example configuration, and systemd unit. Install the `acl` package if `setfacl` is not already available.

Create a dedicated unprivileged account and directories:

```bash
sudo useradd --system --user-group --home-dir /var/lib/pinglow-agent --create-home \
  --shell /usr/sbin/nologin pinglow-agent
sudo install -d -o root -g pinglow-agent -m 0750 /etc/pinglow-agent
sudo install -d -o pinglow-agent -g pinglow-agent -m 0750 \
  /var/lib/pinglow-agent/checks
sudo install -o root -g root -m 0755 pinglow-agent /usr/local/bin/pinglow-agent
sudo install -o root -g root -m 0644 agent.example.yaml /etc/pinglow-agent/agent.yaml
sudo install -o root -g root -m 0644 pinglow-agent.service \
  /etc/systemd/system/pinglow-agent.service
```

Write the API key from the Kubernetes Secret to `/etc/pinglow-agent/api-key` with owner `root:root` and mode `0600`, then grant read access only to the agent service account with an ACL:

```bash
kubectl get secret nas-agent-api-key -n pinglow \
  -o jsonpath='{.data.API_KEY}' | base64 --decode \
  | sudo tee /etc/pinglow-agent/api-key >/dev/null
sudo chown root:root /etc/pinglow-agent/api-key
sudo chmod 0600 /etc/pinglow-agent/api-key
sudo setfacl -m u:pinglow-agent:r /etc/pinglow-agent/api-key
```

Do not put the key in the YAML file or command-line arguments. The ACL keeps the key unavailable to check subprocesses, which run as the unprivileged `nobody` UID.

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

Keep `dropPrivileges` enabled (the default) in service deployments. It requires the unit's restricted `CAP_SETUID` capability and runs scripts as `nobody`. Disabling it is intended only for localhost development when the agent process has no such capability; scripts then run with the agent process's own user permissions. Do not use that setting for a production host.

Use the systemd unit from the release archive, review it for your distribution, then enable it:

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now pinglow-agent
sudo journalctl -u pinglow-agent -f
```

The <a href={useBaseUrl('/downloads/pinglow-agent.service')} download>systemd unit is available to download</a>. It is also included in each agent release archive. The service runs as `pinglow-agent`, not root. Its unit grants only `CAP_SETUID`, used to drop script subprocesses to the `nobody` UID; the subprocess capabilities are cleared. Built-in collectors run in the agent process. Grant additional read access narrowly with groups or ACLs; do not run the agent as root to make a check pass.

The allowlist is an explicit host-owner approval boundary, not a sandbox. An allowlisted script can execute code as the `nobody` user and may receive the Check's referenced secrets. Review changes to allowed Checks and rotate the API key by updating the binding and the host's key file.
