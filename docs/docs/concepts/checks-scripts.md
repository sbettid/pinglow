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