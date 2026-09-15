# Security

What this system protects, how, and where the protection ends.

Read this before giving the agent access to anything you care about.

---

## The one-paragraph version

The agent runs inside a container. Exactly one directory from your machine is
visible to it, mounted read-write at `/workspace`. Everything it writes for its
own operation goes to a separate volume. It has no Docker socket, no elevated
capabilities, and no path to anything outside that container. Writes outside the
workspace are denied by the kernel and the denial is reported.

---

## What the agent can reach

| Capability | Why it exists |
|---|---|
| Read and write `/workspace` | It is the point of the tool |
| Network egress | Model API calls, package installs, web search |
| Spawn processes | Builds, tests, and shell commands |
| Read its own state in `/data` | Sessions, settings, credentials |

## What it cannot reach

| Denied | How |
|---|---|
| Anything outside `/workspace` on your host | Only one bind mount exists |
| The Docker daemon | The socket is never mounted |
| Root inside the container | `cap_drop: ALL`; the process runs unprivileged |
| Other containers on your machine | Its own Compose network only |
| Privileged mode | Not enabled, in any configuration |

---

## Container hardening

| Control | Setting | Effect |
|---|---|---|
| Capabilities | `cap_drop: ALL` | The default 14 capabilities are removed |
| Privilege escalation | `no-new-privileges: true` | setuid/setgid binaries cannot elevate |
| Root filesystem | `read_only: true` | The image cannot be modified at runtime |
| Writable paths | `/workspace`, `/data`, `/tmp`, `/run` only | Nothing else can be written |
| Seccomp | Docker's default profile | Unmodified — we do not weaken it |
| Network | Published on `127.0.0.1` only | Not reachable from the network by default |
| Docker socket | Not mounted | See below |

---

## Why the Docker socket is never mounted

Mounting the Docker socket is equivalent to giving the container **root on your
machine**. The Docker API can start a privileged container, mount any host path,
and read registry credentials.

Docker's own documentation is unambiguous: the `docker` group grants root-level
privileges, and access to the daemon means access to the host.

This is not theoretical. **CVE-2026-6406** (CVSS 8.8) is a bypass where a
convenience flag added the socket through a path that an isolation layer never
inspected — granting full daemon access and any stored registry credentials.

**Our position:** the socket is not mounted, no convenience flag is used, and CI
asserts the rendered configuration contains no socket reference in any form. If
a future feature genuinely needs daemon access, it requires a separate,
non-default deployment mode, an explicit security review, and it will be
documented here first.

---

## Filesystem confinement

The agent's shell runs under a filesystem policy enforced by the Linux kernel
through **Landlock**, an LSM designed for unprivileged processes to restrict
themselves.

| Mode | Effect |
|---|---|
| `read-only` | No writes except required sinks |
| `workspace-write` | Writes under `/workspace` and a private temp area — **the default** |
| `danger-full-access` | Confinement bypassed |

The default is `workspace-write`, because a coding agent that cannot write files
is not a coding agent. The mount boundary still limits the blast radius to the
one directory you chose. Choose `read-only` if you want stricter behaviour.

### If confinement is unavailable

Confinement can be unavailable — an unusual kernel, a locked-down host. When that
happens the system **fails closed**: confined operations are refused rather than
silently run unconfined.

This is reported, never hidden:

- `/health` includes a `sandbox` check with the reason
- the overall status becomes `degraded`, not `unhealthy`
- the interface shows a banner explaining what is disabled and why
- `./start.sh --doctor` prints the detected posture

Confined operations are not silently downgraded. A security control that quietly
stops applying is worse than one that is absent.

---

## Host-side file ownership

This deserves care, because the honest answer is uneven.

| Platform | Behaviour |
|---|---|
| **Linux** | Container UID/GID maps directly. The launcher aligns it with your user, so files land owned by you |
| **macOS** | Ownership is synthesised by Docker Desktop's file-sharing layer. It is **officially undocumented**, and reports exist of it not matching `chown` |
| **Windows** | Synthesised by the VM file-sharing layer, with similar caveats |

**We therefore promise nothing we cannot control.** Two design decisions follow:

1. The launcher sets UID/GID **only on Linux**, where they are meaningful.
2. **All generated state goes to `/data`, never into your project.** The only
   files whose ownership can surprise you are ones the agent deliberately wrote —
   which is the product working as intended. Caches and generated artifacts never
   appear in your `git status`.

---

## Network exposure

The UI is published on `127.0.0.1` by default: reachable only from this machine.

Exposing it further requires setting `BIND_ADDR=0.0.0.0` **and** listing the
hostnames it will be reached by in `TRUSTED_HOSTS`. The launcher enforces both
together, because setting one without the other produces a confusing access
error rather than a working setup.

> The session cookie is deliberately **not** marked `Secure`, because the shipped
> transport is loopback HTTP. If you expose this beyond loopback without TLS,
> that cookie travels in plaintext.

**Recommended for remote access:** an SSH tunnel. It requires no trust
relaxation and no TLS configuration.

---

## Residual risks

Stated plainly, because a security document that only lists strengths is not
useful.

| Risk | Reality |
|---|---|
| **Credential readability** | The agent can read the container's own credential store. Only put keys there you are willing to have inside a container |
| **Network egress** | The file policy does not govern the network. An agent can send data to a public URL, as any networked tool can |
| **Workspace contents** | Anything you place in the workspace the agent can modify. Use version control |
| **Docker Desktop itself** | The container runs in a VM on macOS and Windows. Isolation is strong, but that VM is a component we do not control |
| **A publicly reachable port** | Exposing the UI beyond loopback without TLS exposes the session cookie |

---

## Reporting a vulnerability

Please open a security advisory rather than a public issue, and include
reproduction steps and the affected version.

---

## Verifying these claims yourself

Every control above is testable. The repository's CI asserts the rendered
Compose configuration contains no privileged mode, no Docker socket, no host
networking, a complete capability drop, and a read-only root filesystem — and the
checklist in `CHECKLIST.md` carries the corresponding acceptance tests.
