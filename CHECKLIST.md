# DeepSeek Harness Router — Execution Checklist

> **Companion to:** [`PROPOSAL.md`](./PROPOSAL.md) · **Version:** 2.0.0
> **Repository:** <https://github.com/RatioArtificiosa/DeepSeek-Harness-Router>

---

## Read this first

**The product changed.** The checklist tracked a Docker-packaged public
application. It now tracks a **local multi-instance router**: several DeepSeek
Harness instances on one machine, each on its own port, workspace, and model.

**Phases CT-R1 through CT-R6 below are the current plan.** Everything after them
is the superseded 1.x plan, retained so the reasoning survives.

### The four standing rules

Absolute, and they override convenience:

| Rule | Source |
|---|---|
| **Never touch the owner's existing harness install** — no writes to `~/.dsh`, no changes to its settings or profiles, no restarting its processes | → §P-04.1 |
| **Never disturb other Docker workloads** — no prune commands, no stops outside our project | → §P-04.2 |
| **The router never claims port 3080** — it starts at 3081 | → §P-50.4 |
| **Every failure names its cause and its fix** | → §P-26.1 |

---

# PHASE CT-R1 — Router foundation

> Establish the registry, the instance model, and the router home.

→ §P-50, §P-52

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R1-01** Define the router home layout under `~/.deepseek-router/` | → §P-50.1 |
| `[ ]` | **CT-R1-02** Define the `router.yaml` schema: instances, ports, workspaces, models | → §P-52.1 |
| `[ ]` | **CT-R1-03** Implement `router init` — create the home and an empty registry | → §P-51.1 |
| `[ ]` | **CT-R1-04** Make registry writes atomic (temp file, fsync, rename) | → §P-49.3 |
| `[ ]` | **CT-R1-05** Version the registry schema so a future change can migrate | → §P-52.1 |
| `[ ]` | **CT-R1-06** Validate the registry on load; fail loudly on a malformed file | → §P-26.1 |
| `[ ]` | **CT-R1-07** Reject an instance name that is not filesystem-safe | → §P-52.2 |
| `[ ]` | **CT-R1-08** Expand `~` in workspace paths and canonicalize with `realpath` | → §P-49.4 |
| `[ ]` | **CT-R1-09** Reuse `router-core`'s workspace validation for every configured path | → §P-53.3 |
| `[ ]` | **CT-R1-10** Refuse to register a workspace that does not exist | → §P-49.4 |

**Phase exit:** `router init` produces a valid registry, and a malformed registry fails with a clear message.

---

# PHASE CT-R2 — Instance lifecycle

> The core capability: start, isolate, supervise, stop.

→ §P-50, §P-52

## CT-R2.A — Isolation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R2-01** Create a per-instance `DSH_HOME` under `instances/<name>/dsh` | → §P-50.1 |
| `[ ]` | **CT-R2-02** Pass it to the child as the `DSH_HOME` environment variable | → §P-49.6 |
| `[ ]` | **CT-R2-03** Set the child's working directory to the instance's workspace | → §P-52.3 |
| `[ ]` | **CT-R2-04** Write the instance's `settings.yaml` with its model **before first boot** | → §P-52.2 |
| `[ ]` | **CT-R2-05** Create the instance's `.credentials.yaml` empty unless sharing is enabled | → §P-52.4 |
| `[ ]` | **CT-R2-06** Implement `share_credentials` as a **symlink**, never a copy | → §P-52.4 |
| `[ ]` | **CT-R2-07** **Verify** two instances never open the same state file | → §P-49.3 |
| `[ ]` | **CT-R2-08** **Verify** neither instance writes into the host's `~/.dsh` | → §P-04.1 |

## CT-R2.B — Port allocation

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R2-09** Start allocation at **3081**; never claim 3080 | → §P-50.4 |
| `[ ]` | **CT-R2-10** Confirm a candidate port with an actual **bind test**, not a port listing | → §P-50.4 |
| `[ ]` | **CT-R2-11** Remember each instance's port across restarts | → §P-50.4 |
| `[ ]` | **CT-R2-12** If a remembered port is taken, report it, reassign, and say so | → §P-50.4 |
| `[ ]` | **CT-R2-13** Release the port on stop but retain the assignment | → §P-50.4 |

## CT-R2.C — Supervision

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R2-14** Spawn `dsh web --host 127.0.0.1 --port <n> --no-open` | → §P-49.6 |
| `[ ]` | **CT-R2-15** Wait on the harness's **readiness signal**, never a fixed sleep | → §P-49.6 |
| `[ ]` | **CT-R2-16** Confirm readiness with a probe as well as the signal | → §P-49.6 |
| `[ ]` | **CT-R2-17** Capture stdout and stderr; keep a bounded tail | → §P-11.4 |
| `[ ]` | **CT-R2-18** Supervise **many** children, not one — extend the existing supervisor | → §P-53.3 |
| `[ ]` | **CT-R2-19** Record each child's PID in the registry | → §P-54 (RSK-33) |
| `[ ]` | **CT-R2-20** Implement graceful stop with a timeout, then force | → §P-51.1 |
| `[ ]` | **CT-R2-21** Reap orphans on startup: adopt or kill a harness whose router died | → §P-54 (RSK-33) |
| `[ ]` | **CT-R2-22** Keep instances independent: one crash must not affect the others | → §P-50.2 |

## CT-R2.D — Multi-instance proof (the milestone gate)

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R2-23** Two instances run simultaneously on different ports | → §P-55 step 2–3 |
| `[ ]` | **CT-R2-24** Each UI opens and shows **its own workspace** | → §P-55 step 4 |
| `[ ]` | **CT-R2-25** Each runs **its own model** | → §P-55 step 5 |
| `[ ]` | **CT-R2-26** Each has its own session list; neither sees the other's | → §P-55 step 6 |
| `[ ]` | **CT-R2-27** **A session written in A leaves B's `workspace.json` byte-identical** | → §P-55 step 7 |
| `[ ]` | **CT-R2-28** Stopping A leaves B serving | → §P-55 step 9 |
| `[ ]` | **CT-R2-29** Restarting A reclaims its original port | → §P-55 step 10 |
| `[ ]` | **CT-R2-30** The owner's install on 3080 is untouched throughout | → §P-55 step 11 |
| `[ ]` | **CT-R2-31** Record the passing run as evidence | → §P-55 |

> **CT-R2-27 is the acceptance test for the whole design.** It is the property
> the harness cannot provide alone and the reason this product exists.

---

# PHASE CT-R3 — Control surfaces

> The owner asked for "some kind of control area."

→ §P-51

## CT-R3.A — CLI

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R3-01** `router list` — every instance with port, workspace, model, state | → §P-51.1 |
| `[ ]` | **CT-R3-02** `router add <name> --workspace <dir> [--model <m>]` | → §P-51.1 |
| `[ ]` | **CT-R3-03** `router start` / `stop` / `restart <name>` | → §P-51.1 |
| `[ ]` | **CT-R3-04** `router open <name>` — open that instance's UI | → §P-51.1 |
| `[ ]` | **CT-R3-05** `router logs <name> [-f]` | → §P-51.1 |
| `[ ]` | **CT-R3-06** `router status` — summary; non-zero exit if any instance is down | → §P-51.1 |
| `[ ]` | **CT-R3-07** `router rm <name>` — unregister, **never** delete a workspace | → §P-51.1 |
| `[ ]` | **CT-R3-08** `router doctor` — diagnostics that change nothing | → §P-51.1 |
| `[ ]` | **CT-R3-09** Confirm `rm` cannot delete a user directory under any flag | → §P-51.1 |

## CT-R3.B — Control page

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R3-10** Serve a single page listing every instance | → §P-51.2 |
| `[ ]` | **CT-R3-11** Show name, port, workspace, model, state, uptime per instance | → §P-51.2 |
| `[ ]` | **CT-R3-12** Provide open / stop / restart / logs per row | → §P-51.2 |
| `[ ]` | **CT-R3-13** Serve it from the router's own port, separate from the instances | → §P-51.2 |
| `[ ]` | **CT-R3-14** Reuse `router-relay`'s streaming proxy rather than writing a new server | → §P-53.3 |
| `[ ]` | **CT-R3-15** Do **not** reimplement an agent UI — the harness GUI is the UI | → §P-51.2 |

---

# PHASE CT-R4 — Safety and honesty

→ §P-54

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R4-01** Detect two instances pointed at the same workspace and **warn** | → §P-54 (RSK-31) |
| `[ ]` | **CT-R4-02** Report per-instance memory usage | → §P-54 (RSK-32) |
| `[ ]` | **CT-R4-03** Refuse to start beyond a configurable instance ceiling | → §P-54 (RSK-32) |
| `[ ]` | **CT-R4-04** Validate the configured model route exists; report it in `doctor` | → §P-54 (RSK-35) |
| `[ ]` | **CT-R4-05** Survive router death: running instances keep serving | → §P-54 (RSK-34) |
| `[ ]` | **CT-R4-06** Recover the router's view from the registry and PID checks | → §P-54 (RSK-34) |
| `[ ]` | **CT-R4-07** Document the shared-credentials trade-off honestly | → §P-52.4 |

---

# PHASE CT-R5 — Docker lab workflow

> Docker is how we build, never what we ship.

→ §P-48.3

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R5-01** Dev container with the Rust toolchain and a Node runtime for testing | → §P-43 |
| `[ ]` | **CT-R5-02** Install a harness **inside the lab** for integration testing | → §P-48.3 |
| `[ ]` | **CT-R5-03** Run the multi-instance test entirely inside the lab | → §P-48.3 |
| `[ ]` | **CT-R5-04** Confirm the lab never mounts the owner's `~/.dsh` | → §P-04.1 |
| `[ ]` | **CT-R5-05** Confirm no container is part of the product's runtime path | → §P-48.4 |
| `[ ]` | **CT-R5-06** Build the `router` binary for the host from the lab | → §P-48.3 |

---

# PHASE CT-R6 — Native delivery

> The private installer, now a first-class deliverable.

→ §P-44, §P-48.3

| # | Item | Reference |
|---|---|---|
| `[ ]` | **CT-R6-01** Build the `router` binary for the host platform | → §P-48.3 |
| `[ ]` | **CT-R6-02** Install it to a stable location and add it to `PATH` | → §P-44.4 |
| `[ ]` | **CT-R6-03** Confirm it runs unattended, with no assistant present | → §P-44.2 |
| `[ ]` | **CT-R6-04** Verify the owner's existing install is untouched after install | → §P-04.1 |
| `[ ]` | **CT-R6-05** Prove the router never claims port 3080 | → §P-50.4 |
| `[ ]` | **CT-R6-06** Confirm the installer stays out of the public repository | → §P-44.3 |
| `[ ]` | **CT-R6-07** Run the full §P-55 definition of done on the real machine | → §P-55 |

---

# SUPERSEDED PLAN (1.x)

> The phases below tracked the Docker-packaged public application. They are
> retained for the items that remain useful, each marked in place.
>
> **Do not execute a superseded item.** They are recorded so the reasoning
> survives, not because the work is wanted.

| Phase | 1.x scope | Disposition |
|---|---|---|
| CT-00 | Pre-flight, governance | **Partly useful** — the environment survey and the standing rules still apply |
| CT-01 | Repository foundation | **Done** |
| CT-02 | Dockerized skeleton | **Repurposed** → CT-R5 (the lab) |
| CT-03 | Web surface and relay | **Partly useful** — `router-relay` survives; the container framing does not |
| CT-04 | Launchers and workspace mount | **Superseded** → CT-R2 |
| CT-05 | Onboarding UI | **Superseded** → CT-R3 |
| CT-06 | Cross-platform proof | **Partly useful** — the isolation checks still apply |
| CT-07 | Hardening, docs, release | **Partly useful** — the security and privacy gates still apply |
| CT-08 | Release gate | **Superseded** → §P-55 |
| CT-09 | Rust core | **Active** — `router-core` and `router-relay` are done and retained |
| CT-10 | Premium README | **Active** — tone kept, Docker details to be replaced |
| CT-11 | Private installer | **Promoted** → CT-R6 |

---

## Appendix A — Requirement mapping

| Requirement (owner's words) | Where it is satisfied |
|---|---|
| *"open more than one instance"* | §P-50.2, CT-R2-23 |
| *"one on port 3080, 3081, 3082"* | §P-50.4, CT-R2-09 |
| *"do not take the same session id"* | §P-49.3 — IDs are uuid v4; the real risk is shared files, solved by per-instance `DSH_HOME` |
| *"or write to the same area"* | §P-50.1, CT-R2-07 |
| *"or get in the way of each other"* | §P-50.2, CT-R2-22 |
| *"different workspaces"* | §P-52.3, CT-R2-24 |
| *"connect to different workspaces and different models"* | §P-49.5, CT-R2-25 |
| *"some kind of control area"* | §P-51, CT-R3 |
| *"run different models and/or workspaces in parallel"* | §P-50.2, CT-R2-25 |
| *"docker as a clean lab only"* | §P-48.3, CT-R5 |

## Appendix B — The evidence behind the design

| Claim | Source | Verified |
|---|---|---|
| State root is `$DSH_HOME`, else `~/.dsh` | `dsh-home-paths` README | ✅ |
| No cross-process write locking; last-completion wins | `dsh-storage-json` README | ✅ |
| Cross-process session lease not yet implemented | `dsh-session-persistence` README | ✅ |
| Single-owner SQLite derived index | `dsh-session-query-sqlite` README | ✅ |
| Model default is process-wide | `dsh-agent-default-model` README | ✅ |
| Workspace identity is a uuid over canonical `realpath` | `dsh-workspace` README | ✅ |
| Workspace removal never deletes data | `dsh-workspace` README | ✅ |
| `--port` / `--host` / `--no-open` / `--trusted-host` | `dsh web --help` | ✅ |
| Two live processes share one `~/.dsh` | Live process inspection | ✅ |

---

**End of checklist.** Every line cites the section of [`PROPOSAL.md`](./PROPOSAL.md) that specifies it.
