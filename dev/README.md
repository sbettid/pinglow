## Local development with Kind

Run the controller on your machine against a dedicated Kind cluster:

```bash
./dev/kind-controller.sh dev@example.com
```

The helper creates the `pinglow-dev` Kind cluster if needed, or reuses it if
healthy. Because this cluster is disposable, it deletes and recreates it if
the control plane is unavailable or its node does not become ready. The helper
deploys Redis, TimescaleDB, and Keycloak in the cluster, then installs Pinglow's
CRDs, creates the `pinglow` namespace and an email-based viewer binding plus a
group-based admin binding, applies a sample heartbeat check, and starts the
controller and runner locally. Redis and TimescaleDB are rendered from the
Pinglow Helm chart and applied to Kind; only their templates are selected, so
the chart's controller and runner remain mandatory and are not deployed by
this workflow. Dependency image versions therefore stay defined in one place.
Keycloak is deployed from the local development manifests. The helper uses
ephemeral storage for Redis and TimescaleDB, so their data is lost if those
pods are recreated. If upgrading from a previous setup that used persistent
volumes, the helper recreates those StatefulSets to switch to ephemeral storage
and leaves their existing PVCs untouched. It port-forwards the dependencies to
localhost ports 6379, 5432, and 8081 while they are needed, and stops those
forwards when either Pinglow process exits. Keycloak imports the local realm,
where user `dev` belongs to the `pinglow-admins` group and the `pinglow` client
includes groups in the ID token. This exercises group mapping and verifies
that the admin group binding takes precedence over the email viewer binding.
The heartbeat runs every minute without external dependencies, reports the
current UTC time, and records hour/minute metrics for the performance chart.
The helper uses a temporary kubeconfig for this cluster and does not change the
current kubectl context. The runner executes scripts locally, so Python 3 with
the `venv` module must be installed. The user email must match the email of a
Keycloak user.

In a second terminal, run the frontend in Vite development mode:

```bash
cd ../pinglow-frontend/client
npm run dev
```

Open `http://localhost:5173` and sign in using Keycloak user `dev` / `dev`.
The local Keycloak client uses issuer `http://localhost:8081/realms/pinglow`
and redirect URI `http://localhost:5173/api/auth/callback`. The local
development dependency credentials are `pinglow` / `pinglow` for the database
and Redis, and `admin` / `admin` for the Keycloak admin account. These
credentials are for local development only.