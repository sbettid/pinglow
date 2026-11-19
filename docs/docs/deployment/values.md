---
sidebar_position: 5
---

# Helm values reference

All the options of the chart, with their defaults, as found in `charts/pinglow/values.yaml`.

## Controller (`pinglow`)

| Value | Default | Description |
| --- | --- | --- |
| `pinglow.db.secretName` | `pinglow-db-credentials` | Secret with `DB_HOST`, `DB_USER` and `DB_USER_PASSWORD` of an external database. Ignored when `timescaledb.enabled=true`. |
| `pinglow.dbRetentionCheckResults` | `"7 days"` | Retention of check results. Any PostgreSQL `INTERVAL`, see [data retention](/docs/deployment/#data-retention-configuration). |
| `pinglow.dbRetentionPerfData` | `"7 days"` | Retention of performance data. |
| `pinglow.OidcEnvFromSecret` | `pinglow-oidc` | Secret with the OIDC settings. Optional: if the Secret does not exist OIDC is disabled. See [OIDC authentication](/docs/deployment/oidc). |
| `pinglow.extraCAConfigMap` | `""` | ConfigMap with a `ca.crt` key, trusted when contacting the OIDC provider. |
| `pinglow.resources` | requests `250m` / `256Mi`, limits `500m` / `512Mi` | CPU and memory of the controller. |

## Runner (`runner`)

The runners execute the checks. Their number is only adjusted automatically when `keda.enabled=true`; otherwise `minReplicas` replicas are run.
The runner container starts as UID `0` with only `SETUID` and `SETGID` added and all other Linux capabilities dropped. It switches script Checks to the image's dedicated `pinglow-check` account (UID/GID `10001`), clears supplementary groups and capabilities, installs a seccomp filter blocking cross-process inspection syscalls, and applies a separate Landlock domain to each execution. The sandbox exposes only each Check's own `/proc` entry plus selected system metrics and does not expose `/etc/passwd` or `/etc/group`, preventing access to other Checks' process environments and host account listings. The runner keeps privilege escalation disabled and uses a read-only root filesystem; its own GID is `10000` for the runner work volume. Clusters with Pod Security policies must allow a root container and `SETUID`/`SETGID` for the runner. Script workspaces use POSIX ACLs and require ACL support on the mounted filesystem. Worker nodes must support Landlock ABI v6 (Linux 6.12 or newer); runners fail closed otherwise. Network access is retained for checks, so use a NetworkPolicy or node firewall to block cloud metadata and other credential endpoints.

| Value | Default | Description |
| --- | --- | --- |
| `runner.namespace` | `<release-name>-runner` | Dedicated namespace for the runner Deployment and KEDA resources. Must differ from the release namespace. |
| `runner.createNamespace` | `true` | Create the runner namespace. Set to `false` if it is provisioned separately. |
| `runner.redis.secretName` | `<release-name>-runner-redis-password` | Runner-only Secret with `REDIS_PASSWORD`, required in both namespaces so Redis can configure its ACL and the runner/KEDA can authenticate. When empty, the chart creates matching Secrets in both namespaces with a distinct password. |
| `runner.minReplicas` | `1` | Minimum (and, without KEDA, fixed) number of runners. |
| `runner.maxReplicas` | `10` | Maximum number of runners. |
| `runner.queueLengthThreshold` | `10` | Number of pending checks in the queue that triggers scaling out. |
| `runner.activationThreshold` | `1` | Number of pending checks needed to activate scaling. |
| `runner.pollingInterval` | `15` | How often, in seconds, KEDA checks the queue. |
| `runner.cooldownPeriod` | `300` | Seconds to wait before scaling down. |
| `runner.resources` | requests `500m` / `256Mi`, limits `1000m` / `512Mi` | CPU and memory of each runner. |

## Agent gateway (`agentGateway`)

| Value | Default | Description |
| --- | --- | --- |
| `agentGateway.service.enabled` | `true` | Creates an internal `ClusterIP` Service for the controller's agent endpoints. The chart does not create an Ingress or LoadBalancer. |

## Autoscaling (`keda`)

| Value | Default | Description |
| --- | --- | --- |
| `keda.enabled` | `false` | Creates a KEDA `ScaledObject` for the runners, based on the Redis queue. Requires [KEDA](https://keda.sh/docs/latest/deploy/) installed in the cluster. |

## Redis (`redis`)

| Value | Default | Description |
| --- | --- | --- |
| `redis.secretName` | `""` | Controller Secret with a `REDIS_PASSWORD` key. When empty, the chart generates it as `pinglow-redis-password` by default. The runner has a separate ACL user and password. |

## Bundled TimescaleDB (`timescaledb`)

Single instance for development and testing, see [Deployment](/docs/deployment/#option-b-database-bundled-with-the-chart-development-and-testing).

| Value | Default | Description |
| --- | --- | --- |
| `timescaledb.enabled` | `false` | Deploys TimescaleDB together with Pinglow. |
| `timescaledb.image.repository` | `timescale/timescaledb-ha` | Image repository. |
| `timescaledb.image.tag` | `pg18` | Image tag. |
| `timescaledb.image.pullPolicy` | `IfNotPresent` | Image pull policy. |
| `timescaledb.secretName` | `""` | Own credentials Secret, with `POSTGRES_USER` and the password key. When empty, the chart generates `pinglow-db-credentials`. |
| `timescaledb.username` | `postgres` | Database user of the generated Secret. |
| `timescaledb.database` | `pinglow` | Name of the database. |
| `timescaledb.passwordKey` | `POSTGRES_PASSWORD` | Key holding the password in the credentials Secret. Change it only together with `timescaledb.secretName`: the generated Secret always uses `POSTGRES_PASSWORD`. |
| `timescaledb.persistence.enabled` | `true` | Store the data on a PersistentVolumeClaim. |
| `timescaledb.persistence.storageClassName` | `""` | Storage class of the PVC (cluster default when empty). |
| `timescaledb.persistence.accessModes` | `[ReadWriteOnce]` | PVC access modes. |
| `timescaledb.persistence.size` | `10Gi` | PVC size. |
| `timescaledb.resources` | requests `250m` / `512Mi`, limits `1000m` / `1Gi` | CPU and memory of the database. |
