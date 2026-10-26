MiniCon is a standalone terminal in a single executable — no installer, no
bundled language runtime, and no Visual C++ redistributable. It uses only the
operating system's own desktop libraries.

<!--
This file is the public Release body. release.yml substitutes the version
string and either keeps or drops the OPTIONAL_APE block from
release-policy.json, then publishes the result verbatim. It is a TEMPLATE, not
a changelog: per-version facts are owned by prd/archive/<version>-release-history.md,
and this file must not restate them (it did, and drifted seven releases behind).

The "If a build will not start" section is the deliberate exception in kind: it
names no version and states a standing property of an in-place self-updater, so
it stays true for every release that follows. Do not delete it as stale.

Owner: prd/PRD_02_27_con_delivery.md.
-->

## What changed

Each release's changes are recorded once, in the repository's release history —
one document per version, naming the exact source, the workflow run ids and the
evidence:

<https://github.com/partnernetsoftware/minicon/tree/main/prd/archive>

What the product does today, and how to drive it, are in the
[README](https://github.com/partnernetsoftware/minicon#readme) and in
`minicon --help`.

## Downloads

Harness/provider-pool is experimental and evolving. Complete real multi-account
and external-client acceptance is pending; availability depends on the provider
and account. See the [current Harness scope](https://github.com/partnernetsoftware/minicon/blob/main/prd/PRD_02_31_01_provider_gateway.md#0223-experimental-release-amendment-2026-10-26).


Pick your platform and run the file — there is no installer.

| Platform | Download |
| --- | --- |
| Windows x64 | `minicon-VERSION-windows-x86_64.zip` |
| Windows ARM64 | `minicon-VERSION-windows-arm64.zip` |
| macOS (Apple Silicon + Intel) | `minicon-VERSION-macos-universal.dmg` |
| Linux x64 | `minicon-VERSION-linux-x86_64.tar.gz` |
| Linux ARM64 | `minicon-VERSION-linux-arm64.tar.gz` |
<!-- OPTIONAL_APE_START -->
| Advanced: one signed launcher, all platforms | `minicon.com` |
<!-- OPTIONAL_APE_END -->

The macOS `.dmg` contains a signed, notarized, stapled **`MiniCon.app`**
(universal — Apple Silicon and Intel). A `.tar.gz` of the bare signed binary is
also provided for command-line-only use. Every archive has a SHA-256 beside it.

Linux archives use the distribution runtime libraries `libxkbcommon0` and
`libwayland-client0` (no `-dev` packages required) and bundle the runtime-only
X11 bridge `libxkbcommon-x11.so.0`, so slim X11 systems start MiniCon without
extra packages.

Windows executables and `minicon.com` are Authenticode-signed, and the macOS
build is Developer ID-signed and Apple-notarized, as PARTNERNET SOFTWARE PTY LTD
with an RFC 3161 timestamp. The policy, the publisher facts and the verification
commands are in
[CODE_SIGNING_POLICY.md](https://github.com/partnernetsoftware/minicon/blob/main/CODE_SIGNING_POLICY.md).

Verify a download before running it:

```
sha256sum -c minicon-VERSION-linux-x86_64.tar.gz.sha256
```

## If a build will not start

The in-place update keeps the copy it replaces, so an update that leaves you
with something that will not launch is undone by hand — no installer, no
uninstall step:

```
mv minicon.backup minicon          # Linux and macOS CLI
```

On Windows the same file is `minicon.exe.backup`, next to `minicon.exe`. If the
backup is gone too, download the previous release's archive again: the file
inside is identical to the one you had, and there is nothing else on the machine
to clean up. A Linux binary that arrived without its executable bit is
`chmod +x minicon`.

## Older Windows

**Windows Server 2016 and Windows 10 version 1607** are supported. Those builds
have no pseudoconsole, so MiniCon hosts the shell in a hidden console instead.
See [Running on old Windows](https://minicon.agenterm.work/old-windows.html).

## Reporting a problem

Run `minicon --status` and include the output. It reports the build, the
console backend this machine selected and why, the font the system resolved to
with its measured cell width, and where MiniCon writes when something fails —
without opening a window.

MIT OR Apache-2.0.
