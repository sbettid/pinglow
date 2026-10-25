---
sidebar_position: 6
---

# Pinglow roles

Pinglow ships with the following built-in roles:

- **viewer**: Read-only access to checks and performance data. No modifications are possible with this role.
- **operator**: View checks and perform operations (mute notifications, schedule checks, submit results).
- **admin**: Full access including operator capabilities.

`PinglowUserBinding` and `ApiKeyBinding` can optionally restrict a role to an inclusive list of check groups. This scope limits which checks the identity can access without changing the capabilities granted by its role. Admins are not restricted by check-group scopes.
