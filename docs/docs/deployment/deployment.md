---
sidebar_position: 2
---

# Deployment

To deploy Pinglow, you can follow these steps: 

- Create a dedicated namespace in your Kubernetes cluster
- Deploy timescaledb. For this the [official documentation](https://docs.tigerdata.com/self-hosted/latest/install/installation-kubernetes/)
  can be followed. 
- Deploy the helm charts contained in this repo either throuh ArgoCD or through manual installation after cloning the repository
    
- Adapt the `values.yaml` file to specify the references to the two secrets needed for the deployment
     
    - `DBEnvFromSecret`: which should specify the name of a secret holding the following properties

        - `DB_HOST`: hostname of you timescaledb instance
        - `DB_USER`: username of an user in timescaledb with the privileged to manage a dedicated DB (by default named `pinglow`)
        - `DB_USER_PASSWORD`: password of the aforementioned user

    - `ApiKeyEnvFromSecret`: which specifies the name of a secret holding a single property named `API_KEY` which represents the API key used to authenticate to the RestAPI offered by Pinglow.

    - `RedisPasswordSecret`: which specifies the name of a secret holding a single property named `REDIS_PASSWORD` which represents the password using to authenticate to Redis.