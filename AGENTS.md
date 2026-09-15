# Working in this repository

Instructions for anyone — human or agent — making changes here.

---

## What this project is

**DeepSeek Harness Router** runs several DeepSeek Harness instances on one
machine — each on its own port, in its own workspace, with its own model — so
they can be used in parallel without interfering with each other.

The harness is an external dependency. We do not fork it, patch it, or vendor
it. We start it, supervise it, and give each instance its own state.

---

## The five standing rules

These are absolute. They override convenience, speed, and every other
consideration.

### 1. Never touch a running DeepSeek Harness installation

A developer will already have a harness in daily use on the machine they are
working on. This project must coexist with it and never disturb it.

**Never** modify, move, upgrade, or delete an existing `dsh` installation, its
state root (`~/.dsh` by default), its settings, its credentials, its profiles,
or its sessions.

**Never** start a router instance pointing at the existing state root. Each
instance gets its own directory under the router home.

### 2. Never claim port 3080

3080 is the harness default and is very likely in use by the installation this
project is meant to coexist with. Port allocation starts at **3081** and goes up.

A candidate port is confirmed by an actual **bind test**. A port listing is a
hint, never proof — another process can take a port between the check and the
use.

### 3. One state root per instance

Every instance runs with its own `DSH_HOME`. This is not a convention; it is the
product.

The harness has **no cross-process write locking** — its own documentation says
two processes writing the same unit produce *"last-completion wins"* behaviour,
and that a cross-process session lease is not yet implemented. Two instances
sharing a state root is therefore a data-loss bug waiting to happen, and this
project must never create that situation.

**Never** point two instances at the same state root. **Never** introduce a code
path that writes into another instance's directory.

### 4. Never modify the working directory of an instance

An instance's workspace belongs to its user. The router reads the path, starts
the harness there, and leaves it alone.

**Never** write generated files, caches, logs, or configuration into an
instance's workspace. All router-owned state lives under the router home.

**Never** delete a workspace directory. Not on `router rm`, not on uninstall,
not on reset. A path a user handed us is not ours to remove.

### 5. Never disturb other Docker workloads

Docker is this project's development laboratory, not its runtime. Even so, the
machine running the lab may be running unrelated containers.

- **Never** run `docker system prune`, `docker volume prune`,
  `docker image prune`, or `docker network prune`.
- **Never** stop, remove, or restart a container this project did not create.
- **Never** delete or rename a volume this project does not own.

### 6. Test harnesses use a dedicated port range, and are always stopped

**Automated or exploratory testing must never start a harness on a port a person
might be using.** The default instance ports (`3081+`) are exactly where a real
user's second instance would be, so a test that allocates there can collide with
live work — and, before the identity check was added, could have killed it.

- Test instances allocate from **3400x–3499x only**. Never 3080, 3081, or the
  low `30xx` range.
- **Every start is paired with a stop in the same run.** Use a `try`/`finally`,
  so a failing assertion still stops the harness. A test that leaks a harness
  leaves a process the user can see on their machine but did not create.
- **Before finishing any session that started instances, check for leftovers:**
  list every `node` process whose command line contains `--profile web` (or
  `dsh` and `web`), and verify each is one you meant to leave running. Report
  and stop anything you left behind.
- **Stop by PID after confirming identity**, never by port. Killing by port is
  what the `stop`/`rm` identity check exists to prevent.

This rule exists because it was broken: a session of CLI testing left four
harnesses running on ports 3082–3085, visible to the user as unexplained
services. The failure was not the tool's — it was a person killing the
supervising process and assuming the harness went with it.


---

## The privacy rule

**No personal or machine-specific information appears anywhere in this
repository** — not in code, docs, comments, images, examples, or commit
metadata.

| Prohibited | Example of what not to write |
|---|---|
| Personal names | any real name |
| Email addresses | any address |
| Machine details | hostnames, usernames, drive letters, local paths |
| Real project names | internal or client project identifiers |
| Screenshots of real work | anything containing personal data |

Use generic placeholders: `/workspace`, `~/projects/my-app`, `127.0.0.1`.

Commit identity is set **repo-locally** so the machine's global git identity
never leaks into public history:

```sh
git config --local user.name  "DeepSeek Harness Router"
git config --local user.email "router@users.noreply.github.com"
```

---

## The private-files rule

Two classes of file are deliberately **absent from this repository** and
excluded by `.gitignore`:

| Path | What it is |
|---|---|
| `PROPOSAL.md`, `CHECKLIST.md` | Authoring documents for this machine's working session |
| `installer/`, `*.local.*` | The owner's personal installation tooling |

If you find yourself staging anything from those paths, **stop**. Git history is
effectively immutable once pushed.

---

## Everything builds in Docker

Docker is how this project is developed — a clean room, so that building and
testing never touches a working harness installation. The host is a place to
edit files and run `docker`; it is never a prerequisite for building.

```sh
docker compose -f docker-compose.dev.yml build
docker compose -f docker-compose.dev.yml run --rm dev cargo test
docker compose -f docker-compose.dev.yml run --rm dev cargo clippy -- -D warnings
```

A local Rust toolchain and rust-analyzer are a **convenience for fast feedback in
an editor**, not a build dependency. CI and release builds use container
toolchains only.

---

## The Rust verification loop

No change lands with an unresolved diagnostic.

```text
write  ->  cargo check                  ->  rust-analyzer diagnostics
       ->  cargo clippy -- -D warnings
       ->  cargo test
       ->  cargo fmt --check
       ->  commit
```

`cargo clippy -- -D warnings` is a gate, not advice. A warning is a failure.

---

## Where things live

| Path | Owns |
|---|---|
| `crates/router-core` | Errors, configuration, health, workspace path validation |
| `crates/router-relay` | The streaming HTTP proxy used by the control page |
| `crates/router-dsh` | **The only crate that knows how the harness works** |
| `crates/router-cli` | The `router` binary and its subcommands |
| `docs/` | Compatibility, security, permissions, troubleshooting |

**The adapter rule:** no crate outside `router-dsh` may depend on harness
specifics. The harness is in developer preview and breaking changes are
expected; confining that coupling to one crate is what keeps an upstream change
a localized edit rather than a rewrite.

---

## Before you change something

1. Decide whether the change alters behaviour a user would notice.
2. If it does, write the reasoning down — in the commit message, at minimum.
3. If a comment can explain *why* rather than *what*, prefer that.

A comment restating the code is noise. A comment explaining why a surprising
choice was made is the most valuable thing in the file.
