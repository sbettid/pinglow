---
sidebar_position: 4
---

# OIDC authentication

Pinglow can authenticate browser users through any OpenID Connect (OIDC) provider (Keycloak, Authentik, Dex, Entra ID, Google, ...). OIDC is **optional**: if you only need API access, skip this page and use [API keys](/docs/concepts/automation-credentials) instead.

OIDC handles *who the user is*. *What the user may do* is decided by [`PinglowUserBinding`](/docs/concepts/user-bindings) resources, which map an OIDC identity to a [Pinglow role](/docs/concepts/roles).

## How the login flow works

1. The browser opens `/auth/login`. Pinglow reads the provider's discovery document (`<issuer>/.well-known/openid-configuration`) and redirects the user to the provider, requesting the scopes `openid profile email`.
2. After login, the provider redirects the browser back to `OIDC_REDIRECT_URL` (`/auth/callback`).
3. Pinglow exchanges the code for an ID token and validates its signature, issuer, audience (the client ID) and nonce.
4. Pinglow looks for a `PinglowUserBinding` matching the token's `sub` claim first, then its `email` claim.
   - No match: login is rejected with `403 Forbidden`.
   - Match: a session is stored in Redis and the browser receives a `pinglow_session` cookie (`HttpOnly`, `SameSite=Lax`, `Secure` by default).

Other endpoints: `GET /auth/me` returns the current user and role, `POST /auth/logout` ends the session.

## Prerequisites

- An OIDC provider and a **confidential client** (one that has a client secret) registered for Pinglow.
- The client's **redirect URI** set to exactly `https://<your-pinglow-host>/auth/callback`.
- The provider must put `sub` in the ID token, and `email` if you want to bind users by email.
- Pinglow served over HTTPS (see [Local testing over HTTP](#local-testing-over-http) otherwise).

## 1. Create the OIDC Secret

The Helm chart reads the OIDC settings from a Secret, named `pinglow-oidc` by default, in the same namespace as the release.

| Key | Required | Description |
| --- | --- | --- |
| `OIDC_ISSUER_URL` | yes | Issuer URL of the provider. Must match the `iss` claim of the ID token exactly. |
| `OIDC_CLIENT_ID` | yes | Client ID registered at the provider. |
| `OIDC_CLIENT_SECRET` | yes | Client secret. |
| `OIDC_REDIRECT_URL` | yes | Public callback URL, `https://<host>/auth/callback`. Must match the provider's allowed redirect URI. |
| `OIDC_COOKIE_SECURE` | no | Set to `"false"` to allow the session cookie over plain HTTP. Defaults to secure. |

```yaml
apiVersion: v1
kind: Secret
metadata:
  name: pinglow-oidc
  namespace: pinglow
type: Opaque
stringData:
  OIDC_ISSUER_URL: https://auth.example.com/realms/pinglow
  OIDC_CLIENT_ID: pinglow
  OIDC_CLIENT_SECRET: change-me
  OIDC_REDIRECT_URL: https://pinglow.example.com/auth/callback
```

Or from the command line:

```bash
kubectl create secret generic pinglow-oidc -n pinglow \
  --from-literal=OIDC_ISSUER_URL=https://auth.example.com/realms/pinglow \
  --from-literal=OIDC_CLIENT_ID=pinglow \
  --from-literal=OIDC_CLIENT_SECRET=change-me \
  --from-literal=OIDC_REDIRECT_URL=https://pinglow.example.com/auth/callback
```

:::warning
The four required keys are all-or-nothing. If only some of them are set, Pinglow refuses to start. If none are set (or the Secret does not exist, since it is referenced as optional), OIDC is disabled and the `/auth/*` endpoints return `404`.
:::

## 2. Reference the Secret in the Helm values

```yaml
pinglow:
  OidcEnvFromSecret: "pinglow-oidc"   # the default, change it if you named the Secret differently
```

If your Secret has a different name, set it here. Install or upgrade the chart as usual; the Secret is injected into the controller as environment variables.

## 3. Create user bindings

Without a binding nobody can log in. Create at least one admin:

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

See [OIDC user bindings](/docs/concepts/user-bindings) for all fields and examples.

## Provider with a private or self-signed CA

If the provider's certificate is signed by an internal CA, Pinglow must trust it for discovery, token and JWKS calls.

Create a ConfigMap whose key is **`ca.crt`** (PEM format, the key name is fixed):

```yaml
apiVersion: v1
kind: ConfigMap
metadata:
  name: pinglow-oidc-ca
  namespace: pinglow
data:
  ca.crt: |
    -----BEGIN CERTIFICATE-----
    MIIDdzCCAl+gAwIBAgIUb...
    -----END CERTIFICATE-----
```

Or from a file:

```bash
kubectl create configmap pinglow-oidc-ca -n pinglow --from-file=ca.crt=./internal-ca.pem
```

Then reference it in the values:

```yaml
pinglow:
  extraCAConfigMap: "pinglow-oidc-ca"
```

The chart mounts the ConfigMap at `/custom-ca` and sets `OIDC_EXTRA_CA_CERT_PATH=/custom-ca/ca.crt` for you. The certificate is added on top of the system trust store.

## Complete example

```yaml
# values.yaml
pinglow:
  OidcEnvFromSecret: "pinglow-oidc"
  extraCAConfigMap: "pinglow-oidc-ca"   # only if you need a custom CA
```

## Local testing over HTTP

Browsers do not send `Secure` cookies over plain HTTP, so login would silently fail on e.g. `http://localhost:8000`. For development only, add this key to the OIDC Secret:

```yaml
stringData:
  OIDC_COOKIE_SECURE: "false"
```

Do not use this in production.

## Troubleshooting

| Symptom | Likely cause |
| --- | --- |
| Pinglow pod crashes on startup mentioning `OIDC configuration requires ...` | Only some of the four required keys are present in the Secret. |
| `/auth/login` returns `404` | OIDC is disabled: the Secret is missing or empty. |
| Provider says "invalid redirect URI" | `OIDC_REDIRECT_URL` and the URI registered at the provider differ (scheme, host, path, trailing slash). |
| Callback returns `401 Unauthorized` | Token validation failed: wrong `OIDC_ISSUER_URL` (must equal the `iss` claim), wrong client ID, expired login attempt, or the login cookie was dropped. |
| Callback returns `403 Forbidden` | Login succeeded but no `PinglowUserBinding` matches the user's `sub` or `email`. |
| Login loops back to the login page over HTTP | The `Secure` cookie is being dropped, use HTTPS or `OIDC_COOKIE_SECURE=false` for testing. |
| TLS errors reaching the provider | Provider uses a private CA, see [above](#provider-with-a-private-or-self-signed-ca). |
