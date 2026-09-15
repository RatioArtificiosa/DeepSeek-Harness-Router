<div align="center">

<img src="docs/assets/banner.svg" alt="DeepSeek Harness Router" width="100%">

<br>

**Run more than one DeepSeek Harness at once.**

Each in its own workspace, on its own port, with its own model, its own
sessions and its own settings — reachable from a single local page.

[![License: MIT](https://img.shields.io/badge/License-MIT-4f8cff.svg?style=flat-square)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/core-Rust-f74c00.svg?style=flat-square&logo=rust&logoColor=white)](https://www.rust-lang.org)
[![Harness](https://img.shields.io/badge/harness-0.1.x-37e0c8.svg?style=flat-square)](docs/dsh-compatibility.md)
[![Tests](https://img.shields.io/badge/tests-287%20passing-3ecf8e.svg?style=flat-square)](#quality)
[![Status: working](https://img.shields.io/badge/status-working-3ecf8e.svg?style=flat-square)](#project-status)

<br>

[**Quick start**](#quick-start) · [**Why**](#why-this-exists) · [**What you get**](#what-you-get) · [**How it works**](#how-it-works) · [**FAQ**](#questions-people-actually-ask)

</div>

---

> ### Project status
>
> **Working, and in daily use.**
>
> The router runs multiple harness instances side by side, each with its own
> isolated state, port, workspace and model. The control page and the per-instance
> UI both work end to end in a real browser.
>
> DeepSeek Harness is in developer preview, so its internals move. That coupling
> is confined to a single crate, so an upstream change is a small, local edit
> rather than a rewrite — see [`docs/dsh-compatibility.md`](docs/dsh-compatibility.md).
>
> **Star the repository** to follow what lands next.

---

## The whole thing, in one screen

```text
$ router list --probe

    NAME    PORT   WORKSPACE                          MODEL
-------------------------------------------------------------
*   main    3082   ~/projects/atlas                    deepseek-v4-pro
*   notes   3083   ~/projects/journal                  deepseek-v4-flash

$ router status
2 up  |  0 down

$ router open notes
OK Opened http://127.0.0.1:3090/i/notes/
```

Two agents, two projects, two models, running at the same time. One page reaches
both.

**Open the control page and they are all there:**

```text
$ router serve
OK Control page ready

  →  http://127.0.0.1:3090

  Instances are reachable through this page:
    http://127.0.0.1:3090/i/main
    http://127.0.0.1:3090/i/notes
```

Everything lives on **one origin**. That is not cosmetic — it is the reason a
browser can talk to all of them at once. See
[why one origin matters](#why-one-origin-matters).

---

## Why this exists

[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness) is a powerful
open-source agent — it reads your code, runs commands, edits files, and works
through multi-step tasks on its own.

You get one of it. That is fine until you have more than one thing going.

**You cannot run a second.** Start another and it either fights for the port or,
worse, silently shares state with the first. The harness keeps its settings, its
credentials, its workspace list and its session index in one directory, and its
own documentation is blunt about what happens next:

> *"No cross-process write locking — two processes writing the same unit can
> interleave replacements; writes to the same file use **last-completion wins**."*

So a second instance does not fail loudly. It quietly races the first over the
files that describe your workspaces and your configuration, and whatever is
written last wins.

**And you cannot use two models at once.** The default model is process-wide by
design — the harness documents it as *"one process-wide default."* For a second
model you need a second process, and a second process is exactly what causes the
problem above.

> **DeepSeek Harness Router is the missing layer:** it runs several harnesses side
> by side, gives each one its own isolated state, its own port, its own workspace
> and its own model, and gives you one place to see and manage them all.

---

## What you get

### Many projects, genuinely in parallel

Each instance gets its own state directory. Its settings, its credentials, its
workspace list and its sessions live apart from every other instance's. Two
instances never open the same file, which is what makes "in parallel" safe rather
than merely possible.

### A different model per instance

Model selection is process-wide, so the only way to run two models at once is two
processes. The Router makes that the normal way of working.

### A different workspace per instance

Point each instance at the project it belongs to. Its agent starts there, its
sessions group there, and its history stays separate from the others.

### One page for all of them

A single local origin serves every instance. The page shows port, workspace,
model and live state at a glance, and links straight into each one's own UI.

### Your existing install keeps working

The Router never touches your current harness. It does not read your `~/.dsh`,
does not change your settings, and does not restart your processes. It starts new
instances, on new ports, with new state. Your daily driver on 3080 is untouched.

### It tells you when something is off

Two instances pointed at the same folder is a quiet way to get two agents editing
one tree — the Router notices and says so. So does a port already taken by
something else, a workspace that has been deleted, or a model route that does not
exist.

**It will not touch what it did not start.** If a port is occupied, the Router
names the process holding it and refuses, rather than adopting it and reporting
success.

### Nothing phones home

No telemetry, no account, no analytics. The Router talks to your machine's
harnesses and to nothing else.

---

## Quick start

**Prerequisite:** an installed DeepSeek Harness and a Rust toolchain to build the
router. Nothing else.

```sh
git clone https://github.com/RatioArtificiosa/DeepSeek-Harness-Router.git
cd DeepSeek-Harness-Router
cargo build --release

# one-time setup
./target/release/router init

# a project, a port, a model
./target/release/router add my-project --workspace ~/projects/my-project
```

Then:

```text
router list --probe                  every instance and its live state
router start my-project              start one
router serve                         one page for all of them
router open my-project               open its UI
router stop my-project               stop it, leave the rest alone
router logs my-project               what it said
router doctor                        check everything, change nothing
```

> **Ports start at 3081.** The harness default is 3080, and your existing install
> already has it. The Router leaves it alone and allocates upward from there.

> **`router doctor` reports the harness your instances will actually run** — not
> merely the first `dsh` on `PATH`, which may be a launcher that routes elsewhere.

---

## How it works

```text
                    ~/.deepseek-router/
                    ├── router.yaml            the registry
                    └── instances/
                        ├── rust-refactor/dsh/  ← its own DSH_HOME
                        │     settings.yaml, sessions/, workspace.json …
                        └── py-service/dsh/     ← its own DSH_HOME
                              settings.yaml, sessions/, workspace.json …

   router (Rust)
     ├── allocates a port per instance
     ├── spawns  dsh web --port 3081   with DSH_HOME=…/rust-refactor/dsh
     ├── spawns  dsh web --port 3082   with DSH_HOME=…/py-service/dsh
     └── supervises both, and serves one origin that reaches them all
```

Three things make it work.

**One environment variable does the isolation.** The harness resolves all its
user data from `$DSH_HOME`. Give each instance a different value and their state
is separate by construction — not by convention, and not by hoping.

**One process per instance.** The harness's model default is process-wide, so
separate models require separate processes. That is not a design preference; it
is the only way to satisfy the requirement. The upside is real crash isolation:
one instance falling over does not touch the others.

**One origin serves them all.** The control page and the instances share a single
authority, so the browser treats every instance as the same site.

<details>
<summary><b>Why one origin matters</b></summary>

<br>

This is the least obvious part of the design, and the part that took the longest
to get right.

The harness authenticates its API with a cookie whose **name is derived from the
request authority** — `host:port` — and whose signed payload pins that same
authority. A page served from `127.0.0.1:3082` calling a gateway on
`127.0.0.1:3083` is therefore a different origin in both senses the browser
cares about: the request is blocked as cross-origin, and the cookie would not be
sent even if it were allowed.

The symptom is an error the harness reports as *"failed to fetch gateway"* —
which names the browser's complaint rather than its cause, and sends you looking
at the wrong thing entirely.

Serving every instance under one origin removes the problem at the root. Each
instance is reachable at `/i/<name>/` on the control page's own address. The
proxy connects to the instance's real loopback port, so the harness still sees
`127.0.0.1:<its own port>` as its authority and mints a cookie that matches.

Three details have to be right, and all three fail silently:

- **Root-absolute URLs.** The harness's shell mixes `./assets/…` (fine) with
  `/plugins/??…` and `href="/"`, which would resolve against the gateway and
  404. HTML responses are rewritten so they stay inside the instance.
- **Compressed responses.** Rewriting compressed bytes corrupts them. The relay
  declines `Accept-Encoding` for navigations and confirms the encoding before
  touching a body.
- **Runtime-constructed URLs.** The live agent socket is built in JavaScript from
  the origin, so no response rewrite can reach it. The gateway records which
  instance a browser opened and routes those requests back to it.

</details>

<details>
<summary><b>Where the pieces live</b></summary>

<br>

| Path | Responsibility |
|---|---|
| `crates/router-core` | Errors, configuration, health, workspace path validation |
| `crates/router-relay` | The streaming HTTP/WebSocket proxy behind the single origin |
| `crates/router-dsh` | **The only crate that knows how the harness works** |
| `crates/router-cli` | The `router` binary and the control page |
| `docs/` | Compatibility, security, permissions, troubleshooting |

**The adapter rule:** no crate outside `router-dsh` may depend on harness
specifics. The harness is in developer preview and breaking changes are expected;
confining that coupling to one crate is what keeps an upstream change a localized
edit rather than a rewrite.

</details>

---

## Quality

The things this project is careful about, stated plainly:

| Guarantee | How it is kept |
|---|---|
| Your existing install is never touched | The Router never reads or writes `~/.dsh`; ports start at 3081 and 3080 is never claimed |
| A process the Router did not start is never adopted or stopped | Ownership is proved by a PID recorded at spawn, re-confirmed against the port's real listener |
| A workspace is never deleted | `router rm` unregisters the instance and leaves your project folder alone |
| Two instances never share a state root | One `DSH_HOME` per instance, validated on registration |
| No personal or machine information in this repository | Enforced by the working rules in [`AGENTS.md`](AGENTS.md) |

**287 tests**, `cargo clippy --workspace --all-targets -- -D warnings` clean,
`cargo fmt --check` clean. Every non-obvious decision is commented with *why*,
and the tests that guard a past defect were each confirmed to fail without the
fix — a test that cannot fail proves nothing.

---

## Platform support

| Platform | Status | Notes |
|---|---|---|
| **Windows** | Primary target | Where it is built and run day to day |
| **macOS** | Supported | Same code paths; process and path handling are platform-tested |
| **Linux** | Supported | Same code paths; process and path handling are platform-tested |

**Requirements:** an installed DeepSeek Harness, and Rust to build the router.
The Router adds no runtime of its own beyond the harness it supervises.

---

## Questions people actually ask

<details>
<summary><b>Does this replace DeepSeek Harness?</b></summary>

<br>

No. It runs it. The Router starts, watches and stops harness processes; the
harness remains the thing you actually talk to. Its UI is the UI.

</details>

<details>
<summary><b>Can I use this alongside my existing install?</b></summary>

<br>

Yes, and that is the point. Your existing install keeps its own state in `~/.dsh`
and the Router never reads or writes there. The Router's instances get their own
state directories. They do not see each other.

</details>

<details>
<summary><b>Why do ports start at 3081 instead of 3080?</b></summary>

<br>

3080 is the harness default, and your existing install probably has it. The
Router leaves that alone and allocates upward from 3081.

</details>

<details>
<summary><b>Why one process per instance instead of many workspaces in one?</b></summary>

<br>

Because the model default is process-wide. The harness documents it as "one
process-wide default," so running two models at once requires two processes. The
same is true of credentials and settings, which are single files. Separate
processes give each instance its own everything — and as a bonus, a crash in one
does not touch the others.

</details>

<details>
<summary><b>What happens if something else is already using an instance's port?</b></summary>

<br>

The Router refuses to start that instance, names the process holding the port,
and leaves it alone. It will not adopt a process it did not start, and it will
not stop one either — an instance whose sessions you are actually reading should
never be something the Router guessed at.

</details>

<details>
<summary><b>What happens if two instances point at the same folder?</b></summary>

<br>

The Router warns you at registration. Two agents editing one tree at the same
time is a real hazard, and you should know before it happens rather than after.

</details>

<details>
<summary><b>Do instances share my API key?</b></summary>

<br>

Only if you ask them to. By default each instance has its own credentials file,
so you can use different keys for different projects. If you would rather set one
key once, an instance can share your existing credentials by link, not by copy —
so a key you rotate in one place changes everywhere it is used.

</details>

<details>
<summary><b>What happens to my files if I delete an instance?</b></summary>

<br>

Nothing. `router rm` unregisters the instance and leaves your project folder
completely alone. The Router never deletes a directory you pointed it at.

</details>

<details>
<summary><b>Does it phone home?</b></summary>

<br>

No. There is no telemetry, no analytics, and no account. The only outbound traffic
is what the harness itself causes.

</details>

<details>
<summary><b>Do I need Docker?</b></summary>

<br>

Not to use it. Docker is how this project is developed — a clean room, so that
building and testing never touches a working DeepSeek Harness installation. The
Router itself runs natively.

</details>

<details>
<summary><b>Why is it written in Rust?</b></summary>

<br>

Because the work is process supervision and concurrency: spawning, watching,
restarting and stopping several children, proxying their traffic, and reporting
their state without blocking. That is what a static binary with a real async
runtime is good at.

</details>

---

## Built on

**[DeepSeek Harness](https://github.com/deepseek-ai/deepseek-harness)** — the
open-source agent harness this runs. MIT licensed, under active development.

**[DeepSeek](https://deepseek.com)** — the models and the API.

And the Rust ecosystem: `tokio`, `axum`, `hyper`, `serde`, `clap`, `tracing`.

Docker appears nowhere in the runtime path. It is used only to develop this
project, so that building and testing never touches a working installation.

---

## Contributing

Issues and pull requests are welcome.

Read [`AGENTS.md`](AGENTS.md) first — it carries the working rules, in particular
the constraints that protect a running DeepSeek Harness installation and the
repository's privacy requirements.

Every source file is documented, every public item is typed, and `cargo clippy`
runs with `-D warnings`. A change that leaves a warning is a change that is not
finished.

---

## License

[MIT](LICENSE).

DeepSeek Harness itself is MIT licensed; see
[`docs/dsh-compatibility.md`](docs/dsh-compatibility.md) for the exact pinned
version and third-party notices.
