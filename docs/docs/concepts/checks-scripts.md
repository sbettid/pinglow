---
sidebar_position: 1
---

# Checks and scripts

The basic idea is that monitoring is performed through `Checks` which execute either a `Script` (Python) or a built-in collector. Checks run in the cluster by default; `runOn: agent/<id>` routes one to that host's Pinglow agent.

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

As we can see, the `Check` references a standard secret and so its keys and values will be automatically passed as environment variables to the script when it runs.
Scripts run under a separate unprivileged user. Runner credentials are not inherited; only secrets explicitly referenced by that check are passed to its script.

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Script
metadata:
  name: check-service
  namespace: pinglow
spec:
  language: Python
  python_requirements:
    - requests
  content: |
    import requests
    import os
    import sys

    url = os.environ.get("URL")

    response = requests.get(url, timeout=5)

    if response.status_code != 200:
      print("Error in contacting endpoint")
      sys.exit(2)
```

The exit code of the script is used as the check result: exiting with `2`, as above, marks the check as critical, while a script that exits normally (`0`) is reported as okay.

## Built-in collectors

Collectors use the same Check scheduling and result pipeline as scripts, but are configured directly on the Check and do not need a `Script` resource. Exactly one of `scriptRef` or `collector` must be set on an active Check.

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: host-disk
  namespace: pinglow
spec:
  runOn: agent/nas
  interval: 60
  collector:
    type: disk
    paths: ["/", "/mnt/data"]
    excludeMountPoints: ["/mnt/backup"]
    excludeFilesystemTypes: ["tmpfs", "proc", "sysfs"]
    warningPercent: 80
    criticalPercent: 90
```

The initial collectors are `disk`, `memory`, and `cpu`. Disk checks inspect the filesystems containing the configured paths, deduplicate paths on the same filesystem, and support excluding mount points and filesystem types. Memory reports used percentage and available GiB. CPU reports usage sampled over `sampleSeconds` (1–60, default 1). Thresholds use inclusive warning/critical percentages.

Collector output includes a readable summary followed by comma-separated numeric performance data, for example `Disk /mnt/data: 72.4% used, 38.2 GiB free|disk_mnt_data_used_pct=72.4,disk_mnt_data_free_gib=38.2`.

Collectors observe the Linux environment visible to the process. Disk paths refer to the runner container's filesystem view when run in-cluster; procfs CPU and memory values may be node-wide rather than cgroup-limited. Target an agent when you specifically want to monitor that host's filesystem and local resources.

## Execution targets and scripts

`runOn` defaults to `cluster`. A Check can target an agent with `runOn: agent/nas`. The agent must be configured to allow that Check. Script-based Checks are supported on agents as well as built-in collectors; approving a script means trusting its code to execute as the agent's unprivileged service user. The allowlist is not a sandbox.

## Performance data

When writing a script, it is possible to print not only the general output, but also some performance data that will be stripped out from the output
and wrote separately in a dedicated table in TimescalDB (and returned also separately by the API).

To specify both an output and some performance data, it is possible to use the following format: `output|key=value,key2=value`. 

For example, a script which may read some temperature and humidity data may be partially similar to what depicted below: 

```yaml
apiVersion: pinglow.io/v1alpha1
kind: Script
metadata:
  name: script-temperature-humidity
  namespace: pinglow
spec:
  language: Python
  content: |
    temperature = getTemperature()
    humidity = getHumidity()

    print(f"Your temperature and humidity are OK!|temperature={temperature},humidity={humidity}")
```

## Passive checks

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

## Grouping checks

Checks can be organized into groups using `CheckGroup` resources. A group's Kubernetes name is its stable identity; checks refer to it with `groupRef`. Its `displayName` is a human-readable name that can be changed without changing that identity.

```yaml
apiVersion: pinglow.io/v1alpha1
kind: CheckGroup
metadata:
  name: home-network
  namespace: pinglow
spec:
  displayName: Home network
---
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: router-reachability
  namespace: pinglow
spec:
  groupRef: home-network
  scriptRef: check-router
  interval: 300
```

Checks may omit `groupRef` if they do not belong to a group. Grouping is organizational metadata and does not restrict API access.