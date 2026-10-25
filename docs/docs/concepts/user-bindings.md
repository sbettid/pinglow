---
sidebar_position: 5
---

# OIDC authentication and user roles

To enable OIDC authentication for browser logins, first configure the provider, Secret and Helm values as described in [OIDC authentication](/docs/deployment/oidc).

User bindings connect OIDC identities to Pinglow roles, enabling browser-based authentication with declarative role assignment.

For more information about the specific roles please see the [pinglow roles](/docs/concepts/roles) section.

## PinglowUserBinding resource

Create a binding to map a user's email, subject, or one or more OIDC groups to a specific role:

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

To assign a role to everyone in an OIDC group, use `groups` instead:

```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: platform-admins
  namespace: pinglow
spec:
  groups:
    - platform-admins
  role: admin
```

## Fields

- **email** (optional): The user's email address from the OIDC provider (typically the email claim).
- **subject** (optional): The user's subject identifier from the OIDC provider (the `sub` claim).
- **groups** (optional): OIDC group names that receive this role. The provider must include these group names in the configured ID-token groups claim.
- **check_groups** (optional): Allowlist of `CheckGroup` resource names this binding can access. When omitted, the binding has access to all checks. When set, checks without a group and checks in unlisted groups are not accessible.
- **role** (required): One of `viewer`, `operator`, or `admin`.

At least one of `email`, `subject`, or `groups` must be specified.

## OIDC provider integration

The binding's `email` and `subject` fields are matched against claims in the OIDC ID token returned during browser login. Group bindings are matched against the ID-token claim configured by `OIDC_GROUPS_CLAIM` (default: `groups`). Pinglow grants the most privileged role among all matching subject, email, and group bindings (`admin` > `operator` > `viewer`).

If no binding matches the authenticated user, the login is rejected with a `403 Forbidden` response.

When several bindings match, Pinglow selects the highest role and combines the group allowlists from bindings with that role. If any such binding omits `check_groups`, access is unrestricted. Admins always have access to all groups.

After deploying group-scoped permissions, users with existing browser sessions must sign in again.

**Viewer limited to selected groups:**
```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: home-viewer
  namespace: pinglow
spec:
  email: viewer@example.com
  role: viewer
  check_groups:
    - home-network
    - home-services
```

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

**Viewer group:**
```yaml
apiVersion: pinglow.io/v1alpha1
kind: PinglowUserBinding
metadata:
  name: employees-viewer
  namespace: pinglow
spec:
  groups:
    - employees
  role: viewer
```
