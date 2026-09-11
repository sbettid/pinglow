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
