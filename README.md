# Pinglow

Pinglow is a prototype of a Kubernetes ready monitoring engine. It is a toy project developed in my free time, so please bear with me for any limit you may find in it. Of course, any feedback or contribution is warmely welcomed!

The core idea is to allow defining the core components, such as checks, scripts and notification channels through Custom
Resources, running then the checks as Kubernetes jobs to ensure their full isolation and reduce the possibilty of a compromise.

Pinglow is written in Rust and provides also an Helm Chart for a faster deployment. 

## Requirements

The only requirement is [TimescaleDB](https://github.com/timescale/timescaledb) from TigerData, to store
check results and performance data in hypertables. 

## Deploying Pinglow

To deploy pinglow, you can follow these steps: 

- Create a dedicated namespace in your Kubernetes cluster
- Deploy timescaledb. For this the [official documentation](https://docs.tigerdata.com/self-hosted/latest/install/installation-kubernetes/)
  can be followed. 
- Deploy the helm charts contained in this repo either throuh ArgoCD or through manual installation after cloning the repository
    
- Adapt the `values.yaml` file to specify the references to the two secrets needed for the deployment
     
    - `DBEnvFromSecret`: which should specify the following properties

        - `DB_HOST`: hostname of you timescaledb instance
        - `DB_USER`: username of an user in timescaledb with the privileged to manage a dedicated DB (by default named `pinglow`)
        - `DB_USER_PASSWORD`: password of the aforementioned user

    - `ApiKeyEnvFromSecret`: which specifies a single property named `API_KEY` which represents the API key used to authenticate to the RestAPI offered by Pinglow.

## Core concepts

The basic idea is that monitoring is performed through `Checks` which execute a specific `Script` (Bash and Python are supported for now) as a Kubernetes job. In case a critical or warning is returned by the check, this is automatically reported as a notification in the specified `TelegramChannel`. 

`Check` statuses are defined by the following elements: 

- The check output, represented as a String
- The check result represented by one of the following numbers

    - `0`: okay
    - `1`: warning
    - `2`: critical
    - `3`: unknown

For example, if I would like to monitor the Below an example of a simple `Check`, `Script` and `TelegramChannel`. 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: my-service-reachability
  namespace: pinglow
spec:
  scriptRef: check-service
  interval: 
  secretRefs:
    - my-service-definition
  telegramChannelRefs:
    - main-channel
```

As we can see, the `Check` references a standard secret and so its keys and values will be automatically passed as environment variables in the Kubernetes job used to run the script.

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Script
metadata:
  name: my-service-definition
  namespace: pinglow
spec:
  language: Bash
  content: |
    if ! curl -u "${USER}:${PASSWORD}" "https://${ENDPOINT}"" ; then
        echo "[!] Error in reaching ${ENDPOINT}"
        exit 2
    fi

    exit 0
```

In case the script would have been a Python script, an additional `python_requirements` property is available to specify requirements which need to be installed to run the script.

Below we can find the definition for the `TelegramChannel`, with the `chatId` and the reference to a secret containing the property `botToken`. 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: TelegramChannel
metadata:
  name: main-channel
spec:
  chatId: "-1234567890123"
  botTokenRef: "main-channel-token"
```

For more definition about the specific properties please see the [CRDs definition](helm-charts/pinglow/templates/custom-rd.yaml).

### Continuous deployment

A quick note on the continuous deployment approach of Pinglow: the engine watches the various CustomResources in the namespace and immediately applies any change made to them, reloading only the modified objects. 