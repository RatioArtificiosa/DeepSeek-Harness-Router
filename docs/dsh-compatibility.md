# DeepSeek Harness Compatibility

The exact harness revision this project is built and tested against.

## Pinned version

| Field | Value |
|---|---|
| Package | `@deepseek-ai/dsh` |
| **Version** | **0.1.5-rc.1** |
| Pinned in | `docker/Dockerfile` (`ARG DSH_VERSION`) |
| Node engine | `^22.19.0 \|\| >=24.0.0` |
| Upstream license | MIT |
| Upstream repository | https://github.com/deepseek-ai/deepseek-harness |

## Why pinned rather than floating

DeepSeek Harness is in **developer preview**, and its own documentation states
plainly that breaking changes are expected.

Floating the version would mean:

- two users following the same instructions get different runtimes
- an upstream release can change behaviour on your machine without a decision
- a bug becomes unreproducible, because the environment moved underneath it

Pinning makes the runtime a **known-good artifact**. Upgrades are deliberate and
tested, never incidental.

## Upgrade procedure

1. Change `DSH_VERSION` in `docker/Dockerfile`.
2. Run the image smoke tests — does it install and report its version?
3. Run the integration test — does it boot, serve, and mount the workspace?
4. **Run the relay test suite.** This is the most likely point of breakage: the
   authentication handshake, the WebSocket upgrade, and streaming behaviour are
   the parts we depend on most closely.
5. Run the end-to-end test — does a real session still execute?
6. Complete the manual Windows and macOS checklist.
7. Update this file: version, date, and any code change the upgrade required.
8. Commit with the version in the message.

If step 4 fails, the upgrade is **blocked** until the relay is adapted.

## What we depend on

Deliberately a small surface, so upgrades stay cheap.

| Dependency | Kind | Stability |
|---|---|---|
| CLI invocation (`--profile web`, `--port`, `--host`, `--no-open`, `--trusted-host`) | CLI contract | Stable |
| `--dump-config` for composition validation | CLI contract | Stable |
| `--profile headless` for smoke tests | CLI contract | Stable |
| The startup readiness signal | Documented behaviour | Monitored |
| The SDK wire protocol (JSON-RPC over stdio) | Documented protocol | Monitored |
| `GET /health` | **Ours**, not upstream's | — |

**What we do not depend on:** the session file format, internal plugin APIs, or
any private module path. The harness owns those entirely.

## Known incompatibilities

None recorded yet. This section is updated whenever an upgrade requires a code
change.

## Downgrade policy

Not supported. Session files may be migrated forward when a newer harness opens
them, and that migration is not reversible. Back up the data volume before
upgrading.
