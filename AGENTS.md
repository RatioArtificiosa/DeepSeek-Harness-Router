# Working in this repository

Instructions for anyone — human or agent — making changes here.

---

## The four standing rules

These are absolute. They override convenience, speed, and every other consideration.

### 1. Never touch a host DeepSeek Harness installation

A developer may already have DeepSeek Harness installed on the machine they are
working on. This project must coexist with it and never disturb it.

**Never** modify, move, upgrade, or delete an existing `dsh` installation, its
`$DSH_HOME` (`~/.dsh`), its settings, credentials, profiles, or sessions.

The container gets its **own** `$DSH_HOME` at `/data/dsh`. That separation is
what makes coexistence possible. See `PROPOSAL.md` §P-04.1 and §P-05.

### 2. Never touch Docker resources you did not create

The same machine may be running unrelated containers, volumes, and networks.

- **Never** run `docker system prune`, `docker volume prune`,
  `docker image prune`, or `docker network prune`.
- **Never** stop, remove, or restart a container this project did not create.
- **Never** delete or rename a volume this project does not own.

Every resource we create is namespaced by `COMPOSE_PROJECT_NAME`. Any Docker
command whose blast radius is not provably inside that namespace is forbidden.
See `PROPOSAL.md` §P-04.2.

### 3. Only `/workspace` and `/data`

The container mounts exactly one host directory, at `/workspace`, chosen
explicitly by the user. Application state lives in the named volume at `/data`.
There is no Docker socket, no host networking, and no broad mount — ever.
See `PROPOSAL.md` §P-04.4 and §P-17.3 (T-13).

### 4. Nothing outside `/data` and `/workspace` is written at runtime

We never write generated files into the user's project directory. Ownership of
container-written files on macOS and Windows is not something we control, so we
never create the problem. See `PROPOSAL.md` §P-19.3.1 and §P-22.10.

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

Use generic placeholders: `/workspace`, `localhost`, `/Users/you/projects/my-app`.

Commit identity is set **repo-locally** so the machine's global git identity
never leaks into public history:

```sh
git config --local user.name  "DeepSeek Harness Router"
git config --local user.email "router@users.noreply.github.com"
```

See `PROPOSAL.md` §P-41.3 and §P-45.6.

---

## The private installer rule

`installer/` is **deliberately absent from this repository**. It is a personal
installation tool and is excluded by `.gitignore`.

If you ever find yourself staging anything under `installer/`, `*.local.*`, or
`router-image.tar`, **stop**. Git history is effectively immutable once pushed.
See `PROPOSAL.md` §P-44.

---

## Everything happens in Docker

The host is a place to edit files and run `docker`. It is never a prerequisite
for building, testing, or running the project. See `PROPOSAL.md` §P-43.

```sh
# Build the development image once
docker compose -f docker-compose.dev.yml build

# Rust: check, lint, test — all inside the container
docker compose -f docker-compose.dev.yml run --rm dev cargo check
docker compose -f docker-compose.dev.yml run --rm dev cargo clippy -- -D warnings
docker compose -f docker-compose.dev.yml run --rm dev cargo test

# Run the product
docker compose up --build
```

A local Rust toolchain and rust-analyzer are a **convenience for fast feedback
in an editor**, not a build dependency. CI and the release build use container
toolchains only.

---

## The Rust verification loop

No Rust change lands with an unresolved diagnostic.

```text
write  ->  cargo check         ->  rust-analyzer diagnostics
       ->  cargo clippy -D warnings
       ->  cargo test
       ->  cargo fmt --check
       ->  commit
```

`cargo clippy -- -D warnings` is a gate, not advice. A warning is a failure.
See `PROPOSAL.md` §P-42.8.

---

## Where things live

| Path | Owns |
|---|---|
| `crates/router-core` | Supervision, health, config, workspace validation |
| `crates/router-relay` | The HTTP / WebSocket / SSE loopback relay |
| `crates/router-dsh` | **The only crate that knows about DeepSeek Harness** |
| `crates/router-cli` | The single static binary and its subcommands |
| `docker/` | Dockerfiles, entrypoint, healthcheck |
| `docs/` | Architecture, security, permissions, troubleshooting |
| `docs/research/` | Source conversation and sourced technical briefs |
| `docs/adr/` | One decision per file, with rejected alternatives |
| `PROPOSAL.md` | The specification. Read the cited section before changing code |
| `CHECKLIST.md` | The execution plan. Every line cites a proposal section |

**The adapter rule:** no crate outside `router-dsh` may depend on
DeepSeek Harness specifics. Confining that coupling to one crate is what keeps a
breaking upstream change a localized edit. See `PROPOSAL.md` §P-16.3.

---

## Before you change something

1. Find the section of `PROPOSAL.md` that specifies it.
2. Find the checklist line in `CHECKLIST.md` that tracks it.
3. If either is missing or wrong, **fix the document first**, then the code.

A change that contradicts the proposal is either a bug or a proposal that needs
updating. Decide which, explicitly, and record the decision.
