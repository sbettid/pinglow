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
Each script Check runs under its own unprivileged UID with a private workspace. Runner credentials are not inherited; only secrets explicitly referenced by that check are passed to its script. The UID mapping is persistent, and workspace ACLs prevent one Check from accessing another Check's virtual environment or files.

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

Pinglow provides `disk`, `memory`, and `load` collectors. Each emits a readable
summary followed by numeric performance data in the same `output|key=value`
format used by script checks. Threshold checks are inclusive: a value at the
critical threshold is CRITICAL; otherwise a value at the warning threshold is
WARNING.

The load collector replaces the former CPU-utilization collector. Migrate
existing `type: cpu` checks to `type: load` and configure load-specific
thresholds.

### Disk collector

| Parameter | Required | Meaning |
| --- | --- | --- |
| `paths` | Yes | One or more absolute paths. The collector checks the filesystem containing each path and deduplicates paths on the same filesystem. |
| `excludeMountPoints` | No | Exact mount points to skip. |
| `excludeFilesystemTypes` | No | Filesystem types to skip, such as `tmpfs` or `proc`. |
| `warningPercent` | Yes | Used-space percentage at which the result becomes WARNING; range 0–100. |
| `criticalPercent` | Yes | Used-space percentage at which the result becomes CRITICAL; range 0–100 and must be at least `warningPercent`. |

For example:

```yaml
collector:
  type: disk
  paths: ["/", "/mnt/data"]
  excludeMountPoints: ["/mnt/backup"]
  excludeFilesystemTypes: ["tmpfs", "proc", "sysfs"]
  warningPercent: 80
  criticalPercent: 90
```

### Memory collector

| Parameter | Required | Meaning |
| --- | --- | --- |
| `warningPercent` | Yes | Used-memory percentage at which the result becomes WARNING; range 0–100. |
| `criticalPercent` | Yes | Used-memory percentage at which the result becomes CRITICAL; range 0–100 and must be at least `warningPercent`. |

The collector reads Linux `/proc/meminfo` and reports used percentage and
available GiB. For example:

```yaml
collector:
  type: memory
  warningPercent: 80
  criticalPercent: 90
```

### Load collector

| Parameter | Required | Meaning |
| --- | --- | --- |
| `periodMinutes` | No | Selects the load average used for both status and output: `1`, `5`, or `15` minutes. Defaults to `1`. |
| `warningLoad` | Yes | Selected load average at which the result becomes WARNING; must be non-negative. |
| `criticalLoad` | Yes | Selected load average at which the result becomes CRITICAL; must be non-negative and at least `warningLoad`. |

The collector reads Linux `/proc/loadavg`. Load average is the average number
of runnable or uninterruptible tasks, not CPU utilization, so thresholds are
raw load values rather than percentages and should be tuned for the host.
For example, this check uses and reports only the five-minute average:

```yaml
collector:
  type: load
  periodMinutes: 5
  warningLoad: 2
  criticalLoad: 4
```

Its output is similar to `Load average (5 min): 0.31|load_5m=0.31`.

Collectors observe the Linux environment visible to the process. Disk paths
refer to the runner container's filesystem view when run in-cluster; procfs
load and memory values may be node-wide rather than cgroup-limited. Target an
agent when you specifically want to monitor that host's filesystem and local
resources.

## Execution targets and scripts

`runOn` defaults to `cluster`. A Check can target an agent with `runOn: agent/nas`. The agent must be configured to allow that Check. Script-based Checks are supported on agents as well as built-in collectors. Approving a script grants its code execution; the allowlist controls placement, not behavior. Deployed script Checks run in a filesystem/process sandbox separate from the service, but outbound networking remains available.

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