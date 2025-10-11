---
sidebar_position: 3
---

# Continuous deployment


A quick note on the continuous deployment approach of Pinglow: the engine watches the various CustomResources in the namespace and immediately applies any change made to them, reloading only the modified objects. 