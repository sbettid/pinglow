---
sidebar_position: 1
---

# Requirements

There is one hard external requirement which needs to be managed externally, plus an optional one: 

1) **TimescaleDB**: used to store check results and performance data in hypertables. It can be managed externally, or installed by the chart with `timescaledb.enabled=true`. The bundled deployment is single-instance and intended for development/testing or installations where high availability is not needed; production deployments should use an external or operator-managed database.
2) **Keda (optional)**: in case the worker autoscaling feature is enabled, Keda needs to be installed separately, following the official [procedure](https://keda.sh/docs/latest/deploy/) 

Redis, which is used in the communication between the controller and the runners, is already included in the Helm Chart since it is automatically managed.