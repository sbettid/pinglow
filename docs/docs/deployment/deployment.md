---
sidebar_position: 2
---

# Deployment

To deploy Pinglow, you can follow these steps: 

- Create a dedicated namespace in your Kubernetes cluster
- Deploy the Helm chart contained in this repo either through ArgoCD or through a
  manual installation after cloning the repository. By default, configure
  `DBEnvFromSecret` with an externally managed TimescaleDB secret. To deploy the
  database with Pinglow instead, set `timescaledb.enabled=true`; the chart then
  creates the TimescaleDB StatefulSet, Service, PVC, and credentials Secret and
  configures Pinglow to use it. The generated password is retained across Helm
  upgrades. To provide credentials yourself, set `timescaledb.existingSecret`;
  that Secret must contain `POSTGRES_USER` and `POSTGRES_PASSWORD`.

  The bundled database is a single-instance deployment intended for
  development/testing or installations where high availability is not needed.
  For production high availability, use an external or operator-managed
  TimescaleDB deployment and keep `timescaledb.enabled=false`.

  For an externally managed database, the [official TimescaleDB Kubernetes
  documentation](https://docs.tigerdata.com/self-hosted/latest/install/installation-kubernetes/)
  can be followed.
    
- Adapt the `values.yaml` file to specify the references to the secrets needed for the deployment
     
    - `DBEnvFromSecret`: which should specify the name of a secret holding the following properties

        - `DB_HOST`: hostname of you timescaledb instance
        - `DB_USER`: username of an user in timescaledb with the privileged to manage a dedicated DB (by default named `pinglow`)
        - `DB_USER_PASSWORD`: password of the aforementioned user

    - `OidcEnvFromSecret`: optional; when set, the Secret must hold `OIDC_ISSUER_URL`, `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET`, and `OIDC_REDIRECT_URL`. Omit it for API-key-only deployments.

    - `RedisPasswordSecret`: which specifies the name of a secret holding a single property named `REDIS_PASSWORD` which represents the password using to authenticate to Redis.

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
