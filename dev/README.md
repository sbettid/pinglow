## Local development with Kind

Start local Redis, TimescaleDB, and Keycloak; Compose imports a local Keycloak
realm and client on first startup:

```bash
docker compose up -d
```

Then run the controller on your machine against a dedicated Kind cluster:

```bash
./dev/kind-controller.sh dev@example.com
```

The helper creates or reuses the `pinglow-dev` Kind cluster, installs
Pinglow's CRDs, creates the `pinglow` namespace and an admin login binding,
then starts the controller locally. It uses a temporary kubeconfig for this
cluster and does not change the current kubectl context. The user email must
match the email of a Keycloak user.

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

Keycloak imports the sample realm only if it does not already exist. If you
created the `pinglow` realm before enabling Compose import, recreate the local
Keycloak container/database or add the client and user to that realm manually.
The Keycloak import directory is mounted with Podman's `:Z` SELinux relabel
option so the container can read the realm file. Keep this setting for Podman;
Docker Compose users can remove `,Z` from that volume declaration if Docker
reports that the option is unsupported. These credentials are for local
development only.