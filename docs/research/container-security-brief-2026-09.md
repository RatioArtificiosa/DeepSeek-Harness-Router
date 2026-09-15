# Container Hardening & Landlock — Sourced Technical Brief

**Compiled:** ~September 2026 · **Method:** direct `web_fetch` + `Invoke-WebRequest` + GitHub REST/NVD/OSV APIs. `web_search` was not used (broken in this environment).

> **Three brief premises were wrong and are corrected below:** (1) the seccomp JSON is no longer in `moby/moby`; (2) Landlock is at ABI **v11**, not v6; (3) Docker Engine is at **v29.8.0**, not v28.x.

---

## AREA A — Docker socket / privileged containers

### A1. Documented risk of mounting `/var/run/docker.sock`

There is **no single docs page titled "docker socket is root"**, but the equivalent warnings are on three pages, all fetched:

- **`engine/security/` (Docker Engine security)** — "Docker daemon attack surface" section. States the daemon **"requires `root` privileges unless you opt-in to [Rootless mode]"**, that **"only trusted users should be allowed to control your Docker daemon"**, and that sharing a directory **"without limiting the access rights of the container"** means **"the container can alter your host filesystem without any restriction."** It also notes the API **"can be still accessible from containers, and it can easily result in the privilege escalation"**, and mandates HTTPS+certificates — **"Exposing the daemon API over HTTP without TLS is not permitted"** (daemon now fails early at startup).
- **`engine/install/linux-postinstall.md`** — the sharpest statement: **"The `docker` group grants root-level privileges to the user."** And: *"A group password reduces ambient access to the Docker socket, but it doesn't reduce the root-level privileges granted after access is authorized."* Membership gives "every process in your login session" socket access, inherited by descendants.
- **`engine/security/protect-access.md`** — for TLS keys: **"anyone with the keys can give any instructions to your Docker daemon, giving them root access to the machine hosting the daemon. Guard these keys as you would a root password!"**

**Why the socket = root, mechanically (from the docs):** the API exposes `docker run` with arbitrary `--privileged`, host mounts (`-v /:/host`), and volume/bind control — so socket access is equivalent to unauthenticated root command execution on the host, *bypassed entirely if the container itself is privileged*.

Confirmed CLI flag: **`--use-api-socket`** ("Bind mount Docker API socket and required auth") is a first-class `docker run` flag in the current CLI reference — not just a manual `-v`.

### A2. Current official hardening guidance

All confirmed present in the current `docker run` reference and security docs:

| Control | Flag | Status in docs |
|---|---|---|
| Capability drop | `--cap-drop` / `--cap-add` | "Best practice … remove all capabilities except those explicitly required" |
| No privilege escalation | `--security-opt="no-new-privileges=true"` | "Disable container processes from gaining new privileges" |
| Seccomp | `--security-opt="seccomp=builtin"` / `=unconfined` / `=profile.json` | `builtin` explicitly re-enables the default profile on a daemon with a custom/unconfined default |
| AppArmor | `--security-opt="apparmor=PROFILE"` | `docker-default`, auto-generated into tmpfs and loaded by the binary |
| SELinux | `--security-opt="label=..."` | — |
| Read-only rootfs | `--read-only` | "Mount the container's root filesystem as read only"; docs pair it with volumes: `docker run --read-only -v /icanwrite busybox touch /icanwrite/here` |
| Writable scratch | `--tmpfs` | "Mount a tmpfs directory" |
| Userns | `--userns` | Supported since Docker 1.10; **not enabled by default** |
| Rootless | install via `dockerd-rootless-setuptool.sh` | Daemon **and** containers run without root; prereqs `newuidmap`/`newgidmap` + ≥65,536 subordinate UIDs/GIDs in `/etc/subuid`/`subgid` |

**Read-only rootfs caveat confirmed:** `--read-only` "prohibit[s] writes to locations other than the specified volumes" — so writable paths must come from `--tmpfs` or explicit volumes, not from the rootfs.

**AppArmor default profile is now customisable (new in 29.8):** `apparmor-profile` in `daemon.json` (or `dockerd --apparmor-profile=`) points at a Go `text/template`-rendered template; verify with `docker info --format '{{json .SecurityOptions}}'`. Docs carry a **CAUTION: "A custom template can reduce container isolation."**

### A3. DEFAULT seccomp profile — location changed, contents extracted

**⚠️ The URL in the brief is dead.** `raw.githubusercontent.com/moby/moby/master/profiles/seccomp/default.json` → **HTTP 404**. The profile was split into a **separate repo `moby/profiles`** and is vendored back into moby at `vendor/github.com/moby/profiles/seccomp/default.json`.

**Canonical source:** [`moby/profiles/main/seccomp/default.json`](https://raw.githubusercontent.com/moby/profiles/main/seccomp/default.json) — fetched successfully, **13,470 bytes**, repo last pushed **2026-08-28**.

Extracted facts:

- **`"defaultAction": "SCMP_ACT_ERRNO"`, `"defaultErrnoRet": 1`** → denial yields `EPERM`. Allowlist model.
- **`archMap`: 9 architectures** — x86_64, aarch64, mips64, mips64n32, mipsel64, mipsel64n32, s390x, riscv64, loongarch64.
- **33 syscall groups.** Group 0 = **361 syscall names allowed unconditionally** (`SCMP_ACT_ALLOW`, no capability gate) — this is the real allowlist.
- Docs claim the profile "disables around **44** system calls out of 300+".
- **Important:** `socketcall` (32-bit) **is** allowlisted in group 0. This is exactly the bypass the 29.8.0 notes call out ("These do not cover `socketcall(2)`, which can be easily bypassed") and which 29.8.0 mitigated via LSM rules instead.

**NOTABLE BLOCKED / EPERM-by-default syscalls** (absent from the allowlist — the docs table is authoritative for rationale):

`acct`, `add_key`, `bpf`, `clock_adjtime`, `clock_settime`, `create_module`, `delete_module`, `finit_module`, `get_kernel_syms`, **`io_uring_enter` / `io_uring_register` / `io_uring_setup`** (blocked for container-escape CVEs, [moby/moby#46762](https://github.com/moby/moby/pull/46762)), `kexec_load`/`kexec_file_load`, `keyctl`, `mount`, `move_pages`, `nfsserverctl`, `open_by_handle_at`, `perf_event_open`, `pivot_root`, `ptrace`, `query_module`, `quotactl`, `reboot`, `request_key`, `setns`, `settimeofday`, `stime`, `swapon`/`swapoff`, `sysfs`, `_sysctl`, `umount`/`umount2`, **`unshare`**, `uselib`, `userfaultfd`, `ustat`, `vm86`/`vm86old`.

**Capability-gated groups (13)** — syscalls allowed only with the matching cap:

| Syscall group | Required capability |
|---|---|
| `open_by_handle_at` | `CAP_DAC_READ_SEARCH` |
| `bpf`, `clone`, `clone3`, `fanotify_init`, `fsconfig`, `fsmount`, `fsopen`, `fspick`, `lookup_dcookie`, `lsm_get_self_attr`, `lsm_list_modules`, `lsm_set_self_attr`, `mount`, `mount_setattr`, `move_mount`, `open_tree`, `perf_event_open`, `quotactl`, `quotactl_fd`, `setdomainname`, `sethostname`, `setns`, `syslog`, `umount`, `umount2`, `unshare` | `CAP_SYS_ADMIN` |
| `reboot` | `CAP_SYS_BOOT` |
| `chroot` | `CAP_SYS_CHROOT` |
| `delete_module`, `init_module`, `finit_module` | `CAP_SYS_MODULE` |
| `acct` | `CAP_SYS_PACCT` |
| `kcmp`, `pidfd_getfd`, `process_madvise`, `process_vm_readv`, `process_vm_writev`, `ptrace` | `CAP_SYS_PTRACE` |
| `iopl`, `ioperm` | `CAP_SYS_RAWIO` |
| `settimeofday`, `stime`, `clock_settime`, `clock_settime64` | `CAP_SYS_TIME` |
| `vhangup` | `CAP_SYS_TTY_CONFIG` |
| `get_mempolicy`, `mbind`, `set_mempolicy`, `set_mempolicy_home_node` | `CAP_SYS_NICE` |
| `syslog` | `CAP_SYSLOG` |
| `bpf` | `CAP_BPF` |
| `perf_event_open` | `CAP_PERFMON` |

**Socket domain filtering (args-based, 3 groups):** `socket` is allowed only when the domain argument is `< 38` (group 2), `== 39` (group 3), or `> 40` (group 4). **38 = `AF_ALG`, 39 = `AF_NETLINK`, 40 = `AF_VSOCK`.** So **`AF_ALG` and `AF_VSOCK` are specifically blocked** while other domains pass. Per the docs, `AF_ALG` is blocked for **CVE-2026-31431** (in-container privilege escalation via the kernel crypto API) and `AF_VSOCK` for host↔VM communication; `AF_ALG` is additionally denied by `deny network alg,` in the default AppArmor profile, whereas **`AF_VSOCK` has no equivalent LSM rule for the `socketcall(2)` path** ([moby/moby#52494](https://github.com/moby/moby/pull/52494)).

### A4. Default container capabilities — exactly **14**

From [`moby/moby/daemon/pkg/oci/caps/defaults.go`](https://raw.githubusercontent.com/moby/moby/master/daemon/pkg/oci/caps/defaults.go), an allowlist (not a denylist):

`CAP_CHOWN`, `CAP_DAC_OVERRIDE`, `CAP_FSETID`, `CAP_FOWNER`, `CAP_MKNOD`, `CAP_NET_RAW`, `CAP_SETGID`, `CAP_SETUID`, `CAP_SETFCAP`, `CAP_SETPCAP`, `CAP_NET_BIND_SERVICE`, `CAP_SYS_CHROOT`, `CAP_KILL`, `CAP_AUDIT_WRITE`.

Note `CAP_SYS_CHROOT`, `CAP_SETUID`/`CAP_SETGID`/`CAP_SETPCAP` are *in* the default set; `CAP_SYS_ADMIN`, `CAP_SYS_PTRACE`, `CAP_BPF`, `CAP_PERFMON` are **not**.

### A5. Current Docker Engine version (Sept 2026)

**Latest stable: `v29.8.0`, published 2026-09-03** (tag `docker-v29.8.0`, 22:31 UTC).
Bundled: **API 1.56.0**, BuildKit **v0.33.0**, containerd **v2.3.4**, runc **v1.5.1**, Go **1.26.8**, RootlessKit **v3.1.0**.
Other current lines: `v29.7.2` (2026-08-06), `v29.6.2` (2026-07-16), and the maintained **`v25.0.17`** LTS backport (2026-08-13).

**29.8.0 Security changes:** configurable default container AppArmor template ([moby/moby#52771](https://github.com/moby/moby/pull/52771)); AppArmor+SELinux rules blocking the 32-bit **`socketcall(2)`** path to `AF_VSOCK` ([moby/moby#53551](https://github.com/moby/moby/pull/53551)).
**New in 29.8.0:** `--umask <octal>` / `HostConfig.Umask` ([moby/moby#53463](https://github.com/moby/moby/pull/53463)).

### A6. Current (2025–2026) CVEs — verified via NVD + OSV

**Docker Desktop ECI / docker.sock bypasses (most directly on-topic):**

| CVE | Date | CVSS | Summary |
|---|---|---|---|
| **CVE-2026-6406** | 2026-05-22 | **8.8 HIGH** | The Docker CLI **`--use-api-socket`** flag bypasses Enhanced Container Isolation. It adds the socket mount via `HostConfig.Mounts` instead of `HostConfig.Binds`, but **"ECI enforcement in the Docker Desktop API proxy only inspected Binds, allowing the mount to pass unchecked."** Grants a container **full Docker Engine socket access**, plus registry auth credentials if the host user is logged in. |
| **CVE-2025-10657** | 2025-09-26 | **8.7 HIGH** (CVSS4) | With ECI enabled, admin's **socket command-restrictions config was ignored** when passed to ECI, allowing unrestricted powerful Docker commands. Affects Docker Desktop 4.46.0. |

**Docker Engine / Moby (2026):**

| CVE / GHSA | Date | Summary |
|---|---|---|
| **CVE-2026-41567** (GHSA-x86f-5xw2-fm2r) | 2026-06-05 | `PUT /containers/{id}/archive` **executes a container binary on the host** — daemon resolved decompression helpers (`xz`, `unpigz`) from the *container's* filesystem. Fixed in 29.5.1 / 28.5.2-and-below backports / moby v2.0.0-beta.14. |
| **CVE-2026-42306** (GHSA-rg2x-37c3-w2rh) | 2026-06-12 | Race in `docker cp` mount setup lets a malicious container **redirect a bind mount to an arbitrary host path** (overwrite host files / DoS). Fixed in 29.5.1. |
| **CVE-2026-41568** (GHSA-vp62-88p7-qqf5) | 2026-06-12 | Race in `docker cp` lets a malicious container **create empty files/dirs at arbitrary absolute host paths**. Fixed in 29.5.1. |
| **CVE-2026-33997** (GHSA-pxq6-2prw-chj9) | 2026-03-27 | Off-by-one in **plugin privilege validation**. |
| **CVE-2026-34040** (GHSA-x744-4wpc-v9h2) | 2026-04-02 | **AuthZ plugin bypass** via oversized request bodies. |
| **CVE-2026-32288** | 2026-05-14 | Fixed in `v29.5.0`. |
| **CVE-2026-31431** | 2026-04-22 (CVSS 7.8) | Linux kernel `crypto: algif_aead` — the reason `AF_ALG` is now blocked in seccomp. Fixed across 29.4.2/29.4.3. |
| **CVE-2026-17106** | 2026-07-30 | Fixed in `v29.7.0`. |
| **CVE-2026-15793 / 15792 / 15791 / 15789 / 15788** | 2026-07-16 | BuildKit bundle command injection, frontend panic, `/tmp` wipe, destination-dir validation bypass, WCOW junction escape. Fixed in `v29.6.2`. |
| **CVE-2025-54410**, **CVE-2025-54388** | 2025-07-29 | **firewalld reload removes bridge network isolation** / makes published ports remotely accessible. |

**Kernel-adjacent (2026), LPE that container escapes chain through:** **CVE-2026-53359 "Januscape"** (public July 6, 2026, Linux kernel LPE; Canonical published mitigations 2026-07-11).

Ecosystem (docker.sock exposure via third-party tools): **CVE-2026-27002** (OpenClaw Docker tool sandbox allowed dangerous Docker options), **CVE-2026-79755** (Nuclio local Docker platform function namespace), **CVE-2026-73040** (Dockge stack-name validation).

---

## AREA B — Landlock in containers

### B1. Landlock ABI versions — **latest is ABI v11**, kernel mainline **v7.3**

**⚠️ The brief's "v1 through v6 (or later)" framing is obsolete.** The kernel docs (`landlock.rst`) are **dated August 2026** and document **ABI 1–11**. Mainline `Makefile` = **VERSION 7 / PATCHLEVEL 3 / rc3**; latest release **v7.2 (2026-08-16)**.

| ABI | Feature introduced | Commit date (upstream) | First release |
|---|---|---|---|
| **1** | Filesystem access control (base) | 2021-04-22 | **Linux 5.13** |
| **2** | `LANDLOCK_ACCESS_FS_REFER` (cross-dir link/rename) | 2022-05-06 | 5.19 |
| **3** | `LANDLOCK_ACCESS_FS_TRUNCATE` | 2022-10-18 | 6.2 |
| **4** | `LANDLOCK_ACCESS_NET_BIND_TCP` / `_CONNECT_TCP` | 2023-10-26 | 6.7 |
| **5** | `LANDLOCK_ACCESS_FS_IOCTL_DEV` | 2024-04-19 | 6.10 |
| **6** | `LANDLOCK_SCOPE_ABSTRACT_UNIX_SOCKET`, `LANDLOCK_SCOPE_SIGNAL` | 2024-09-05 | 6.12 |
| **7** | `LANDLOCK_RESTRICT_SELF_LOG_*` audit flags | 2025-03-20 | 6.15 |
| **8** | `LANDLOCK_RESTRICT_SELF_TSYNC` (multithread enforcement) | 2025-11-27 | 6.19-era (⚠️ not tag-verified) |
| **9** | `LANDLOCK_ACCESS_FS_RESOLVE_UNIX` (pathname UNIX sockets) | 2026-03-27 | **v7.0** (2026-04-12) |
| **10** | `LANDLOCK_ACCESS_NET_BIND_UDP` / `_CONNECT_SEND_UDP`; `LANDLOCK_ADD_RULE_QUIET` | 2026-06-11/12 | **v7.1** (2026-06-14) |
| **11** | `LANDLOCK_RESTRICT_SELF_NO_NEW_PRIVS` | 2026-08-09 | **v7.2** (2026-08-16) |

**ABI 9, 10, 11 first shipped in v7.0, v7.1, v7.2 respectively** — derived by matching commit dates to release dates. **ABIs 2–7 kernel versions are well-established historical values but were NOT individually tag-verified in this session** — flagging that explicitly.

Also present: an **errata mechanism** (`LANDLOCK_CREATE_RULESET_ERRATA`) with **4 documented errata** in `security/landlock/errata/` (`abi-1.h` → errata 3 & 4, `abi-4.h` → erratum 1, `abi-6.h` → erratum 2). Docs warn: **"Most applications should NOT check errata."** Erratum 4 changed OverlayFS whiteout creation to require `LANDLOCK_ACCESS_FS_MAKE_REG` instead of `MAKE_CHAR` (affects `fuse-overlayfs`). **Hard limit: 16 stacked ruleset layers** (`restrict_self` returns `E2BIG` beyond it).

### B2. Is Landlock enabled by default in Docker containers?

**Three separate answers, and the answer is yes for all three:**

1. **Kernel side — must be configured.** Landlock needs `CONFIG_SECURITY_LANDLOCK=y` **and** must be in the boot-time LSM list (`CONFIG_LSM=landlock,[...]` or `lsm=landlock,...` on the cmdline). Verify with `dmesg | grep landlock || journalctl -kb -g landlock` → `landlock: Up and running.` **If it is not in `CONFIG_LSM`/cmdline, it is off system-wide.**
2. **Privileges — none required.** Kernel docs, verbatim: **"Landlock empowers any process, including unprivileged ones, to securely restrict themselves."** This is the core design point.
3. **Docker seccomp — ✅ ALLOWED, no change needed.** Grepping the fetched `default.json` returns exactly three matches:
   ```
   198: "landlock_add_rule",
   199: "landlock_create_ruleset",
   200: "landlock_restrict_self",
   ```
   All three sit in **group 0** — the unconditional `SCMP_ACT_ALLOW` list — with **no `caps` entry and no `args` filter**. So **`--cap-drop=ALL` does not break Landlock**, and the default profile needs no modification. This is the single most useful operational fact in this brief.

**No_new_privs interaction (important gotcha):** For **unprivileged** processes, Landlock **requires `no_new_privs`**. Processes holding **`CAP_SYS_ADMIN` in their namespace can enforce a ruleset without it** — but the docs warn that leaving it unset is **"risky even when Landlock does not require this attribute"**, since SUID/SGID/file-capability binaries could run as **confused deputies** inside the domain. As of **ABI 11**, `LANDLOCK_RESTRICT_SELF_NO_NEW_PRIVS` sets it atomically **only if enforcement succeeds**. Note this interacts with `--security-opt=no-new-privileges=true` — the flag satisfies the requirement, but is set process-wide at container start.

**Landlock limits that matter in containers:** cannot modify filesystem topology (`mount`/`pivot_root` denied; **`chroot` is NOT denied**); `OverlayFS` layers/merge are **standalone hierarchies** for policy purposes (unlike bind mounts, where rules propagate); special filesystems (pipe/socket, nsfs via `/proc/*/ns/*`) cannot be named explicitly; `LANDLOCK_ACCESS_FS_IOCTL_DEV` applies only to **newly opened** device fds, so inherited stdin/stdout/stderr are unaffected.

### B3. Node.js / npm ecosystem support

**There is no first-party Node.js Landlock API** (Node core docs contain no Landlock binding). But the ecosystem is real and current:

| Package | Version | Notes |
|---|---|---|
| **`@anthropic-ai/sandbox-runtime`** | **0.0.76** (created 2025-10-20, modified **2026-09-10**) | Apache-2.0. CLI `srt`. **Linux path uses `bubblewrap`**, not Landlock — bwrap bind-mounts for FS, **removed network namespace** + host-side HTTP/SOCKS5 proxies, and a **seccomp BPF filter blocking `socket(AF_UNIX)`** + `io_uring_*`. `sandbox-exec`/Seatbelt on macOS, a dedicated `srt-sandbox` account + WFP egress fence + NTFS ACLs on Windows. |
| **`@deepseek-ai/node-addon-landlock-run`** | **0.1.1** (created 2026-08-10) | BSD-3-Clause. **Actual Landlock**: "self-restrict-then-exec launcher", per-platform prebuilt static (musl) binaries + JS seam. Repo `deepseek-harness/deepseek-harness`. |
| `@deepseek-ai/node-addon-landlock-run-linux-x64` / `-arm64` | 0.1.1 | Prebuilt launcher binaries, resolved as file paths, never imported. |
| **`@deepseek-ai/node-addon-system`** | 0.1.2 | Node-API package: Linux Landlock launcher + async POSIX `flock`. |
| `@deepseek-ai/dsh-sandbox-local` | 0.0.1-rc.1 | Backends: **bwrap, the npm landlock-run launcher, macOS Seatbelt, Windows ACL restricted token** — "functionally probed, fail-closed". |
| `node-addon-landlock-run` | 0.0.1 (2026-07-08) | Unscoped earlier publish of the same launcher. |

**AI-agent tooling documenting bwrap/Landlock in containers:**
- **Anthropic SRT** explicitly documents a container escape hatch: **`enableWeakerNestedSandbox`** — "Enable weaker sandbox mode for **Docker environments** … enables it to work inside of Docker environments without privileged namespaces. **This option considerably weakens security** and should only be used in cases where additional isolation is otherwise enforced." (Bubblewrap needs unprivileged userns — see B4.)
- SRT's own **Privilege Escalation via Unix Sockets** warning: *"if [`allowUnixSockets`] is used to allow access to `/var/run/docker.sock` this would effectively grant access to the host system through exploiting the docker socket."* Its example `srt-settings.json` literally lists `"allowUnixSockets": ["/var/run/docker.sock"]` — worth flagging as a footgun.
- Also relevant: OpenCode (`opencode-sandbox`), `@agentick/sandbox-local`, `@evelandhq/sandbox-bwrap`, `@downcity/sandbox-linux`, `@junction41/secure-setup` (gVisor/bwrap/seccomp/AppArmor), `@lite-agent/sandbox-anthropic` — a broad 2026 ecosystem converging on **bwrap/seccomp/ACLs**.

**Linux gap worth noting:** SRT's Linux implementation **does not support glob matching** for paths (literal paths only) and has **no automatic violation monitoring** (macOS taps the sandbox violation log store; Linux requires manual `strace`).

### B4. Ubuntu / Debian / Fedora — restricting unprivileged user namespaces

**Ubuntu — YES, still the mechanism, and it is AppArmor-based.**

- Introduced as **opt-in in Ubuntu 23.10** (announced 2023-10-09), then enabled by default in 23.10 via SRU. **Ubuntu 24.04+ inherits it.**
- Two sysctls: **`kernel.apparmor_restrict_unprivileged_userns`** and **`kernel.apparmor_restrict_unprivileged_unconfined`** (both set to `1` to enable).
- Mechanism: unprivileged processes can create user namespaces **only if confined and their AppArmor profile contains the `userns,` rule** (or they have `CAP_SYS_ADMIN`). Canonical added a **`default_allow` profile mode** that keeps apps effectively unconfined while adding `userns,` — shipped for Firefox, Chrome (`/opt/google/chrome/chrome`), etc., in the **`apparmor` binary package**.
- **Breakage conditions stated by Canonical:** apps are denied userns if (1) **no AppArmor profile exists for them** (not in the Ubuntu archives / no profile shipped), or (2) **they are installed at a different path**. Vendors are asked to ship their own profiles.
- **Practical impact for containers/sandboxes:** any bwrap/Landlock-less sandbox relying on unprivileged userns (including **bubblewrap**, and therefore SRT's Linux backend and much of the npm agent-sandbox ecosystem) can be **denied on stock Ubuntu 24.04+** unless the binary is profiled or the sysctl is relaxed. This is the direct link between B3 and B4.
- ⚠️ **I could not confirm a 2026-specific policy change** to the Ubuntu mechanism; my evidence is the original 23.10 announcement page plus current behavior. The Ubuntu security docs URLs I tried (`documentation.ubuntu.com/security/.../privilege-restrictions/...`) both **404'd** — the page has moved or been restructured, so the current canonical doc URL is unverified.

**`unprivileged_userns_clone` is NOT a mainline sysctl.** Grepping `Documentation/admin-guide/sysctl/kernel.rst` on master returns **no match**. That knob (and `kernel.unprivileged_userns_apparmor_policy`) is a **downstream Debian/Ubuntu/Arch patch**, not upstream — consistent with the canonical AppArmor wiki framing it as "several distro kernels carry a patch".

**Debian — could not verify.** `wiki.debian.org/UserNamespaces` is **JS-gated** ("to use this page you'll need JavaScript enabled") and returned no usable content. The salsa kernel-team patch URL **did resolve HTTP 200** but returned only the wiki's 3,311-byte JS shell (content not retrievable without a browser). **Treat Debian's 2026 default as unconfirmed** — Debian historically ships the `unprivileged_userns_clone` patch, but I did not verify its current default.

**Fedora — could not verify.** The `fedoraproject.org/wiki/Changes/...` URL 404'd; `src.fedoraproject.org/rpms/linux-fedora-config` returned HTTP 200 but is a **package page, not config content**. Fedora's historical approach (a `sysctl` + SELinux/`userns` boolean rather than AppArmor) is **not confirmed here**.

---

## Could NOT verify (explicit gaps)

1. **ABI 2–7 exact kernel versions** — historical values given, not tag-verified this session. **ABI 8's first release** is inferred, not confirmed.
2. **Debian 2026 unprivileged-userns default** — wiki is JS-gated; content unreachable.
3. **Fedora 2026 unprivileged-userns default** — referenced pages 404'd or lacked content.
4. **Current canonical Ubuntu security-docs URL** for userns restriction — all guessed paths 404'd; only the 2023 blog post was retrievable.
5. **`moby/moby#53551` PR body** — the 32-bit `socketcall`/`AF_VSOCK` claim comes from the **29.8.0 release notes**, not the PR diff (release notes fetched; PR body not opened).
6. **Docker's docs table listing "≈44 blocked syscalls"** vs. my computed 361-name allowlist — the doc's count and my extraction use different accounting (300+ total syscalls across arches); I did not reconcile them.
7. **GitHub code search and `api.github.com` repos listing** were **rate-limited (HTTP 403/401)** mid-session; I worked around via raw.githubusercontent.com, NVD, and OSV. GitHub Security Advisory API queries with `affects=` returned empty and were replaced by OSV/NVD.
8. **No Docker-context test was run** — all Landlock/seccomp/AppArmor conclusions are from reading the profile JSON and docs, not from executing `docker run` on a Linux host.

---

## Sources (all actually fetched)

**Area A — Docker**
1. https://docs.docker.com/engine/security/ (landing) → raw: https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/_index.md
2. https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/seccomp.md
3. https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/protect-access.md
4. https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/rootless/_index.md
5. https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/security/apparmor.md
6. https://raw.githubusercontent.com/docker/docs/main/content/manuals/engine/install/linux-postinstall.md
7. https://docs.docker.com/reference/cli/docker/container/run/ (live page, fetched via Invoke-WebRequest)
8. **https://raw.githubusercontent.com/moby/profiles/main/seccomp/default.json** (canonical default seccomp profile, 13,470 B)
9. https://raw.githubusercontent.com/moby/moby/master/daemon/pkg/oci/caps/defaults.go (default caps)
10. https://raw.githubusercontent.com/moby/profiles/main/apparmor/template.go
11. https://api.github.com/repos/moby/moby/releases?per_page=15 (v29.8.0, 2026-09-03)
12. https://api.github.com/repos/moby/profiles (repo metadata)
13. https://api.osv.dev/v1/query (OSV, `github.com/docker/docker`, 67 records)
14. https://services.nvd.nist.gov/rest/json/cves/2.0?keywordSearch=docker+socket
15. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2026-6406
16. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2025-10657
17. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2026-41567
18. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2026-42306
19. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2026-41568
20. https://services.nvd.nist.gov/rest/json/cves/2.0?cveId=CVE-2026-31431

**Area B — Landlock & sandboxing**
21. https://raw.githubusercontent.com/torvalds/linux/master/Documentation/userspace-api/landlock.rst (dated **August 2026**)
22. https://docs.kernel.org/userspace-api/landlock.html (referenced canonical render)
23. https://raw.githubusercontent.com/torvalds/linux/master/include/uapi/linux/landlock.h
24. https://raw.githubusercontent.com/torvalds/linux/master/security/landlock/errata/abi-1.h
25. https://raw.githubusercontent.com/torvalds/linux/master/security/landlock/errata/abi-4.h
26. https://raw.githubusercontent.com/torvalds/linux/master/security/landlock/errata/abi-6.h
27. https://raw.githubusercontent.com/torvalds/linux/master/Makefile (7.3.0-rc3)
28. https://raw.githubusercontent.com/torvalds/linux/master/Documentation/admin-guide/sysctl/kernel.rst (proves `unprivileged_userns_clone` absent)
29. https://api.github.com/repos/torvalds/linux/tags (+ git/ref/tags for release dates)
30. https://api.github.com/repos/torvalds/linux/commits?path=include/uapi/linux/landlock.h (ABI commit dates)
31. https://registry.npmjs.org/-/v1/search?text=landlock · `text=bubblewrap` · `text=sandbox+bwrap`
32. https://registry.npmjs.org/@anthropic-ai/sandbox-runtime (0.0.76)
33. https://registry.npmjs.org/@deepseek-ai/node-addon-landlock-run (0.1.1)
34. https://raw.githubusercontent.com/anthropics/sandbox-runtime/main/README.md
35. https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces
36. https://wiki.debian.org/UserNamespaces (**JS-gated — unusable**)
37. https://src.fedoraproject.org/rpms/linux-fedora-config (**no config content**)
