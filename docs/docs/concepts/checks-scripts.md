---
sidebar_position: 1
---

# Checks and scripts

The basic idea is that monitoring is performed through `Checks` which execute a specific `Script` (Bash and Python are supported for now) as a Kubernetes job. 

`Check` statuses are defined by the following elements: 

- The check output, represented as a String
- The check result represented by one of the following numbers

    - `0`: okay
    - `1`: warning
    - `2`: critical
    - `3`: unknown
    - `4`: pending

For example, if we would like to monitor a certain service, we could define a `Check` as follows: 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: my-service-reachability
  namespace: pinglow
spec:
  scriptRef: check-service
  interval: 300
  secretRefs:
    - my-service-definition
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

# Passive checks

Sometimes, we do not want an active action from a check but instead we would like an external system to send the results of a certain operation
to our system. For this reason, it is possible to define a `Check` as `passive`. 

In this case, no script or check interval is needed and it is possible to set its status through the associated API (check out the
API reference for more information).

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: my-passive-check
  namespace: pinglow
spec:
  passive: true
```

Clearly, it is possible to get notifications also for passive check results. See the [notifications](notifications) section for more
information on how to configure them!