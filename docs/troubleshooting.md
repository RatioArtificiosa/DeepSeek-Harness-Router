# Troubleshooting

Every failure the launcher can detect, what it means, and how to fix it.

---

## Docker is not installed

```
✗ Docker is not installed.
```

Install Docker Desktop for Windows or macOS, or Docker Engine plus the Compose
plugin for Linux: <https://docs.docker.com/get-docker/>

---

## The Docker daemon is not running

```
✗ Docker is installed, but the daemon is not running.
```

- **Windows / macOS** — start Docker Desktop and wait for the whale icon to stop
  animating.
- **Linux** — `sudo systemctl start docker`

---

## Docker Compose v2 is missing

```
✗ Docker Compose v2 is required.
```

Confirm with `docker compose version`. If only `docker-compose` (v1) exists,
install the Compose plugin.

---

## The port is already in use

```
! Port 3080 is in use by <process>. Using port 3081 instead.
```

This is normal and handled automatically. To pin a specific port, use
`./start.sh --port 4000`.

---

## The workspace path is rejected

```
✗ / is a filesystem root.
```

The launcher mounts **one project directory**, never a whole drive or filesystem
root. Common rejections:

| Path | Why |
|---|---|
| `/` or `C:\` | A filesystem root |
| `/etc`, `/usr`, `C:\Windows` | System directories |
| `~` | Your home directory itself |
| `C:foo` | Drive-relative — not the same as `C:\foo` |

Choose a folder inside one of these, such as `~/projects/my-app`.

---

## The UI returns "access refused"

The hostname in your browser is not in the allowed list.

This happens when the UI is published on all interfaces but the hostname you
used is not declared. The refusal page names the exact hostname it saw and prints
the line to add:

```env
TRUSTED_HOSTS=192.168.1.50
```

Then restart with `docker compose up -d`.

**Better option:** use an SSH tunnel instead. It needs no trust relaxation.

---

## The application does not become healthy

```
✗ The application did not become healthy within 300 seconds.
```

The launcher prints the last log lines. For more:

```sh
docker compose -p deepseek-harness-router logs --tail=200
./start.sh --doctor
```

Common causes:

| Symptom | Likely cause |
|---|---|
| No model configured | Expected on first run — the UI still opens; add a key in Settings |
| Out of memory | The harness image needs roughly 4 GB available |
| A slow first start | The initial image build takes several minutes |

---

## Confinement is reported as unavailable

```
⚠ Sandbox  unavailable on this host
```

Verification of the kernel's confinement facility failed. **Confined operations
are disabled rather than silently run unconfined.**

This is a deliberate safety behaviour. The interface still works; read-only
operations still work; commands requiring confinement are refused.

Full context in [`security.md`](security.md#if-confinement-is-unavailable).

---

## Files in my project are owned by root

On **Linux**, check that `UID` and `GID` in `.env` match your user. On **macOS
and Windows**, this is a property of Docker Desktop's file-sharing layer — see
[`permissions.md`](permissions.md).

---

## It worked, then stopped after a reboot

The container is started with `restart: unless-stopped`, but Docker itself must
be running. Start Docker Desktop, then:

```sh
docker compose -p deepseek-harness-router up -d
```

---

## Starting over

```sh
./stop.sh                     # stop; your sessions are kept
./reset.sh                    # wipe sessions and settings (asks twice)
```

`reset.sh` removes the data volume and requires two confirmations. It never
touches your project directory — that was never ours to remove.

---

## Getting help

Run `./start.sh --doctor` and include its output. It reports versions, detected
platform, workspace status, port availability, and the confinement posture —
without changing anything.
