# DeepSeek Harness Compatibility

What this project needs from the harness, and how that surface is kept small.

## Version policy

| Field | Value |
|---|---|
| Package | `@deepseek-ai/dsh` |
| **Tested against** | whatever `dsh --version` reports on the machine running the tests |
| **Target** | `0.1.5-rc.1` |
| Node engine | `^22.19.0 \|\| >=24.0.0` |
| Upstream license | MIT |
| Upstream repository | https://github.com/deepseek-ai/deepseek-harness |

**This file deliberately does not pin a version in code.** An earlier revision
of it claimed a specific number as "the exact revision this project is built and
tested against", which was not a claim the project could support: the harness in
daily use on the development machine reports a different version, and nothing in
the build verifies the pin. A compatibility number that is not enforced by a
build is decoration, and a wrong one is worse than none.

What is real instead: `router doctor` reads the installed harness's own
`--version` and reports it, so the answer is always about the machine in front
of you rather than about a document. The integration test
(`crates/router-dsh/tests/isolation.rs`) runs a real harness and will fail on a
genuinely incompatible one.

**Target** records the version this project aims at. It is a direction, not a
guarantee, and it is expected to lag or lead any particular installation.

## Why the surface is kept small

DeepSeek Harness is in **developer preview**, and its own documentation states
that breaking changes are expected. Every interface this project touches is
therefore an interface that can break, so the durable strategy is not to pin
harder but to depend on less.


## Upgrade procedure

1. Install the new harness where the integration tests can see it.
2. Run the integration test — it boots a real harness against its own state root
   and asserts the two instances stay isolated.
3. **Run the relay test suite.** This is the most likely point of breakage: the
   authentication handshake, the WebSocket upgrade, and streaming behaviour are
   the parts we depend on most closely.
4. Run the end-to-end test — does a real session still execute?
5. Complete the manual Windows and macOS checklist.
6. Update the **Target** row above, and note any code change the upgrade needed
   under Known incompatibilities.
7. Commit with the version in the message.

If step 3 fails, the upgrade is **blocked** until the relay is adapted.

## What we depend on

Deliberately a small surface, so upgrades stay cheap.

| Dependency | Kind | Stability |
|---|---|---|
| Profile selection (`--profile web`) | CLI contract | Stable |
| `--host`, `--port`, `--no-open`, `--trusted-host` | CLI contract | Stable |
| `--version` for doctor's report | CLI contract | Stable |
| The startup readiness signal | Documented behaviour | Monitored |
| The SDK wire protocol (JSON-RPC over stdio) | Documented protocol | Monitored |

**Why `--profile web` and not `dsh web`.** The harness accepts `dsh web …` as a
convenience alias and it works, but its own help documents the interface as
`dsh --profile web [options]` and lists only that form under Examples. An alias
can be withdrawn or re-pointed; a documented profile selector is the contract.
The router emits the documented form.

**What we do not depend on:** the session file format, internal plugin APIs, or
any private module path. The harness owns those entirely. We also do not depend
on `--dump-config`, which earlier revisions of this file listed: nothing in the
code calls it.

## Known incompatibilities

None recorded yet. This section is updated whenever an upgrade requires a code
change.

## Downgrade policy

Not supported. Session files may be migrated forward when a newer harness opens
them, and that migration is not reversible. Back up the data volume before
upgrading.
