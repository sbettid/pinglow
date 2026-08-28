---
title: User bindings
---

User bindings connect OIDC identities to Pinglow roles, enabling browser-based authentication with declarative role assignment.

## PinglowUserBinding resource

Create a binding to map a user's email or subject to a specific role:

```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: alice-admin
  namespace: pinglow
spec:
  email: alice@example.com
  role: admin
```

## Fields

- **email** (optional): The user's email address from the OIDC provider (typically the email claim).
- **subject** (optional): The user's subject identifier from the OIDC provider (the `sub` claim).
- **role** (required): One of `viewer`, `operator`, or `admin`.

At least one of `email` or `subject` must be specified.

## Role permissions

- **viewer**: Read-only access to checks and performance data.
- **operator**: View checks and perform operations (mute notifications, schedule checks, submit results).
- **admin**: Full access including operator capabilities.

## OIDC provider integration

The binding's `email` and `subject` fields are matched against claims in the OIDC ID token returned during browser login. Pinglow checks both fields in order and grants the role of the first match.

If no binding matches the authenticated user, the login is rejected with a `403 Forbidden` response.

## Examples

**Admin user by email:**
```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: admin-user
  namespace: pinglow
spec:
  email: admin@company.com
  role: admin
```

**Operator user by subject:**
```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: operator-user
  namespace: pinglow
spec:
  subject: "user-id-123"
  role: operator
```

**Viewer user:**
```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: viewer-user
  namespace: pinglow
spec:
  email: viewer@company.com
  role: viewer
```
