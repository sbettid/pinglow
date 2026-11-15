---
sidebar_position: 2
---

# Architecture

Pinglow follows a controller-workers architecture, with the following roles:

- Pinglow: loads Checks and Scripts from Kubernetes, schedules them to the cluster runner or a selected remote agent, and processes results.
- Pinglow runner(s): poll the cluster Redis stream and execute scripts or built-in collectors. Results are published to a separate Redis stream.
- Pinglow agent(s): connect outbound to the controller over HTTPS long-polling, execute only Checks assigned to their target and locally allowlisted, then submit results to the controller.
- Redis: used in the indirect communication between the controller and the runners, both for scheduling check execution and for sending back the results.
- TimescaleDB: used to store permanently the check results and possibly related performance data. 

The agent gateway is part of the controller API and accesses Redis internally. The chart provides a `ClusterIP` service for the gateway but does not create an Ingress, LoadBalancer, or other external exposure. Remote agents need a separately configured private HTTPS path such as a VPN or an authenticated reverse proxy; Redis must remain private.

Active Checks choose exactly one execution type: a referenced Script or a typed built-in collector. Both use the shared executor. `runOn` selects the environment: `cluster` (the default) or `agent/<id>`.

Below a diagram depicts the overall architecture: 

![Pinglow Architecture](/img/pinglow_architecture.svg)