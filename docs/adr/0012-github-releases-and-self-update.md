---
id: CM-ADR-0012
title: "ADR 0012: Publish GitHub Releases and let the CLI update itself"
status: draft
---

# ADR 0012: Publish GitHub Releases and let the CLI update itself

- **Date:** 2026-11-15
- **Related:** [ADR 0004](./0004-trunk-based-development-and-release.md), [Release process](../release.md)

## Context

ADR 0004 chose tags on `main` published as GitHub Releases but left the artifacts, the checks, and the client side open. Agents run the `cairn` CLI, the hooks, and `cairn mcp` on several machines; those installs must be easy to update without anyone building from source, and an update must never interrupt an agent.

## Decision

1. A tag `vMAJOR.MINOR.PATCH` on `main` runs a workflow that verifies the tag against the workspace version, builds `cairn` and `cairn-server` natively for `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, `aarch64-unknown-linux-musl`, and `x86_64-pc-windows-msvc`, and creates a GitHub Release.
2. Assets are raw binaries (`cairn-<target>[.exe]`, `cairn-server-<target>[.exe]`), the two hook scripts, and a `SHA256SUMS` file over all of them. There are no archives, so the updater needs no unpacking code or extra dependency.
3. `cairn update` reads the latest release from the GitHub API over HTTPS, compares semantic versions, downloads the asset for its own compile-time target, verifies its SHA-256 against the `SHA256SUMS` of the same release, stages it beside the running executable, requires the staged binary to run `--version` and report the expected version, and then replaces the running executable. It also installs the hook scripts of the same release into `hook/` beside the CLI config; `cairn setup` installs those of the release matching the running binary. A script edited locally is kept as `.bak`. `cairn update --check` only reports.
4. Interactive CLI commands print a one-line notice when a newer release is known. The check runs at most once per 24 hours with a 2 second timeout and fails silently. `cairn mcp`, the hooks, and non-terminal use never check and never update.
5. The updater follows redirects itself (GitHub serves assets from another host), checking every hop is HTTPS, and sends no credential. The REST client's no-redirect rule for the device token is unchanged. `CAIRN_UPDATE_URL` (a loopback HTTP server for tests) is honoured in debug builds only.
6. Pre-release tags and versions are out of scope: the release workflow rejects them and the updater ignores them.

## Security notes

- Transport is HTTPS with the system trust store. A downgrade to HTTP is refused; only debug builds accept a loopback test server through `CAIRN_UPDATE_URL`, and release builds ignore that variable.
- `SHA256SUMS` comes from the same release as the binaries. It detects a corrupted or truncated download and a tampered single asset, but it does not protect against a compromised release, a compromised GitHub account or workflow, or an attacker who can replace both the binary and the checksum file. Signing (for example Sigstore or minisign, with a public key shipped in the binary) is future work and would supersede this limit.
- The updater never reads or sends the device token. The update-check cache holds only a timestamp and a version number.
- A staged binary must run and report the expected version before it replaces the installed one, so a download that cannot execute on this machine (wrong architecture or libc) cannot brick the CLI. Linux builds are static musl binaries so one asset runs on any distribution.
- Replacing the executable needs write access to its directory. The updater does not escalate privileges; it reports the problem.
- macOS binaries are not notarized. A binary fetched by `cairn update` is not quarantined by the browser, but one downloaded by hand may need the usual Gatekeeper approval.

## Consequences

- One tag produces every artifact; the version is defined once in `Cargo.toml`.
- Installs of `cairn` update with one command and stay current without background activity.
- Server operators still upgrade `cairn-server` themselves, with migration notes from the release, as ADR 0004 requires.
- Trust rests on GitHub account and Actions security until signing exists.

## Alternatives considered

### Archives (`.tar.gz`, `.zip`) per target

Rejected for now: the updater would need an extraction dependency, and the binaries are small enough to ship raw.

### Background auto-update

Rejected: an agent must not be interrupted or have its binary replaced mid-session. The update is an explicit command and the notice is passive.

### Package managers only (Homebrew, apt, winget)

Deferred: they add per-platform maintenance, and a plain download covers the four supported targets first.
