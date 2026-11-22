# Documentation

Start with the [project README](../README.md) for the current preview and download.
Plait currently supports local agent work on Apple Silicon macOS. Remote hosts
are not available. Linux hosts can run trusted commands and trusted MCP servers; sandboxed execution is not available.

## Use Plait

- [Mac setup](MACOS.md): installation, authentication, keyboard controls, recovery,
  and independent client/host updates.
- [Build Plait with Plait](SELF-HOSTING.md): prepare a checkout, approve a change,
  run tests, review the diff, and continue.
- [Verification](VERIFICATION.md): versioned release evidence and known gaps.

## Develop Plait

- [Contributing](../CONTRIBUTING.md): prerequisites, checks, and code ownership.
- [Architecture](ARCHITECTURE.md): client, state, Git, host, and runtime boundaries.
- [Local host contract](HOST.md): protocol, persistence, approvals, and sandbox rules.
- [Desktop design](../DESIGN.md): layout, visual tokens, and native interaction.
- [Originality policy](ORIGINALITY.md): application code, assets, and dependencies.

## Planned work

- [Roadmap](ROADMAP.md): milestones, scope, and performance goals.
- [Antigravity through ACP](ACP.md): agreed adapter contract, not implemented.
- [Host-first Plait](HOST-FIRST.md): agreed direction; the host owns all state and the desktop reads its stream. Not built.
- [Remote hosts](REMOTE.md): SSH and tailnet hosts, built for one machine per window.
- [Linux sandbox](LINUX-SANDBOX.md): the open containment decision and enablement gates.
- [ADEMark plan](ADEMARK.md): measurement requirements, not a published benchmark result.
