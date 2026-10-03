---
sidebar_position: 1
---

# Pinglow

Pinglow is a prototype of a Kubernetes ready monitoring engine. It is a toy project developed in my free time, so please bear with me for any limit you may find in it. Of course, any feedback or contribution is warmly welcomed!

The core idea is to allow defining the core components, such as checks, scripts and notification channels through Custom
Resources, running the checks inside one or more runners, with optional auto-scaling capabilities.

Pinglow is written in Rust and provides also a Helm Chart for a faster deployment. 