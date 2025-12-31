---
sidebar_position: 2
---

# Architecture

Pinglow follows a controller-workers architecture, with the following roles: 

- Pinglow: is the main controller and is in charge of loading the checks and scripts from the CRDs defined in the cluster, schedules the checks by pushing
  them to a Redis stream and handles the results, writing them in the DB and handling the correspondent notifications, if applicable.
- Pinglow worker(s): poll the Redis stream to obtain a check to run, load the related environment and execute the checks. Result are made available
  to the controller using a separate Redis stream. 
- Redis: used in the indirect communication between the controller and the runners, both for scheduling check execution and for sending back the results.
- TimescaleDB: used to store permanently the check results and possibly related performance data. 

Below a diagram depicts the overall architecture: 

![Pinglow Architecture](/img/pinglow_architecture.svg)