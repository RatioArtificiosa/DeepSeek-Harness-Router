# Cross-Platform Docker Packaging — Technical Brief (2026)

**Compiled:** September 2026.
**Method:** `web_search` was unavailable in this environment, so every claim comes from either (a) a URL fetched directly with `web_fetch` / `Invoke-WebRequest`, or (b) a **live experiment run on this machine** against Docker Desktop 29.6.1 / Engine 29.6.1 / Compose v5.2.0 / Buildx v0.35.0-desktop.2 on Windows 11 with the WSL2 backend (kernel `6.18.33.2-microsoft-standard-WSL2`). Live results are marked **[TESTED]**; they are reproduced with exact commands. Platform-dependent claims are labelled as such.

**Note on "Compose v2/v5":** Compose v1 is long gone and no longer relevant. The current line is **Compose v5** — v5.0.0 shipped 2025-12-02, latest **v5.5.1 on 2026-09-03**. Docker Container Engine is at **v29.8.0 (2026-09-03)**; Docker Desktop is at **4.91.0 (2026-09-14)**.

---

## 1. Compose `.env` and variable interpolation edge cases

Interpolation happens **after** `.env` parsing and **before** YAML merge, on a per-file basis, and applies only to YAML *values*, not keys. The killer finding is that the hard part is not Compose's interpolation of `WORKSPACE_PATH` at all — a spaced or drive-lettered value survives interpolation intact (verified). The hard part is **YAML scalar quoting of the result**, because a Windows path is full of backslashes and `\U`/`\T`/`\p` are illegal escapes inside a double-quoted YAML scalar, and a `C:` drive letter looks exactly like the `VOLUME:CONTAINER_PATH` separator in short syntax. Two separate failure modes, both reproducible. `COMPOSE_CONVERT_WINDOWS_PATHS` still exists in Compose v2+ but **defaults to `0`** (off), and Docker now explicitly tells you *not* to set it when using Synchronized file shares. The safest cross-platform form is **long syntax with a single-quoted `source:`**, which removes the colon-separator ambiguity entirely.

**[TESTED]** Interpolation of a spaced host path works — `.env` containing `WORKSPACE_PATH=C:\Users\...\my project`, referenced as `${WORKSPACE_PATH}:/ws` in the short form, resolved correctly and the bind mount read the file:

```
volumes:
  - type: bind
    source: C:\Users\Usuario\AppData\Local\Temp\dshpath\my project
    target: /ws
```
→ `docker compose run --rm t` printed `HELLO_SPACE_PATH`. Exit 0.

**[TESTED]** Double-quoted Windows path = hard parse failure. This YAML:
```yaml
volumes:
  - "C:\Users\Usuario\Temp\p:/ws"
```
fails with `failed to parse compose.yaml: yaml: while scanning a quoted scalar at line 5, column 14: did not find expected hexadecimal number`. The `\U` is read as the start of a `\uXXXX`-style escape. Compose never even reaches the path logic.

**[TESTED]** Single-quoted forward-slash path works, spaces and all:
```yaml
volumes:
  - 'C:/Users/Usuario/AppData/Local/Temp/dshpath2/proj space:/ws'
```
→ parsed to `source: C:/Users/.../proj space`, `target: /ws`; run succeeded, printed `BS_TEST`, exit 0. Forward slashes are accepted by the Windows engine and sidestep backslash escaping.

**[TESTED]** Unquoted short syntax with spaces also worked (Compose split on the *last* colon correctly here), but this is the fragile case: it depends on Compose's separator heuristic and will break on paths containing `:` beyond the drive letter. **Do not rely on it.**

**[TESTED]** Long syntax is unambiguous and preferable — the source is its own value, so no `:`/space splitting occurs at all:
```yaml
volumes:
  - type: bind
    source: 'C:/Users/.../proj space'
    target: /ws
    read_only: true
```
→ parsed identically with or without quotes; run succeeded, exit 0.

**`COMPOSE_CONVERT_WINDOWS_PATHS`:** still documented in Compose V2+ under pre-defined variables, "Supported values: `true`/`1` to enable, `false`/`0` to disable", **"Defaults to: `0`"**. It is *not* in the "Unsupported in Compose V2" list (that list is `COMPOSE_API_VERSION`, `COMPOSE_HTTP_TIMEOUT`, `COMPOSE_TLS_VERSION`, `COMPOSE_FORCE_WINDOWS_HOST`, `COMPOSE_INTERACTIVE_NO_CLI`, `COMPOSE_DOCKER_CLI_BUILD`). However, Docker's Synchronized file shares doc states a **known issue**: *"POSIX-style Windows paths are not supported. Avoid setting the `COMPOSE_CONVERT_WINDOWS_PATHS` environment variable in Docker Compose."* So: still functional, default-off, and actively discouraged.

**`--env-file`:** yes, still the norm and the documented mechanism; `COMPOSE_ENV_FILES` (comma-separated, used only when `--env-file` is absent) and `COMPOSE_DISABLE_ENV_FILE` are the env-var equivalents. Precedence is **shell env > `--env-file` > project-directory `.env`**; multiple `--env-file` flags are read in order, later overriding earlier.

### Recommendations
- Use **long syntax** (`type: bind` / `source:` / `target:` / `read_only:`) for every bind mount. It is the only form immune to colon and space ambiguity, and it is the only form that lets you set `create_host_path: false`.
- **Always single-quote** `source:`. Never double-quote a path containing backslashes.
- Normalize host paths to **forward slashes** before injecting them; keep the drive letter (`C:/...`).
- Set `create_host_path: false` where a missing source must be an error rather than a silently-created empty directory (short syntax always auto-creates the source dir).
- Keep `WORKSPACE_PATH` in `.env` (git-ignored) or pass `--env-file`; do not bake absolute host paths into the committed compose file.
- Leave `COMPOSE_CONVERT_WINDOWS_PATHS` unset. Do not build a design that depends on it.
- Use `${VAR:?error}` for required paths so a missing variable fails loudly instead of silently becoming an empty string (Compose only warns and substitutes empty otherwise).

Sources:
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/interpolation.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/environment-variables/variable-interpolation.md
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/services.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/environment-variables/envvars.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/features/synchronized-file-sharing.md
- Live tests on Docker Compose v5.2.0 / Engine 29.6.1 (commands and output above).

---

## 2. Docker Desktop file sharing, performance, and UID/GID

The single most important correction to common belief: **there is no stable, documented cross-platform UID/GID contract for bind mounts.** On Linux native the kernel enforces ownership literally and predictably. On Docker Desktop for Windows and Mac the bind mount is a network-style share into a VM, and ownership is synthesized by the sharing layer — which differs between gRPC-FUSE, VirtioFS, Docker VMM, and Synchronized File Shares. Docker documents **none** of the macOS mapping (the old `osxfs` page has been deleted), and the behavior has been actively regressing into 2026. So the correct posture is: *do not design anything that depends on the host-observed owner of a container-written file*; instead make the writing process run as the intended UID and verify per-engine in CI. Docker Desktop for Mac currently defaults to **VirtioFS** (gRPC-FUSE still selectable), and Docker added a third option, **Docker VMM** (Beta), which is Docker's own hypervisor since 4.86.

**[TESTED]** Inside the container, ownership is exactly as asked — `-u`/`user:` and per-process UIDs are honored:
```
total 0
-rw-r--r-- 1 1000 1000  f1000.txt
-rw-r--r-- 1 1234 1234  f1234.txt
-rw-r--r-- 1    0    0  froot.txt
```
`--user 1000:1000` → `id` reported `uid=1000 gid=1000`, file created as `1000 1000`. So **the container-side contract is reliable**; the host-side view is the unreliable part.

**[TESTED]** On this Windows/WSL2 host the host-visible ownership is not meaningful in the way a Linux-native host would be — files land in the host directory and are readable, but there is no host-visible numeric uid/gid to assert on. This is the expected consequence of the shared-folder layer.

**Platform state (2026):**
- **Windows:** WSL2 is the **default** backend and needs no admin. **Hyper-V is still supported** (all-users install mode only, needs admin) — it is *not* deprecated or removed. **Docker VMM (Beta)** is now available on Windows too and is marketed as a real-VM boundary alternative to WSL2. On the WSL2 backend the kernel is **Microsoft's WSL kernel**, not Docker's — verified locally: `6.18.33.2-microsoft-standard-WSL2`. Docker Desktop's own Linux VM reported **kernel v7.0.12 as of Desktop 4.87.0**, up from the long-lived `6.12.x` line (`v6.12.72` at 4.63.0). *Caveat: this 7.0.x claim comes only from the release notes; linuxkit upstream carries no 7.0.x series (its newest is 6.12.x, pinned `KERNEL_VERSION=6.12.59`), so the Docker VM kernel has evidently forked from public linuxkit. Treat the exact number as documentation-only.*
- **macOS:** **VirtioFS is the default**; gRPC-FUSE remains selectable. Docker's own setting text: VirtioFS "reduced the time taken to complete filesystem operations by up to 98%", and it "is the only file sharing implementation supported by Docker VMM". The dedicated `desktop/features/virtiofs` docs page now **404s** — guidance lives only in the settings table and `vmm.md`. **HyperKit is deprecated.**
- **UID/GID on macOS: undocumented and known-buggy.** `docker/for-mac#6243` ("VirtioFS is not handling permissions as expected. All mount permissions are owned by root regardless of chown") is **open, Docker-acknowledged** (`status/acknowledged`, `area/VirtioFS`). `docker/for-mac#6734` ("Bind-mounted volume has owner:group set as root when running container as non-root user") is **open**. Release notes record *new* ownership bugs in **4.90.0 (2026-09-07)**: bind-mount root intermittently reported as `0:0`, breaking git's "dubious ownership" check. So the classic "macOS ignores UID/GID mapping" folklore is directionally right, but **it is not documented, it is not uniform across sharing implementations, and it is still changing.**
- **Linux native:** plain kernel semantics. Container UID/GID maps 1:1 to host UID/GID (absent userns-remap). This is the only platform where you can reason about ownership analytically.

### Recommendations
- **Preferred pattern: never write as root.** Set `user:` in Compose (or `USER` in the Dockerfile) to a non-root UID/GID so files are created by a non-root process. This is the only approach that works on all three platforms and needs no entrypoint magic.
- On **Linux native** where you must match the host user exactly, pass the host uid/gid in via `.env` (`UID=${UID:-1000}`) and use `user: "${UID}:${GID}"`. This keeps ownership correct without a privileged entrypoint.
- **PUID/PGID is still current** — LinuxServer.io's documentation still recommends it and explicitly says *"We are aware that recent versions of the Docker engine have introduced the `--user` flag. Our images are not yet compatible with this, so we recommend continuing usage of PUID and PGID."* Use it **only** if you are consuming an LSIO-style image or need to adopt an existing base image that expects it.
- **Pitfalls of PUID/PGID:** (1) the entrypoint must start as root to `usermod`/`groupmod`/`chown`, so the container is root-capable and any RCE before the drop is root — pair it with `cap_drop`, `no-new-privileges`, and ideally userns; (2) it conflicts with `--user`, as LSIO states; (3) it breaks with `--read-only` unless the paths it rewrites are writable mounts; (4) re-chowning large trees on every start is slow; (5) it silently mangles ownership if PUID/PGID collide with existing in-image users.
- Avoid `--userns=host` (it *disables* the isolation you want). Rootless Docker or `userns-remap` are the real hardening levers, but both change what UID the container sees, so they interact with any host-uid-matching scheme — decide deliberately, don't stack them by accident.
- **Do not assume macOS ownership works.** If the product depends on file ownership from a macOS bind mount, test it explicitly on VirtioFS *and* gRPC-FUSE, and prefer named volumes for anything ownership-sensitive (Docker's own advice: for caches/databases "performance will be much better if they are stored in the Linux VM, using a data volume").
- For big monorepos, consider **Synchronized file shares** (Pro/Team/Business; not available with Windows containers) with a `.syncignore`.

Sources:
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/settings-and-maintenance/settings.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/features/vmm.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/features/wsl/_index.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/features/synchronized-file-sharing.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/release-notes.md
- https://raw.githubusercontent.com/linuxserver/docker-documentation/master/docs/general/understanding-puid-and-pgid.md
- https://github.com/docker/for-mac/issues/6243
- https://github.com/docker/for-mac/issues/6734
- Live tests on Docker Desktop 29.6.1 / WSL2 kernel 6.18.33.2 (commands and output above).

---

## 3. Healthchecks, `depends_on: service_healthy`, and blocking launchers

`depends_on` with `condition: service_healthy` makes Compose *wait* before creating the dependent service, and it is the documented mechanism — "Compose waits for healthchecks to pass on dependencies marked with `service_healthy`". But it only orders startup; it does **not** give the launcher a signal about the whole project. For a launcher script the correct primitive is **`docker compose up -d --wait`**, which implies detached mode and blocks until all services are `running|healthy`. **[TESTED]** it returns **exit code 1** on an unhealthy service, and it fails **as soon as the healthcheck reports unhealthy** rather than burning the whole timeout. On success it returns 0. `--wait-timeout` is an int in **seconds**, default `0` meaning no limit — set it explicitly, because with the default a genuinely stuck service blocks the launcher forever. `docker compose wait` is a different command with different semantics and a mandatory service argument.

**[TESTED] `up -d --wait` failing service → exit 1:**
```
Container dshwaittest-good-1 Healthy
container dshwaittest-bad-1 is unhealthy
EXITCODE=1
```
Note it reported `Healthy` for the good service and then failed on the unhealthy one — so a partially-healthy project still fails overall.

**[TESTED] `up -d --wait` all healthy → exit 0:**
```
Container dshwaitok-good-1 Waiting
Container dshwaitok-good-1 Healthy
EXITCODE_SUCCESS=0
```
Re-running `up -d --wait` against an already-healthy project is **idempotent and returns 0** (`Container ... Running` → `Waiting` → `Healthy`, `EXITCODE_RERUN=0`) — safe to call unconditionally at launcher start.

**[TESTED] `docker compose wait` requires a SERVICE argument.** Calling it bare fails:
```
docker: 'docker compose wait' requires at least 1 argument
Usage:  docker compose wait SERVICE [SERVICE...] [OPTIONS]
EXITCODE_WAIT=1
```
This contradicts a common assumption that `compose wait` waits on the project. It blocks until the named services *stop*, and **[TESTED] it propagates the container's own exit code** — a service exiting 7 produced:
```
container "4525..." exited with status code 7
EXITCODE_WAIT_FAILER=7
```
On a still-running service it correctly blocked past a 6-second probe. Its `--down-project` flag drops the project when the first container stops.

Documented exit codes for `up`: **1 on error**; **0 on SIGINT/SIGTERM** (containers stopped, exit 0 — so your launcher must distinguish "user pressed Ctrl-C" from failure by other means). Build/serve failures are also documented at 1.

Healthcheck mechanics: `test` accepts `NONE`, `CMD`, or `CMD-SHELL`; a bare string is equivalent to `CMD-SHELL`. `interval`/`timeout`/`start_period`/`start_interval` are durations. `start_period` is the important one for launchers — failures during it don't count toward `retries`, which prevents flapping on slow first boots. **A healthcheck is required for `service_healthy`**: with only `depends_on` short syntax, "Compose does not wait for dependency services to be 'healthy'". Also note `restart: true` on a dependency re-restarts dependents on explicit Compose operations (added 2.17.0), and `required: false` downgrades a missing dependency to a warning (added 2.20.0).

### Recommendations
- Launcher: `docker compose up -d --wait --wait-timeout <N>` and branch on the exit code. Treat non-zero as fatal. This is one command, no polling loop, no log scraping.
- Always pass `--wait-timeout`; the default `0` is unbounded.
- Give every service a `healthcheck` with a sensible `start_period` (30–60s for a Node app doing migrations), and gate dependents with `condition: service_healthy`. Without a healthcheck the condition cannot be satisfied.
- Use `CMD-SHELL` (or a bare string) if you need shell operators like `||`; use `CMD` to avoid a shell entirely. Beware `$` in healthcheck commands — inside Compose, `$` is interpolation, so escape as `$$` (as Docker's own `pg_isready` example does).
- **Do not** use `docker compose wait` to gate startup — it waits for *stop*, and needs an explicit service list.
- To surface a one-shot task's failure (e.g. migrations), prefer `--exit-code-from <service>` with `--abort-on-container-exit` over inferring it from logs.
- Because SIGINT yields exit 0, have the launcher check project state (`docker compose ps --format json`) if you must distinguish an interrupted run from a clean one.

Sources:
- https://raw.githubusercontent.com/docker/compose/main/docs/reference/compose_up.md
- https://raw.githubusercontent.com/docker/compose/main/docs/reference/compose_wait.md
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/services.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/startup-order.md
- Live tests on Docker Compose v5.2.0 / Engine 29.6.1 (commands and output above).

---

## 4. Multi-stage Dockerfile for a Node 22/24 TypeScript pnpm monorepo

The big change since most guides were written: **Corepack is no longer distributed with Node.js.** Node PR #57617 ("build: stop distributing Corepack") landed as a **SEMVER-MAJOR** change, and the Corepack README now states it is distributed with Node.js only "from version 14.19.0 up to (but not including) **25.0.0**". Node's own docs page for Corepack was removed (PR #57663), and the release tarballs stopped shipping it (#59835). So on **Node 24** the `corepack enable` recipe still works, but on **Node 25+/26 it will not** — the image has no `corepack` binary. Correspondingly **pnpm now publishes an official image, `ghcr.io/pnpm/pnpm`**, which its own docs recommend as the base, and pnpm 12 is a standalone native binary that does not even need Node to run. Package the pnpm *store* via BuildKit cache mounts, and keep `pnpm fetch` as the CI-friendly fallback where cache mounts are unavailable.

**Node line status (2026):** Node **24 ("Krypton") is the Active LTS** — 24.21.0 released 2026-09-07; Node **26** exists and is where Yarn v1 bundling was dropped. `node:24-bookworm-slim` is the sensible default for a glibc Debian target; `node:24-trixie` exists (Debian 13). **[TESTED]** the multi-arch digest for `node:24-bookworm-slim` today is an **index** digest:
```
Digest: sha256:2fe369e969550cde8e867afc3fe370b260140cab4a23d467074295b42163d553
  linux/amd64 -> sha256:713cfbf4a0ac19f40e1bb9919893e126b74a5c8cf5d0623c9f89515c8f74c6fa
  unknown/unknown -> sha256:1fd07dc5... (attestation-manifest)
```
Note the second entry is the **attestation manifest** — a reminder that `imagetools inspect` now shows attestation entries alongside platforms.

**pnpm-in-Docker specifics from pnpm's own docs:** BuildKit cache mount is the recommended store-sharing mechanism, and they warn that *"Baking a warm store into an image layer … does not make linking free"* because overlayfs copies the hardlinked file into the writable layer — with the explicit note that a cache mount avoids this. They also state the security caveat: *"keep the pnpm store cache scoped to mutually trusted builds. A store cache that can be written by an untrusted build should not be reused by trusted builds."* The official pattern is `RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile`. For CI/ephemeral builders where cache mounts don't persist, pnpm recommends `pnpm fetch --prod` against only `pnpm-lock.yaml` so the layer cache is invalidated only on lockfile change. For monorepos, `pnpm deploy --filter=<pkg> --prod` produces a self-contained output directory per app.

**Configurable UID/GID at runtime:** the PUID/PGID entrypoint pattern is **still recommended for consuming images that expect it** (LinuxServer.io still documents it as their supported mechanism) but it is *not* the modern default and it has real costs. The clean 2026 answer for an image you control is: build a non-root user at a known UID, and let the *runtime* select the UID via `user:` / `--user`, because the kernel then applies it without any privileged step.

### Recommendations
- Base: `node:24-bookworm-slim` (pinned by digest in production, see §9). Avoid Alpine for a pnpm/Node monorepo unless you have verified every native addon — musl is a different libc, and Node's own docs classify musl/amd64 as "Experimental".
- **Get pnpm in without Corepack:** either use `ghcr.io/pnpm/pnpm:<major>` as the base (pnpm's documented preference, Node not bundled — pin Node yourself via `devEngines.runtime`), or install the standalone binary in a build stage. If you deliberately stay on Node ≤24, `corepack enable && corepack prepare pnpm@<ver> --activate` still works, but **do not** write that recipe if a Node 25/26 upgrade is plausible.
- Pin the pnpm version in `package.json` `packageManager` (with the hash) so builds are deterministic regardless of how pnpm arrives.
- Multi-stage: `deps` stage installs with the cache mount → `build` stage compiles TS → final stage copies only `dist` + prod `node_modules`. pnpm's own monorepo example uses `pnpm deploy --filter=<app> --prod /prod/<app>` and `COPY --from=build`.
- Cache the store, not `node_modules`: `RUN --mount=type=cache,id=pnpm,target=/pnpm/store pnpm install --frozen-lockfile`. Always `--frozen-lockfile` in image builds so a lockfile drift fails the build instead of silently resolving.
- Order COPY for cacheability: `package.json` + `pnpm-lock.yaml` + `pnpm-workspace.yaml` first, install, then `COPY . .`. Add a `.dockerignore` with `node_modules`, `.git`, `dist` — a stale host `node_modules` copied into the context is a classic source of "works locally, broken in CI".
- Run as non-root: create the user during build (`RUN useradd -u 1001 -m app`), `USER app`. Prefer `user:` at runtime over an entrypoint that must start as root and chown.
- **If you need a runtime-configurable UID/GID** (e.g. matching an arbitrary host user), the least-bad pattern is a **build-time** build-arg that bakes the UID, plus `user: "${UID}:${GID}"` at runtime — not a root entrypoint. Only adopt PUID/PGID if you're wrapping an image that already requires it, and if you do, pair it with `cap_drop`/`no-new-privileges` and never combine it with `--user`.
- `docker buildx build --platform linux/amd64,linux/arm64` with `docker-container` driver; add `--sbom`/`--provenance` per §9.

Sources:
- https://raw.githubusercontent.com/nodejs/docker-node/main/README.md
- https://raw.githubusercontent.com/pnpm/pnpm.io/main/docs/docker.md
- https://raw.githubusercontent.com/pnpm/pnpm.io/main/docs/installation.md
- https://raw.githubusercontent.com/nodejs/corepack/main/README.md
- https://github.com/nodejs/node/pull/57617
- https://github.com/nodejs/node/pull/57663
- https://github.com/nodejs/node/pull/59835
- https://nodejs.org/dist/index.json (Node 24.21.0 LTS, 2026-09-07)
- Live `docker buildx imagetools inspect node:24-bookworm-slim` (digest above).

---

## 5. Docker socket / privileged security for an AI coding agent with shell access

The threat model is inverted from a normal app: the agent is *supposed* to execute arbitrary commands, so any capability you grant it is a capability an attacker can reach through prompt injection or a compromised dependency. The one rule that matters most: **mounting `/var/run/docker.sock` is equivalent to handing over the host**, because the API allows `docker run --privileged -v /:/host`. Docker's own docs are blunt — the `docker` group "grants root-level privileges to the user", and TLS keys for the daemon "giv[e] them root access to the machine". A container that both runs untrusted agent commands *and* has the socket is not a sandbox at all. Docker's default seccomp profile is a decent baseline and Docker explicitly says "It is not recommended to change the default seccomp profile" — and **[TESTED]** that default importantly does *not* block Landlock (§6), so you lose nothing by keeping it.

**Socket risk, since Docker docs don't put it on one page:** the daemon "requires `root` privileges unless you opt-in to Rootless mode"; "only trusted users should be allowed to control your Docker daemon"; sharing a host directory "without limiting the access rights of the container" means "the container can alter your host filesystem without any restriction". The API "can be still accessible from containers, and it can easily result in the privilege escalation". Exposing the daemon over HTTP without TLS is now **disallowed** (the daemon fails at startup). There is also a first-class `--use-api-socket` flag — and a relevant 2026 CVE, **CVE-2026-6406 (CVSS 8.8)**, where `--use-api-socket` bypassed Docker Desktop's Enhanced Container Isolation because the ECI proxy only inspected `HostConfig.Binds` and not `HostConfig.Mounts`, yielding full engine socket access *and* registry credentials. That is a precise illustration of why socket exposure is hard to get right.

**Defaults worth knowing:** default container capabilities are the classic 14 (`CAP_CHOWN`, `DAC_OVERRIDE`, `FSETID`, `FOWNER`, `MKNOD`, `NET_RAW`, `SETGID`, `SETUID`, `SETFCAP`, `SETPCAP`, `NET_BIND_SERVICE`, `SYS_CHROOT`, `KILL`, `AUDIT_WRITE`). The default seccomp profile is an **allowlist** (`defaultAction: SCMP_ACT_ERRNO`, errno 1) — Docker's docs describe it as disabling "around 44 system calls out of 300+" and being "moderately protective while providing wide application compatibility". Notable blocked syscalls include `mount`, `pivot_root`, `setns`, `unshare`, `bpf`, `perf_event_open`, `ptrace`, the `io_uring_*` family (blocked as container-escape vectors per moby/moby#46762), `kexec_*`, `init_module`/`finit_module`/`delete_module`, `add_key`/`keyctl`/`request_key`, and `userfaultfd`. In 2026 the profile also gates `socket()` for `AF_ALG` — for **CVE-2026-31431** ("copy.fail") — and `AF_VSOCK` (moby/moby#52494), with the important caveat that 32-bit `socketcall` is still allowlisted and is mitigated by the default AppArmor/SELinux policy instead.

**[TESTED]** read-only rootfs + writable bind mount works exactly as intended:
```
$ docker run --rm --read-only --tmpfs /tmp -v "$t:/work" alpine \
    sh -c "echo writable > /work/out.txt && ... echo fail > /rootfile ..."
WROTE_OK
ROOTFS_IS_READONLY
sh: can't create /rootfile: Read-only file system
```
The bind mount was writable and the file appeared on the host; the rootfs rejected writes. So `--read-only` + a writable workspace mount is a viable and effective combination for an agent container.

**Rootless / userns:** rootless mode is the strongest available lever because the daemon itself runs unprivileged and the container's "root" is a mapped non-root host user. `userns_mode`/`userns-remap` achieves similar for the container while the daemon stays privileged. Both change the UID the container sees, so they interact with §2's ownership strategy. On Windows/macOS, Docker Desktop **Enhanced Container Isolation** (Business subscription) is the equivalent hardening layer, and Docker Desktop also keeps the container inside its Linux VM — a genuine boundary that WSL2's shared-kernel model weakens (Docker's own docs recommend Hyper-V mode "to avoid the shared-kernel model entirely" where stricter isolation is required).

### Recommendations
- **Do not mount `/var/run/docker.sock`.** If the agent must build/run containers, use a dedicated socket proxy that allowlists endpoints, or a separate sibling/rootless daemon — never the host socket. If you must, treat it as full host compromise in your threat model and say so in the architecture doc.
- Baseline flags for the agent container: `--cap-drop=ALL` then add back only what's needed (usually nothing), `--security-opt no-new-privileges=true`, `--read-only`, `--tmpfs /tmp`, and a **narrow writable bind mount** for the workspace only. **[TESTED]** this costs you nothing for Landlock.
- Keep Docker's **default** seccomp profile. Only go `seccomp=unconfined` if you specifically need bwrap (§6), and prefer the narrower options there.
- Add `--pids-limit`, `--memory`, and `--cpus` to bound fork-bombs and runaway builds.
- Prefer **rootless Docker** or **userns-remap** for the daemon if the host is Linux; accept that it changes container-visible UIDs.
- On Docker Desktop, enable **Enhanced Container Isolation** (Business) and note that on Windows the WSL2 shared-kernel model, not the container config, is the weaker link — Docker recommends Hyper-V mode when isolation matters more than convenience.
- Drop `NET_RAW` (in the default 14) unless the agent genuinely needs it; it enables raw-socket attacks inside the container network.
- Keep `docker scout`/SBOM gates (§9) because the agent's dependency tree is now an attack surface.

Sources:
- https://raw.githubusercontent.com/moby/profiles/main/seccomp/default.json
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/seccomp.md
- https://docs.docker.com/engine/security/
- https://docs.docker.com/engine/security/protect-access/
- https://docs.docker.com/engine/install/linux-postinstall/
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/services.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/features/wsl/_index.md
- https://github.com/moby/moby/pull/52494
- https://nvd.nist.gov/vuln/detail/CVE-2026-31431
- Live tests on Engine 29.6.1 (commands and output above).

---

## 6. bubblewrap / Landlock inside Docker containers — **the critical one**

**Headline: `bwrap` does NOT work in a default Docker container, and Landlock DOES — with zero extra flags.** This is not a nuance; it is the difference between an architecture that works everywhere and one that only works where you can grant the container privileges. I verified both by running them. The cause is a single seccomp rule: Docker's default profile allows `clone`/`unshare` only when the namespace-creating flag bits are **clear** (`SCMP_CMP_MASKED_EQ`, mask `0x7E020000`, which excludes `CLONE_NEWUSER`) unless the container has `CAP_SYS_ADMIN`. Bubblewrap's setuid fallback was **removed** years ago, so bwrap is now *entirely* dependent on unprivileged user namespaces. Landlock, by contrast, is an LSM designed for unprivileged self-restriction, and its three syscalls are unconditionally allowlisted.

**[TESTED] bwrap in a default container fails:**
```
$ docker run --rm alpine sh -c "apk add --no-cache bubblewrap && bwrap --ro-bind / / --dev /dev --proc /proc echo BWRAP_OK"
bwrap: Creating new namespace failed: Operation not permitted
```

**[TESTED] the kernel is fine — it is seccomp.** Running privileged on the same host:
```
$ docker run --rm --privileged alpine sh -c "unshare --user --map-root-user echo HOST_KERNEL_ALLOWS_USERNS"
HOST_KERNEL_ALLOWS_USERNS
```

**[TESTED] isolation of the fix — either knob alone is sufficient:**
| Container flags | Result |
|---|---|
| (default) | `unshare(0x10000000): Operation not permitted` |
| `--security-opt seccomp=unconfined` only | `OK_SECCOMP_ONLY` |
| `--security-opt apparmor=unconfined` only | `unshare(0x10000000): Operation not permitted` |
| `--cap-add SYS_ADMIN` only | `USERNS_OK_CAPSYSADMIN` |
| `--security-opt seccomp=unconfined` + `apparmor=unconfined` + `--cap-add SYS_ADMIN` | `BWRAP_OK` |
| `bwrap` binary with `seccomp=unconfined` only | `BWRAP_SECCOMP_ONLY` |

So **the minimal requirement for bwrap is `--security-opt seccomp=unconfined`**; `CAP_SYS_ADMIN` is an alternative that is *not* better (it re-grants mount operations). `apparmor=unconfined` alone does nothing on this host but is required on AppArmor-enforcing hosts (Ubuntu). You generally need **both** seccomp=unconfined **and** apparmor=unconfined in practice, because on Ubuntu the outer AppArmor profile also gates user-namespace creation.

**[TESTED] Landlock in a completely default container — no flags at all:**
```
Landlock ABI version = 7
landlock_create_ruleset -> ret=7 errno=0
landlock_add_rule      -> ret=-1 errno=77 (File descriptor in bad state)
landlock_restrict_self -> ret=-1 errno=1  (Operation not permitted)
```
The first call succeeding with `ret=7` proves the syscall is permitted and the kernel reports **ABI v7**; the later "errors" are just my passing null/invalid descriptors, not permission failures.

**[TESTED] Landlock survives full hardening — `--cap-drop=ALL` + `no-new-privileges`:**
```
$ docker run --rm --cap-drop=ALL --security-opt no-new-privileges alpine ... 
Landlock ABI = 7 errno= 0
```
This is the decisive practical result: **you can run the most hardened container configuration and still keep Landlock.** Confirmed against the profile source: `landlock_add_rule`, `landlock_create_ruleset`, `landlock_restrict_self` are in the allowlist group with **no capability gate and no argument filter**.

**Landlock ABI status (a correction to the brief's premise):** the user's brief assumed "ABI v1–v6". Landlock is at **ABI v11** on mainline in 2026 (kernel documentation dated **August 2026**; mainline is v7.3-rc3, v7.2 released 2026-08-16). History: ABI 1 = Linux 5.13; ABI 2 = `LANDLOCK_ACCESS_FS_REFER`; ABI 3 = `TRUNCATE`; ABI 4 = TCP bind/connect; ABI 5 = `IOCTL_DEV`; ABI 6 = abstract-unix-socket + signal scoping; ABI 7 = audit-log flags; ABI 8 = `LANDLOCK_RESTRICT_SELF_TSYNC` (enforce across all threads); ABI 9 = `FS_RESOLVE_UNIX` (pathname unix sockets); ABI 10 = UDP bind/connect/send + per-object quiet-rule flag; ABI 11 = `LANDLOCK_RESTRICT_SELF_NO_NEW_PRIVS`. **Design implication: ABI v6 is a floor you will rarely see; target "v4+" and feature-detect, and consider TSYNC (v8) if your Node app is multithreaded** — below v8 a ruleset only covers the calling thread and its children, not sibling threads, which is a subtle correctness trap for a multi-threaded Node process. Also note the kernel's own guidance: unprivileged enforcement **requires** `no_new_privs`; a process with `CAP_SYS_ADMIN` can skip it, but the docs call that risky (confused-deputy via SUID) — ABI 11 exists precisely to set it atomically.

**Docker Desktop (Windows/macOS): bwrap works only with the same overrides, and this is the biggest cross-platform risk.** The container runs in a Linux VM, so the situation is identical to Linux *at the container level* — same seccomp profile, same result. **[TESTED]** on Docker Desktop for Windows with the WSL2 backend, bwrap failed by default and worked once seccomp was relaxed. What differs is the **kernel**, which is platform-supplied: on Windows/WSL2 it is **Microsoft's WSL kernel** (verified locally: `6.18.33.2-microsoft-standard-WSL2`); on macOS and Hyper-V it is Docker's own VM kernel (**v7.0.12 as of Desktop 4.87.0**, per release notes). All three are far newer than 5.13, so Landlock is available everywhere; but because the kernel version is outside your control and changes with Docker Desktop upgrades, **you must feature-detect the Landlock ABI at runtime rather than assume it.**

**The AppArmor wrinkle (Linux hosts):** Ubuntu 24.04+ restricts unprivileged user namespaces via `kernel.apparmor_restrict_unprivileged_userns`, requiring a confined AppArmor profile containing `userns,` or `CAP_SYS_ADMIN`. The bubblewrap maintainer (smcv) states this plainly: Ubuntu's kernel "does not allow [programs like bubblewrap] to create a new user namespace unless they are given an AppArmor profile that contains the `userns` permission", that this is "their choice", and that Ubuntu is deliberately declining to add a generic bwrap profile. This means on a stock Ubuntu 24.04+ host, an agent sandbox built on bwrap can be **denied even inside a container with seccomp relaxed**, because the outer AppArmor policy applies. On a Docker Desktop host this doesn't apply (it's Docker's VM), but it absolutely applies to Linux-native deployments.

### Recommendations
- **Architecture: use Landlock as the primary sandbox and treat bwrap as an optional enhancement.** Landlock needs no container flags, survives `--cap-drop=ALL`, is portable to every Docker Desktop variant and Linux host, and is exactly the "restrict this child process" primitive you want. bwrap requires weakening container security and still can be blocked by host AppArmor policy.
- Feature-detect the Landlock ABI at runtime (`landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)`), log it, and apply the best-effort subset per the kernel docs' own `switch (abi)` pattern. Do not fail hard if the ABI is lower than expected.
- If you keep bwrap, the documented flag set is `--security-opt seccomp=unconfined --security-opt apparmor=unconfined` (and on some hosts `--cap-add SYS_ADMIN`). Prefer a **custom seccomp profile** that adds only the namespace syscalls rather than `unconfined` wholesale — the bubblewrap issue thread notes that the **Podman seccomp profile** can be used with Docker as a middle ground precisely because "Podman allows unshare".
- If bwrap reports `Can't mount proc ... Operation not permitted`, the fix documented in the bubblewrap tracker is `--security-opt unmask=ALL` (it is one of the things `--privileged` does) — but note this unmask trick is a **Docker/Podman `--security-opt`** feature surfaced there for Podman; verify on your engine. Alternatively avoid the problem by not unsharing PID, or by `--tmpfs /dev --dev-bind /dev/null /dev/null` instead of `--dev /dev` (which needs `/dev/pts`).
- **Make the sandbox pluggable and fail-closed:** detect Landlock → use it; else detect a working bwrap → use it; else refuse to run untrusted code rather than silently running unsandboxed. For a Node app, an important additional layer exists in-process: **Node's own Permission Model** (`--permission`, stable since v22.13/v23.5) restricts fs/net/child-process/worker/native-addon access and supports irreversible `process.permission.drop()`. Note Node's own docs are explicit that this "does not provide security guarantees in the presence of malicious code" and is a "seat belt" — so use it as defense-in-depth, not as the boundary.
- Do not build a design that requires `--privileged`. It defeats the purpose and is unavailable in most managed environments.
- On Linux-native deployments, verify the host's `kernel.apparmor_restrict_unprivileged_userns` before promising bwrap works; on Ubuntu 24.04+ assume it is on.

Sources:
- https://raw.githubusercontent.com/moby/profiles/main/seccomp/default.json
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/seccomp.md
- https://github.com/containers/bubblewrap/issues/505 (bwrap inside unprivileged docker; seccomp/apparmor/unmask guidance; 2022–2026)
- https://github.com/containers/bubblewrap/issues/284 (documenting nested docker/podman; `/proc` and unmask=ALL)
- https://github.com/containers/bubblewrap/issues/269 (SELinux denying mount(tmpfs))
- https://raw.githubusercontent.com/containers/bubblewrap/main/README.md (setuid mode removed; userns required)
- https://raw.githubusercontent.com/torvalds/linux/master/Documentation/userspace-api/landlock.rst (ABI table through v11, no_new_privs, TSYNC; doc dated Aug 2026)
- https://raw.githubusercontent.com/nodejs/node/main/doc/api/permissions.md (Node Permission Model, stable, "seat belt" caveat)
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/desktop/release-notes.md (Docker Desktop VM kernel v7.0.12 @ 4.87.0)
- Live tests on Docker Desktop 29.6.1 / WSL2 kernel 6.18.33.2 (**all bwrap and Landlock results in this section are from live runs**).

---

## 7. Opening a browser from a container / launcher cross-platform

The core insight: **a container cannot open the host's browser at all** (no host display, no host IPC, and on Docker Desktop the "host" is a VM). Browser-opening must therefore be the **launcher's** job, on the host, and it is strictly best-effort — every platform has a way for it to silently do nothing. Treat it as a convenience notification, never as something the product depends on, and always also print the URL. On Docker Desktop the port is reachable at `127.0.0.1:PORT` from the host, which I verified locally.

**[TESTED] host reachability of a published port:** `docker run -p 127.0.0.1:18099:80 nginx:alpine` then `Invoke-WebRequest http://127.0.0.1:18099` → `status=200`. So the launcher's browser step targets `http://127.0.0.1:<published port>`. Bind to `127.0.0.1` explicitly rather than `0.0.0.0`/`::` so the dev port isn't exposed on the LAN.

**Per-platform invocations (all standard):**
- **Windows PowerShell:** `Start-Process "http://localhost:PORT"` — **[TESTED]** `Start-Process` is available. This uses the shell association and does *not* need a browser named. Note this is **not** the same as `start` in `cmd`, though `cmd /c start "" "URL"` also works and needs the empty `""` title argument to avoid `start` treating a quoted URL as a window title — a classic bug.
- **macOS:** `open "http://localhost:PORT"`.
- **Linux:** `xdg-open "http://localhost:PORT"` (respect it failing — headless hosts have no `xdg-open` or no browser).
- **`BROWSER` env var:** the convention (used by Python's `webbrowser`, and similar tooling) is that a program honors `$BROWSER` as an override before platform defaulting. If you support it, treat it as an explicit user instruction and do not override it — but do not *require* it.

**Pitfalls:**
- **Headless / SSH / CI:** there is no browser. `xdg-open` may not exist; `Start-Process` may throw. **Always wrap in try/catch and continue.**
- **WSL:** the browser lives on Windows, not in the distro. From inside WSL, `xdg-open` typically fails or is absent; the reliable path is to invoke the Windows host, e.g. via `wslview` (from `wslu`) or `powershell.exe -Command Start-Process <url>`. Since the product must run "**Windows native PowerShell, no WSL**", this matters mainly if a user runs *your* launcher from a WSL shell — detect and handle, don't assume.
- **Docker Desktop's own behavior:** Docker Desktop can open a *container's* exposed port in the browser from the GUI ("Open in browser"), which is unrelated to what your launcher does and is not scriptable for this purpose. Don't depend on it.
- **`localhost` vs `127.0.0.1`:** prefer `127.0.0.1` in the URL you open — some setups resolve `localhost` to `::1` first and a Docker Desktop port published only on IPv4 `127.0.0.1` will then appear "refused" in the browser even though `curl 127.0.0.1` works.
- **Port collisions:** if the port is taken, `docker compose up` fails; detect that and surface the actual error rather than opening a browser to a wrong service ("docker compose port <svc> <port>" returns the real mapping — prefer asking Compose for the port over hardcoding it, especially with `0:` dynamic publishing).
- **Don't open the browser before the app is healthy** — combine with §3: open only after `up --wait` returns 0, else the user sees a connection error and blames the product.

### Recommendations
- Do browser-opening in the **launcher**, after `--wait` succeeds, guarded by try/catch, and always echo the URL to stdout as well.
- Support opt-out (e.g. `--no-browser` or `NO_BROWSER=1`) and honor `BROWSER` if set.
- Detect headless/CI (`CI`, missing `DISPLAY` on Linux, no browser binary) and skip silently.
- On Windows use `Start-Process`; on macOS `open`; on Linux `xdg-open`; and on WSL delegate to `powershell.exe`/`wslview` if you support WSL launchers.
- Open `http://127.0.0.1:<port>`; get `<port>` from `docker compose port` rather than hardcoding, especially if you ever publish with `:0`.

Sources:
- https://raw.githubusercontent.com/python/cpython/main/Lib/webbrowser.py (`BROWSER` / try-order semantics)
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/services.md (ports)
- https://docs.docker.com/reference/cli/docker/compose/port/
- Live tests on Docker Desktop 29.6.1 (port publish + `Start-Process` availability).

---

## 8. Compose profiles and multiple compose files

Merge semantics are the thing people get wrong, and the rule is precise: **sequences append, but `ports`/`volumes`/`secrets`/`configs` have a "unique key" so same-key entries are merged rather than duplicated; `command`/`entrypoint`/`healthcheck.test` are replaced outright.** I verified all of this. In particular `ports` and `volumes` **append** when the targets differ — they are *not* "replaced" as many blog posts claim — which is exactly why a `dev` override that adds a debug port works, and exactly why a dev override that intends to *remove* a production volume silently doesn't. `!reset` and `!override` are the explicit escape hatches. Profiles are the right tool for optional services, and I confirmed a profiled service is correctly excluded by default.

**[TESTED] `ports`/`volumes` append, `command` replaces:**
```
base: command: ["echo","BASE"];  ports: ["8080:80"]; volumes: ["/a:/x"]
over: command: ["echo","OVERRIDE"]; ports: ["9090:90"]; volumes: ["/b:/y"]
```
Result: both ports present (`target: 80 published: "8080"` **and** `target: 90 published: "9090"`), both volumes present (`/a→/x` **and** `/b→/y`), and `command` resolved to the override only.

**[TESTED] same unique key → the override wins (it merges, doesn't duplicate):**
```
base: volumes: ["/a:/work"]
over: volumes: ["/b:/work"]
```
Result: only `source: /b, target: /work` — the `/a` entry was replaced because `target` is the unique key for volumes (and `{ip, target, published, protocol}` for ports).

**[TESTED] `!override` forces full replacement** (documented example) and produces the same single-entry result, but by discarding the whole list rather than merging per-key. `!reset []` / `!reset null` clears an attribute entirely — the documented way to *remove* a port or volume in an override.

**[TESTED] profiles exclude services by default:**
```
$ docker compose config --services                      → core
$ docker compose config --services --profile debug      → core, debug
```
Matches the documented rule: "Services without a `profiles` attribute are always enabled." Also documented and worth knowing: **explicitly targeting a profiled service auto-enables its profile** (`docker compose run db-migrations` works without `--profile tools`), but references from other services (`links`, `extends`, `service:xxx`) do **not** auto-enable a profile — Compose returns an error instead. Enable all with `--profile "*"`. Valid profile names match `[a-zA-Z0-9][a-zA-Z0-9_.-]+`.

**Interpolation interacts with merge:** interpolation is applied **before merge, on a per-file basis**, so a variable defined in `.env` is baked into each file's model before the two files are combined — you cannot use a variable in the base file to reference something introduced by the override.

Other top-level elements are **not** affected by profiles and are always active.

### Recommendations
- Use `compose.yaml` (base, production-shaped) + `compose.dev.yaml` (override) and invoke as `docker compose -f compose.yaml -f compose.dev.yaml up`. Note the modern filename is `compose.yaml`; `docker-compose.yml` still works but the docs use `compose.yaml`.
- **Remember lists append.** To remove or replace a `ports`/`volumes` entry in an override, you must use `!override` (replace the whole list) or `!reset []` (clear it). Do not assume the override replaces the list.
- Set `COMPOSE_FILE` or use `COMPOSE_ENV_FILES` deliberately rather than relying on directory search order; if you use `COMPOSE_FILE` on Windows remember the separator is `;` there and `:` on macOS/Linux.
- Use **profiles** for genuinely optional services (debug tools, seed data, a mail catcher), and keep the core services unprofiled so `docker compose up` always does the right thing.
- Keep profile-gated dependencies in the *same* profile as the service that needs them, or leave them unprofiled — otherwise targeting the service produces an invalid model.
- Do not use `container_name:` in files you intend to merge or scale; it conflicts with the project-scoped naming and with `--scale`.

Sources:
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/merge.md
- https://raw.githubusercontent.com/docker/docs/main/content/reference/compose-file/profiles.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/profiles.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/multiple-compose-files/_index.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/compose/how-tos/environment-variables/envvars.md
- Live tests on Docker Compose v5.2.0 (commands and output above).

---

## 9. Container image reproducibility

Modern Docker builds are **reproducible-by-attestation rather than bit-for-bit reproducible**: provenance is now attached **by default** (`mode=min`), SBOM is opt-in, and the recommended posture is digest-pinned inputs + full attestations + a `reproducible: false` flag read honestly. Two practical traps: (1) digest pinning an *index* vs a *platform manifest* changes what you're pinning, and Buildx/compose must reconcile digests correctly — Compose 5.4.0 explicitly fixed "use platform image-manifest digest, not attested index", and 5.5.0 **overhauled digest reconciliation**, warning that "existing containers may be recreated the first time you run `compose up` after upgrading"; (2) attestations require an image store that supports image indices, so the default `docker` driver **fails** unless the containerd image store is on, and `--load` has the same constraint — `--push` preserves attestations. `docker build --no-cache` is not a reproducibility tool; it is a cache-defeat tool that makes builds *less* reproducible.

**Attestation mechanics:** provenance attestations "with the `mode=min` level are added to images by default". `min` includes build timestamps, frontend, materials, source repo/revision, build platform, reproducibility — and explicitly **not** build-arg values or secret identities, so it is "safe to use for all builds". `max` adds the full LLB definition, the base64'd Dockerfile, and source maps, and **does expose build-arg values** — hence Docker's warning to move credentials to `--secret` mounts (secrets "are never included in provenance attestations"). `--provenance=false` opts out; `BUILDX_NO_DEFAULT_ATTESTATIONS` disables the default globally. Schema supports SLSA v0.2 (default) and v1. SBOMs are **SPDX**, generated by the **BuildKit Syft scanner** by default, attached as an in-toto SPDX predicate. By default **only the final stage is scanned** — for a multi-stage Node build the pnpm/Node build-stage packages would be invisible, so set `ARG BUILDKIT_SBOM_SCAN_STAGE=true` (and `BUILDKIT_SBOM_SCAN_CONTEXT=true` for the build context); these must be declared with `ARG` in the Dockerfile to have any effect.

**Driver support matrix (from the docs):** `docker` driver = attestations require the containerd image store (otherwise the build fails with "Attestation is not supported for the docker driver"); `docker-container`, `kubernetes`, `remote` = supported. Docker Desktop enables the containerd image store **by default** now.

**[TESTED]** `docker buildx build --sbom` and `--provenance` are present in the local Buildx **v0.35.0-desktop.2**, with `--attest stringArray` for the general form.

**[TESTED]** real digest values for pinning (`node:24-bookworm-slim`):
- index: `sha256:2fe369e969550cde8e867afc3fe370b260140cab4a23d467074295b42163d553`
- linux/amd64 manifest: `sha256:713cfbf4a0ac19f40e1bb9919893e126b74a5c8cf5d0623c9f89515c8f74c6fa`

`docker scout` is alive and current (vendored in Docker Desktop; `docker scout version` runs), with CLI docs, a policy system (`.docker/scout/policy`), and CI integrations for GitHub Actions/Azure/CircleCI/GitLab/Jenkins. Docker Desktop also has an optional "background SBOM indexing" setting.

### Recommendations
- **Pin base images by digest** in any image you ship: `FROM node:24-bookworm-slim@sha256:<platform-manifest-digest>`. Decide index-vs-platform deliberately: pinning the **index** digest keeps multi-arch builds working; pinning a **platform manifest** digest locks one architecture. Use Renovate/Dependabot to bump digests, since a digest pin silently stops receiving security updates.
- Record the resolution: keep `FROM node:24-bookworm-slim` in a readable comment next to the digest so the intent survives.
- Generate attestations in CI and **push** (not `--load`): `docker buildx build --sbom=true --provenance=mode=max --push`. Ensure the builder uses the `docker-container` driver or the containerd image store.
- Add `ARG BUILDKIT_SBOM_SCAN_STAGE=true` (and `BUILDKIT_SBOM_SCAN_CONTEXT=true` if useful) to multi-stage Dockerfiles so build-stage dependencies appear in the SBOM — otherwise your pnpm/Node toolchain is invisible to scanning.
- Never pass secrets as build args; use `--mount=type=secret`. `mode=max` provenance will otherwise publish them.
- Do **not** treat `--no-cache` as a reproducibility measure. Reproducibility comes from pinned inputs (digests + lockfile + `--frozen-lockfile`), not from discarding the cache.
- Gate CI on `docker scout cves --exit-code` (or equivalent) and store the SBOM as a build artifact for incident response.
- Note the Compose digest-reconciliation change in 5.5.0: expect a one-time container recreate after upgrading Compose, and don't be surprised by it in a launcher's logs.
- Be aware that reproducibility claims should be read from the attestation: Docker's own examples show `"reproducible": false`. Bit-for-bit reproducibility is not something the default toolchain guarantees.

Sources:
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/build/metadata/attestations/_index.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/build/metadata/attestations/sbom.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/build/metadata/attestations/slsa-provenance.md
- https://raw.githubusercontent.com/docker/docs/main/content/manuals/build/ci/github-actions/attestations.md
- https://api.github.com/repos/docker/compose/releases (v5.0.0, v5.4.0, v5.5.0, v5.5.1 release notes)
- Live `docker buildx version`, `docker buildx build --help`, `docker scout version`, and `docker buildx imagetools inspect node:24-bookworm-slim`.

---

## 10. CI for cross-platform Docker validation in 2026

The honest summary: **CI can validate the Linux build and run for real, and can validate the Windows/macOS *build* only partially — it cannot validate Windows or macOS Docker *runtime* behavior at all on GitHub-hosted runners.** `ubuntu-latest` has a full Docker daemon and can genuinely build and run (including multi-arch via QEMU emulation). `windows-latest` has Docker, but it is the **Windows container** engine — it cannot run your Linux container. `macos-latest` has **no Docker, no colima, no containerd at all**, and hosted runners provide no KVM, so you cannot start a Linux Docker VM there (with a narrow exception, below). Anything about WSL2 vs Hyper-V, VirtioFS vs gRPC-FUSE, Docker Desktop's VM kernel, or macOS UID/GID mapping is therefore **untestable in GitHub-hosted CI** and must be covered by a documented manual test matrix.

**`ubuntu-latest` (currently Ubuntu 24.04):** full Docker preinstalled — **Docker Client/Server 28.0.4, Buildx 0.37.0, Compose 2.38.2**, plus Podman 4.9.3, Buildah, Skopeo, Kind, Minikube, Kubectl. Kernel 6.17.0-1022-azure. This is the only hosted runner where a real Linux container build+run is fully exercised. Note the preinstalled Docker is **28.0.4 while windows-latest has 29.7.2** — a version skew to be aware of if you rely on new engine behavior; consider installing a pinned Docker in CI if that matters.

**`windows-latest` (currently Windows Server 2025, `10.0.26100` Build **33296**, image `20260907.255.1`):** Docker **29.7.2**, Compose 2.40.3, WSL2 2.7.13.0, Kind 0.33.0. This is a **Windows Server host running the Windows container engine**, which is precisely what you need to test **Windows containers** — and precisely why you **cannot test Linux containers** here: doing so needs a Linux VM (WSL2 distro with its own Docker, plus nested virtualization), and GitHub-hosted runners do not expose nested virtualization (see KVM note). So the "clarify what is actually possible" answer is: **Windows containers yes, Linux containers no.** Note also that GitHub explicitly does not support Windows Server *as a Docker Desktop host*; you're testing the Windows container engine natively.

**`macos-latest` (currently macOS 26.6.2, arm64):** **Docker is not present and not supported.** The macOS runner images list no Docker, no colima, no containerd, no Podman, no lima. There is also no KVM — a GitHub maintainer confirmed (Aug 2026) that `/dev/kvm` is a host/hypervisor limitation and that "**nested virtualization isn't officially supported on GitHub-hosted runners in general**". One genuine exception exists: **Apple Silicon macOS runners support nested virtualization via Parallels Desktop** (the images expose a `PARALLELS_DMG_URL` and state "A system extension is allowed for this version"), which is the resolution of the long-standing runner-images#2187. That makes a real Linux VM *technically* possible on macOS runners, but it is heavy, slow, and not a sane default for per-PR validation. On **standard** runners only three things are validated for macOS: that your TypeScript/Node code builds, that your Dockerfile *parses* (e.g. `docker buildx build --print`/`hadolint`, no daemon needed), and that any platform-independent tests pass. **Windows/macOS Docker Desktop runtime behavior is not testable in hosted CI.**

**The one KVM exception (documented, since 2023):** GitHub-hosted **larger Linux runners** do expose `/dev/kvm`; the documented recipe adds the runner user to the kvm group:
```yaml
- name: Enable KVM group perms
  run: |
    echo 'KERNEL=="kvm", GROUP="kvm", MODE="0666", OPTIONS+="static_node=kvm"' | sudo tee /etc/udev/rules.d/99-kvm4all.rules
    sudo udevadm control --reload-rules
    sudo udevadm trigger --name-match=kvm
```
So if you truly need to boot a VM in CI, use larger Linux runners (paid) or self-hosted bare-metal.

**Multi-arch builds:** fully supported and standard — `docker/setup-qemu-action@v4` (registers binfmt_misc so foreign-arch containers can *run*) + `docker/setup-buildx-action@v4` (default `docker-container` driver, required for multi-platform and for cache export). QEMU is user-space emulation, so it is correct but slow — use it to *validate* arm64, not to run a test suite.

### Recommended matrix
| Runner | Build linux/amd64 | Run linux/amd64 | Build linux/arm64 | Run linux/arm64 | Windows containers | macOS Docker runtime |
|---|---|---|---|---|---|---|
| `ubuntu-latest` | ✅ native | ✅ native | ✅ buildx | ⚠️ QEMU only (slow) | ❌ | ❌ |
| `windows-latest` | ❌ (Linux) | ❌ | ❌ | ❌ | ✅ (Windows engine) | ❌ |
| `macos-latest` | ⚠️ no daemon | ❌ | ⚠️ no daemon | ❌ | ❌ | ❌ (no Docker) |

### Recommendations
- **Primary CI = `ubuntu-latest`**: build the image, run it, execute a real smoke test (`docker compose up -d --wait`, hit the health endpoint, assert exit 0), run unit/integration tests inside the container, and generate SBOM/provenance. This is where genuine confidence comes from.
- **Add an arm64 build job** on `ubuntu-latest` with `setup-qemu-action@v4` + `setup-buildx-action@v4`, `--platform linux/amd64,linux/arm64`. Validate that it *builds* and that a simple `uname -m`/node-version check passes; don't run the full suite under emulation.
- **Add a `windows-latest` job only if you ship Windows containers.** It cannot validate your Linux image. A cheap and genuinely useful Windows job instead: run your **PowerShell launcher** and the `docker compose config` validation there (Compose interpolation/quoting bugs from §1 are exactly what this catches) — that is testable without a Linux engine.
- **Add a `macos-latest` job to catch build/parse and platform-specific Node issues**, not Docker runtime behavior. Use `docker buildx build --print`-style checks or a Dockerfile linter to at least fail on syntax errors.
- **Be explicit in the architecture doc** that Docker Desktop behavior (WSL2 vs Hyper-V vs Docker VMM; VirtioFS vs gRPC-FUSE; Docker Desktop VM kernel; macOS bind-mount UID/GID) is **out of scope for CI** and covered by a manual, documented release checklist on real Windows and macOS machines.
- Install a **pinned** Docker/Compose version in CI jobs rather than relying on the runner's preinstalled version, since `ubuntu-latest` carries 28.0.4 today and will drift.
- Only reach for larger runners / self-hosted hardware if you need KVM.

Sources:
- https://raw.githubusercontent.com/actions/runner-images/main/images/ubuntu/Ubuntu2404-Readme.md (Ubuntu 24.04, Docker 28.0.4, Buildx 0.37.0, Compose 2.38.2)
- https://raw.githubusercontent.com/actions/runner-images/main/images/windows/Windows2025-Readme.md (Windows Server 2025, Docker 29.7.2, WSL2 2.7.13.0)
- https://raw.githubusercontent.com/actions/runner-images/main/images/macos/macos-26-arm64-Readme.md (macOS 26.6.2 arm64; no Docker/colima/containerd)
- https://raw.githubusercontent.com/actions/runner-images/main/README.md (image/label table)
- https://raw.githubusercontent.com/docker/setup-buildx-action/master/README.md (default `docker-container` driver, multi-platform)
- https://raw.githubusercontent.com/docker/setup-qemu-action/master/README.md (binfmt_misc emulation for foreign-arch)
- https://github.com/actions/runner-images/issues/14062 (maintainer: no `/dev/kvm`, nested virtualization not officially supported on hosted runners; closed 2026-08-20)
- https://github.blog/changelog/2023-02-23-hardware-accelerated-android-virtualization-on-actions-windows-and-linux-larger-hosted-runners/ (KVM on larger Linux runners)

---

## Cross-cutting risks for the architecture proposal

1. **The bwrap/Landlock finding should drive the design.** Landlock works in the most hardened possible container with zero flags; bwrap requires weakening seccomp (and often AppArmor), and can still be blocked by Ubuntu's host policy. If the sandbox is a load-bearing requirement, make Landlock primary and bwrap optional/best-effort.
2. **There is no portable UID/GID contract for bind mounts.** Windows and macOS go through a VM share with undocumented, actively-regressing ownership behavior. Design so that ownership doesn't matter (write as a non-root user; use named volumes for caches).
3. **The Docker Desktop VM kernel is not yours.** Windows/WSL2 uses Microsoft's kernel; macOS/Hyper-V use Docker's (v7.0.12 per release notes, unverifiable against public linuxkit). Both change under you on upgrade — feature-detect at runtime.
4. **CI cannot cover the risky platforms.** Only `ubuntu-latest` gives a real Linux build+run. Windows/macOS Docker Desktop behavior needs a manual release checklist; say so explicitly rather than implying CI covers it.
5. **`COMPOSE_CONVERT_WINDOWS_PATHS` is a trap**, not a solution: default-off and actively discouraged. Solve spaces/colons/backslashes with long-syntax + single-quoted forward-slash paths instead.

## Verification gaps (stated honestly)
- The Docker Desktop **VM kernel v7.0.12** claim comes solely from Docker's release notes; linuxkit upstream has no 7.0.x series (newest 6.12.x / pinned 6.12.59). Treat the exact number as unverified.
- **macOS bind-mount UID/GID mapping is officially undocumented** — the `osxfs` page has been deleted. Evidence is open issues (for-mac#6243 acknowledged, #6734) plus 2026 release-note bug fixes. It could not be tested on real macOS hardware here.
- **Windows-container behavior and Linux-containers-on-windows-latest** are inferences from the runner image contents plus the blanket nested-virtualization statement, not from an explicit official sentence.
- Compose-file interpolation and merge results, all bwrap/Landlock/unshare results, `up --wait` and `wait` exit codes, read-only-rootfs behavior, port publishing, and the node digest are from **live runs** on Docker Desktop 29.6.1 / Compose v5.2.0 / Buildx v0.35.0-desktop.2 / WSL2 kernel 6.18.33.2 — i.e. **Windows/WSL2 only**. They were not re-run on Linux-native or macOS; the seccomp-based bwrap result should be identical on any engine using the default profile, but Landlock ABI numbers will differ by kernel.
- Debian/Fedora current userns defaults were not verified.
