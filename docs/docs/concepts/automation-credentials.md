---
title: Automation credentials
---

Automation clients authenticate with Pinglow using API keys generated from `ApiKeyBinding` resources. These are separate from browser-based OIDC authentication.

## ApiKeyBinding resource

Create an API key binding to generate a role-scoped credential for automation:

```yaml
apiVersion: pinglow.io/v1alpha1
kind: ApiKeyBinding
metadata:
  name: checks-runner
  namespace: pinglow
spec:
  role: operator
  secret_name: checks-runner-api-key
```

Pinglow will generate a Kubernetes Secret in the same namespace containing the `data.API_KEY` field.

## Retrieving the API key

Read the generated Secret to obtain the API key:

```bash
kubectl get secret checks-runner-api-key \
  -n pinglow \
  -o jsonpath='{.data.API_KEY}' | base64 -d
```

## Using the API key

Send the key as the `x-api-key` header on all requests:

```bash
curl -H "x-api-key: <your-api-key>" \
  http://pinglow:8000/checks
```

## Fields

- **role** (required): One of `viewer`, `operator`, or `admin`.
- **secret_name** (optional): The name of the Secret to manage. Defaults to `<binding-name>-api-key`.

## Lifecycle

- **Creation**: Pinglow creates the Secret with the API key in `data.API_KEY`.
- **Update**: Modifying the binding revokes the previous cached key; a new key is generated.
- **Deletion**: The finalizer ensures the Secret is deleted when the binding is deleted.

## Role permissions

Same as user bindings:

- **viewer**: Read-only access to checks and performance data.
- **operator**: View checks and perform operations (mute notifications, schedule checks, submit results).
- **admin**: Full access including operator capabilities.

## Secret ownership

The generated Secret is owned by the binding as its controller. Deleting the binding will trigger Secret deletion.

## Examples

**Runner with operator access:**

```yaml
apiVersion: pinglow.io/v1alpha1
kind: ApiKeyBinding
metadata:
  name: ci-runner
  namespace: pinglow
spec:
  role: operator
```

This creates a Secret named `ci-runner-api-key` in the `pinglow` namespace.

**Monitoring service with viewer access:**

```yaml
apiVersion: pinglow.io/v1alpha1
kind: ApiKeyBinding
metadata:
  name: monitoring-svc
  namespace: pinglow
spec:
  role: viewer
  secret_name: monitoring-svc-credentials
```

This creates a Secret named `monitoring-svc-credentials` with read-only access.
