# Releasing Plait

Installed per-user apps and headless hosts update from GitHub Releases. A
merge to `main` releases nothing. A release is made when a person starts one:
GitHub builds `main` as it stands, signs the update manifest, and publishes
the next preview. Nobody tags by hand.

## How a change reaches a release

1. **Work on a branch.** `main` changes only through a pull request; GitHub
   refuses a direct push.
2. **Open a pull request.** GitHub runs the tests on it: the macOS check
   (format, lint, tests, and the real command sandbox) and the Linux check. A
   request that changes only prose skips the macOS check. The request can merge
   when both pass. It need not be up to date with `main`. A change that writes
   a person's files, or a file an older build reads, is also read by someone
   who did not write it before it merges.
3. **Merge.** The push to `main` runs the macOS check again, so changes that
   passed apart are tested together. A failure there is fixed with another
   pull request before the next release. A merge that changes only prose
   skips it. Nothing is built for release and nothing is published.
4. **Release, when ready.** Run the **macOS build** workflow on `main` with
   **Build main and publish the next preview** checked, or:

   ```
   gh workflow run "macOS build" --ref main -f release=true
   ```

   The workflow refuses a release from any other branch. If `main` holds code
   that ships and has not been released, it builds it once: the package job
   builds the installers and runs the installer smoke test, including
   migration to the prebuilt per-user install, and the publish job writes
   `update.json`, signs it, and publishes the release with the app archive,
   host archive, packages, DMG, and checksums. The
   [native acceptance check](VERIFICATION.md#scenes-and-native-checks) drives
   the same package in a job of its own; it is advisory, and publishing does
   not wait for it. Check that the latest run on `main` passed before
   starting a release: the release run does not repeat those tests.

A release run on a `main` that changed only docs, tests or CI since the last
preview publishes nothing; `scripts/release_plan.py` holds the list of paths
that never reach an installed app.

## Preview numbers

Keep the Cargo version for `plait` and `plait-host` the same
(`MAJOR.MINOR.PATCH`). A preview is `v<version>-preview.N`. The workflow takes
the next `N` after the highest tag for that version, and creates the tag with
the release, on the commit it built, only when the build and the installer
test have passed. A build that fails uses up no number: fix it with another
pull request.

Installed apps see the release on their next launch or **Check for Updates…**.
The updater compares the signed version, so each release needs a newer tag.
Published tags and assets are not rewritten. A few earlier numbers have a tag
and no release, from when tags were pushed by hand; they stay as they are.

## Update signing key

Plait accepts only manifests signed by the key whose public half is
`PUBLIC_KEY` in `crates/plait-client/src/update.rs`. The private half is the
Actions secret **`PLAIT_UPDATE_SIGNING_KEY`**: 64 hex digits. The publish job
fails if the secret is missing or does not match the built-in public key.

The maintainer keeps a local copy outside Git with mode 0600. Back it up,
encrypted, off this computer: GitHub cannot return a secret's value, and losing
the key strands installed apps on their current version until they are
reinstalled with a build that trusts a new key. Do not generate a key per release
or sign test builds with it.

Create a key only when none exists, then store it as the secret:

```sh
cargo run -p plait-protocol --example update-key -- generate <private-key-file>
gh secret set PLAIT_UPDATE_SIGNING_KEY < <private-key-file>
```

`generate` refuses to overwrite a file and prints the public key to put in
`update.rs`. `update-key verify <dir>/update.json` checks a downloaded manifest
against the built-in key.

## What a signature covers

The manifest records the version, tag, revision, host build identity, and each
artifact's name, size, and SHA-256. Clients verify the signature before reading
any field, reject a tag that differs from the release they asked for, and
discard downloads whose size or digest differ. Workflow changes that can read
the signing secret can sign updates; review them like release code.

## Independent client and server releases

**Agreed contract; host operations in step 2 are built.** It implements the roadmap's
[independent release contract](ROADMAP.md#host-and-release-boundaries).
Today one signed `update.json` describes the app and its host together, and
the in-app update replaces both.

### What a release publishes

- Every release keeps one tag counter, `v<version>-preview.N`. The `plait` and
  `plait-host` Cargo versions stay equal until 0.2.
- Two new signed manifests use the same release key, `schema_version: 2`:
  - `update-client.json` names `component: "client"`, the client archive,
    and the host protocol range the client speaks (`protocol.min`,
    `protocol.max`).
  - `update-host.json` names `component: "host"`, the host archive, its
    `build_id`, protocol range, capabilities, and a monotonic
    `storage_schema`. A host is never offered when its storage schema is
    older than the installed one.
- A release with only a host publishes `update-host.json` and the host
  archive. A release with only a client publishes `update-client.json` and
  the app. A release that carries both publishes both manifests.
- `update.json` (schema 1) is not changed. Its clients reject unknown fields
  and other schema versions, and it names a client and a host together. So
  until 0.2, every release that changes the client is published with its
  host and also publishes `update.json`; an older Plait can always update to
  one that reads the new manifests. Releases with only a host omit it, so an
  older Plait never offers them. Client-only releases begin at 0.2.
- A client archive keeps its bundled host for first-launch setup, which
  stays offline.

### How a client judges a server

- The client negotiates status from protocol 7 down to 1. A new
  versionless, read-only `host_info` reply gives the host's release, build,
  protocol range, capabilities, and storage schema. An older host refuses it,
  and the client falls back to the negotiated protocol and the existing
  feature probes.
- A server inside the client's range is compatible. A feature it lacks
  disables only that feature's controls, as today.
- A server newer than the client can speak asks the person to update Plait.
  One older than the client's minimum asks them to update the server. Neither
  sends, retries, or replays anything.
- The server update a client offers is the newest signed host whose
  protocol range overlaps its own, and never an older version or storage
  schema. When the newest host needs a newer client, Plait says so instead of
  offering it. Only the per-user LaunchAgent server is updated in the app;
  a manual or development host shows `plait-host update`.
- Protocols 1–6 stay supported until 0.2. Raising the minimum takes a
  release that announces it first.

### Update flows

- **Update Plait** replaces only the app and relaunches it. It never reads
  runs, unpacks a host, writes the service definition or `installed.json`,
  or prunes releases, so it works while threads run.
- **Update Server** follows these steps:
  1. It stages the verified host as an immutable release without marking it
     installed.
  2. It asks the host to drain (`drain_for_update`, capability `drain_v1`).
     New admissions are refused with `busy` ("Host is updating"), while
     accepted work finishes.
  3. While work is still running, the app shows how many runs remain, with
     **Cancel** (`cancel_drain`). When none remain, it switches over by itself.
  4. Activation restarts the service on the new release, waits until it
     answers, and confirms launchd runs it. Only then does it record the
     release as installed and prune the others. A failed start restores the
     previous definition and keeps both releases.

  A host without `drain_v1` falls back to today's double idle check.
- `plait-host update` uses the same stage, drain, and activate path, and
  adds `--stage`, `--wait`, and `--version`.
- First-launch setup stays bootstrap-only. It never replaces or downgrades
  an existing server.

### Order of work

1. Version 2 manifests, verification, and the release plan. These have
   Python and Rust tests for host-only, client-only, and combined releases,
   component mismatch, and mixed feeds.
2. **Built:** `host_info`, `drain_for_update`, and `cancel_drain` in the host. Runtime
   tests show that version 1–6 replies are unchanged, that drain blocks new
   admissions and queue drains while active work finishes, and that cancel
   restores admissions.
3. Compatibility checks and separate client and host update queries, tested
   against protocol 1–6 fixtures and out-of-range hosts.
4. Install primitives: a client-only update, host staging, and activation
   with rollback. macOS tests show a client update leaves the host
   untouched during an active run.
5. Separate update controls, drain progress, incompatibility notices, and
   the `plait-host update` options, each with scenes.
6. A cross-version check on CI, pairing the current client with the previous
   release's host and the reverse, and documentation.

## Packaging and signing

Building macOS distribution artifacts requires an Apple Silicon Mac with rustup,
Python 3, Node/npm, and full Xcode with Metal:

```sh
bash scripts/check.sh --require-command-sandbox
bash scripts/build-macos.sh
```

`scripts/build-macos.sh` compiles the locked workspace, generates Rust license
notices with pinned `cargo-about` 0.9.3, verifies the pinned Node **26.12.0**
download checksum, installs locked production npm dependencies without
lifecycle scripts, and validates Mach-O architecture, system-only dynamic links,
signatures, property lists, package contents, and archives. Building never
installs into `/Applications` or starts a LaunchAgent. Inspect a built app
locally with `open dist/Plait.app --args /absolute/path/to/repository`.

| Artifact | Purpose |
| --- | --- |
| `Plait-…-macos-arm64.dmg` | `Plait.app`, Applications shortcut, migration script, and instructions |
| `Plait-…-macos-arm64.zip` | App bundle with its bundled host archive and per-user install script |
| `Plait-Installer-…-macos-arm64.pkg` | Legacy system-wide installer (client and host) |
| `Plait-Client-…-macos-arm64.pkg` | Legacy system-wide client-only package (`/Applications/Plait.app`) |
| `Plait-Host-…-macos-arm64.pkg` | Legacy system-wide host-only package (`/Library/Application Support/Plait/Host`) |
| `Plait-Host-…-linux-{x86_64,arm64}.tar.gz` and `.sha256` | Linux host, bundled Node and runtime, and checksums |
| `update-host-linux-{x86_64,arm64}.json` and `.sig` | Signed Linux host update manifests |
| `Plait-Release-….json` and `.sha256` | Component metadata and artifact checksums |

**Signing status:** The client and Rust host are ad-hoc signed, and bundled Node
keeps its upstream signature. Releases are **not** Developer ID signed or
notarized ([Issue #6](https://github.com/heath0xFF/plait/issues/6)). Update
manifest signatures cover update verification only; standalone artifact
checksums detect corruption rather than publisher identity.

Workflow runs upload `plait-macos-arm64-…` artifacts (14-day retention) before
the publish job creates the immutable GitHub prerelease. Per-user installs place
immutable host releases under `~/Library/Application Support/Plait/Host/releases/<build-id>/`
(with `installed.json` and `~/Library/LaunchAgents/dev.plait.host.plist`);
legacy system-wide packages use `/Library/Application Support/Plait/Host/` and
`/Library/LaunchAgents/dev.plait.host.plist`. Threads, caches, and credentials
live outside program files.
