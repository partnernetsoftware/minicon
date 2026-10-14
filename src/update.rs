//! Check for updates, download, verify, swap, restart.
//!
//! This is the product's first outbound network call and its first
//! self-modifying behaviour, so the shape is deliberately narrow:
//!
//! * **Manual and per-click.** Nothing here runs on a timer, at startup, or in
//!   the background. The user presses a button and this module answers.
//! * **Nothing blocking on the paint thread.** The whole exchange and the swap
//!   run on a worker thread; the panel only reads a state value and drains a
//!   channel, in the same pattern the terminal clipboard read uses.
//! * **Bytes are refused unless the release vouches for them.** The archive is
//!   checked against the SHA-256 sidecar the release publishes before anything
//!   is staged, and every failure leaves the running executable untouched.
//! * **A restart is never a side effect.** Replacing the file and running the
//!   new one are separate, explicit actions: a restart drops every open
//!   terminal and ends the `--control` endpoint.
//!
//! Mechanism lives where the issue put it: the HTTP exchange is
//! `minicon_platform::network_http`, the atomic write is
//! `minicon_platform::filesystem_publish`, and the version comparator is
//! `minicon_core::version`. What lives here is product meaning — which repo,
//! what counts as an update, and what the panel says.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::Duration;

pub use minicon_core::version::Version;

use crate::update_archive;

/// Where the latest release is read from.
///
/// Overridable so the whole state machine — including a SHA mismatch, a
/// truncated body and a rate-limit response — is provable against a loopback
/// server instead of depending on GitHub being reachable in CI.
pub const RELEASE_API_ENV: &str = "MINICON_UPDATE_API";
pub const RELEASE_API_DEFAULT: &str =
    "https://api.github.com/repos/partnernetsoftware/minicon/releases/latest";

/// GitHub rejects a request without one, and identifies the caller by it.
const USER_AGENT: &str = concat!("minicon/", env!("CARGO_PKG_VERSION"));
const RELEASE_BODY_LIMIT: usize = 512 * 1024;
/// The archive ceiling. The network module caps a response at
/// `NETWORK_HTTP_MAX_RESPONSE_BYTES` and the product's own archives are a few
/// megabytes, so that ceiling is the bound rather than a second number that
/// could drift past it.
const ARCHIVE_BODY_LIMIT: usize = minicon_platform::network_http::NETWORK_HTTP_MAX_RESPONSE_BYTES;
/// The ceiling for one archive member *after* decompression.
///
/// `ARCHIVE_BODY_LIMIT` bounds the compressed download and nothing else: gzip
/// reaches roughly 1000:1, and a zip entry declares its own output size, so a
/// few megabytes on the wire can ask this process for gigabytes. Derived from
/// the download bound so there is one number rather than two that can drift,
/// and four times it because that bound is a *download* budget: 0.2.7's
/// largest archive is 2.4 MB and inflates to a few megabytes of executable.
const ARCHIVE_DECODED_LIMIT: usize = ARCHIVE_BODY_LIMIT * 4;
const CHECK_TIMEOUT: Duration = Duration::from_secs(20);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);
/// The name a release asset's checksum sidecar carries, appended to it.
const SIDECAR_SUFFIX: &str = ".sha256";
/// The only scheme the update path may fetch over.
///
/// The release document names the archive URL, so the scheme is a claim made by
/// whatever answered the check; admitting `http` would let that claim downgrade
/// the download that carries the product's own replacement to plaintext.
const REQUIRED_SCHEME: &str = "https";
/// Hosts exempt from [`REQUIRED_SCHEME`], because they are the machine itself.
///
/// Loopback is a test seam, not a policy hole: `src/update.rs`'s own suite
/// serves a release from a local `TcpListener`, and traffic to 127.0.0.1,
/// `::1` or `localhost` never reaches a wire anyone else can read.
const LOOPBACK_HOSTS: [&str; 3] = ["127.0.0.1", "::1", "localhost"];

/// Where this build's archive comes from, and how it can be installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    /// The `-<os>-<arch>` token the release names this platform's archive with.
    pub suffix: &'static str,
    /// The archive extension, which also selects the reader.
    pub extension: &'static str,
    /// The member the archive must contain.
    pub executable: &'static str,
    /// Whether this platform can be updated by replacing one file.
    ///
    /// macOS cannot: the product ships as a signed, notarized `.app`, and
    /// swapping only the inner Mach-O breaks the signature and Gatekeeper
    /// refuses the result. The macOS flow hands off to the `.dmg` instead.
    pub in_place: bool,
}

/// The target for the running binary, or `None` for a platform the release does
/// not publish a one-file archive for. `None` is a safe failure with a link to
/// the Releases page, never a wrong-architecture download.
pub fn current_target() -> Option<Target> {
    let target = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Target {
            suffix: "windows-x86_64",
            extension: ".zip",
            executable: "minicon.exe",
            in_place: true,
        },
        ("windows", "aarch64") => Target {
            suffix: "windows-aarch64",
            extension: ".zip",
            executable: "minicon.exe",
            in_place: true,
        },
        ("linux", "x86_64") => Target {
            suffix: "linux-x86_64",
            extension: ".tar.gz",
            executable: "minicon",
            in_place: true,
        },
        ("linux", "aarch64") => Target {
            suffix: "linux-aarch64",
            extension: ".tar.gz",
            executable: "minicon",
            in_place: true,
        },
        ("macos", _) => Target {
            suffix: "macos-universal",
            extension: ".dmg",
            executable: "minicon",
            in_place: false,
        },
        _ => return None,
    };
    Some(target)
}

/// One downloadable file on a release.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// The part of a release this product reads.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Release {
    pub version: Version,
    pub assets: Vec<Asset>,
}

impl Release {
    /// The archive for this target, and the sidecar that vouches for it.
    ///
    /// Selection is by the archive's own naming pattern rather than by an
    /// index, so a release that grows an extra asset (the `.com` payload, an
    /// aarch64 sibling) cannot shift this onto the wrong file.
    pub fn archive_for(&self, target: Target) -> Option<(&Asset, Option<&Asset>)> {
        let wanted = format!("-{}{}", target.suffix, target.extension);
        let archive = self
            .assets
            .iter()
            .find(|asset| asset.name.ends_with(&wanted))?;
        let sidecar = self
            .assets
            .iter()
            .find(|asset| asset.name == format!("{}{SIDECAR_SUFFIX}", archive.name));
        Some((archive, sidecar))
    }
}

/// Parses the `releases/latest` body.
///
/// `serde_json` is already a product dependency; only the two fields this
/// product acts on are read, and a tag that is not a stable version is refused
/// here rather than compared later.
pub fn parse_release(body: &[u8]) -> Result<Release, String> {
    let document: serde_json::Value =
        serde_json::from_slice(body).map_err(|error| format!("release is not JSON: {error}"))?;
    let tag = document
        .get("tag_name")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "release has no tag_name".to_owned())?;
    let version =
        Version::parse(tag).ok_or_else(|| format!("tag {tag:?} is not a stable version"))?;
    let mut assets = Vec::new();
    for asset in document
        .get("assets")
        .and_then(|value| value.as_array())
        .unwrap_or(&Vec::new())
    {
        let (Some(name), Some(url)) = (
            asset.get("name").and_then(|value| value.as_str()),
            asset
                .get("browser_download_url")
                .and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        assets.push(Asset {
            name: name.to_owned(),
            url: url.to_owned(),
        });
    }
    Ok(Release { version, assets })
}

/// The first whitespace-delimited token of a `sha256sum` sidecar.
///
/// Only the digest is read: the file name in the sidecar is a second claim
/// about a name this caller already chose, and trusting two sources for one
/// fact is how a mismatch hides.
pub fn parse_sidecar(body: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(body).map_err(|_| "checksum sidecar is not text".to_owned())?;
    let digest = text.split_whitespace().next().unwrap_or("");
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("checksum sidecar is not a SHA-256 digest".to_owned());
    }
    Ok(digest.to_ascii_lowercase())
}

/// The hex SHA-256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest as _, Sha256};

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

/// What went wrong, as a closed set rather than a formatted sentence.
///
/// The failure text used to be a `format!`ed English string built here and
/// painted verbatim, which made a Chinese UI show English and never told the
/// user the one thing that matters after a failed swap -- whether the product
/// still starts. The kind is the part a human reads, so it is translated by the
/// panel's own string tables; `cause` stays the untranslated diagnostic from
/// the platform layer, kept beside it for `ui-snapshot` and for a bug report.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FailureKind {
    /// No release archive is published for this platform.
    Platform,
    /// The release document could not be read or understood.
    Release,
    /// The release document, the archive or the checksum could not be fetched.
    Transport,
    /// The download did not match the release's own checksum.
    Checksum,
    /// The archive did not yield an executable.
    Archive,
    /// The new executable could not be written beside the current one.
    Staging,
    /// The swap could not be completed; the current executable is still there.
    Swap,
    /// The new copy was placed but would not start.
    Launch,
    /// The new copy would not start *and* the previous one could not be put
    /// back: the one state that may need a reinstall.
    LaunchUnrestored,
}

/// Whether the install still runs, which is the second half of every failure
/// sentence and not derivable from "what failed".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InstallState {
    /// The failed step ran before the swap, so the executable was never moved.
    Untouched,
    /// The swap began, so the previous executable was put back and kept.
    Restored,
    /// The previous executable could not be confirmed back in place.
    Unknown,
}

impl FailureKind {
    /// The install state this kind leaves behind. Stated here, next to the
    /// kinds, so the invariant is one table rather than one sentence per call
    /// site: everything before the swap is `Untouched`, a failed launch is
    /// `Restored`, an unrestorable one is `Unknown`.
    #[must_use]
    pub const fn install_state(self) -> InstallState {
        match self {
            Self::Platform
            | Self::Release
            | Self::Transport
            | Self::Checksum
            | Self::Archive
            | Self::Staging
            | Self::Swap => InstallState::Untouched,
            Self::Launch => InstallState::Restored,
            Self::LaunchUnrestored => InstallState::Unknown,
        }
    }

    /// A stable, language-independent name for this kind, published by
    /// `ui-snapshot` so automation asserts the state without matching a
    /// translated sentence.
    #[must_use]
    pub const fn token(self) -> &'static str {
        match self {
            Self::Platform => "platform",
            Self::Release => "release",
            Self::Transport => "transport",
            Self::Checksum => "checksum",
            Self::Archive => "archive",
            Self::Staging => "staging",
            Self::Swap => "swap",
            Self::Launch => "launch",
            Self::LaunchUnrestored => "launch-unrestored",
        }
    }
}

/// A failed update action: what happened, and the detail behind it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Failure {
    pub kind: FailureKind,
    /// The platform layer's own words. Diagnostic; never painted alone.
    pub cause: String,
}

impl Failure {
    pub fn new(kind: FailureKind, cause: impl Into<String>) -> Self {
        Self {
            kind,
            cause: cause.into(),
        }
    }
}

/// The kind and the cause, for a log line or an assertion message. The panel
/// never paints this: it paints the translated sentence for the kind.
impl std::fmt::Display for Failure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.kind.token(), self.cause)
    }
}

/// What the panel shows. Product state, not panel-local: the phase outlives the
/// settings panel being closed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Phase {
    /// Nothing asked yet.
    Idle,
    /// A check is in flight.
    Checking,
    /// The running build is the latest release.
    UpToDate,
    /// A newer release exists.
    Available { remote: Version },
    /// The running build is ahead of the latest release, as a `main` build is.
    Ahead { remote: Version },
    /// The verified archive is staged and waiting for a restart.
    Ready { remote: Version, opens_dmg: bool },
    /// The last action failed. The running executable is untouched.
    Failed(Failure),
}

/// The update state the product owns and the panel renders.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct State {
    pub current: Version,
    pub phase: Phase,
    /// The asset chosen for this platform, once a release has been read.
    pub asset: Option<String>,
}

impl State {
    /// The state a fresh process starts in.
    pub fn new(current: Version) -> Self {
        Self {
            current,
            phase: Phase::Idle,
            asset: None,
        }
    }

    /// Whether a `Check for updates` press is meaningful right now.
    pub const fn can_check(&self) -> bool {
        matches!(
            self.phase,
            Phase::Idle
                | Phase::UpToDate
                | Phase::Available { .. }
                | Phase::Ahead { .. }
                | Phase::Failed { .. }
        )
    }

    /// Folds one event into the state. Pure, so the whole machine is provable
    /// without a network or a file system.
    pub fn apply(&mut self, event: Event) {
        match event {
            Event::Checked(release) => {
                self.asset = None;
                if release.version > self.current {
                    self.phase = Phase::Available {
                        remote: release.version,
                    };
                } else if release.version < self.current {
                    self.phase = Phase::Ahead {
                        remote: release.version,
                    };
                } else {
                    self.phase = Phase::UpToDate;
                }
            }
            Event::AssetChosen(name) => self.asset = Some(name),
            Event::Staged { remote, opens_dmg } => self.phase = Phase::Ready { remote, opens_dmg },
            Event::Failed(failure) => self.phase = Phase::Failed(failure),
        }
    }
}

/// A command for the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Check,
    /// Download, verify and stage the chosen asset.
    Install,
}

/// A result from the worker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Checked(Release),
    AssetChosen(String),
    Staged { remote: Version, opens_dmg: bool },
    Failed(Failure),
}

/// Where the worker is in the download, for a progress line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Progress {
    Downloading,
    Verifying,
    Staging,
    Opening,
}

/// The update state as `ui-snapshot` publishes it.
///
/// A stable phase tag plus the versions and, when something failed, both the
/// stable kind and the untranslated cause — so automation asserts the state
/// without matching painted strings, and the panel's own sentence stays free to
/// be translated. The tag is spelled once, here.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub phase: &'static str,
    pub remote: Option<String>,
    /// The platform layer's own words, for a bug report. Diagnostic only.
    pub cause: Option<String>,
    /// The stable failure kind. `None` unless the phase is `failed`.
    pub failure: Option<&'static str>,
    /// Whether the download was handed to the browser rather than swapped.
    pub handoff: bool,
}

/// Reads the phases out of a state.
#[must_use]
pub fn snapshot(state: &State) -> Snapshot {
    let mut published = Snapshot {
        phase: "idle",
        remote: None,
        cause: None,
        failure: None,
        handoff: false,
    };
    match &state.phase {
        Phase::Idle => {}
        Phase::Checking => published.phase = "checking",
        Phase::UpToDate => published.phase = "up-to-date",
        Phase::Available { remote } => {
            published.phase = "available";
            published.remote = Some(remote.to_string());
        }
        Phase::Ahead { remote } => {
            published.phase = "ahead";
            published.remote = Some(remote.to_string());
        }
        Phase::Ready { remote, opens_dmg } => {
            published.phase = "ready";
            published.remote = Some(remote.to_string());
            published.handoff = *opens_dmg;
        }
        Phase::Failed(failure) => {
            published.phase = "failed";
            published.failure = Some(failure.kind.token());
            published.cause = Some(failure.cause.clone());
        }
    }
    published
}

/// The worker's side of one install, so the file work is testable on its own.
///
/// `archive` is the downloaded archive, `sidecar` the release's checksum body.
pub struct InstallInput<'a> {
    pub archive: &'a [u8],
    pub sidecar: &'a [u8],
    pub target: Target,
    pub destination: PathBuf,
}

/// Verifies and stages the new executable beside `destination`.
///
/// Returns the staged path. Nothing here touches `destination`: staging is a
/// write to a sibling name, so a failure at any point leaves the running
/// executable exactly as it was. The caller performs the rename-aside and the
/// place, which is the only part that must be atomic.
pub fn verify_and_stage(input: InstallInput<'_>) -> Result<PathBuf, Failure> {
    let expected =
        parse_sidecar(input.sidecar).map_err(|cause| Failure::new(FailureKind::Checksum, cause))?;
    let actual = sha256_hex(input.archive);
    if actual != expected {
        return Err(Failure::new(
            FailureKind::Checksum,
            "downloaded archive does not match the release checksum",
        ));
    }
    let executable = match input.target.extension {
        ".zip" => update_archive::read_zip(
            input.archive,
            input.target.executable,
            ARCHIVE_DECODED_LIMIT,
        ),
        ".tar.gz" => update_archive::read_tar_gz(
            input.archive,
            input.target.executable,
            ARCHIVE_DECODED_LIMIT,
        ),
        other => {
            return Err(Failure::new(
                FailureKind::Archive,
                format!("no reader for {other} archives"),
            ));
        }
    }
    .map_err(|error| Failure::new(FailureKind::Archive, error.to_string()))?;
    if executable.is_empty() {
        return Err(Failure::new(
            FailureKind::Archive,
            "the archive's executable is empty",
        ));
    }
    let staged = staged_path(&input.destination);
    minicon_platform::filesystem_publish::write_file_atomic(&staged, |file| {
        use std::io::Write as _;
        file.write_all(&executable)
    })
    .map_err(|error| Failure::new(FailureKind::Staging, format!("{error}")))?;
    apply_staged_mode(&staged, &input.destination)
        .map_err(|cause| Failure::new(FailureKind::Staging, cause))?;
    Ok(staged)
}

/// Makes the staged executable runnable on the hosts that decide that with a
/// file mode.
///
/// Independent of [`staged_path`] used above on purpose: the staged file is
/// written by a generic atomic writer that deliberately creates a plain
/// private file, and `read_tar_gz` reads only the member's *bytes*, so nothing
/// on the Unix path would otherwise carry the tar header's mode. The
/// destination is the running product, which is executable by definition:
/// copying its mode is what keeps the install launchable, and it preserves
/// whatever mode the user had chosen for it.
#[cfg(unix)]
fn apply_staged_mode(staged: &Path, destination: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = match std::fs::metadata(destination) {
        Ok(metadata) => metadata.permissions().mode(),
        // The destination could not be stat'ed. Refusing is wrong -- the swap
        // would otherwise install a file that cannot run -- and guessing 0644
        // is worse, so fall back to the mode a released binary always has.
        Err(_) => 0o755,
    };
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(mode))
        .map_err(|error| format!("could not make the staged copy executable: {error}"))
}

/// Windows decides "executable" by the file's extension, not by a mode, so the
/// staged sibling needs nothing here.
#[cfg(not(unix))]
fn apply_staged_mode(_staged: &Path, _destination: &Path) -> Result<(), String> {
    Ok(())
}

/// The sibling the new executable is staged as.
pub fn staged_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(".update");
    destination.with_file_name(name)
}

/// Removes a staged copy a previous session left behind.
///
/// `Ready` is session-scoped — nothing carries it across a restart — so a
/// `*.update` beside the executable at startup is the residue of an update that
/// was staged and then not swapped, which is what happens when a user presses
/// Update and closes the window instead of pressing Restart. Clearing it is
/// what keeps the README's "nothing left behind" true one file later; a session
/// that has not staged anything has nothing to clear.
pub fn clear_stale_staged(destination: &Path) {
    // Best-effort on purpose: a staged file that cannot be removed is not a
    // reason to refuse to start, and the next update overwrites it anyway.
    let _ = remove_if_present(&staged_path(destination));
}

/// The backup the replaced executable is moved aside to.
pub fn backup_path(destination: &Path) -> PathBuf {
    let mut name = destination.file_name().unwrap_or_default().to_os_string();
    name.push(".backup");
    destination.with_file_name(name)
}

/// The name the replaced executable is kept under, for a sentence that has to
/// name it.
///
/// Best-effort by construction: the panel has to say something even when the
/// running executable cannot be resolved, so the fallback is the same suffix
/// [`backup_path`] appends.
#[must_use]
pub fn backup_file_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            backup_path(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "minicon.backup".to_owned())
}

/// Replaces `destination` with the staged executable, keeping the old one.
///
/// The order is the one Windows allows: the running executable can be renamed
/// but not overwritten or deleted, so it is moved aside first and the staged
/// file takes its name. If the second rename fails the first is rolled back, so
/// the caller is never left with no executable at the destination.
pub fn commit_swap(staged: &Path, destination: &Path) -> Result<PathBuf, Failure> {
    let backup = backup_path(destination);
    remove_if_present(&backup).map_err(|cause| Failure::new(FailureKind::Swap, cause))?;
    std::fs::rename(destination, &backup).map_err(|error| {
        Failure::new(
            FailureKind::Swap,
            format!("could not move the current executable aside: {error}"),
        )
    })?;
    if let Err(error) = std::fs::rename(staged, destination) {
        // Put the previous executable back. Its own failure is part of the
        // message: swallowing it would leave the user reading "could not place
        // the new executable" about a directory that no longer holds either.
        return Err(match rollback_swap(&backup, destination) {
            Ok(()) => Failure::new(
                FailureKind::Swap,
                format!("could not place the new executable: {error}"),
            ),
            Err(rollback) => Failure::new(
                FailureKind::Swap,
                format!("could not place the new executable: {error}; and {rollback}"),
            ),
        });
    }
    Ok(backup)
}

/// Puts the executable [`commit_swap`] moved aside back at `destination`.
///
/// Deliberately **not** a second `commit_swap`. That function begins by
/// deleting `backup_path(destination)`, and on this path `backup` *is* that
/// path, so re-entering it deleted the only good copy, moved the new file into
/// its place and then renamed it back — reporting success while leaving the
/// non-launchable file installed and the previous one gone
/// (`plan/archive/plan-v0.2.8.md`, `{ROLLBACK}`). One `rename` is the whole operation.
pub fn rollback_swap(backup: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(backup, destination).map_err(|error| {
        format!(
            "could not restore the previous executable from {}: {error}",
            backup.display()
        )
    })
}

/// Deletes a file the backup replaced, tolerating its absence.
pub fn remove_if_present(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("could not remove {}: {error}", path.display())),
    }
}

/// The release endpoint, with the test override applied.
pub fn release_api() -> String {
    std::env::var(RELEASE_API_ENV).unwrap_or_else(|_| RELEASE_API_DEFAULT.to_owned())
}

/// Runs the blocking half of an update on a worker thread.
///
/// One exchange, one download and one staging operation per command, each with
/// a caller-supplied ceiling. Every failure is an `Event::Failed` carrying a
/// specific cause; the worker never panics the caller and never touches the
/// running executable.
pub fn worker(
    commands: Receiver<Command>,
    events: Sender<Event>,
    destination: PathBuf,
    endpoint: String,
    progress: impl Fn(Progress) + Send + 'static,
) {
    let Some(target) = current_target() else {
        while let Ok(Command::Check) = commands.recv() {
            let _ = events.send(Event::Failed(Failure::new(
                FailureKind::Platform,
                "no release archive is published for this platform",
            )));
        }
        return;
    };
    while let Ok(command) = commands.recv() {
        let outcome = match command {
            Command::Check => check(&events, &endpoint),
            Command::Install => install(&events, &progress, target, &destination, &endpoint),
        };
        if let Err(failure) = outcome {
            let _ = events.send(Event::Failed(failure));
        }
    }
}

/// Reads the latest release and reports its version.
fn check(events: &Sender<Event>, endpoint: &str) -> Result<(), Failure> {
    let body = fetch(endpoint, RELEASE_BODY_LIMIT, CHECK_TIMEOUT)?;
    let release =
        parse_release(&body).map_err(|cause| Failure::new(FailureKind::Release, cause))?;
    let _ = events.send(Event::Checked(release));
    Ok(())
}

/// Downloads the chosen archive, verifies it and stages the executable.
fn install(
    events: &Sender<Event>,
    progress: &impl Fn(Progress),
    target: Target,
    destination: &Path,
    endpoint: &str,
) -> Result<(), Failure> {
    progress(Progress::Downloading);
    let body = fetch(endpoint, RELEASE_BODY_LIMIT, CHECK_TIMEOUT)?;
    let release =
        parse_release(&body).map_err(|cause| Failure::new(FailureKind::Release, cause))?;
    let (archive, sidecar) = release.archive_for(target).ok_or_else(|| {
        Failure::new(
            FailureKind::Platform,
            format!(
                "release {} publishes no archive for this platform",
                release.version
            ),
        )
    })?;
    let sidecar = sidecar
        .ok_or_else(|| Failure::new(FailureKind::Release, "the release has no checksum sidecar"))?;
    let _ = events.send(Event::AssetChosen(archive.name.clone()));
    // macOS cannot replace one file: the product ships as a signed, notarized
    // `.app`, and swapping only the inner Mach-O breaks the signature. Hand the
    // release's own download over instead of pretending to install it.
    if !target.in_place {
        require_secure_url(&archive.url)?;
        progress(Progress::Opening);
        minicon_platform::host_open::open(&archive.url, Default::default()).map_err(|error| {
            Failure::new(
                FailureKind::Transport,
                format!("could not open the download page: {error}"),
            )
        })?;
        let _ = events.send(Event::Staged {
            remote: release.version,
            opens_dmg: true,
        });
        return Ok(());
    }
    let bytes = fetch(&archive.url, ARCHIVE_BODY_LIMIT, DOWNLOAD_TIMEOUT)?;
    progress(Progress::Verifying);
    let checksum = fetch(&sidecar.url, RELEASE_BODY_LIMIT, CHECK_TIMEOUT)?;
    progress(Progress::Staging);
    verify_and_stage(InstallInput {
        archive: &bytes,
        sidecar: &checksum,
        target,
        destination: destination.to_path_buf(),
    })?;
    let _ = events.send(Event::Staged {
        remote: release.version,
        opens_dmg: !target.in_place,
    });
    Ok(())
}

/// Refuses a URL the update path must not speak to.
///
/// The archive's URL is not this product's choice — it is a claim made by
/// whatever answered the release check — so the scheme is checked before any
/// request goes out. See [`REQUIRED_SCHEME`] and [`LOOPBACK_HOSTS`].
fn require_secure_url(url: &str) -> Result<(), Failure> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("HTTPS://"));
    if rest.is_some() {
        return Ok(());
    }
    let refusal = |scheme: &str| {
        Failure::new(
            FailureKind::Transport,
            format!("refused a {scheme} URL for an update; {REQUIRED_SCHEME} is required"),
        )
    };
    for scheme in ["http://", "HTTP://"] {
        if let Some(authority) = url.strip_prefix(scheme) {
            let host = authority_host(authority);
            if LOOPBACK_HOSTS.contains(&host.to_ascii_lowercase().as_str()) {
                return Ok(());
            }
            return Err(refusal(scheme));
        }
    }
    Err(Failure::new(
        FailureKind::Transport,
        format!("refused a URL for an update that is not {REQUIRED_SCHEME}"),
    ))
}

/// The host of a URL authority: userinfo stripped, port stripped, and an IPv6
/// literal returned without its brackets. No 7-bit fallback for a host that is
/// percent-encoded -- such a host is not a loopback literal, so it is refused,
/// which is the safe direction.
fn authority_host(authority: &str) -> &str {
    let authority = authority.split(['/', '?', '#']).next().unwrap_or(authority);
    let authority = authority.rsplit('@').next().unwrap_or(authority);
    if let Some(rest) = authority.strip_prefix('[') {
        return rest.split(']').next().unwrap_or(rest);
    }
    authority.split(':').next().unwrap_or(authority)
}

/// One bounded `GET`, with non-2xx and truncation surfaced as failures.
fn fetch(url: &str, limit: usize, timeout: Duration) -> Result<Vec<u8>, Failure> {
    use minicon_platform::network_http::{
        NetworkHttpMethod, NetworkHttpRequest, request, validate,
    };

    require_secure_url(url)?;
    let http = NetworkHttpRequest::new(NetworkHttpMethod::Get, url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json")
        .timeout(timeout)
        .max_response_bytes(limit);
    validate(&http).map_err(|error| Failure::new(FailureKind::Transport, error.to_string()))?;
    let response =
        request(&http).map_err(|error| Failure::new(FailureKind::Transport, error.to_string()))?;
    if !response.is_success() {
        return Err(Failure::new(
            FailureKind::Transport,
            format!("the release server answered {}", response.status),
        ));
    }
    if response.truncated {
        return Err(Failure::new(
            FailureKind::Transport,
            "the release response was cut short",
        ));
    }
    Ok(response.body)
}

/// Drains the worker's channel into the state, calling `progress` per event.
///
/// Returns `true` when something changed, so the caller knows to repaint.
pub fn drain(
    state: &mut State,
    events: &Receiver<Event>,
    mut report: impl FnMut(Progress),
) -> bool {
    let mut changed = false;
    loop {
        match events.try_recv() {
            Ok(Event::Checked(release)) => {
                state.apply(Event::Checked(release));
                changed = true;
            }
            Ok(Event::AssetChosen(name)) => {
                state.apply(Event::AssetChosen(name));
                changed = true;
            }
            Ok(Event::Staged { remote, opens_dmg }) => {
                state.apply(Event::Staged { remote, opens_dmg });
                changed = true;
            }
            Ok(Event::Failed(failure)) => {
                state.apply(Event::Failed(failure));
                changed = true;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => break,
        }
    }
    if changed {
        report(Progress::Downloading);
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::io::Write as _;
    use std::net::TcpListener;

    /// One served response.
    struct Served {
        status: u16,
        body: Vec<u8>,
        /// Advertise more bytes than are sent, so the client sees a cut-short
        /// body rather than a complete one.
        short: bool,
    }

    impl Served {
        fn ok(body: impl Into<Vec<u8>>) -> Self {
            Self {
                status: 200,
                body: body.into(),
                short: false,
            }
        }

        fn coded(status: u16, body: &str) -> Self {
            Self {
                status,
                body: body.as_bytes().to_vec(),
                short: false,
            }
        }
    }

    /// A loopback host, and the base URL its paths hang off.
    fn loopback() -> (TcpListener, String) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let address = listener.local_addr().expect("loopback address");
        (listener, format!("http://{address}"))
    }

    /// Answers each route once, in the order given.
    fn serve(listener: TcpListener, routes: Vec<(&'static str, Served)>) {
        std::thread::spawn(move || {
            for (_path, served) in routes {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let mut buffer = [0u8; 2048];
                let _ = std::io::Read::read(&mut stream, &mut buffer);
                let length = if served.short {
                    served.body.len() + 64
                } else {
                    served.body.len()
                };
                let _ = stream.write_all(
                    format!(
                        "HTTP/1.1 {} X\r\nContent-Length: {length}\r\nConnection: close\r\n\r\n",
                        served.status
                    )
                    .as_bytes(),
                );
                let _ = stream.write_all(&served.body);
                let _ = stream.flush();
            }
        });
    }

    fn scratch(name: &str) -> std::path::PathBuf {
        let directory = std::env::temp_dir().join(format!("minicon-update-{name}"));
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn fixture_archive() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/update-archive.zip"
        ))
        .unwrap()
    }

    /// The release document, `latest` and nothing else.
    fn latest_for(base: &str, tag: &str) -> Vec<u8> {
        format!(
            r#"{{"tag_name":"{tag}","assets":[
                {{"name":"minicon-9.9.9-windows-x86_64.zip","browser_download_url":"{base}/archive"}},
                {{"name":"minicon-9.9.9-windows-x86_64.zip.sha256","browser_download_url":"{base}/archive.sha256"}}
            ]}}"#
        )
        .into_bytes()
    }

    /// The target the loopback release advertises, independent of the host
    /// running the suite, so the same test works on every cell.
    fn loopback_target() -> Target {
        Target {
            suffix: "windows-x86_64",
            extension: ".zip",
            executable: "minicon.exe",
            in_place: true,
        }
    }

    #[test]
    fn a_check_against_loopback_reads_the_release_version() {
        let (listener, base) = loopback();
        serve(
            listener,
            vec![("latest", Served::ok(latest_for(&base, "v9.9.9")))],
        );
        let (events, received) = std::sync::mpsc::channel();
        check(&events, &format!("{base}/releases/latest")).unwrap();
        let Event::Checked(release) = received.recv().unwrap() else {
            panic!("a check reports the release");
        };
        assert_eq!(release.version, Version::parse("9.9.9").unwrap());
        assert!(release.archive_for(loopback_target()).is_some());
    }

    #[test]
    fn a_rate_limited_answer_is_a_failure_not_a_partial_success() {
        let (listener, base) = loopback();
        serve(
            listener,
            vec![("latest", Served::coded(403, "rate limited"))],
        );
        let (events, _received) = std::sync::mpsc::channel();
        let error = check(&events, &format!("{base}/releases/latest")).unwrap_err();
        assert_eq!(error.kind, FailureKind::Transport, "{error}");
        assert!(error.cause.contains("403"), "{error}");
    }

    #[test]
    fn a_truncated_answer_is_a_failure() {
        let (listener, base) = loopback();
        let mut short = Served::ok(latest_for(&base, "v9.9.9"));
        short.short = true;
        serve(listener, vec![("latest", short)]);
        let (events, _received) = std::sync::mpsc::channel();
        let error = check(&events, &format!("{base}/releases/latest")).unwrap_err();
        assert!(!error.cause.is_empty());
    }

    #[test]
    fn an_install_from_loopback_stages_a_verified_executable() {
        let directory = scratch("loopback-install");
        let destination = directory.join("minicon.exe");
        let archive = fixture_archive();
        let (listener, base) = loopback();
        serve(
            listener,
            vec![
                ("latest", Served::ok(latest_for(&base, "v9.9.9"))),
                ("archive", Served::ok(archive.clone())),
                (
                    "archive.sha256",
                    Served::ok(format!("{}  archive\n", sha256_hex(&archive)).into_bytes()),
                ),
            ],
        );
        let (events, received) = std::sync::mpsc::channel();
        install(
            &events,
            &|_| {},
            loopback_target(),
            &destination,
            &format!("{base}/releases/latest"),
        )
        .unwrap();
        let mut staged = false;
        while let Ok(event) = received.try_recv() {
            if let Event::Staged { remote, opens_dmg } = event {
                assert_eq!(remote, Version::parse("9.9.9").unwrap());
                assert!(!opens_dmg);
                staged = true;
            }
        }
        assert!(staged, "a verified install reports the staged version");
        assert_eq!(
            std::fs::read(staged_path(&destination)).unwrap(),
            b"minicon-update-fixture-payload\n"
        );
        assert!(!destination.exists(), "the destination is untouched");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn an_install_whose_checksum_disagrees_stages_nothing() {
        let directory = scratch("loopback-mismatch");
        let destination = directory.join("minicon.exe");
        let (listener, base) = loopback();
        serve(
            listener,
            vec![
                ("latest", Served::ok(latest_for(&base, "v9.9.9"))),
                ("archive", Served::ok(fixture_archive())),
                (
                    "archive.sha256",
                    Served::ok(format!("{}  archive\n", "b".repeat(64)).into_bytes()),
                ),
            ],
        );
        let (events, _received) = std::sync::mpsc::channel();
        let error = install(
            &events,
            &|_| {},
            loopback_target(),
            &destination,
            &format!("{base}/releases/latest"),
        )
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::Checksum, "{error}");
        assert!(error.cause.contains("checksum"), "{error}");
        assert!(!staged_path(&destination).exists());
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// The real endpoint, opt-in: `cargo test -- --ignored`.
    ///
    /// Proves the document GitHub actually serves still parses into the two
    /// fields this product reads. Deliberately not part of the default gate:
    /// CI has no reason to depend on GitHub being reachable, and a rate-limited
    /// runner would fail for a reason that is not this product's.
    #[test]
    #[ignore = "hits the live GitHub release API"]
    fn the_real_release_api_still_parses() {
        let (events, received) = std::sync::mpsc::channel();
        check(&events, RELEASE_API_DEFAULT).unwrap();
        let Event::Checked(release) = received.recv().unwrap() else {
            panic!("a check reports the release");
        };
        assert!(
            release.version >= Version::parse("0.1.0").unwrap(),
            "the release version parsed to {}",
            release.version
        );
        assert!(
            !release.assets.is_empty(),
            "a released version publishes assets"
        );
    }

    fn release_json(tag: &str, names: &[&str]) -> Vec<u8> {
        let assets: Vec<String> = names
            .iter()
            .map(|name| {
                format!(
                    r#"{{"name":"{name}","browser_download_url":"https://example.test/{name}"}}"#
                )
            })
            .collect();
        format!(r#"{{"tag_name":"{tag}","assets":[{}]}}"#, assets.join(",")).into_bytes()
    }

    #[test]
    fn the_newest_release_wins_by_semantics() {
        let mut state = State::new(Version::parse("0.2.9").unwrap());
        state.apply(Event::Checked(
            parse_release(&release_json("v0.2.10", &[])).unwrap(),
        ));
        assert_eq!(
            state.phase,
            Phase::Available {
                remote: Version::parse("0.2.10").unwrap()
            }
        );
        let published = snapshot(&state);
        assert_eq!(
            (
                published.phase,
                published.remote.as_deref(),
                published.cause.as_deref(),
                published.failure,
                published.handoff,
            ),
            ("available", Some("0.2.10"), None, None, false)
        );
    }

    /// A failure publishes both halves of what a reader needs: the stable kind
    /// for automation, and the cause for a bug report. Neither is a painted
    /// string, so the panel stays free to translate.
    #[test]
    fn a_failure_publishes_its_kind_and_its_cause() {
        let mut state = State::new(Version::parse("0.2.8").unwrap());
        state.apply(Event::Failed(Failure::new(
            FailureKind::Checksum,
            "downloaded archive does not match the release checksum",
        )));
        let published = snapshot(&state);
        assert_eq!(published.phase, "failed");
        assert_eq!(published.failure, Some("checksum"));
        assert_eq!(
            published.cause.as_deref(),
            Some("downloaded archive does not match the release checksum")
        );
        assert!(state.can_check(), "a failure is not a dead end");
    }

    /// Every kind answers whether the install still runs -- the fact the old
    /// panel never gave, and the one a user has to know before retrying.
    #[test]
    fn every_failure_kind_states_whether_the_install_still_runs() {
        use InstallState as State;
        let expected = [
            (FailureKind::Platform, State::Untouched),
            (FailureKind::Release, State::Untouched),
            (FailureKind::Transport, State::Untouched),
            (FailureKind::Checksum, State::Untouched),
            (FailureKind::Archive, State::Untouched),
            (FailureKind::Staging, State::Untouched),
            (FailureKind::Swap, State::Untouched),
            (FailureKind::Launch, State::Restored),
            (FailureKind::LaunchUnrestored, State::Unknown),
        ];
        for (kind, state) in expected {
            assert_eq!(kind.install_state(), state, "{kind:?}");
            assert!(!kind.token().is_empty(), "{kind:?}");
        }
    }

    /// The token is what automation matches, so two kinds may never share one.
    #[test]
    fn failure_tokens_are_unique() {
        let tokens: Vec<&str> = [
            FailureKind::Platform,
            FailureKind::Release,
            FailureKind::Transport,
            FailureKind::Checksum,
            FailureKind::Archive,
            FailureKind::Staging,
            FailureKind::Swap,
            FailureKind::Launch,
            FailureKind::LaunchUnrestored,
        ]
        .into_iter()
        .map(FailureKind::token)
        .collect();
        let mut unique = tokens.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), tokens.len(), "{tokens:?}");
    }

    /// The update path speaks https. A plaintext URL is refused *by the scheme
    /// check*, which is observable: a request that actually went out would come
    /// back as a connect failure, and `192.0.2.1` (TEST-NET-1) is unrouted, so
    /// the two outcomes cannot be confused.
    #[test]
    fn a_plaintext_release_url_is_refused_by_the_scheme_check() {
        let (events, _received) = std::sync::mpsc::channel();
        let error = match check(&events, "http://192.0.2.1/releases/latest") {
            Ok(()) => panic!("an http release URL must be refused"),
            Err(error) => error,
        };
        assert_eq!(error.kind, FailureKind::Transport, "{error}");
        assert!(error.cause.contains("https"), "{error}");
        assert!(
            error.cause.contains("refused"),
            "the refusal must say it refused, not that the network failed: {error}"
        );
    }

    /// The loopback exemption is a test seam and stays a seam: without it the
    /// suite's own release server dies with the policy.
    #[test]
    fn a_loopback_http_url_is_the_one_exemption() {
        assert!(require_secure_url("https://example.test/x").is_ok());
        assert!(require_secure_url("http://127.0.0.1:8000/x").is_ok());
        assert!(require_secure_url("http://localhost:8000/x").is_ok());
        assert!(require_secure_url("http://user@localhost:8000/x").is_ok());
        assert!(require_secure_url("http://[::1]:8000/x").is_ok());
        assert!(require_secure_url("http://example.test/x").is_err());
        assert!(require_secure_url("ftp://example.test/x").is_err());
        assert!(
            require_secure_url("http://127.0.0.1.example.test/x").is_err(),
            "a host that merely starts with a loopback literal is not loopback"
        );
    }

    #[test]
    fn a_build_ahead_of_the_release_is_not_nudged_to_downgrade() {
        let mut state = State::new(Version::parse("0.2.8").unwrap());
        state.apply(Event::Checked(
            parse_release(&release_json("v0.2.7", &[])).unwrap(),
        ));
        assert_eq!(
            state.phase,
            Phase::Ahead {
                remote: Version::parse("0.2.7").unwrap()
            }
        );
    }

    #[test]
    fn the_same_version_is_up_to_date() {
        let mut state = State::new(Version::parse("0.2.7").unwrap());
        state.apply(Event::Checked(
            parse_release(&release_json("0.2.7", &[])).unwrap(),
        ));
        assert_eq!(state.phase, Phase::UpToDate);
    }

    #[test]
    fn a_tag_that_is_not_a_stable_version_is_refused() {
        let error = parse_release(&release_json("v0.2.7-rc1", &[])).unwrap_err();
        assert!(error.contains("stable version"), "{error}");
    }

    #[test]
    fn the_archive_is_chosen_by_pattern_not_by_index() {
        let release = parse_release(&release_json(
            "v0.2.7",
            &[
                "minicon-0.2.7-windows-x86_64.zip.sha256",
                "minicon-0.2.7-windows-aarch64.zip",
                "minicon-0.2.7-windows-x86_64.zip",
                "minicon-0.2.7-linux-x86_64.tar.gz",
            ],
        ))
        .unwrap();
        let target = Target {
            suffix: "windows-x86_64",
            extension: ".zip",
            executable: "minicon.exe",
            in_place: true,
        };
        let (archive, sidecar) = release.archive_for(target).unwrap();
        assert_eq!(archive.name, "minicon-0.2.7-windows-x86_64.zip");
        assert_eq!(
            sidecar.map(|asset| asset.name.as_str()),
            Some("minicon-0.2.7-windows-x86_64.zip.sha256")
        );
        assert!(
            release
                .archive_for(Target {
                    suffix: "solaris-sparc",
                    ..target
                })
                .is_none()
        );
    }

    #[test]
    fn a_sidecar_keeps_only_the_digest() {
        let digest = "0".repeat(64);
        assert_eq!(
            parse_sidecar(format!("{digest}  minicon-0.2.7.zip\n").as_bytes()),
            Ok(digest.clone())
        );
        assert!(parse_sidecar(b"not a digest  file.zip").is_err());
        assert!(parse_sidecar(format!("{digest}\n").as_bytes()).is_ok());
    }

    #[test]
    fn a_sha_mismatch_refuses_before_anything_is_staged() {
        let directory = std::env::temp_dir().join("minicon-update-mismatch");
        let _ = std::fs::create_dir_all(&directory);
        let destination = directory.join("minicon.exe");
        let error = verify_and_stage(InstallInput {
            archive: b"not the archive the sidecar describes",
            sidecar: format!("{}  x\n", "a".repeat(64)).as_bytes(),
            target: Target {
                suffix: "windows-x86_64",
                extension: ".zip",
                executable: "minicon.exe",
                in_place: true,
            },
            destination: destination.clone(),
        })
        .unwrap_err();
        assert_eq!(error.kind, FailureKind::Checksum, "{error}");
        assert!(error.cause.contains("checksum"), "{error}");
        assert!(!staged_path(&destination).exists(), "nothing may be staged");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_verified_archive_stages_beside_the_destination() {
        let directory = std::env::temp_dir().join("minicon-update-stage");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon.exe");
        let archive = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/update-archive.zip"
        ))
        .unwrap();
        let sidecar = format!("{}  fixture\n", sha256_hex(&archive));
        let staged = verify_and_stage(InstallInput {
            archive: &archive,
            sidecar: sidecar.as_bytes(),
            target: Target {
                suffix: "windows-x86_64",
                extension: ".zip",
                executable: "minicon.exe",
                in_place: true,
            },
            destination: destination.clone(),
        })
        .unwrap();
        assert_eq!(staged, staged_path(&destination));
        assert_eq!(
            std::fs::read(&staged).unwrap(),
            b"minicon-update-fixture-payload\n"
        );
        assert!(!destination.exists(), "the destination is untouched");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn the_swap_keeps_the_old_executable_and_rolls_back_on_failure() {
        let directory = std::env::temp_dir().join("minicon-update-swap");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon.exe");
        std::fs::write(&destination, b"old").unwrap();
        let staged = staged_path(&destination);
        std::fs::write(&staged, b"new").unwrap();
        let backup = commit_swap(&staged, &destination).unwrap();
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        assert_eq!(std::fs::read(&backup).unwrap(), b"old");
        // Placing a staged file that is not there must leave the old one alone.
        let missing = directory.join("absent.exe");
        assert!(commit_swap(&missing, &destination).is_err());
        assert_eq!(std::fs::read(&destination).unwrap(), b"new");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `{ROLLBACK}`, asserted on the **final on-disk state** rather than on the
    /// return value — the defect this replaces returned `Ok` while leaving the
    /// non-launchable file installed and the previous one deleted, so a test
    /// that stopped at the call would have passed.
    #[test]
    fn a_failed_launch_puts_the_previous_executable_back_on_disk() {
        let directory = std::env::temp_dir().join("minicon-update-rollback");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon.exe");
        std::fs::write(&destination, b"old-and-good").unwrap();
        let staged = staged_path(&destination);
        std::fs::write(&staged, b"new-and-broken").unwrap();

        let backup = commit_swap(&staged, &destination).unwrap();
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"new-and-broken",
            "the swap is what puts the new file in place"
        );
        rollback_swap(&backup, &destination).expect("the previous executable goes back");

        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"old-and-good",
            "the destination must hold the old bytes again"
        );
        assert!(
            !backup.exists(),
            "the rollback consumes its own backup; nothing is left behind"
        );
        assert!(
            !staged.exists(),
            "the file that would not launch must not survive anywhere"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A rollback that cannot do its job must say so rather than reporting a
    /// state it did not reach.
    #[test]
    fn a_rollback_with_no_backup_reports_itself() {
        let directory = std::env::temp_dir().join("minicon-update-rollback-missing");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon.exe");
        let error = rollback_swap(&backup_path(&destination), &destination).unwrap_err();
        assert!(error.contains("restore"), "{error}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// A staged file that was never swapped must not outlive the session that
    /// made it: `Ready` does not survive a restart, so leaving the file would
    /// be the one thing the product's own sentence says it does not do.
    #[test]
    fn a_staged_file_from_a_previous_session_is_cleared_at_startup() {
        let directory = std::env::temp_dir().join("minicon-update-stale");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon.exe");
        std::fs::write(&destination, b"running").unwrap();
        std::fs::write(staged_path(&destination), b"never swapped").unwrap();

        clear_stale_staged(&destination);

        assert!(!staged_path(&destination).exists(), "the residue is gone");
        assert_eq!(
            std::fs::read(&destination).unwrap(),
            b"running",
            "the executable itself is untouched"
        );
        // A directory with nothing staged is the ordinary case, and must not
        // be mistaken for a failure.
        clear_stale_staged(&destination);
        let _ = std::fs::remove_dir_all(&directory);
    }

    /// `{EXECBIT}`. Unix decides "runnable" with a file mode and the staged
    /// file is written by a mode-less atomic writer, so without this an
    /// in-place update installs a file the OS refuses to run. Windows has no
    /// such concept, which is why this is `cfg(unix)` — the Linux cells of
    /// `six-cell-qualify.sh` are where it runs.
    #[cfg(unix)]
    #[test]
    fn the_staged_executable_carries_the_destinations_mode() {
        use std::os::unix::fs::PermissionsExt as _;

        let directory = std::env::temp_dir().join("minicon-update-mode");
        let _ = std::fs::remove_dir_all(&directory);
        std::fs::create_dir_all(&directory).unwrap();
        let destination = directory.join("minicon");
        std::fs::write(&destination, b"old").unwrap();
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700)).unwrap();

        let archive = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/update-archive.zip"
        ))
        .unwrap();
        let sidecar = format!("{}  fixture\n", sha256_hex(&archive));
        let staged = verify_and_stage(InstallInput {
            archive: &archive,
            sidecar: sidecar.as_bytes(),
            target: Target {
                suffix: "windows-x86_64",
                extension: ".zip",
                executable: "minicon.exe",
                in_place: true,
            },
            destination: destination.clone(),
        })
        .unwrap();

        let mode = std::fs::metadata(&staged).unwrap().permissions().mode();
        assert_eq!(
            mode & 0o7777,
            0o700,
            "the staged copy must inherit the destination's mode, not the \
             writer's 0644"
        );
        assert!(
            mode & 0o111 != 0,
            "a staged executable with no execute bit is the defect"
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
