---
sidebar_position: 2
---

# Quick start

This walkthrough installs Pinglow with the bundled TimescaleDB on a test cluster, creates a first check and reads its result through the API. It is meant for trying Pinglow out; for a production setup see [Deployment](/docs/deployment/).

## Prerequisites

- A Kubernetes cluster and `kubectl` configured for it
- [Helm](https://helm.sh/docs/intro/install/) 3
- A clone of the repository: `git clone https://github.com/sbettid/pinglow.git && cd pinglow`

## 1. Install Pinglow

```bash
kubectl create namespace pinglow

helm install pinglow ./charts/pinglow \
  --namespace pinglow \
  --set timescaledb.enabled=true
```

The release name must be `pinglow`. Wait until the controller, runner, Redis and TimescaleDB pods are running:

```bash
kubectl get pods -n pinglow
```

## 2. Create a script and a check

A `Check` runs a `Script` every `interval` seconds, passing the keys of the referenced Secrets as environment variables.

```yaml
# first-check.yaml
apiVersion: v1
kind: Secret
metadata:
  name: my-service-definition
  namespace: pinglow
stringData:
  URL: https://example.com
---
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
    import os
    import sys
    import requests

    response = requests.get(os.environ["URL"], timeout=5)

    if response.status_code != 200:
      print("Error in contacting endpoint")
      sys.exit(2)

    print("Endpoint reachable")
---
apiVersion: pinglow.io/v1alpha1
kind: Check
metadata:
  name: my-service-reachability
  namespace: pinglow
spec:
  scriptRef: check-service
  interval: 60
  secretRefs:
    - my-service-definition
```

```bash
kubectl apply -f first-check.yaml
```

Pinglow picks up the new resources automatically, with no restart needed.

## 3. Create an API key

```yaml
# api-key.yaml
apiVersion: pinglow.io/v1alpha1
kind: ApiKeyBinding
metadata:
  name: quick-start
  namespace: pinglow
spec:
  role: viewer
```

```bash
kubectl apply -f api-key.yaml

export API_KEY=$(kubectl get secret quick-start-api-key -n pinglow \
  -o jsonpath='{.data.API_KEY}' | base64 -d)
```

## 4. Read the result

```bash
kubectl port-forward -n pinglow svc/pinglow 8000:80
```

In another terminal:

```bash
curl -H "x-api-key: $API_KEY" http://localhost:8000/checks
```

After the first interval has passed you should see `my-service-reachability` with its status. See the [REST API reference](/docs/restapi) for the other endpoints.

## Next steps

- [Notifications](/docs/concepts/notifications): get a Telegram message when a check fails
- [OIDC authentication](/docs/deployment/oidc): log in from the browser
- [Helm values reference](/docs/deployment/values): tune the runners, autoscaling and database
