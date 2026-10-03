---
sidebar_position: 3
---

# Deployment

This page describes a regular installation of Pinglow with the Helm chart. If you just want to try it out, the [quick start](/docs/deployment/quick-start) is faster.

## 1. Prepare the namespace

Pinglow watches the custom resources (`Check`, `Script`, ...) of the namespace it is installed in, so create a dedicated one and keep all your Pinglow resources there:

```bash
kubectl create namespace pinglow
```

## 2. Provide the database

Pinglow needs a TimescaleDB instance (see the [requirements](/docs/deployment/requirements)). You have two options.

### Option A: externally managed database (recommended for production)

Create a Secret, named `pinglow-db-credentials` by default, with the connection details:

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: pinglow-db-credentials
  namespace: pinglow
type: Opaque
stringData:
  DB_HOST: timescaledb.database.svc.cluster.local
  DB_USER: pinglow
  DB_USER_PASSWORD: change-me
```

| Key | Description |
| --- | --- |
| `DB_HOST` | Hostname of your TimescaleDB instance. |
| `DB_USER` | User with the privileges to manage a dedicated database (named `pinglow` by default). |
| `DB_USER_PASSWORD` | Password of that user. |

If you use a different Secret name, set it in the values:

```yaml
pinglow:
  db:
    secretName: "my-db-secret"
```

For installing TimescaleDB on Kubernetes, see the [official documentation](https://docs.tigerdata.com/self-hosted/latest/install/installation-kubernetes/).

### Option B: database bundled with the chart (development and testing)

```yaml
timescaledb:
  enabled: true
```

The chart then creates the TimescaleDB StatefulSet, Service, PVC and a credentials Secret, and configures Pinglow to use it. The generated password is kept across Helm upgrades.

To provide the credentials yourself, set `timescaledb.secretName`; that Secret must contain `POSTGRES_USER` and `POSTGRES_PASSWORD`.

:::caution
The bundled database is a single instance, intended for development/testing or installations that do not need high availability. For production use an external or operator-managed TimescaleDB and keep `timescaledb.enabled=false`.
:::

## 3. Redis

Redis is deployed by the chart. By default the chart generates a password in a Secret named `pinglow-redis-password`. To use your own, create a Secret holding a single key `REDIS_PASSWORD` and reference it:

```yaml
redis:
  secretName: "my-redis-secret"
```

## 4. OIDC (optional)

To let users log in from the browser through an OIDC provider, create the OIDC Secret as described in [OIDC authentication](/docs/deployment/oidc). Without it, Pinglow can still be used through [API keys](/docs/concepts/automation-credentials).

## 5. Install the chart

The chart is part of the repository, and a packaged version is attached to every [GitHub release](https://github.com/sbettid/pinglow/releases). Install it from a clone of the repository:

```bash
helm install pinglow ./charts/pinglow \
  --namespace pinglow \
  --values my-values.yaml
```

:::important
Keep the release name `pinglow`. The container images (`ghcr.io/sbettid/<release name>`), the service account and the role are all derived from, or hardcoded to, this name.
:::

Alternatively the chart can be deployed with ArgoCD, pointing it to the `charts/pinglow` path of the repository.

All the available options are listed in the [Helm values reference](/docs/deployment/values).

## Upgrading

```bash
helm upgrade pinglow ./charts/pinglow --namespace pinglow --values my-values.yaml
```

Helm installs the CRDs contained in the chart's `crds/` directory on the first install, but does not update them on upgrade. After pulling a new version, apply them manually:

```bash
kubectl apply -f charts/pinglow/crds/
```

## Data Retention Configuration

Pinglow stores check results and performance data in TimescaleDB hypertables with automatic data retention policies. You can configure separate retention periods for each table using Helm values:

```yaml
pinglow:
  # Retention policy for check results (default: 7 days)
  dbRetentionCheckResults: "7 days"
  
  # Retention policy for performance data (default: 7 days)
  dbRetentionPerfData: "30 days"
```

### Retention Policy Format

The retention policy follows PostgreSQL `INTERVAL` syntax. Common examples:
- `7 days` - 7 days (default)
- `30 days` - 30 days
- `1 year` - 1 year
- `3 months` - 3 months
- `1 week` - 1 week
- `12 hours` - 12 hours

### How It Works

- **Automatic Application**: Retention policies are applied automatically when Pinglow starts up
- **Independent Policies**: You can set different retention periods for check results and performance data
- **Idempotent**: The retention policies can be updated by changing the Helm values and redeploying; they will be automatically reconfigured
- **Compression**: Data is automatically compressed before reaching the end of its retention period to save disk space

### Example: Store performance data longer than results

```yaml
pinglow:
  # Keep check results for 7 days
  dbRetentionCheckResults: "7 days"
  
  # But keep performance metrics for 90 days for historical analysis
  dbRetentionPerfData: "90 days"
```

This is useful when you want to keep trend analysis data longer than raw check results.
