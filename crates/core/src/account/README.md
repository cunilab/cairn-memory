# account

## What
People and their credentials: accounts, password hashing and token hashing, devices (the agent
credential) and audit events. Tokens are random and stored only as hashes.

## Entry points
- `Account`, `AccountStatus`, `normalize_email`, `validate_password`.
- `hash_password`, `verify_password`, `random_token`, `hash_token`.
- `Device`, `DeviceStatus`, `AuditEvent`.

## Depends on
Nothing in `cairn-core` except `error`.
