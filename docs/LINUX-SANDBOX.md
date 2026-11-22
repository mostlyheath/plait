# Linux command sandbox — open design decision

**Status:** investigation, not an enabled Linux backend. The implementation
choice is open; the current safety rules remain
in force. Landlock ABI 9 is a candidate requirement, not a product decision.

## Why this matters to the daemon architecture

Plait's daemon owns checkouts, durable threads, tool execution, and process
cleanup. Clients attach to that daemon; a client disconnect must not stop a run.
The machine that runs a command must enforce its permissions. A desktop approval
or a private network connection is not a substitute for local containment.

Trusted commands and trusted MCP servers run on Linux without containment. Sandboxed commands and MCP servers remain refused: no Linux backend has passed the containment gates below. Linux command support is a local-host prerequisite, not permission to expose the
existing Unix-socket protocol over TCP. Remote authentication, authorization,
and machine-scoped checkout state remain a separate slice.

## Required behavior versus implementation choice

The current command contract is in [HOST.md](HOST.md). Preserve these rules
unless a revised contract is explicitly agreed and reviewed:

- Commands read the checkout but cannot write it or its Git metadata. Approved
  file helpers remain the only checkout writers.
- Writes are limited to private scratch and the current thread's private build
  cache. Other thread caches, the host store, credentials, and general home
  contents are not accessible. Cache roots cannot be renamed or have their
  ownership or permissions changed by a command.
- No external network or outside Unix-socket access. Tests may use Unix sockets
  in disposable scratch. Anonymous socket pairs must also work.
- Commands cannot signal outside processes. Stop, timeout, and host/sidecar
  failure must end command work before the writer leases are released.
- Approval applies to one exact call. Recovery must not replay an interrupted
  effect. The deadline remains 15 minutes; detached daemons remain unsupported.

These are behavior requirements. Bubblewrap, Landlock, namespace layout, and
system policy are implementation choices. Do not turn one investigated mechanism
into a permanent requirement without checking the alternatives.

## Probe evidence — 2026-11-15

| Item | Result |
|---|---|
| System | Ubuntu x86-64 |
| Kernel | `7.0.0-43-generic` |
| Bubblewrap | `0.11.1`, installed at `/usr/bin/bwrap` |
| Unprivileged user namespaces | `kernel.unprivileged_userns_clone = 1` |
| AppArmor restriction | `kernel.apparmor_restrict_unprivileged_userns = 1` |
| Actual namespace probe | Bubblewrap `--unshare-all` succeeded as an unprivileged user |
| Landlock query | `landlock_create_ruleset(NULL, 0, LANDLOCK_CREATE_RULESET_VERSION)` returned **8** |
| Read-only socket probe | A sandbox connected to an outside fixture server through a read-only bind mount |

The socket probe created a disposable Unix server outside Bubblewrap. It mounted
only that fixture directory at `/fixture` with `--ro-bind`, plus system tools,
private `/proc`, `/dev`, and `/tmp`. It used `--unshare-all`, `--die-with-parent`,
`--new-session`, and a cleared environment. Python inside the sandbox connected
to `/fixture/fixture.sock` and received `DISPOSABLE_FIXTURE_ONLY`. The outside
server recorded one connection. No real service, checkout, home directory, or
credentials were exposed. The probe cleaned up its fixture.

This demonstrates a specific gap: **a read-only mount prevents filesystem writes,
but does not prevent communication through an existing pathname Unix socket.**
A network namespace does not close this gap. A checkout or an allowed source/cache
mount can contain such an endpoint even when the host store is not mounted.

The Linux documentation lists `LANDLOCK_ACCESS_FS_RESOLVE_UNIX` from **ABI 9**
as the control for outside pathname Unix sockets. ABI 8 does not provide it.
Abstract Unix-socket scoping is a different control and is not a replacement.
Query the ABI at runtime; do not infer it from a kernel version or distro name.

ABI 9 alone is not proof of a complete sandbox. Cache-root metadata protection,
mount changes, inherited descriptors, signal isolation, and cancellation still
need a design and real negative tests.

## Options to evaluate

1. **Bubblewrap plus a newer Landlock ABI and any other required controls.**
   Keep scratch IPC while denying connections to outside pathname sockets.
   This may require a newer kernel on development and CI hosts. Reject execution if any
   required control is missing; never silently use fewer restrictions.
2. **A stricter first Linux command policy with no Unix-socket creation.**
   A syscall policy could reduce the IPC surface, but this breaks the current
   scratch-IPC promise and some tests. It requires explicit agreement and
   changes to approvals, tool instructions, documentation, and acceptance tests.
   It is not an implementation-only substitution.
3. **A different containment design.**
   Evaluate a managed AppArmor/SELinux policy, a private materialized input tree,
   or another mechanism that can enforce the same behavior. Account for machine
   setup, filesystem freshness, build-cache behavior, and maintenance cost.
   Do not add a syscall broker or a privileged helper as an unreviewed shortcut.
4. **Keep sandboxed Linux commands disabled while completing the independent daemon.**
   Build and test persistence, approvals, file helpers, ownership checks,
   subscriptions, reconnect, and crash recovery with scripted providers.
   This is the current direction, not a claim of sandboxed Linux agent execution.

An unrestricted shell fallback is not an option. Do not disable AppArmor, change
sysctls, upgrade the kernel, or restart machine services just to pass a test.

## Gates before enabling sandboxed Linux commands

- Select and document a containment mechanism and its tested prerequisites.
- Keep platform policy separate from common admission, persistence, approval,
  and subprocess lifecycle code. Do not copy the whole runtime for Linux.
- Keep connected-peer ownership checks for local clients and the daemon. These
  are now implemented with Linux `SO_PEERCRED` and macOS `getpeereid`.
- Test denied checkout/Git writes, source-cache writes, outside reads and writes,
  symlink/hard-link escapes, cache-root changes, and cross-thread cache access.
- Test outside pathname sockets **inside readable mounts**, host/sibling sockets,
  abstract sockets, TCP/UDP, and outside signals. Also test permitted scratch IPC.
- Test Stop, timeout, shell exit, host SIGKILL, and sidecar SIGKILL. Check process
  exit and lease lifetime, not only the UI status or an error string.
- Test missing kernel features and failed sandbox startup. Neither may execute
  the approved command. Keep these failures distinct from command failures.
- Re-run the macOS safety suite without weakening its assertions. A Linux-only
  test target must be labeled; it must not imply that platform sandbox tests ran.

When revising this decision, update this note, the affected rules in HOST.md,
user-facing approval/tool text, and the tests together. Record what changed and
why. Do not replace a failed security assertion with a skip.

## References

- [Host contract](HOST.md)
- [Architecture](ARCHITECTURE.md)
- [Linux Landlock userspace API](https://docs.kernel.org/userspace-api/landlock.html),
  especially pathname Unix sockets, ABI versions, and current limitations
- Installed `bwrap --help` and `/usr/include/linux/landlock.h` from the probe environment

No kernel settings, system policy, sandbox rules, or host protocol were changed
by this investigation.
