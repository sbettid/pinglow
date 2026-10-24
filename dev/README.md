## Local development with Kind

Start local Redis, TimescaleDB, and Keycloak; Compose imports a local Keycloak
realm and client on first startup:

```bash
docker compose up -d
docker compose run --rm keycloak-setup
```

The setup command ensures the `pinglow-admins` group exists, assigns the
Keycloak `dev` user to it, and ensures the `pinglow` client includes groups in
the ID token. It is safe to run repeatedly, including when the realm existed
before group-based login was configured.

Then run the controller on your machine against a dedicated Kind cluster:

```bash
./dev/kind-controller.sh dev@example.com
```

The helper creates the `pinglow-dev` Kind cluster if needed, or reuses it if
healthy. Because this cluster is disposable, it deletes and recreates it if
the control plane is unavailable or its node does not become ready. It then
installs Pinglow's CRDs, creates the `pinglow` namespace and an email-based
viewer binding plus a group-based admin binding, applies a sample heartbeat
check, and starts the controller locally. The Keycloak `dev` user belongs to the
`pinglow-admins` group, which Keycloak adds to the ID token as the `groups`
claim. This exercises group mapping and verifies that the admin group binding
takes precedence over the email viewer binding. The
heartbeat runs every minute without external dependencies, reports the current
UTC time, and records hour/minute metrics for the performance chart. It uses a
temporary kubeconfig for this cluster and does not change the current kubectl
context. The helper starts both the controller and runner, and stops the other
process if either one exits. The runner executes scripts locally, so Python 3
with the `venv` module must be installed. The user email must match the email
of a Keycloak user.

In a second terminal, run the frontend in Vite development mode:

```bash
cd ../pinglow-frontend/client
npm run dev
```

Open `http://localhost:5173` and sign in using Keycloak user `dev` / `dev`.
The local Keycloak client uses issuer `http://localhost:8081/realms/pinglow`
and redirect URI `http://localhost:5173/api/auth/callback`. Set `DB_*` or
`REDIS_*` environment variables before running the helper if your local service
credentials differ from the defaults.

The Keycloak import directory is mounted with Podman's `:Z` SELinux relabel
option so the container can read the realm file. Keep this setting for Podman;
Docker Compose users can remove `,Z` from that volume declaration if Docker
reports that the option is unsupported. These credentials are for local
development only.