# DeepSeek Harness Router — Execution Checklist

> **Companion to:** [`PROPOSAL.md`](./PROPOSAL.md) · **Version:** 1.1.0
> **Repository:** <https://github.com/RatioArtificiosa/DeepSeek-Harness-Router>

---

## How to use this checklist

Every line carries a reference of the form **→ §P-XX.Y**. That reference points at a numbered section of `PROPOSAL.md` containing the rationale, the constraints, the exact commands, and the acceptance criteria for that line.

**Rule: read the referenced proposal section before executing the line.** If the section is unclear, the proposal is incomplete — fix the proposal first, then execute. Do not guess.

### Item syntax

```text
[ ] CT-NN-MM  Short imperative line
              → §P-XX.Y     (proposal section: the rationale and detail)
              ⇢ AC: …       (acceptance criterion: how you know it is done)
```

### Status legend

| Mark | Meaning |
|---|---|
| `[ ]` | Not started |
| `[~]` | In progress |
| `[x]` | Complete, acceptance criterion verified |
| `[!]` | Blocked — record the blocker inline |
| `[-]` | Deliberately deferred — record the target milestone |

### The four standing rules (apply to **every** item)

Before executing any line, re-read these. They are absolute.

| Rule | Source |
|---|---|
| **Never touch the host DSH installation** — no writes to `~/.dsh`, no changes to the host `web` profile, no restarting DSH processes | → §P-04.1 (C-01…C-06) |
| **Never touch other Docker workloads** — no prune commands, no stops, no removals outside `deepseek-router` | → §P-04.2 (C-07…C-11) |
| **Nothing outside `/workspace` and `/data`** — one bind mount, one named volume, no socket | → §P-04.4 (C-17…C-20) |
| **Every failure names its cause and its fix** — a message without a remedy is incomplete | → §P-26.1 rule 4 |

---

# PHASE CT-00 — Pre-flight and governance

> Establishes the rules of engagement before a single file is created.

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-00-01** Confirm Docker Engine is reachable and record its exact version, Compose version, backend, and kernel | → §P-03.1 |
| `[ ]` | **CT-00-02** Record the current free-space figure on the target drive (needed for the image-size budget check) | → §P-03.1 |
| `[ ]` | **CT-00-03** Enumerate running containers and confirm which are **off-limits**; write the list into `AGENTS.md` | → §P-03.3, C-07 |
| `[ ]` | **CT-00-04** Enumerate Docker networks; confirm only the three built-ins exist so our project network cannot collide | → §P-03.3 |
| `[ ]` | **CT-00-05** Enumerate volumes; record the named volumes owned by other projects | → §P-03.3, C-09 |
| `[ ]` | **CT-00-06** Confirm no destructive prune command is ever needed; add a written prohibition to `AGENTS.md` | → §P-04.2, C-08 |
| `[ ]` | **CT-00-07** Locate the host DSH install and record its path and version **as read-only reference** | → §P-03.4 |
| `[ ]` | **CT-00-08** Record the host `~/.dsh` directory listing and a recursive hash manifest, to be re-verified at CT-06-08 | → §P-05.2, ST-10 |
| `[ ]` | **CT-00-09** Probe which ports are occupied; specifically confirm the status of 3080 and 3081 | → §P-03.2, C-16 |
| `[ ]` | **CT-00-10** Write the port-occupancy finding into `docs/troubleshooting.md` as a known local condition | → §P-04.3 |
| `[ ]` | **CT-00-11** Record the npm dist-tags for `@deepseek-ai/dsh` and choose the exact version to pin | → §P-03.4 |
| `[ ]` | **CT-00-12** Confirm the pinned version differs from no floating tag; document why floating is forbidden | → §P-03.4, R-52 |
| `[ ]` | **CT-00-13** Review the ten-step Definition of Done and agree it is the release gate | → §P-02.2, §P-35 |
| `[ ]` | **CT-00-14** Review the decision register and challenge each entry before work begins | → §P-06 |
| `[ ]` | **CT-00-15** Review the risk register and agree the three watch-list risks and their escalation triggers | → §P-33.1 |
| `[ ]` | **CT-00-16** Confirm the milestone ordering rule: **no custom UI work before M3** | → §P-31.1 |

**Phase exit:** the environment is fully characterised, the prohibitions are written down, and the plan's premises have been challenged.

---

# PHASE CT-01 — Milestone M0: Repository foundation

> A correctly-structured repository that builds nothing but is reviewable.

→ §P-31.2

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-01-01** Initialise the git repository; set `main` as the default branch | → §P-03.1 |
| `[ ]` | **CT-01-02** Add an MIT `LICENSE` (matching upstream DSH's licence) | → §P-08.1 |
| `[ ]` | **CT-01-03** Write `.gitignore`; it **must** ignore `.env`, `/data`, `/workspace`, and `node_modules` | → §P-08.3 |
| `[ ]` | **CT-01-04** Verify `.gitignore` by attempting to stage a `.env` — it must be refused | → §P-08.3, R-31 |
| `[ ]` | **CT-01-05** Write `.dockerignore` covering `.git`, `node_modules`, `.env*` (except `.env.example`), and logs | → §P-22.4 |
| `[ ]` | **CT-01-06** Add `.editorconfig` for consistent whitespace across contributors | → §P-31.2 |
| `[ ]` | **CT-01-07** Create the full directory skeleton from the proposal's target tree | → §P-08.1 |
| `[ ]` | **CT-01-08** Create `docs/` and `docs/adr/` with placeholder index files | → §P-29.1 |
| `[ ]` | **CT-01-09** Write `AGENTS.md` containing the four standing rules verbatim | → §P-04.1, §P-04.2, §P-04.4, §P-26.1 |
| `[ ]` | **CT-01-10** Write `AGENTS.md` section listing off-limit containers by name | → §P-03.3, C-07 |
| `[ ]` | **CT-01-11** Write `docs/adr/0001-loopback-relay.md` recording decision D-03 with rejected alternatives | → §P-06 (D-03), §P-09 |
| `[ ]` | **CT-01-12** Write `docs/adr/0002-landlock-first.md` recording D-04 **with the empirical evidence** | → §P-06 (D-04), §P-18.3, §P-39.2 (V-11) |
| `[ ]` | **CT-01-13** Write `docs/adr/0003-single-container.md` recording D-01 and the future split seams | → §P-06 (D-01), §P-23.4 |
| `[ ]` | **CT-01-14** Author `.env.example` with every documented variable and **no** secret placeholder | → §P-13.2, §P-13.5 |
| `[ ]` | **CT-01-15** Verify `.env.example` contains no absolute path from any contributor's machine | → §P-13.2, R-31 |
| `[ ]` | **CT-01-16** Add a CI workflow file that passes trivially, so the pipeline exists from commit one | → §P-31.2 |
| `[ ]` | **CT-01-17** Push to the repository and confirm CI is green | → §P-31.2 |

**Phase exit:** the repository exists, is correctly structured, contains no secrets, and its first CI run passes.

---

# PHASE CT-02 — Milestone M1: Dockerized skeleton

> **The highest-information milestone.** Proves the harness actually runs in a container. If this fails, everything downstream is blocked — and we learn it in hours.

→ §P-31.3

## CT-02.A — The Dockerfile

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-02-01** Obtain the base image digest and record it for pinning | → §P-22.3, §P-39.2 (V-19) |
| `[ ]` | **CT-02-02** Write the multi-stage Dockerfile with the `base` / `deps` / `dsh` / `runtime` stages | → §P-22.1, §P-22.2 |
| `[ ]` | **CT-02-03** Pin the base image by **digest**, not tag | → §P-19.5, R-17 |
| `[ ]` | **CT-02-04** Install `zstd` **explicitly**, with an inline comment explaining the session-log dependency | → §P-03.6, D-16, RSK-14 |
| `[ ]` | **CT-02-05** Install `git` explicitly (absent from the slim base) | → §P-03.6, D-16 |
| `[ ]` | **CT-02-06** Install `python3` and `ca-certificates` explicitly | → §P-03.6, D-16 |
| `[ ]` | **CT-02-07** Install `tini` for PID-1 reaping and signal forwarding | → §P-22.2 |
| `[ ]` | **CT-02-08** **Do NOT install `bubblewrap`**; add a comment explaining why its absence is deliberate | → §P-18.3 (S-01) |
| `[ ]` | **CT-02-08a** **Do NOT use `corepack`** — it no longer ships with Node.js and breaks on Node 25+; source pnpm from the standalone image or an explicit global install | → §P-22.8, RSK-15 |
| `[ ]` | **CT-02-08b** Add a CI guard failing the build if `corepack` appears in the Dockerfile | → §P-22.8 |
| `[ ]` | **CT-02-08c** Set `BUILDKIT_SBOM_SCAN_STAGE=true` so the SBOM covers **every** stage, not just the last | → §P-22.9, RSK-17 |
| `[ ]` | **CT-02-08d** Confirm no secret is ever passed as a build arg (provenance mode `max` would embed it) | → §P-22.9, §P-13.5 |
| `[ ]` | **CT-02-08e** Confirm **no** Docker socket appears in any form, including CLI convenience flags | → §P-17.3 (T-13, S-30) |
| `[ ]` | **CT-02-08f** Confirm `COMPOSE_CONVERT_WINDOWS_PATHS` is **not set** anywhere | → §P-25.12 |
| `[ ]` | **CT-02-09** Install the pinned DSH from npm into an isolated prefix (`/opt/dsh`) | → §P-22.1, §P-22.2 |
| `[ ]` | **CT-02-10** Use BuildKit cache mounts for both the pnpm store and the npm cache | → §P-22.6 |
| `[ ]` | **CT-02-11** Set `ENV DSH_HOME=/data/dsh` — the line that isolates us from the host harness | → §P-05.2, D-08 |
| `[ ]` | **CT-02-12** Set `HOME` to a path inside the data volume, never a host home directory | → §P-05.2 |
| `[ ]` | **CT-02-13** Set `WORKDIR /workspace` and declare `VOLUME ["/data"]` | → §P-10.1, R-13 |
| `[ ]` | **CT-02-14** Declare `EXPOSE 3080` and add OCI labels including the DSH version | → §P-22.2 |
| `[ ]` | **CT-02-15** Add a `HEALTHCHECK` invoking the healthcheck script with a generous `start-period` | → §P-12.3 |
| `[ ]` | **CT-02-16** Set the entrypoint to `tini --` plus the Node entrypoint script | → §P-22.2 |
| `[ ]` | **CT-02-17** Add `dsh` to `PATH` so the runtime manager can invoke it by name | → §P-11.3 |

## CT-02.B — M1 validation (the milestone's reason to exist)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-02-18** Build the image from a **clean checkout** and confirm success | → R-19, §P-28.5 |
| `[ ]` | **CT-02-19** Run `dsh --version` in the image; confirm it equals the pinned version | → §P-39.2 (V-2) |
| `[ ]` | **CT-02-20** Run `which zstd` in the image; confirm it resolves | → §P-03.6, §P-28.5 |
| `[ ]` | **CT-02-21** Confirm the **frontend dist** is present in the image | → §P-39.2 (V-6), RSK-02 |
| `[ ]` | **CT-02-22** Run `dsh --profile web --dump-config` in the image; confirm exit 0 and a non-empty tree | → §P-39.2 (V-4), §P-28.4 |
| `[ ]` | **CT-02-23** Count the composed plugin rows and record the number as a regression baseline | → §P-39.2 (V-5) |
| `[ ]` | **CT-02-24** Set `DSH_HOME=/data/dsh` and confirm the profile initialises under it | → §P-39.2 (V-7), §P-05.2 |
| `[ ]` | **CT-02-25** Confirm **nothing** was written outside `/data` and `/workspace` during that init | → §P-04.4 (C-17, C-18), §P-05.2 |
| `[ ]` | **CT-02-26** Run a `headless` task in the container and confirm the process starts and exits meaningfully | → §P-28.7, §P-31.3 |
| `[ ]` | **CT-02-27** Confirm the container runs as **non-root** | → §P-19.2, §P-28.5 |
| `[ ]` | **CT-02-28** Confirm `capsh --print` shows an empty permitted capability set | → §P-19.1, ST-11 |
| `[ ]` | **CT-02-28a** Confirm Landlock is reachable under the **fully hardened** configuration (`cap_drop: ALL` + `no-new-privileges`) | → §P-18.3, §P-39.2 (V-22) |
| `[ ]` | **CT-02-28b** Implement the runtime Landlock **ABI query** — never assume a version | → §P-18.3 (S-02), RSK-16 |
| `[ ]` | **CT-02-28c** Implement **TSYNC** enforcement where ABI ≥ 8, and **fail closed with a diagnostic** below 8 for multi-threaded operations | → §P-18.3 (S-02), RSK-16 |
| `[ ]` | **CT-02-29** Confirm the root filesystem is read-only (`touch /usr/x` must fail) | → §P-19.1, ST-12 |
| `[ ]` | **CT-02-30** Record the image size and compare against the documented budget | → §P-22.5 |

**Phase exit:** `docker run --rm <image> dsh --profile headless "<trivial task>"` starts, runs, and exits with a meaningful code — **inside the container, without touching the host harness**.

---

# PHASE CT-03 — Milestone M2: Web surface and the loopback relay

> **The highest-risk milestone.** Everything else is conventional engineering; proxying a WebSocket-upgrading, cookie-issuing, streaming application without breaking it is the hard part.

→ §P-31.4 · Risk RSK-01

## CT-03.A — Understanding the constraint

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-03-01** Confirm from the CLI that `dsh web` rejects `--host 0.0.0.0` at startup | → §P-03.8 |
| `[ ]` | **CT-03-02** Confirm the `web` app's actual flags (`--host`, `--port`, `--no-open`, `--trusted-host`) | → §P-03.4 |
| `[ ]` | **CT-03-03** Read and understand the browser-trust fence rules (loopback-or-trusted; Origin must match; cross-site refused) | → §P-03.7, §P-09.3 |
| `[ ]` | **CT-03-04** Read and understand the token→cookie exchange at `GET /` | → §P-03.7, §P-09.4 |
| `[ ]` | **CT-03-05** Confirm the decision: loopback relay, not host networking and not patching the bind | → §P-06 (D-03), §P-09.2 |

## CT-03.B — The relay implementation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-03-06** Implement `docker/relay.mjs`: listen on `0.0.0.0:<published>`, forward to `127.0.0.1:<internal>` | → §P-09.2 |
| `[ ]` | **CT-03-07** Forward the HTTP `Upgrade` handshake so WebSockets work | → §P-09.2, §P-09.4 |
| `[ ]` | **CT-03-08** **Preserve `Host` and `Origin` headers unchanged** — do not rewrite to loopback | → §P-09.3 |
| `[ ]` | **CT-03-09** Do not buffer streaming responses; pass chunks through | → §P-09.4 |
| `[ ]` | **CT-03-10** Respect the configured request-body bound; never truncate an upload | → §P-09.4 |
| `[ ]` | **CT-03-11** Keep the relay small and auditable (target ≈150 lines) | → §P-09.2 |
| `[ ]` | **CT-03-12** Add relay logging that never records cookies or tokens | → §P-12.4, ST-13 |

## CT-03.C — Relay validation (each is an exit criterion)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-03-13** Verify `GET /` returns the `302` → `?token=…` → `302` → cookie sequence | → §P-09.4 |
| `[ ]` | **CT-03-14** Verify the cookie replays and the app shell loads (`200`) | → §P-09.4 |
| `[ ]` | **CT-03-15** Verify the WebSocket upgrade returns `101` and streams flow | → §P-09.4 |
| `[ ]` | **CT-03-16** Verify SSE / streaming responses are not buffered | → §P-09.4 |
| `[ ]` | **CT-03-17** Verify a large upload is not truncated | → §P-09.4 |
| `[ ]` | **CT-03-18** Verify **no `403`** occurs from the trust fence on the ordinary `localhost` path | → §P-09.3 |
| `[ ]` | **CT-03-19** Confirm the real DSH UI loads through the relay in a browser | → §P-31.4 |
| `[ ]` | **CT-03-20** Confirm a session streams live events in the browser | → §P-31.4 |
| `[ ]` | **CT-03-21** If any check fails and cannot be fixed, **invoke the §P-09.5 fallback now**, before the launchers exist | → §P-09.5, RSK-01 |

## CT-03.D — Health endpoint

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-03-22** Implement `GET /health` returning all four required sections | → §P-12.1, R-33 |
| `[ ]` | **CT-03-23** Implement `checks[]` with per-check status and timing | → §P-12.1 |
| `[ ]` | **CT-03-24** Implement the `healthy` / `degraded` / `unhealthy` semantics, with `degraded` still returning `200` | → §P-12.2 |
| `[ ]` | **CT-03-25** Implement `GET /health/live` and `GET /health/ready` | → §P-12.3 |
| `[ ]` | **CT-03-26** Wire the Docker `HEALTHCHECK` to the full `/health` probe | → §P-12.3, R-34 |
| `[ ]` | **CT-03-27** Ensure a missing model key yields `degraded`, **not** `unhealthy` | → §P-12.2 |
| `[ ]` | **CT-03-28** Implement readiness detection by parsing DSH's readiness signal **plus** an HTTP probe — never a sleep | → §P-11.4, R-36 |

**Phase exit:** the real DSH UI loads through the relay and a session streams live — or the documented fallback is invoked with a recorded decision.

---

# PHASE CT-04 — Milestone M3: Launchers and the workspace mount

> **The Definition-of-Done milestone.**

→ §P-31.5

## CT-04.A — Workspace validation and path normalization

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-01** Implement the shared validation algorithm exactly as specified | → §P-10.4 |
| `[ ]` | **CT-04-02** Resolve paths with **native** APIs only (PowerShell `Resolve-Path`; POSIX `pwd -P`) | → §P-10.3, R-28 |
| `[ ]` | **CT-04-03** Reject filesystem roots on both platforms | → §P-10.4 step 5 |
| `[ ]` | **CT-04-04** Reject system directories via a documented deny-list | → §P-10.4 step 5 |
| `[ ]` | **CT-04-05** Reject **drive-relative** Windows paths (`C:foo` without a separator) | → §P-10.3 hazard 3 |
| `[ ]` | **CT-04-06** Detect and warn about UNC paths | → §P-10.3 hazard 4 |
| `[ ]` | **CT-04-07** Handle paths containing spaces | → §P-10.3 hazard 1, §P-39.2 (V-17) |
| `[ ]` | **CT-04-08** Handle paths containing non-ASCII characters | → §P-10.3 hazard 6 |
| `[ ]` | **CT-04-09** Neutralize `$`/`%` interpolation hazards by passing values via `.env`, never inline | → §P-10.3 hazard 5 |
| `[ ]` | **CT-04-10** Reject any path containing a `..` segment after resolution | → §P-10.4 step 4 |
| `[ ]` | **CT-04-11** For "new workspace" mode, **create the host directory first** (DSH rejects nonexistent paths) | → §P-10.2, §P-03.7 |
| `[ ]` | **CT-04-12** Ensure every rejection message is plain language and names the offending path | → §P-10.4, §P-26.1 |

## CT-04.B — Port selection

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-13** Default to port 3080 per the source requirement | → §P-04.3, R-37 |
| `[ ]` | **CT-04-14** Probe the port before starting and identify the holding process | → §P-04.3, C-13 |
| `[ ]` | **CT-04-15** On collision, select the next free port from a documented range | → §P-04.3 |
| `[ ]` | **CT-04-16** Write the chosen port to `.env` | → §P-04.3, C-14 |
| `[ ]` | **CT-04-17** **Always** print the final URL | → §P-04.3, R-38 |
| `[ ]` | **CT-04-18** Never silently bind a random port | → §P-04.3, C-15 |
| `[ ]` | **CT-04-19** Verify the auto-selection path works — it is the default path on this machine | → §P-04.3, C-16, §P-39.2 (V-20) |

## CT-04.C — `.env` generation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-20** Write `.env` atomically (write-temp-then-rename) | → §P-25.2 step 7 |
| `[ ]` | **CT-04-21** Write `.env` as UTF-8 **without a BOM** (a BOM corrupts the first key) | → §P-25.4 |
| `[ ]` | **CT-04-22** Normalize Windows backslashes to forward slashes **only** in the value Docker consumes | → §P-25.4 |
| `[ ]` | **CT-04-23** Auto-detect and write `UID`/`GID` on **Linux only**, where they are meaningful | → §P-19.2, §P-19.3 |
| `[ ]` | **CT-04-23a** Do **not** implement the `PUID`/`PGID` root-entrypoint pattern; use `user: "${UID}:${GID}"` | → §P-22.10 |
| `[ ]` | **CT-04-23b** Ensure **all** application state goes to `/data`, never `/workspace`, so ownership never becomes a user-visible problem | → §P-19.3.1 |
| `[ ]` | **CT-04-23c** Ensure the launcher does **not** promise host-side ownership behaviour on macOS/Windows | → §P-19.3, R-64 |
| `[ ]` | **CT-04-24** Never commit `.env`; confirm it stays ignored | → §P-13.2, R-31 |

## CT-04.D — The Compose file

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-25** Write `docker-compose.yml` per the proposal's specification | → §P-23.1 |
| `[ ]` | **CT-04-26** Use the **long-form** bind with the `${WORKSPACE_PATH:?…}` guard | → §P-10.5, §P-39.2 (V-16) |
| `[ ]` | **CT-04-27** Verify the guard fails loudly with our message when the variable is unset | → §P-39.2 (V-16) |
| `[ ]` | **CT-04-28** Set `name:` at top level and omit the obsolete `version:` key | → §P-23.2 |
| `[ ]` | **CT-04-29** Publish the port bound to `127.0.0.1` on the host | → §P-19.6, §P-24.1 |
| `[ ]` | **CT-04-30** Add `security_opt: no-new-privileges`, `cap_drop: ALL`, `read_only: true` | → §P-19.1 |
| `[ ]` | **CT-04-31** Add `tmpfs` for `/tmp` and `/run`, with `noexec` on `/tmp` | → §P-19.1 |
| `[ ]` | **CT-04-32** Add `pids_limit` and a `stop_grace_period` generous enough for log flushing | → §P-19.4, §P-23.2 |
| `[ ]` | **CT-04-33** Configure log rotation on the json-file driver | → §P-23.1 |
| `[ ]` | **CT-04-34** **Confirm the absence** of `privileged`, host networking, and any Docker socket mount | → §P-19.1, R-42/R-43/R-44 |
| `[ ]` | **CT-04-35** Confirm only **one** bind mount and **one** named volume exist | → §P-10.5, C-17/C-18 |
| `[ ]` | **CT-04-36** Use an explicit project name so every resource is namespaced | → §P-04.2, C-10 |

## CT-04.E — `start.sh` (POSIX)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-37** Implement all ten steps in order | → §P-25.2 |
| `[ ]` | **CT-04-38** Verify Docker exists, with the install URL in the failure message | → §P-25.2 step 1 |
| `[ ]` | **CT-04-39** Verify the daemon is reachable, with platform-specific start instructions | → §P-25.2 step 2 |
| `[ ]` | **CT-04-40** Verify Compose v2 specifically | → §P-25.2 step 3 |
| `[ ]` | **CT-04-41** Implement workspace resolution priority: CLI arg > env var > prompt | → §P-25.2 step 4, R-26 |
| `[ ]` | **CT-04-42** Wait for health via `--wait --wait-timeout`, **not** a sleep | → §P-25.2 step 9, R-35/R-36 |
| `[ ]` | **CT-04-42a** **Always** pass an explicit `--wait-timeout` (default `0` is unbounded) | → §P-25.9 |
| `[ ]` | **CT-04-42b** Confirm the service has a healthcheck, without which `--wait` hangs forever | → §P-25.9 |
| `[ ]` | **CT-04-42c** Confirm success via a `/health` query before printing the banner — `up` returns 0 on SIGINT too | → §P-25.9 |
| `[ ]` | **CT-04-42d** Read the host port from `docker compose port`, never hardcode it | → §P-25.8 (S-22) |
| `[ ]` | **CT-04-43** On failure, dump the last 80 log lines and exit non-zero | → §P-25.2 step 9 |
| `[ ]` | **CT-04-44** Print the URL and open the browser platform-aware | → §P-25.2 step 10, R-39/R-40 |
| `[ ]` | **CT-04-45** Support `--port`, `--no-open`, `--new`, `--no-build`, `--doctor`, `--help` | → §P-25.2, R-41 |
| `[ ]` | **CT-04-46** Never fail the launch because the browser could not open | → §P-25.7 |
| `[ ]` | **CT-04-46a** Open and print **`http://127.0.0.1:<port>`**, not `localhost` | → §P-25.8 (S-20, S-21) |
| `[ ]` | **CT-04-46b** Open the browser **only after** `up --wait` exits 0 | → §P-25.8 (S-23) |
| `[ ]` | **CT-04-46c** Wrap browser opening in try/catch — best-effort, never fatal | → §P-25.8 (S-24) |

## CT-04.F — `start.ps1` (Windows PowerShell, first-class)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-47** Implement the **same ten logical steps** | → §P-25.3, R-23 |
| `[ ]` | **CT-04-48** Use native PowerShell throughout — **no WSL, no Git Bash, no Cygwin** | → §P-25.3, R-24 |
| `[ ]` | **CT-04-49** Use `Resolve-Path` / `[IO.Path]` APIs; **never** string-manipulate paths | → §P-25.4, R-28 |
| `[ ]` | **CT-04-50** Declare `#Requires -Version 5.1` and avoid PS7-only syntax | → §P-25.4 |
| `[ ]` | **CT-04-51** Set console output encoding to UTF-8 before printing | → §P-26.7 |
| `[ ]` | **CT-04-52** Write `.env` as UTF-8 without BOM | → §P-25.4 |
| `[ ]` | **CT-04-53** Detect and warn about paths exceeding Windows MAX_PATH limits | → §P-25.4 |
| `[ ]` | **CT-04-54** Document `-ExecutionPolicy Bypass` in the README for blocked users | → §P-25.4, R-74 |

## CT-04.G — Message parity and companion scripts

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-55** Implement the shared message table so both launchers emit identical text | → §P-25.5, R-21 |
| `[ ]` | **CT-04-56** Write a test asserting message parity between the two implementations | → §P-25.5, §P-28.3 |
| `[ ]` | **CT-04-57** Write `stop.sh` / `stop.ps1` that **retain** the data volume | → §P-25.6, §P-30.1 |
| `[ ]` | **CT-04-58** Write `doctor.sh` / `doctor.ps1` that mutate nothing | → §P-25.6 |
| `[ ]` | **CT-04-59** Write `reset.sh` / `reset.ps1` requiring **double confirmation** | → §P-25.6, RSK-09 |
| `[ ]` | **CT-04-60** Verify `reset` cannot be triggered non-interactively by accident | → §P-25.6 |

## CT-04.H — The Definition of Done (the milestone gate)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-04-61** Step 1 — install Docker | → §P-35 |
| `[ ]` | **CT-04-62** Step 2 — clone the repository | → §P-35 |
| `[ ]` | **CT-04-63** Step 3 — run the platform launcher | → §P-35 |
| `[ ]` | **CT-04-64** Step 4 — choose a workspace | → §P-35 |
| `[ ]` | **CT-04-65** Step 5 — start the Dockerized application | → §P-35 |
| `[ ]` | **CT-04-66** Step 6 — open the browser UI | → §P-35 |
| `[ ]` | **CT-04-67** Step 7 — launch a real DSH session | → §P-35 |
| `[ ]` | **CT-04-68** Step 8 — read/write the mounted project | → §P-35 |
| `[ ]` | **CT-04-69** Step 9 — observe real agent events | → §P-35 |
| `[ ]` | **CT-04-70** Step 10 — shut everything down cleanly | → §P-35 |
| `[ ]` | **CT-04-71** Confirm the process worked **without installing DSH on the host** | → §P-35 |
| `[ ]` | **CT-04-72** Record the passing run with evidence (log + screenshots) | → §P-34.3 sequencing rule 5 |
| `[ ]` | **CT-04-73** Verify the mount is **bidirectional**: a container write appears on the host | → §P-28.6 step 11 |

**Phase exit:** the ten-step DoD passes and is recorded as evidence.

---

# PHASE CT-05 — Milestone M4: Onboarding UI

> Gated behind M3. **Do not start before the DoD passes.**

→ §P-31.6 · Anti-goal: §P-27.1

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-05-01** Re-read the anti-goal: **do not rebuild the DSH chat UI** | → §P-27.1 |
| `[ ]` | **CT-05-02** Decide client-plugin vs shell-application approach, against the **currently pinned** DSH version | → §P-27.3 |
| `[ ]` | **CT-05-03** Record that decision as an ADR with its trade-off | → §P-27.3 |
| `[ ]` | **CT-05-04** Build the first-run onboarding flow (workspace + model) | → §P-27.4 |
| `[ ]` | **CT-05-05** Render the "what the agent can reach" trust-boundary block | → §P-27.4 |
| `[ ]` | **CT-05-06** Build the environment panel sourced from `/health` | → §P-27.2 |
| `[ ]` | **CT-05-07** Build the sandbox-status banner for the degraded case | → §P-27.2, §P-18.4 |
| `[ ]` | **CT-05-08** Build the trusted-host self-service remediation page | → §P-24.3, §P-27.2 |
| `[ ]` | **CT-05-09** Build runtime controls (start/stop/restart with visible state) | → §P-27.2 |
| `[ ]` | **CT-05-10** Show version and pinned-DSH info with a link to the compatibility doc | → §P-27.2, §P-29.3 |
| `[ ]` | **CT-05-11** Show session/disk usage with a safe prune action | → §P-27.2 |
| `[ ]` | **CT-05-12** Verify a new user reaches a working session unaided | → §P-31.6 exit |
| `[ ]` | **CT-05-13** Audit against the rejected UX anti-patterns | → §P-27.5 |

---

# PHASE CT-06 — Milestone M5: Cross-platform proof and isolation verification

→ §P-31.7

## CT-06.A — Platform validation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-06-01** Run the DoD on Windows and record the evidence | → §P-35, R-76 |
| `[ ]` | **CT-06-02** Run the DoD on macOS (Apple Silicon) and record the evidence | → §P-35 |
| `[ ]` | **CT-06-03** Run the DoD on macOS (Intel) if available | → §P-31.7 |
| `[ ]` | **CT-06-04** Run the DoD on Linux and record the evidence | → §P-35 |
| `[ ]` | **CT-06-05** Validate Docker Desktop file sharing behaviour on macOS using the manual checklist | → §P-28.9, RSK-05 |
| `[ ]` | **CT-06-06** Validate Docker Desktop file sharing behaviour on Windows manually | → §P-28.9, RSK-05 |
| `[ ]` | **CT-06-07** Date and sign each manual validation | → §P-31.7 |

## CT-06.B — Isolation verification (the standing rules, proven)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-06-08** Re-verify the host `~/.dsh` hash manifest is unchanged after a full lifecycle | → §P-05.2, ST-10, C-01…C-06 |
| `[ ]` | **CT-06-09** Confirm both pre-existing containers are still running and untouched | → §P-03.3, C-07 |
| `[ ]` | **CT-06-10** Confirm no other volume, image, or network was modified or removed | → §P-04.2, C-08/C-09 |
| `[ ]` | **CT-06-11** Confirm every resource we created is namespaced under the project name | → §P-04.2, C-10 |
| `[ ]` | **CT-06-12** Confirm the host `settings.yaml` is byte-identical | → §P-04.1, C-05 |

## CT-06.C — CI honesty

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-06-13** Implement the CI matrix exactly as scoped, without overclaiming | → §P-28.8 |
| `[ ]` | **CT-06-14** Implement `launcher-windows` (PSScriptAnalyzer + doctor mode) | → §P-28.8 |
| `[ ]` | **CT-06-15** Implement `launcher-macos` (shellcheck + doctor mode) | → §P-28.8 |
| `[ ]` | **CT-06-16** Implement the `docs-truth` job asserting the README table matches tested platforms | → §P-28.10, R-77 |
| `[ ]` | **CT-06-17** Write the README platform-support table stating exactly what was validated and how | → §P-29.2, R-77 |
| `[ ]` | **CT-06-18** Confirm no claim in the README exceeds the evidence | → §P-28.8, R-77 |
| `[ ]` | **CT-06-19** Confirm Docker/Compose are **pinned** in CI rather than taken from the runner image | → §P-25.11 |
| `[ ]` | **CT-06-20** Add the arm64 **build** job (QEMU) validating build only, not the full suite | → §P-28.8.1 |
| `[ ]` | **CT-06-21** State explicitly in the docs that Docker Desktop runtime behaviour is **out of CI scope** | → §P-28.8, §P-28.9 |
| `[ ]` | **CT-06-22** Record the manual release checklist for Docker Desktop behaviour on real macOS/Windows machines | → §P-28.9 |

---

# PHASE CT-07 — Milestone M6: Hardening, documentation, release

→ §P-31.8

## CT-07.A — Security acceptance tests

Every line is an explicit test with a pass criterion.

→ §P-21

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-07-01** **ST-01** — write outside `/workspace` from a confined shell → denied | → §P-21 |
| `[ ]` | **CT-07-02** **ST-02** — path traversal from the workspace → confined, no host file reachable | → §P-21 |
| `[ ]` | **CT-07-03** **ST-03** — reach the Docker socket → no socket present, connection refused | → §P-21 |
| `[ ]` | **CT-07-04** **ST-04** — launcher given `/` or `C:\` → rejected with a plain-language message | → §P-21 |
| `[ ]` | **CT-07-05** **ST-05** — launcher given `..\..\etc` → rejected after normalization | → §P-21 |
| `[ ]` | **CT-07-06** **ST-06** — foreign `Host` header → `403` from the trust fence | → §P-21 |
| `[ ]` | **CT-07-07** **ST-07** — request with no cookie → `401` | → §P-21 |
| `[ ]` | **CT-07-08** **ST-08** — `sec-fetch-site: cross-site` → refused | → §P-21 |
| `[ ]` | **CT-07-09** **ST-09** — read-only workspace → reported, no crash | → §P-21 |
| `[ ]` | **CT-07-10** **ST-10** — host `~/.dsh` unchanged across a lifecycle | → §P-21 |
| `[ ]` | **CT-07-11** **ST-11** — `cap_drop: ALL` in effect inside the container | → §P-21 |
| `[ ]` | **CT-07-12** **ST-12** — rootfs read-only; `/data` writable | → §P-21 |
| `[ ]` | **CT-07-13** **ST-13** — grep full logs for the configured key pattern → zero matches | → §P-21 |
| `[ ]` | **CT-07-14** **ST-14** — published port is loopback-only on the host | → §P-21 |
| `[ ]` | **CT-07-14a** Assert the **rendered** Compose config contains no Docker socket reference (S-31) | → §P-17.3 (T-13, S-31) |
| `[ ]` | **CT-07-14b** Assert the README and `AGENTS.md` state the socket prohibition **with the CVE rationale** | → §P-17.3 (S-32) |
| `[ ]` | **CT-07-14c** Confirm the SBOM includes packages from **all** build stages, not only the last | → §P-22.9, RSK-17 |

## CT-07.B — Supply chain

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-07-15** Confirm the base image is pinned by digest | → §P-19.5 |
| `[ ]` | **CT-07-16** Confirm DSH is pinned by exact version | → §P-19.5, R-51 |
| `[ ]` | **CT-07-17** Confirm `--frozen-lockfile` is used and the lockfile is committed | → §P-19.5 |
| `[ ]` | **CT-07-18** Generate an SBOM in CI and attach it to the release | → §P-19.5 |
| `[ ]` | **CT-07-19** Generate provenance attestations | → §P-19.5 |
| `[ ]` | **CT-07-20** Run a vulnerability scan; record **documented exceptions**, never silent suppression | → §P-19.5 |
| `[ ]` | **CT-07-21** Add a CI guard failing on any `latest` / `next` / `alpha` reference in build files | → §P-22.3 |
| `[ ]` | **CT-07-22** Confirm no `@deepseek-ai/*` import exists outside the adapter package | → §P-07.3, R-78/R-79 |

## CT-07.C — Documentation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-07-23** Write `README.md` with exactly the three flows and nothing more | → §P-29.2, R-74 |
| `[ ]` | **CT-07-24** Write the README platform-support table | → §P-29.2, R-77 |
| `[ ]` | **CT-07-25** Write `docs/dsh-compatibility.md` with the exact pinned version and upgrade procedure | → §P-29.3, R-51 |
| `[ ]` | **CT-07-26** Write `docs/architecture.md` describing the layering and seams | → §P-29.1 |
| `[ ]` | **CT-07-27** Write `docs/security.md` including **all residual risks** honestly | → §P-17.5, §P-29.1 |
| `[ ]` | **CT-07-28** Write `docs/permissions.md` covering UID/GID per platform | → §P-19.3, R-64 |
| `[ ]` | **CT-07-29** Write `docs/troubleshooting.md` covering every launcher error message | → §P-29.1, §P-26.4 |
| `[ ]` | **CT-07-30** Write `docs/workspace-model.md` | → §P-10.1, §P-29.1 |
| `[ ]` | **CT-07-31** Add a link checker to CI; confirm zero dead links | → §P-29.4 |
| `[ ]` | **CT-07-32** Add a test asserting the documented error messages match the code | → §P-29.4 |

## CT-07.D — Operations

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-07-33** Verify the backup procedure for the data volume | → §P-30.2 |
| `[ ]` | **CT-07-34** Verify the upgrade procedure preserves the data volume | → §P-30.3 |
| `[ ]` | **CT-07-35** Verify the uninstall procedure is scoped to our project only | → §P-30.4, C-11 |
| `[ ]` | **CT-07-36** Implement `doctor --bundle` with mandatory redaction | → §P-30.6 |
| `[ ]` | **CT-07-37** Assert no secret survives redaction in a support bundle | → §P-30.6, ST-13 |
| `[ ]` | **CT-07-38** Confirm the product sends no telemetry | → §P-30.5 |

---

# PHASE CT-08 — Release gate

> v1.0.0 may be tagged only when **every** box is checked.

→ §P-36

## CT-08.A — Functional

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-01** All ten DoD steps pass on Windows, macOS, and Linux | → §P-36.1 |
| `[ ]` | **CT-08-02** The relay passes its full test suite | → §P-36.1 |
| `[ ]` | **CT-08-03** Port auto-selection works and is demonstrated | → §P-36.1 |
| `[ ]` | **CT-08-04** `stop` and `reset` behave as documented | → §P-36.1 |
| `[ ]` | **CT-08-05** A freshly-cloned repo on a clean machine reaches a working UI | → §P-36.1 |

## CT-08.B — Security

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-06** All 14 security acceptance tests pass | → §P-36.2 |
| `[ ]` | **CT-08-07** No `privileged`, no socket, no host networking in the rendered config | → §P-36.2 |
| `[ ]` | **CT-08-08** `cap_drop: ALL`, `no-new-privileges`, read-only rootfs verified | → §P-36.2 |
| `[ ]` | **CT-08-09** Container runs as non-root | → §P-36.2 |
| `[ ]` | **CT-08-10** No secret appears in any log or support bundle | → §P-36.2 |
| `[ ]` | **CT-08-11** Residual risks documented in `docs/security.md` | → §P-36.2, §P-17.5 |

## CT-08.C — Isolation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-12** Host `~/.dsh` byte-identical before and after a full lifecycle | → §P-36.3 |
| `[ ]` | **CT-08-13** No other container, volume, image, or network touched | → §P-36.3 |
| `[ ]` | **CT-08-14** All created resources namespaced under `deepseek-router` | → §P-36.3 |

## CT-08.D — Quality

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-15** Every doc exists and has no dead links | → §P-36.4 |
| `[ ]` | **CT-08-16** The README platform table matches what was validated | → §P-36.4, R-77 |
| `[ ]` | **CT-08-17** The launcher message-parity test passes | → §P-36.4 |
| `[ ]` | **CT-08-18** `docs/dsh-compatibility.md` names the exact pinned version | → §P-36.4 |
| `[ ]` | **CT-08-19** CI is green on `main` for all jobs | → §P-36.4 |

## CT-08.E — Reproducibility

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-20** The image builds from a clean checkout | → §P-36.5, R-19 |
| `[ ]` | **CT-08-21** Base pinned by digest; DSH pinned by exact version | → §P-36.5 |
| `[ ]` | **CT-08-22** SBOM and provenance generated | → §P-36.5 |
| `[ ]` | **CT-08-23** No `latest` / `next` / `alpha` anywhere in build files | → §P-36.5 |

## CT-08.F — Tag

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-08-24** Tag `v1.0.0` | → §P-31.8 |
| `[ ]` | **CT-08-25** Verify a **clean clone on a clean machine** reaches the DoD | → §P-31.8 |

---

# APPENDIX — Quick reference

## A.1 Requirement → checklist phase

| Requirement range | Meaning | Phases |
|---|---|---|
| R-01 … R-06 | Cross-platform, platform model | CT-01, CT-04, CT-06 |
| R-07 … R-12 | Workspace model | CT-04.A, CT-04.D |
| R-13, R-14 | Runtime filesystem | CT-04.D |
| R-15, R-16 | Compose, no Kubernetes | CT-04.D |
| R-17 … R-19 | Image | CT-02.A |
| R-20 … R-25 | Launch scripts | CT-04.E, CT-04.F, CT-04.G |
| R-26 … R-28 | Workspace selection | CT-04.A |
| R-29 … R-31 | Environment config | CT-04.C |
| R-32 … R-36 | Health checks | CT-03.D |
| R-37, R-38 | Port handling | CT-04.B |
| R-39 … R-41 | Browser launch | CT-04.E, CT-04.F |
| R-42 … R-46 | Docker isolation | CT-04.D, CT-07.B |
| R-47 … R-49 | Local development | CT-04.D (dev overlay) |
| R-50 … R-52 | DSH installation | CT-02.A, CT-07.C |
| R-53 … R-55 | Host DSH independence | CT-00, CT-06.B |
| R-56 … R-58 | Container architecture | CT-01, CT-02 |
| R-59, R-60 | Path abstraction | CT-04.A |
| R-61 … R-64 | File permissions | CT-04.C, CT-07.C |
| R-65 … R-71 | Security | CT-07.A |
| R-72, R-73 | CLI compatibility | CT-04.E, CT-04.F |
| R-74 | README | CT-07.C |
| R-75, R-76 | CI | CT-06.C |
| R-77 | No false claims | CT-06.C, CT-07.C |
| R-78 … R-80 | Future hosted | CT-07.B (import guard) |
| R-81 | Definition of done | CT-04.H, CT-08 |

## A.2 Risk → mitigation checklist

| Risk | Mitigation checklist items |
|---|---|
| **RSK-01** Relay correctness | CT-03-06 … CT-03-21 |
| **RSK-02** DSH won't run in container | CT-02-18 … CT-02-30 |
| **RSK-03** Sandbox unavailable | CT-02-28, CT-03-24, CT-05-07 |
| **RSK-04** Windows path corruption | CT-04-01 … CT-04-12, CT-04-47 … CT-04-53 |
| **RSK-05** Docker Desktop file sharing | CT-06-05, CT-06-06 |
| **RSK-06** DSH version churn | CT-07-16, CT-07-22, CT-07-25 |
| **RSK-07** Image size/time | CT-02-30, CT-00-02 |
| **RSK-08** Port collision | CT-04-13 … CT-04-19 |
| **RSK-09** Data loss on reset | CT-04-59, CT-04-60 |
| **RSK-10** UI scope creep | CT-05-01, CT-05-02 |
| **RSK-11** Host DSH damaged | CT-00-08, CT-06-08, CT-06-12 |
| **RSK-12** Other workloads disturbed | CT-00-03 … CT-00-06, CT-06-09 … CT-06-11 |
| **RSK-13** Windows CI gap | CT-06-13 … CT-06-22 |
| **RSK-14** `zstd` omission | CT-02-04, CT-02-20 |
| **RSK-15** Corepack removed from Node | CT-02-08a, CT-02-08b |
| **RSK-16** Landlock ABI < 8 leaves threads unrestricted | CT-02-28b, CT-02-28c |
| **RSK-17** SBOM omits non-final stages | CT-02-08c, CT-07-14c |
| **RSK-18** Docker socket added "for convenience" | CT-02-08e, CT-07-14a, CT-07-14b |

## A.3 The watch-list risks

- **RSK-06 (16)** — DSH version churn. Mitigation is architectural: all `@deepseek-ai/*` imports in one package (**CT-07-22**).
- **RSK-10 (16)** — UI scope creep. The only likely way this project fails non-technically. **CT-05-01** exists to be quoted.
- **RSK-16 (15)** — the Landlock threading trap. Dangerous because it is **invisible**: a sandbox that reports success while sibling threads run unrestricted. The ABI on this machine is 7, **below** the TSYNC threshold of 8, so the failure mode is live. **CT-02-28b/CT-02-28c** are the guard.
- **RSK-01 / RSK-04 (15)** — the relay and Windows paths. Both front-loaded so they surface early.

## A.4 Milestone exit criteria summary

| Milestone | Exit criterion | Gate item |
|---|---|---|
| M0 | CI green on a correctly-structured repo | CT-01-17 |
| M1 | `dsh --profile headless` runs in the container | CT-02-26 |
| M2 | Real UI loads through the relay; streaming works | CT-03-19, CT-03-20 |
| M3 | **The ten-step DoD passes and is recorded** | CT-04-61 … CT-04-73 |
| M4 | A new user reaches a session unaided | CT-05-12 |
| M5 | DoD passes on all three platforms; docs truthful | CT-06-01 … CT-06-07, CT-06-18 |
| M6 | The release gate is fully checked | CT-08-01 … CT-08-25 |

---

# PHASE CT-09 — Rust core (Repository, language, Docker-first)

> **This phase runs in parallel with CT-02 onward.** It establishes the Router core in Rust and the Docker-first workflow.
>
> → §P-42 · §P-43

## CT-09.A — Repository and workspace setup

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-01** Confirm all references point at `DeepSeek-Harness-Router`, not the superseded repo | → §P-41.2 |
| `[ ]` | **CT-09-02** Set the repo-local git identity to the neutral, non-personal value | → §P-41.3 |
| `[ ]` | **CT-09-03** Verify no commit metadata carries a personal name or email | → §P-41.3, §P-45.6 |
| `[ ]` | **CT-09-04** Confirm `docs/research/` preserves the source conversation and both briefs | → §P-41.2 |
| `[ ]` | **CT-09-05** Add a Cargo workspace root with `crates/` members | → §P-42.4 |
| `[ ]` | **CT-09-06** Pin the Rust toolchain in `rust-toolchain.toml` to the version the container uses | → §P-42.7, §P-43.5 |
| `[ ]` | **CT-09-07** Commit `Cargo.lock` and build with `--locked` | → §P-43.4 |

## CT-09.B — Crate structure

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-08** `router-core` — supervision, health, config, path validation | → §P-42.4 |
| `[ ]` | **CT-09-09** `router-relay` — HTTP/WebSocket/SSE loopback relay | → §P-42.4, §P-09 |
| `[ ]` | **CT-09-10** `router-dsh` — the DSH adapter: SDK/JSON-RPC client, process supervision | → §P-42.3 |
| `[ ]` | **CT-09-11** `router-cli` — the single static binary with subcommands (`serve`, `entrypoint`, `doctor`) | → §P-42.6 |
| `[ ]` | **CT-09-12** Keep the DSH-facing code confined to `router-dsh` — the Rust equivalent of the adapter rule | → §P-16.3, §P-42.4 |

## CT-09.C — Rust engineering standards

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-13** Use `tokio` for async; `axum`/`tower`/`hyper` for HTTP and WebSocket | → §P-42.7 |
| `[ ]` | **CT-09-14** Use `serde`/`serde_json` for JSON-RPC and config | → §P-42.7 |
| `[ ]` | **CT-09-15** Use `clap` (derive) for CLI parsing | → §P-42.7 |
| `[ ]` | **CT-09-16** Use `thiserror` for typed errors; `anyhow` only at the binary boundary | → §P-42.7 |
| `[ ]` | **CT-09-17** Use `tracing` + `tracing-subscriber` for structured JSON logs | → §P-42.7, §P-12.4 |
| `[ ]` | **CT-09-18** Run `cargo fmt --check` and `cargo clippy -- -D warnings` in CI | → §P-42.7 |
| `[ ]` | **CT-09-19** Confirm **no** commit lands with an unresolved rust-analyzer diagnostic | → §P-42.8 |

## CT-09.D — Implement the SDK/JSON-RPC client

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-20** Implement newline-delimited JSON-RPC 2.0 framing over stdio | → §P-42.3 |
| `[ ]` | **CT-09-21** Implement `initialize` and read `serverInfo` | → §P-42.3 |
| `[ ]` | **CT-09-22** Implement `session/prompt` with the documented result shape | → §P-42.3 |
| `[ ]` | **CT-09-23** Implement `shutdown` | → §P-42.3 |
| `[ ]` | **CT-09-24** Handle the four server→client notifications: `session.event`, `session.status`, `subagent.started`, `subagent.finished` | → §P-42.3 |
| `[ ]` | **CT-09-25** Map JSON-RPC errors (`-32601`, `-32603`) to typed Rust errors | → §P-42.3 |
| `[ ]` | **CT-09-26** Test framing against a mock server, including malformed lines | → §P-42.7 |

## CT-09.E — Port the core logic to Rust

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-27** Implement workspace path validation and the deny-list in Rust | → §P-10.4, §P-42.5 |
| `[ ]` | **CT-09-28** Unit-test all seven Windows path hazards | → §P-10.3, §P-28.3 |
| `[ ]` | **CT-09-29** Implement the relay (HTTP + WebSocket upgrade + SSE pass-through) | → §P-09.2 |
| `[ ]` | **CT-09-30** Implement `/health`, `/health/live`, `/health/ready` | → §P-12.1, §P-12.3 |
| `[ ]` | **CT-09-31** Implement readiness detection: readiness signal **plus** HTTP probe | → §P-11.4 |
| `[ ]` | **CT-09-32** Implement process supervision with restart budget and backoff | → §P-11.2 |
| `[ ]` | **CT-09-33** Implement the error-translation table | → §P-11.5 |
| `[ ]` | **CT-09-34** Implement UID/GID privilege handling and the writability check | → §P-19.2 |

## CT-09.F — Docker-first workflow

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-09-35** Add the `rust-builder` stage to the Dockerfile | → §P-43.4 |
| `[ ]` | **CT-09-36** Build with `--release --locked --target x86_64-unknown-linux-musl` | → §P-43.4 |
| `[ ]` | **CT-09-37** Use BuildKit cache mounts for the cargo registry and `target/` | → §P-43.4 |
| `[ ]` | **CT-09-38** Copy **only** the resulting static binary into the runtime stage | → §P-43.4 |
| `[ ]` | **CT-09-39** Create `docker/Dockerfile.dev` with the toolchain and test tooling | → §P-43.3 |
| `[ ]` | **CT-09-40** Create `docker-compose.ci.yml` reproducing CI locally | → §P-43.3 |
| `[ ]` | **CT-09-41** Verify `cargo check`, `cargo clippy`, `cargo test` all run **inside** the container | → §P-43.5 |
| `[ ]` | **CT-09-42** Confirm the host toolchain is **not** required for a clean build | → §P-43.2 |
| `[ ]` | **CT-09-43** Confirm the static binary runs on a bare `alpine` with no libc shims | → §P-42.5 |

**Phase exit:** the Rust core builds in Docker, passes `clippy -D warnings` and its tests, drives DSH over SDK/JSON-RPC, and serves `/health` — with **no** host toolchain involvement.

---

# PHASE CT-10 — The premium README

> The README is the product's public face. It is held to a higher standard than any other file.
>
> → §P-45

## CT-10.A — Content

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-10-01** Write the one-paragraph promise in plain language | → §P-45.3 |
| `[ ]` | **CT-10-02** Write "Why this exists" — the friction of installing DSH by hand | → §P-45.3 |
| `[ ]` | **CT-10-03** Write the benefits section: outcome-led, five to six claims | → §P-45.3, §P-45.5 |
| `[ ]` | **CT-10-04** Pair **every** feature with its benefit | → §P-45.3 |
| `[ ]` | **CT-10-05** Write "What you can do" — concrete, benefit-framed capabilities | → §P-45.3 |
| `[ ]` | **CT-10-06** Write "How it works" against the architecture diagram | → §P-45.3 |
| `[ ]` | **CT-10-07** Write the security section making the trust boundary legible | → §P-45.3, §P-17.4 |
| `[ ]` | **CT-10-08** Write the three installation flows, kept ruthlessly short | → §P-45.3, R-74 |
| `[ ]` | **CT-10-09** Write the honest platform-support table | → §P-45.3, R-77 |
| `[ ]` | **CT-10-10** Write the FAQ covering the objections a reader actually has | → §P-45.3 |
| `[ ]` | **CT-10-11** Credit DeepSeek Harness and the ecosystem | → §P-45.3 |
| `[ ]` | **CT-10-12** Route depth to `docs/` rather than bloating the README | → §P-45.7 |
| `[ ]` | **CT-10-13** Read the whole README end-to-end applying the "would this make me scroll past?" test | → §P-45.5 |

## CT-10.B — Visual assets

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-10-14** Create the hero banner (SVG, dark-first) | → §P-45.4 |
| `[ ]` | **CT-10-15** Create the terminal capture showing the full startup experience | → §P-45.4 |
| `[ ]` | **CT-10-16** Create the benefits grid (six outcomes, scannable) | → §P-45.4 |
| `[ ]` | **CT-10-17** Create the architecture diagram | → §P-45.4 |
| `[ ]` | **CT-10-18** Create the security-boundary diagram | → §P-45.4 |
| `[ ]` | **CT-10-19** Create the install→run→work flow diagram | → §P-45.4 |
| `[ ]` | **CT-10-20** Prefer SVG throughout for crispness and reviewability | → §P-45.4 |
| `[ ]` | **CT-10-21** Verify every asset is legible at both GitHub desktop and mobile widths | → §P-45.4 |
| `[ ]` | **CT-10-22** Confirm no stock photography is used | → §P-45.4 |

## CT-10.C — Privacy (absolute)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-10-23** Grep the entire repo for personal names, emails, and usernames | → §P-45.6 |
| `[ ]` | **CT-10-24** Confirm no diagram or capture contains a real local path, drive letter, or hostname | → §P-45.6 |
| `[ ]` | **CT-10-25** Confirm diagrams use only generic references (`/workspace`, `localhost`) | → §P-45.6 |
| `[ ]` | **CT-10-26** Confirm no screenshot depicts real work or personal data | → §P-45.6 |
| `[ ]` | **CT-10-27** Confirm commit metadata carries no identity | → §P-41.3, §P-45.6 |

## CT-10.D — Accuracy under CI

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-10-28** Ensure the platform-support table matches what was actually tested | → §P-45.8, R-77 |
| `[ ]` | **CT-10-29** Ensure every command in a code block is CI-executed or marked illustrative | → §P-45.8 |
| `[ ]` | **CT-10-30** Add the dead-link check over the README | → §P-45.8 |
| `[ ]` | **CT-10-31** Confirm no claim in the README exceeds the evidence | → §P-45.8 |
| `[ ]` | **CT-10-32** Record the rule that diagrams are regenerated when the CLI output they depict changes | → §P-45.8 |

**Phase exit:** the README reads as a landing page, every claim is reproducible, every image renders on desktop and mobile, and nothing about the author's environment appears anywhere.

---

# PHASE CT-11 — Private installer (NOT in the repository)

> **Scheduling:** this phase begins **only after** the product is complete and validated in Docker.
>
> → §P-44

## CT-11.A — Containment (verify first, build second)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-11-01** Confirm `installer/` is ignored by `.gitignore` | → §P-44.3 |
| `[ ]` | **CT-11-02** Confirm `installer/` is **not tracked**, with `git ls-files` | → §P-44.3 |
| `[ ]` | **CT-11-03** Confirm `router-image.tar` and `*.local.*` are ignored | → §P-44.3 |
| `[ ]` | **CT-11-04** Confirm no installer artifact appears in any commit on any branch | → §P-44.3 |

## CT-11.B — Implementation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-11-05** Implement OS/architecture/shell detection with a clear unsupported message | → §P-44.4 step 1 |
| `[ ]` | **CT-11-06** Implement the Docker presence, daemon, and Compose v2 checks | → §P-44.4 steps 2–4 |
| `[ ]` | **CT-11-07** Implement locate-or-clone of the repository | → §P-44.4 step 5 |
| `[ ]` | **CT-11-08** Implement image build **or** `docker load` from the local archive | → §P-44.4 step 6, §P-44.5 |
| `[ ]` | **CT-11-09** Implement workspace selection and validation | → §P-44.4 step 7 |
| `[ ]` | **CT-11-10** Implement free-port selection | → §P-44.4 step 8 |
| `[ ]` | **CT-11-11** Implement atomic `.env` generation | → §P-44.4 step 9 |
| `[ ]` | **CT-11-12** Implement start-and-wait-for-health | → §P-44.4 step 10 |
| `[ ]` | **CT-11-13** Implement best-effort browser opening with the URL always printed | → §P-44.4 step 11 |
| `[ ]` | **CT-11-14** Register a desktop shortcut / launcher for future starts | → §P-44.4 step 12 |
| `[ ]` | **CT-11-15** Provide a complete uninstall path | → §P-44.4 step 13 |

## CT-11.C — Unattended reliability

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-11-16** Confirm every failure names the cause, names the fix, and exits non-zero | → §P-44.2, §P-44.8 criterion 2 |
| `[ ]` | **CT-11-17** Confirm every step is idempotent — the installer is safe to re-run | → §P-44.7 |
| `[ ]` | **CT-11-18** Confirm no step requires human interpretation of an error | → §P-44.2 |
| `[ ]` | **CT-11-19** Confirm the installer never silently chooses a different workspace | → §P-44.6 |
| `[ ]` | **CT-11-20** Confirm no half-installed state can persist | → §P-44.6 |
| `[ ]` | **CT-11-21** Confirm the owner's existing DSH installation is untouched | → §P-44.6, §P-04.1 |
| `[ ]` | **CT-11-22** Confirm no Docker resource outside our project is removed or pruned | → §P-44.6, §P-04.2 |
| `[ ]` | **CT-11-23** Confirm no telemetry or network egress beyond the image source | → §P-44.6, §P-30.5 |

## CT-11.D — Acceptance on the target machine

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-11-24** Run to completion unattended on a machine with only Docker installed | → §P-44.8 criterion 1 |
| `[ ]` | **CT-11-25** Install from the local image archive with the network disabled | → §P-44.8 criterion 3 |
| `[ ]` | **CT-11-26** Confirm the UI is reachable at the printed URL | → §P-44.8 criterion 4 |
| `[ ]` | **CT-11-27** Run install → uninstall → install again with no manual cleanup | → §P-44.7, §P-44.8 criterion 6 |
| `[ ]` | **CT-11-28** Write the private `README.local.md` so it runs months later without this conversation | → §P-44.8 criterion 8 |
| `[ ]` | **CT-11-29** Re-verify the installer is absent from the public repository after all work | → §P-44.8 criterion 5 |

**Phase exit:** the installer works unattended on the target machine, from a local archive, and is verifiably absent from the public repository.

---

# APPENDIX — Quick reference

## A.1 Requirement → checklist phase

| Requirement range | Meaning | Phases |
|---|---|---|
| R-01 … R-06 | Cross-platform, platform model | CT-01, CT-04, CT-06 |
| R-07 … R-12 | Workspace model | CT-04.A, CT-04.D |
| R-13, R-14 | Runtime filesystem | CT-04.D |
| R-15, R-16 | Compose, no Kubernetes | CT-04.D |
| R-17 … R-19 | Image | CT-02.A |
| R-20 … R-25 | Launch scripts | CT-04.E, CT-04.F, CT-04.G |
| R-26 … R-28 | Workspace selection | CT-04.A, CT-09.E |
| R-29 … R-31 | Environment config | CT-04.C |
| R-32 … R-36 | Health checks | CT-03.D, CT-09.E |
| R-37, R-38 | Port handling | CT-04.B |
| R-39 … R-41 | Browser launch | CT-04.E, CT-04.F |
| R-42 … R-46 | Docker isolation | CT-04.D, CT-07.B |
| R-47 … R-49 | Local development | CT-04.D, CT-09.F |
| R-50 … R-52 | DSH installation | CT-02.A, CT-07.C |
| R-53 … R-55 | Host DSH independence | CT-00, CT-06.B, CT-11.C |
| R-56 … R-58 | Container architecture | CT-01, CT-02, CT-09.B |
| R-59, R-60 | Path abstraction | CT-04.A, CT-09.E |
| R-61 … R-64 | File permissions | CT-04.C, CT-07.C, CT-09.E |
| R-65 … R-71 | Security | CT-07.A |
| R-72, R-73 | CLI compatibility | CT-04.E, CT-04.F |
| R-74 | README | CT-07.C, **CT-10.A** |
| R-75, R-76 | CI | CT-06.C |
| R-77 | No false claims | CT-06.C, CT-07.C, **CT-10.D** |
| R-78 … R-80 | Future hosted | CT-07.B, CT-09.B |
| R-81 | Definition of done | CT-04.H, CT-08 |

## A.2 Risk → mitigation checklist

| Risk | Mitigation checklist items |
|---|---|
| **RSK-01** Relay correctness | CT-03-06 … CT-03-21, **CT-09-29** |
| **RSK-02** DSH won't run in container | CT-02-18 … CT-02-30 |
| **RSK-03** Sandbox unavailable | CT-02-28, CT-03-24, CT-05-07 |
| **RSK-04** Windows path corruption | CT-04-01 … CT-04-12, CT-09-27, CT-09-28 |
| **RSK-05** Docker Desktop file sharing | CT-06-05, CT-06-06 |
| **RSK-06** DSH version churn | CT-07-16, CT-07-22, CT-07-25, CT-09-12 |
| **RSK-07** Image size/time | CT-02-30, CT-09-38 |
| **RSK-08** Port collision | CT-04-13 … CT-04-19, CT-11-10 |
| **RSK-09** Data loss on reset | CT-04-59, CT-04-60, CT-11-15 |
| **RSK-10** UI scope creep | CT-05-01, CT-05-02 |
| **RSK-11** Host DSH damaged | CT-00-08, CT-06-08, CT-06-12, CT-11-21 |
| **RSK-12** Other workloads disturbed | CT-00-03 … CT-00-06, CT-06-09 … CT-06-11, CT-11-22 |
| **RSK-13** Windows CI gap | CT-06-13 … CT-06-22 |
| **RSK-14** `zstd` omission | CT-02-04, CT-02-20 |
| **RSK-15** Corepack removed from Node | CT-02-08a, CT-02-08b |
| **RSK-16** Landlock ABI < 8 leaves threads unrestricted | CT-02-28b, CT-02-28c |
| **RSK-17** SBOM omits non-final stages | CT-02-08c, CT-07-14c |
| **RSK-18** Docker socket added "for convenience" | CT-02-08e, CT-07-14a, CT-07-14b |
| **RSK-19** Rust toolchain friction / build times | CT-09-06, CT-09-37, CT-09-42 |
| **RSK-20** Private installer accidentally committed | CT-11-01 … CT-11-04, CT-11-29 |
| **RSK-21** Personal or machine info leaks into the public repo | CT-09-03, CT-10-23 … CT-10-27 |

## A.3 The watch-list risks

- **RSK-06 (16)** — DSH version churn. Mitigation is architectural: all DSH-facing code confined to one adapter (**CT-09-12**).
- **RSK-10 (16)** — UI scope creep. The only likely way this project fails non-technically. **CT-05-01** exists to be quoted.
- **RSK-16 (15)** — the Landlock threading trap. Dangerous because it is **invisible**: a sandbox that reports success while sibling threads run unrestricted. **CT-02-28b/CT-02-28c** are the guard.
- **RSK-01 / RSK-04 (15)** — the relay and Windows paths. Both front-loaded; both now have a **Rust implementation** with dedicated tests (CT-09-27 … CT-09-29).
- **RSK-20 (new)** — the private installer leaking into the public repo. Mitigated by ignore rules **and** a verification item, because a single `git add -A` would publish it permanently.
- **RSK-21 (new)** — personal or machine information in the public repo. Mitigated by a repo-local git identity and a grep-based privacy gate (**CT-10-23 … CT-10-27**).

## A.4 Milestone exit criteria summary

| Milestone | Exit criterion | Gate item |
|---|---|---|
| M0 | CI green on a correctly-structured repo | CT-01-17 |
| M1 | `dsh --profile headless` runs in the container | CT-02-26 |
| M2 | Real UI loads through the relay; streaming works | CT-03-19, CT-03-20 |
| M3 | **The ten-step DoD passes and is recorded** | CT-04-61 … CT-04-73 |
| M4 | A new user reaches a session unaided | CT-05-12 |
| M5 | DoD passes on all three platforms; docs truthful | CT-06-01 … CT-06-07, CT-06-18 |
| M6 | The release gate is fully checked | CT-08-01 … CT-08-25 |
| **R** | **Rust core builds in Docker; relay and health verified** | **CT-09 exit** |
| **D** | **README is premium, accurate, and privacy-clean** | **CT-10 exit** |
| **I** | **Installer works unattended and is absent from the repo** | **CT-11 exit** |

## A.5 The three new phases at a glance

| Phase | Scope | Reference | When |
|---|---|---|---|
| **CT-09** | Rust core, crate structure, SDK client, Docker-first workflow | → §P-42, §P-43 | **Parallel with CT-02 onward** |
| **CT-10** | The premium README, visual assets, privacy, accuracy | → §P-45 | After CT-04 (needs real output to depict) |
| **CT-11** | The private installer | → §P-44 | **After the product is complete and validated in Docker** |

## A.6 Sequencing rules introduced by Part VI

| Rule | Reference |
|---|---|
| All Rust builds, tests, and linting happen **in Docker** — the host toolchain is never a requirement | → §P-43.2 |
| Rust lands with **no unresolved rust-analyzer diagnostic** | → §P-42.8 |
| Nothing lands in the repo that reveals personal or machine information | → §P-45.6 |
| The installer is **never** committed — verified, not assumed | → §P-44.3 |
| The installer is built **last**, once the product is proven | → §P-44.1 |

---

**End of checklist.** Every line above references a section of [`PROPOSAL.md`](./PROPOSAL.md). If a referenced section does not clearly specify what to do, the proposal — not the checklist — is the thing to fix.
