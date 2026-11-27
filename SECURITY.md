# Security policy

Report a suspected vulnerability privately using [Report a vulnerability on GitHub](https://github.com/mostlyheath/plait/security/advisories/new). Include the affected build, operating system, steps to reproduce, and what access or data the issue exposes. Remove API keys, tokens, and private project content from examples.

## Supported versions

Security fixes target the latest preview and the planned v0.1.0 release. Earlier previews are superseded by the latest preview.

## Scope and trust boundaries

- Reports can concern the host, its Node runtime sidecar, or exposure of provider credentials or the host’s Linear API key. The local socket is owner-only and checks the peer's user ID. Processes already running as that user are inside the local trust boundary. [Host protocol](docs/HOST.md)
- Remote access includes the SSH bridge and the optional Tailscale listener for authorized peers. Report failures in either path's access checks. [Remote hosts](docs/REMOTE.md)
- The checkout’s command policy governs Plait’s Pi command runner. Trusted Pi commands run with the host user’s access. For Sandboxed checkouts, macOS denies Pi shell commands checkout writes and IP networking, and Linux refuses to run those commands. Codex uses its thread permissions and defaults to Full access, which allows checkout writes and networking even in a Sandboxed checkout. Claude Code uses its thread permission mode and Claude Code settings. Report a command that exceeds the selected policy. [Command policies](docs/HOST.md)
- The updater, release signing workflow, and verification keys are in scope. The updater verifies the signed manifest and checks the download's size and SHA-256. The macOS app is ad-hoc signed and has no Apple notarization. [Release verification](docs/RELEASING.md)
