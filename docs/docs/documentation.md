---
sidebar_position: 1
---

# Pinglow

Pinglow is a prototype of a Kubernetes ready monitoring engine. It is a toy project developed in my free time, so please bear with me for any limit you may find in it. Of course, any feedback or contribution is warmly welcomed!

The core idea is to define checks, scripts and notification channels through Kubernetes Custom Resources, then run checks in
cluster runners or on allowlisted remote Linux agents. Checks can use scripts or built-in disk, CPU and memory collectors.

Pinglow is written in Rust and provides also a Helm Chart for a faster deployment.

## Web interface

The [Pinglow frontend](https://github.com/sbettid/pinglow-frontend) provides a web interface for exploring check statuses and performance data.

![Pinglow web frontend home screen](/img/screenshots/home.png)
