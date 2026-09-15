# Permissions and file ownership

Why files sometimes look like they belong to someone else, and what this project
does about it.

---

## The short version

| Platform | Host-side ownership of container-written files |
|---|---|
| **Linux** | Matches the UID/GID the launcher selected — normally yours |
| **macOS** | Synthesised by Docker Desktop's file-sharing layer |
| **Windows** | Synthesised by Docker Desktop's VM file sharing |

On macOS and Windows, this is **not governed by the container's UID/GID**. It is
controlled by Docker Desktop, and Docker documents it poorly — the older `osxfs`
documentation page has been removed entirely.

**We do not promise behaviour we cannot control.** Instead, we are designed so
that ownership rarely matters.

---

## The design decision that makes this mostly moot

**All generated state goes to the volume at `/data`. Never into your project.**

| Written by | Location |
|---|---|
| Session logs, settings, credentials | `/data` |
| Caches, build artifacts, logs | `/data` |
| **Your project files** | `/workspace` — only when the agent edits them |

The consequence: the only files in your project directory whose ownership could
surprise you are ones the agent **intentionally** created or modified. That is
the tool doing its job.

Generated caches and intermediate artifacts never appear in your `git status`,
and never arrive owned by `root`.

---

## Linux: aligned automatically

On Linux the launcher detects your user and group and writes them into the
configuration, so the container runs as you rather than as `root`. Files it
creates in the workspace are owned by you, immediately.

You do not need to configure this. It is detected.

If you want to override it — for a shared machine or a build server — set
`UID` and `GID` in `.env` before starting.

---

## macOS and Windows: what to expect

Docker Desktop presents your mounted directory through a virtualisation layer
that assigns ownership itself. Two things follow:

1. Setting container UID/GID has little or no effect on what your host sees.
   The launcher still sets them, harmlessly, for consistency.
2. The reported owner may not match what you would expect from `chown`. This is a
   property of the file-sharing implementation, not of this project.

If you see unexpected ownership on these platforms, that is the layer to
investigate — we cannot change it from inside the container.

---

## Why we do not use the PUID/PGID pattern

A popular approach in the container ecosystem is a root entrypoint that rewrites
ownership of the mounted directory and then drops to a target user. We do not do
this, deliberately:

| Reason | Detail |
|---|---|
| It requires a root entrypoint | Anything exploiting the process before the privilege drop is root |
| It conflicts with `user:` | And with a read-only root filesystem |
| It re-writes ownership at every start | Slow on a large project directory |
| It mangles ownership on collisions | Silently, in ways that are hard to diagnose |
| **It does nothing on macOS and Windows** | Which is where people most often need it |

The alternative we use — run unprivileged, keep state in a volume — is simpler,
portable, and honest about its limits.

---

## If files in your project are owned by root (Linux)

This should not happen with the default configuration. If it does:

1. Run `./start.sh --doctor` and check the reported UID/GID.
2. Confirm `UID` and `GID` in `.env` match `id -u` and `id -g` on your host.
3. Check that your Docker daemon is not running in a mode that forces a
   different user namespace.

On macOS and Windows, files showing an unexpected owner is expected behaviour of
Docker Desktop's sharing layer, not a misconfiguration.

---

## Backing up and restoring

Ownership is simpler to reason about if you back up the volume:

```sh
docker run --rm \
  -v deepseek-harness-router_agent-data:/data:ro \
  -v "$PWD:/backup" \
  alpine tar czf /backup/router-backup.tgz -C /data .
```

Your project directory is not part of this backup. It is your code, and your
existing version control already covers it — which is the point of mounting a
real directory rather than copying files into a container.
