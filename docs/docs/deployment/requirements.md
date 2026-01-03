---
sidebar_position: 1
---

# Requirements

There is one hard external requirement which needs to be managed externally, plus an optional one: 

1) **TimescaleDB**: used to store check results and performance data in hypertables. To allow an easier management and deduplication from Pinglow, it needs to be installed separately by following the official [documentation](https://github.com/timescale/timescaledb).
2) **Keda (optional)**: in case the worker autoscaling feature is enabled, Keda needs to be installed separately, following the official [procedure](https://keda.sh/docs/2.18/deploy/) 

Redis, which is used in the communication between the controller and the runners, is already included in the Helm Chart since it is automatically managed.