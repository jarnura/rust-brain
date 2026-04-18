# Workspace Volumes

This document describes how the rust-brain API manages Docker volumes for
per-workspace storage.

## Naming Convention

Every workspace gets exactly one Docker volume named:

```
rustbrain-ws-{workspace_id_short}
```

where `workspace_id_short` is the **first 8 characters** of the workspace UUID
after stripping hyphens.

| Workspace UUID | Volume name |
|---|---|
| `abc12345-0000-0000-0000-000000000000` | `rustbrain-ws-abc12345` |
| `f7e3b091-4a2c-4d5e-8f1a-9b3c2d4e5f6a` | `rustbrain-ws-f7e3b091` |

## Labels

All workspace volumes carry the label `rustbrain.workspace=true`. Use this to
list or clean up all workspace volumes without touching other Docker volumes:

```bash
# List all workspace volumes
docker volume ls --filter label=rustbrain.workspace=true

# Remove all workspace volumes (caution: irreversible)
docker volume rm $(docker volume ls -q --filter label=rustbrain.workspace=true)
```

## Quota Enforcement

The API passes `--opt size=2g` when creating volumes. This flag is honoured by:

- **`tmpfs` volumes** — the Linux kernel enforces the size limit at mount time.
- **Standard `local` driver on `ext4`** — the flag is accepted but **not
  enforced** by Docker. Disk quotas require filesystem-level project quotas
  (see `man quota`). For the MVP this is documented-only; enforcement is left
  to the host operator.

The default quota is **2 GB** per workspace. This can be adjusted per-call
in `DockerClient::create_volume(name, size_gb)`.

## Volume Lifecycle

```
workspace created  →  create_volume()  →  volume exists
workspace deleted  →  remove_volume()  →  volume gone
```

Status transitions tracked in Postgres `workspaces.volume_name` column.

## Implementation

Volumes are managed by `DockerClient` in `services/api/src/docker.rs`. The
client shells out to the `docker` binary rather than using the Docker daemon
API, keeping the Rust build dependency-free of Docker SDK crates.

The API container mounts the host Docker socket at `/var/run/docker.sock`
(configured in `docker-compose.yml`). Operators may override the daemon target
via the `DOCKER_HOST` environment variable, which the Docker CLI honours
automatically.

## Troubleshooting

### Volume not found after workspace creation

Check API logs for errors from `DockerClient::create_volume`. Ensure the
Docker socket is mounted:

```bash
docker exec rustbrain-api ls -la /var/run/docker.sock
```

### Remove a specific workspace volume

```bash
docker volume rm rustbrain-ws-<workspace_id_short>
```

### Prune all workspace volumes (dangerous)

```bash
docker volume rm $(docker volume ls -q --filter label=rustbrain.workspace=true)
```

This is irreversible. All workspace data will be lost.
