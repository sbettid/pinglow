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

| Value | Default | Description |
| --- | --- | --- |
| `runner.minReplicas` | `1` | Minimum (and, without KEDA, fixed) number of runners. |
| `runner.maxReplicas` | `10` | Maximum number of runners. |
| `runner.queueLengthThreshold` | `10` | Number of pending checks in the queue that triggers scaling out. |
| `runner.activationThreshold` | `1` | Number of pending checks needed to activate scaling. |
| `runner.pollingInterval` | `15` | How often, in seconds, KEDA checks the queue. |
| `runner.cooldownPeriod` | `300` | Seconds to wait before scaling down. |
| `runner.resources` | requests `500m` / `256Mi`, limits `1000m` / `512Mi` | CPU and memory of each runner. |

## Autoscaling (`keda`)

| Value | Default | Description |
| --- | --- | --- |
| `keda.enabled` | `false` | Creates a KEDA `ScaledObject` for the runners, based on the Redis queue. Requires [KEDA](https://keda.sh/docs/latest/deploy/) installed in the cluster. |

## Redis (`redis`)

| Value | Default | Description |
| --- | --- | --- |
| `redis.secretName` | `""` | Secret with a `REDIS_PASSWORD` key. When empty, the chart generates one named `pinglow-redis-password`. |

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
