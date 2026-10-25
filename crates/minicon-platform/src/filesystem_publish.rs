//! Recoverable publication of caller-prepared filesystem directories.

use std::{
    ffi::OsString,
    fmt, fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

static BACKUP_SERIAL: AtomicU64 = AtomicU64::new(0);
static FILE_SERIAL: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum FilePublishErrorKind {
    InvalidInput,
    Inspect,
    Create,
    Write,
    SyncFile,
    Install,
    Remove,
    Durability,
}

#[derive(Debug)]
pub struct FilePublishError {
    kind: FilePublishErrorKind,
    detail: String,
    published: bool,
    source: Option<io::Error>,
}

impl FilePublishError {
    pub const fn kind(&self) -> FilePublishErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// True only when complete replacement bytes were installed or intended
    /// absence was published, but the operation's durability barrier failed or
    /// is unsupported. Crash durability is then unconfirmed; do not report the
    /// old bytes/name as still applied.
    pub const fn published(&self) -> bool {
        self.published
    }

    fn new(kind: FilePublishErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            published: false,
            source: None,
        }
    }

    /// Original I/O category when an operation supplied one. Logical input
    /// errors retain InvalidInput; other errors without an I/O source use Other.
    pub fn io_kind(&self) -> io::ErrorKind {
        self.source.as_ref().map_or_else(
            || {
                if self.kind == FilePublishErrorKind::InvalidInput {
                    io::ErrorKind::InvalidInput
                } else {
                    io::ErrorKind::Other
                }
            },
            io::Error::kind,
        )
    }

    fn from_io(kind: FilePublishErrorKind, step: &str, error: io::Error) -> Self {
        Self {
            kind,
            detail: format!("{step}: {error}"),
            published: false,
            source: Some(error),
        }
    }

    // Legacy private writes returned inspection/create OS errors directly.
    // Keep raw_os_error for those steps, and retain the typed boundary on
    // wrapped write/sync/install/durability failures.
    #[cfg(feature = "filesystem")]
    pub(crate) fn into_private_io(mut self) -> io::Error {
        if matches!(
            self.kind,
            FilePublishErrorKind::Inspect | FilePublishErrorKind::Create
        ) && let Some(source) = self.source.take()
        {
            return source;
        }
        self.into()
    }

    fn after_publish(mut self) -> Self {
        self.published = true;
        self
    }
}

impl fmt::Display for FilePublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for FilePublishError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|error| error as &dyn std::error::Error)
    }
}

impl From<FilePublishError> for io::Error {
    fn from(error: FilePublishError) -> Self {
        Self::new(error.io_kind(), error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FilePublishOutcome {
    replaced_existing: bool,
}

impl FilePublishOutcome {
    pub const fn replaced_existing(self) -> bool {
        self.replaced_existing
    }
}

/// Atomically installs a caller-prepared regular file over `destination`.
///
/// Both entries must have one physical parent. Success consumes `staging` and
/// guarantees that observers see either the complete old file or the complete
/// new file. A `Durability` error reports `published() == true`: replacement
/// completed, but the Unix parent-directory sync failed. Windows replacement
/// uses the adapter's write-through barrier.
pub fn publish_file(
    staging: &Path,
    destination: &Path,
) -> Result<FilePublishOutcome, FilePublishError> {
    let destination = normalized_destination(destination)?;
    let destination_parent = destination.parent().expect("normalized parent");
    let staging_metadata = fs::symlink_metadata(staging).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("inspect prepared file failed: {error}"),
        )
    })?;
    if !staging_metadata.file_type().is_file() || staging_metadata.file_type().is_symlink() {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "staging must be a real regular file entry",
        ));
    }
    let staging_parent = staging.parent().filter(|path| !path.as_os_str().is_empty());
    let staging_parent = fs::canonicalize(staging_parent.unwrap_or_else(|| Path::new(".")))
        .map_err(|error| {
            FilePublishError::new(
                FilePublishErrorKind::Inspect,
                format!("inspect staging parent failed: {error}"),
            )
        })?;
    if staging_parent != destination_parent {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "staging and destination must share one physical parent directory",
        ));
    }
    if fs::canonicalize(staging).ok().as_deref() == Some(destination.as_path()) {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "staging and destination must be distinct file entries",
        ));
    }
    let replaced_existing = match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_file() && !metadata.file_type().is_symlink() => {
            true
        }
        Ok(_) => {
            return Err(FilePublishError::new(
                FilePublishErrorKind::InvalidInput,
                "an existing destination must be a real regular file entry",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(FilePublishError::new(
                FilePublishErrorKind::Inspect,
                format!("inspect destination file failed: {error}"),
            ));
        }
    };
    publish_file_with(
        staging,
        &destination,
        destination_parent,
        replaced_existing,
        crate::selected::filesystem_publish::replace_file,
        crate::selected::filesystem_publish::sync_parent,
    )
}

/// Makes a staged executable runnable on the hosts that decide that with a
/// file mode. Unix copies the destination's mode (falling back to 0o755 when
/// the destination cannot be stat'ed) onto the staged sibling; Windows
/// decides by extension and needs nothing. The single seam the product's
/// updater calls, so no `std::os` import lives above this crate.
pub fn copy_executable_mode(staged: &Path, destination: &Path) -> Result<(), FilePublishError> {
    crate::selected::filesystem_publish::copy_executable_mode(staged, destination).map_err(
        |error| {
            FilePublishError::new(
                FilePublishErrorKind::Install,
                format!("could not make the staged copy executable: {error}"),
            )
        },
    )
}

/// Creates the `install-cli` symlink: a plain link into a user-named prefix.
/// Only meaningful on Unix; Windows answers a clear error instead so product
/// code needs no platform branch for this mechanism.
pub fn install_cli_symlink(target: &Path, link: &Path) -> Result<(), FilePublishError> {
    crate::selected::filesystem_publish::install_cli_symlink(target, link).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Install,
            format!(
                "could not create {link_display}: {error}",
                link_display = link.display()
            ),
        )
    })
}

/// Creates, writes, synchronizes and atomically publishes a unique sibling
/// temporary file. The temporary is removed on every pre-publication failure.
pub fn write_file_atomic<T>(
    destination: &Path,
    write: impl FnOnce(&mut fs::File) -> io::Result<T>,
) -> Result<T, FilePublishError> {
    let destination = owned_destination(destination)?;
    let parent = destination.parent().expect("normalized parent");
    let name = destination.file_name().expect("normalized file name");
    let (temporary, mut file) = create_temporary(parent, name)?;
    let mut cleanup = TemporaryFile::new(temporary.clone());
    let value = write(&mut file).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Write,
            format!("write prepared file failed: {error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::SyncFile,
            format!("sync prepared file failed: {error}"),
        )
    })?;
    drop(file);
    publish_reserved_sibling(&temporary, &destination)?;
    cleanup.disarm();
    Ok(value)
}

/// Atomically publish private bytes with an explicit publication boundary.
///
/// The caller creates and protects the parent with
/// [`crate::filesystem::protect_private_directory`]. Exclusive staging requests
/// Unix 0600 or inherits the protected Windows parent ACL. Real regular entries,
/// file synchronization, replacement and parent synchronization are checked.
/// A failure with `published() == true` means the complete new bytes are already
/// installed but the parent durability barrier failed; do not roll back live
/// state as if the old bytes remained. False means this call did not publish,
/// without making a claim about independent writers. Ownership:
/// `prd/PRD_02_28_shared_core.md`.
#[cfg(feature = "filesystem")]
pub fn write_private_atomic(destination: &Path, bytes: &[u8]) -> Result<(), FilePublishError> {
    write_private_atomic_with(
        destination,
        bytes,
        crate::selected::filesystem_publish::replace_file,
        crate::selected::filesystem_publish::sync_parent,
    )
}

#[cfg(feature = "filesystem")]
fn write_private_atomic_with<R, S>(
    destination: &Path,
    bytes: &[u8],
    replace: R,
    sync_parent: S,
) -> Result<(), FilePublishError>
where
    R: FnOnce(&Path, &Path) -> io::Result<()>,
    S: FnOnce(&Path) -> io::Result<()>,
{
    static NEXT_TEMPORARY: AtomicU64 = AtomicU64::new(1);

    let parent = destination.parent().ok_or_else(|| {
        FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "private atomic target requires a parent directory",
        )
    })?;
    let name = destination.file_name().ok_or_else(|| {
        FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "private atomic target requires a file name",
        )
    })?;
    match fs::symlink_metadata(destination) {
        Ok(metadata)
            if !crate::filesystem_entry::metadata_entry_facts(&metadata).is_real_file() =>
        {
            return Err(FilePublishError::new(
                FilePublishErrorKind::InvalidInput,
                "private atomic target must be a real regular file entry",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(FilePublishError::from_io(
                FilePublishErrorKind::Inspect,
                "inspect private atomic target failed",
                error,
            ));
        }
    }
    let mut created = None;
    for _ in 0..32 {
        let mut temporary_name = OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(
            ".platform-private-{}-{}",
            std::process::id(),
            NEXT_TEMPORARY.fetch_add(1, Ordering::Relaxed)
        ));
        let temporary = parent.join(temporary_name);
        match crate::filesystem::private_create_new_options().open(&temporary) {
            Ok(file) => {
                created = Some((temporary, file));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(FilePublishError::from_io(
                    FilePublishErrorKind::Create,
                    "create private atomic temporary failed",
                    error,
                ));
            }
        }
    }
    let (temporary, mut file) = created.ok_or_else(|| {
        FilePublishError::from_io(
            FilePublishErrorKind::Create,
            "create private atomic temporary failed",
            io::Error::new(
                io::ErrorKind::AlreadyExists,
                "private temporary name attempts exhausted",
            ),
        )
    })?;
    let mut cleanup = TemporaryFile::new(temporary.clone());
    io::Write::write_all(&mut file, bytes).map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::Write,
            "write private atomic temporary failed",
            error,
        )
    })?;
    file.sync_all().map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::SyncFile,
            "sync private atomic temporary failed",
            error,
        )
    })?;
    drop(file);
    publish_reserved_sibling_with(&temporary, destination, replace, sync_parent)?;
    cleanup.disarm();
    Ok(())
}

/// The existing name was absent; no new durability barrier is claimed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateRemoveOutcome {
    Removed,
    NotFound,
}

/// Publishes intended absence of a private regular file. This is a real remove,
/// not an empty-byte replacement or secure erase. The caller owns/protects the
/// parent; independent concurrent writers are outside this operation's claim.
///
/// A published error means absence was made effective but crash durability was
/// not confirmed. Unix checks parent sync. Windows has no established removal
/// barrier and reports that uncertainty instead of reusing the replacement
/// adapter's no-op parent sync. NotFound makes no new durability assertion.
#[cfg(feature = "filesystem")]
pub fn remove_private_file(destination: &Path) -> Result<PrivateRemoveOutcome, FilePublishError> {
    remove_private_file_with(
        destination,
        |path| fs::remove_file(path),
        crate::selected::filesystem_publish::sync_removed_parent,
    )
}

#[cfg(feature = "filesystem")]
fn remove_private_file_with(
    destination: &Path,
    remove: impl FnOnce(&Path) -> io::Result<()>,
    sync_parent: impl FnOnce(&Path) -> io::Result<()>,
) -> Result<PrivateRemoveOutcome, FilePublishError> {
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .ok_or_else(|| {
            FilePublishError::new(
                FilePublishErrorKind::InvalidInput,
                "private remove requires a parent directory",
            )
        })?;
    if destination.file_name().is_none() {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "private remove requires a file name",
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(metadata)
            if !crate::filesystem_entry::metadata_entry_facts(&metadata).is_real_file() =>
        {
            return Err(FilePublishError::new(
                FilePublishErrorKind::InvalidInput,
                "private remove target must be a real regular file entry",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PrivateRemoveOutcome::NotFound);
        }
        Err(error) => {
            return Err(FilePublishError::from_io(
                FilePublishErrorKind::Inspect,
                "inspect private remove target failed",
                error,
            ));
        }
    }
    match remove(destination) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(PrivateRemoveOutcome::NotFound);
        }
        Err(error) => {
            return Err(FilePublishError::from_io(
                FilePublishErrorKind::Remove,
                "remove private target failed",
                error,
            ));
        }
    }
    sync_parent(parent).map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::Durability,
            "sync removed private target parent failed",
            error,
        )
        .after_publish()
    })?;
    Ok(PrivateRemoveOutcome::Removed)
}

/// Lets a path-oriented encoder fill a reserved sibling temporary, then
/// synchronizes and atomically publishes that complete file.
///
/// The callback may replace or truncate the reserved regular file, but the
/// resulting entry is revalidated before publication. This is the path-based
/// counterpart to [`write_file_atomic`] for native codecs that cannot consume
/// a Rust [`fs::File`].
pub fn write_path_atomic<T>(
    destination: &Path,
    write: impl FnOnce(&Path) -> io::Result<T>,
) -> Result<T, FilePublishError> {
    let destination = owned_destination(destination)?;
    let parent = destination.parent().expect("normalized parent");
    let name = destination.file_name().expect("normalized file name");
    let (temporary, file) = create_temporary(parent, name)?;
    let mut cleanup = TemporaryFile::new(temporary.clone());
    drop(file);
    let value = write(&temporary).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Write,
            format!("write prepared path failed: {error}"),
        )
    })?;
    fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .and_then(|file| file.sync_all())
        .map_err(|error| {
            FilePublishError::new(
                FilePublishErrorKind::SyncFile,
                format!("sync prepared path failed: {error}"),
            )
        })?;
    publish_reserved_sibling(&temporary, &destination)?;
    cleanup.disarm();
    Ok(value)
}

/// Creates a complete sibling file and atomically installs it only when the
/// destination name is absent.
///
/// The final install is one no-replace filesystem operation, so an existing
/// regular file, symlink, or Windows reparse point is never followed or
/// replaced. The temporary is removed on every pre-publication failure.
pub fn write_path_atomic_no_clobber<T>(
    destination: &Path,
    write: impl FnOnce(&Path) -> io::Result<T>,
) -> Result<T, FilePublishError> {
    let destination = owned_destination(destination)?;
    let parent = destination.parent().expect("normalized parent");
    let name = destination.file_name().expect("normalized file name");
    let (temporary, file) = create_temporary(parent, name)?;
    let _cleanup = TemporaryFile::new(temporary.clone());
    let reserved_identity = crate::file_identity::file_identity(&file).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("inspect reserved file identity failed: {error}"),
        )
    })?;
    let value = write(&temporary).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Write,
            format!("write prepared path failed: {error}"),
        )
    })?;
    file.sync_all().map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::SyncFile,
            format!("sync reserved file handle failed: {error}"),
        )
    })?;
    let staging_metadata = fs::symlink_metadata(&temporary).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("inspect prepared file failed: {error}"),
        )
    })?;
    if !staging_metadata.file_type().is_file() || staging_metadata.file_type().is_symlink() {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "prepared path must remain a real regular file entry",
        ));
    }
    let prepared_identity = crate::file_identity::path_identity(&temporary).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("revalidate prepared file identity failed: {error}"),
        )
    })?;
    if !reserved_identity.same_object(prepared_identity) {
        return Err(FilePublishError::new(
            FilePublishErrorKind::Inspect,
            "path encoder replaced the reserved temporary file object",
        ));
    }
    crate::selected::filesystem_publish::install_file_no_replace(&temporary, &destination)
        .map_err(|error| {
            FilePublishError::new(
                FilePublishErrorKind::Install,
                format!("install prepared file without replacement failed: {error}"),
            )
        })?;
    crate::selected::filesystem_publish::sync_parent(parent).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Durability,
            format!("sync published file parent failed: {error}"),
        )
        .after_publish()
    })?;
    Ok(value)
}

/// Publishes a temporary created by `create_temporary` beside an already
/// normalized destination. Unlike the public `publish_file`, this path does
/// not need to rediscover whether two caller-owned paths share a physical
/// parent: both names were derived from the same canonical parent here.
fn publish_reserved_sibling(
    staging: &Path,
    destination: &Path,
) -> Result<FilePublishOutcome, FilePublishError> {
    publish_reserved_sibling_with(
        staging,
        destination,
        crate::selected::filesystem_publish::replace_file,
        crate::selected::filesystem_publish::sync_parent,
    )
}

fn publish_reserved_sibling_with<R, S>(
    staging: &Path,
    destination: &Path,
    replace: R,
    sync_parent: S,
) -> Result<FilePublishOutcome, FilePublishError>
where
    R: FnOnce(&Path, &Path) -> io::Result<()>,
    S: FnOnce(&Path) -> io::Result<()>,
{
    let staging_metadata = fs::symlink_metadata(staging).map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::Inspect,
            "inspect prepared file failed",
            error,
        )
    })?;
    if !crate::filesystem_entry::metadata_entry_facts(&staging_metadata).is_real_file() {
        return Err(FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "prepared path must remain a real regular file entry",
        ));
    }
    let replaced_existing = match fs::symlink_metadata(destination) {
        Ok(metadata) if crate::filesystem_entry::metadata_entry_facts(&metadata).is_real_file() => {
            true
        }
        Ok(_) => {
            return Err(FilePublishError::new(
                FilePublishErrorKind::InvalidInput,
                "an existing destination must be a real regular file entry",
            ));
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(FilePublishError::from_io(
                FilePublishErrorKind::Inspect,
                "inspect destination file failed",
                error,
            ));
        }
    };
    let parent = destination.parent().expect("normalized parent");
    publish_file_with(
        staging,
        destination,
        parent,
        replaced_existing,
        replace,
        sync_parent,
    )
}

fn normalized_destination(destination: &Path) -> Result<PathBuf, FilePublishError> {
    let name = destination.file_name().ok_or_else(|| {
        FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "destination requires a final file name",
        )
    })?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let parent = fs::canonicalize(parent).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("inspect destination parent failed: {error}"),
        )
    })?;
    Ok(parent.join(name))
}

fn owned_destination(destination: &Path) -> Result<PathBuf, FilePublishError> {
    destination.file_name().ok_or_else(|| {
        FilePublishError::new(
            FilePublishErrorKind::InvalidInput,
            "destination requires a final file name",
        )
    })?;
    crate::selected::filesystem_publish::normalize_owned_destination(destination).map_err(|error| {
        FilePublishError::new(
            FilePublishErrorKind::Inspect,
            format!("inspect destination parent failed: {error}"),
        )
    })
}

fn create_temporary(
    parent: &Path,
    name: &std::ffi::OsStr,
) -> Result<(PathBuf, fs::File), FilePublishError> {
    for _ in 0..64 {
        let serial = FILE_SERIAL.fetch_add(1, Ordering::Relaxed);
        let mut temporary_name = OsString::from(".");
        temporary_name.push(name);
        temporary_name.push(format!(".platform-write-{}-{serial}", std::process::id()));
        let temporary = parent.join(temporary_name);
        match fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(FilePublishError::new(
                    FilePublishErrorKind::Create,
                    format!("create prepared file failed: {error}"),
                ));
            }
        }
    }
    Err(FilePublishError::new(
        FilePublishErrorKind::Create,
        "unique temporary file attempts exhausted",
    ))
}

fn publish_file_with<R, S>(
    staging: &Path,
    destination: &Path,
    parent: &Path,
    replaced_existing: bool,
    replace: R,
    sync_parent: S,
) -> Result<FilePublishOutcome, FilePublishError>
where
    R: FnOnce(&Path, &Path) -> io::Result<()>,
    S: FnOnce(&Path) -> io::Result<()>,
{
    replace(staging, destination).map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::Install,
            "install prepared file failed",
            error,
        )
    })?;
    sync_parent(parent).map_err(|error| {
        FilePublishError::from_io(
            FilePublishErrorKind::Durability,
            "sync published file parent failed",
            error,
        )
        .after_publish()
    })?;
    Ok(FilePublishOutcome { replaced_existing })
}

struct TemporaryFile {
    path: PathBuf,
    armed: bool,
}

impl TemporaryFile {
    fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.armed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum DirectoryPublishErrorKind {
    InvalidInput,
    Inspect,
    Backup,
    Install,
    InstallRolledBack,
    Rollback,
}

#[derive(Debug)]
pub struct DirectoryPublishError {
    kind: DirectoryPublishErrorKind,
    detail: String,
    retained_backup: Option<PathBuf>,
}

impl DirectoryPublishError {
    pub const fn kind(&self) -> DirectoryPublishErrorKind {
        self.kind
    }

    pub fn detail(&self) -> &str {
        &self.detail
    }

    /// Backup retained after both installation and rollback failed.
    pub fn retained_backup(&self) -> Option<&Path> {
        self.retained_backup.as_deref()
    }

    fn new(kind: DirectoryPublishErrorKind, detail: impl Into<String>) -> Self {
        Self {
            kind,
            detail: detail.into(),
            retained_backup: None,
        }
    }

    fn with_backup(mut self, path: PathBuf) -> Self {
        self.retained_backup = Some(path);
        self
    }
}

impl fmt::Display for DirectoryPublishError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.detail)
    }
}

impl std::error::Error for DirectoryPublishError {}

#[derive(Debug)]
pub struct DirectoryPublishOutcome {
    replaced_existing: bool,
    retained_backup: Option<PathBuf>,
    cleanup_error: Option<String>,
}

impl DirectoryPublishOutcome {
    pub const fn replaced_existing(&self) -> bool {
        self.replaced_existing
    }

    /// Obsolete backup retained because post-install cleanup failed.
    pub fn retained_backup(&self) -> Option<&Path> {
        self.retained_backup.as_deref()
    }

    pub fn cleanup_error(&self) -> Option<&str> {
        self.cleanup_error.as_deref()
    }
}

/// Publish `staging` at `destination`, restoring an existing destination if
/// installation fails.
///
/// Both paths must name distinct entries in the same existing directory, and
/// `staging` must already be a directory. The caller owns and quiesces both
/// entries and must serialize competing publishers. Each rename is atomic on
/// the host filesystem, but replacement as a whole spans two renames and is
/// neither crash-atomic nor a durability barrier.
pub fn publish_directory(
    staging: &Path,
    destination: &Path,
) -> Result<DirectoryPublishOutcome, DirectoryPublishError> {
    validate(staging, destination)?;
    publish_with(
        staging,
        destination,
        |from, to| fs::rename(from, to),
        crate::filesystem_cleanup::remove_tree,
    )
}

fn validate(staging: &Path, destination: &Path) -> Result<(), DirectoryPublishError> {
    if staging == destination {
        return Err(DirectoryPublishError::new(
            DirectoryPublishErrorKind::InvalidInput,
            "staging and destination must be distinct directory entries",
        ));
    }
    let staging_parent = staging.parent().ok_or_else(|| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::InvalidInput,
            "staging requires a parent directory",
        )
    })?;
    let destination_parent = destination.parent().ok_or_else(|| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::InvalidInput,
            "destination requires a parent directory",
        )
    })?;
    let staging_parent = fs::canonicalize(staging_parent).map_err(|error| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::Inspect,
            format!("inspect staging parent failed: {error}"),
        )
    })?;
    let destination_parent = fs::canonicalize(destination_parent).map_err(|error| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::Inspect,
            format!("inspect destination parent failed: {error}"),
        )
    })?;
    if staging_parent != destination_parent {
        return Err(DirectoryPublishError::new(
            DirectoryPublishErrorKind::InvalidInput,
            "staging and destination must share one physical parent directory",
        ));
    }
    let metadata = fs::symlink_metadata(staging).map_err(|error| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::Inspect,
            format!("inspect staging directory failed: {error}"),
        )
    })?;
    if !crate::filesystem_entry::metadata_is_real_directory(&metadata) {
        return Err(DirectoryPublishError::new(
            DirectoryPublishErrorKind::InvalidInput,
            "staging must be a real directory entry",
        ));
    }
    match fs::symlink_metadata(destination) {
        Ok(metadata) if !crate::filesystem_entry::metadata_is_real_directory(&metadata) => {
            return Err(DirectoryPublishError::new(
                DirectoryPublishErrorKind::InvalidInput,
                "an existing destination must be a real directory entry",
            ));
        }
        Ok(_) => {
            let staging_identity = fs::canonicalize(staging).map_err(|error| {
                DirectoryPublishError::new(
                    DirectoryPublishErrorKind::Inspect,
                    format!("resolve staging directory failed: {error}"),
                )
            })?;
            let destination_identity = fs::canonicalize(destination).map_err(|error| {
                DirectoryPublishError::new(
                    DirectoryPublishErrorKind::Inspect,
                    format!("resolve destination directory failed: {error}"),
                )
            })?;
            if staging_identity == destination_identity {
                return Err(DirectoryPublishError::new(
                    DirectoryPublishErrorKind::InvalidInput,
                    "staging and destination resolve to the same directory",
                ));
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DirectoryPublishError::new(
                DirectoryPublishErrorKind::Inspect,
                format!("inspect destination failed: {error}"),
            ));
        }
    }
    Ok(())
}

fn publish_with<R, C>(
    staging: &Path,
    destination: &Path,
    mut rename: R,
    mut cleanup: C,
) -> Result<DirectoryPublishOutcome, DirectoryPublishError>
where
    R: FnMut(&Path, &Path) -> io::Result<()>,
    C: FnMut(&Path) -> io::Result<()>,
{
    let destination_exists = match fs::symlink_metadata(destination) {
        Ok(_) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            return Err(DirectoryPublishError::new(
                DirectoryPublishErrorKind::Inspect,
                format!("inspect destination failed: {error}"),
            ));
        }
    };
    if !destination_exists {
        rename(staging, destination).map_err(|error| {
            DirectoryPublishError::new(
                DirectoryPublishErrorKind::Install,
                format!("install prepared directory failed: {error}"),
            )
        })?;
        return Ok(DirectoryPublishOutcome {
            replaced_existing: false,
            retained_backup: None,
            cleanup_error: None,
        });
    }

    let backup = unique_backup(destination)?;
    rename(destination, &backup).map_err(|error| {
        DirectoryPublishError::new(
            DirectoryPublishErrorKind::Backup,
            format!("backup existing directory failed: {error}"),
        )
    })?;
    if let Err(install_error) = rename(staging, destination) {
        if let Err(rollback_error) = rename(&backup, destination) {
            return Err(DirectoryPublishError::new(
                DirectoryPublishErrorKind::Rollback,
                format!(
                    "install prepared directory failed: {install_error}; restore backup failed: {rollback_error}"
                ),
            )
            .with_backup(backup));
        }
        return Err(DirectoryPublishError::new(
            DirectoryPublishErrorKind::InstallRolledBack,
            format!(
                "install prepared directory failed; existing directory restored: {install_error}"
            ),
        ));
    }

    match cleanup(&backup) {
        Ok(()) => Ok(DirectoryPublishOutcome {
            replaced_existing: true,
            retained_backup: None,
            cleanup_error: None,
        }),
        Err(error) => Ok(DirectoryPublishOutcome {
            replaced_existing: true,
            retained_backup: Some(backup),
            cleanup_error: Some(error.to_string()),
        }),
    }
}

fn unique_backup(destination: &Path) -> Result<PathBuf, DirectoryPublishError> {
    let parent = destination.parent().expect("validated destination parent");
    let name = destination
        .file_name()
        .ok_or_else(|| {
            DirectoryPublishError::new(
                DirectoryPublishErrorKind::InvalidInput,
                "destination requires a final path component",
            )
        })?
        .to_string_lossy();
    for _ in 0..1024 {
        let serial = BACKUP_SERIAL.fetch_add(1, Ordering::Relaxed);
        let candidate = parent.join(format!(
            ".{name}.platform-backup-{}-{serial}",
            std::process::id()
        ));
        match fs::symlink_metadata(&candidate) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(candidate),
            Ok(_) => continue,
            Err(error) => {
                return Err(DirectoryPublishError::new(
                    DirectoryPublishErrorKind::Inspect,
                    format!("inspect backup candidate failed: {error}"),
                ));
            }
        }
    }
    Err(DirectoryPublishError::new(
        DirectoryPublishErrorKind::Backup,
        "could not allocate a unique sibling backup name",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "filesystem")]
    #[test]
    fn private_removal_uses_actual_namespace_and_explicit_native_durability() {
        let root = private_fixture("remove-native");
        let path = root.join("key");
        fs::write(&path, b"synthetic-key").unwrap();
        let result = remove_private_file(&path);
        #[cfg(unix)]
        assert_eq!(result.unwrap(), PrivateRemoveOutcome::Removed);
        #[cfg(windows)]
        {
            let error = result.unwrap_err();
            assert!(error.published());
            assert_eq!(error.kind(), FilePublishErrorKind::Durability);
            assert_eq!(error.io_kind(), io::ErrorKind::Unsupported);
        }
        assert!(
            !path.exists(),
            "successful namespace removal must publish actual absence"
        );
        assert_eq!(
            remove_private_file(&path).unwrap(),
            PrivateRemoveOutcome::NotFound
        );
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(feature = "filesystem")]
    #[test]
    fn private_removal_separates_precommit_failure_and_failed_post_unlink_barrier() {
        let root = private_fixture("remove-boundary");
        let path = root.join("key");
        fs::write(&path, b"synthetic-old").unwrap();
        let before = remove_private_file_with(
            &path,
            |_| Err(io::Error::new(io::ErrorKind::PermissionDenied, "synthetic")),
            |_| Ok(()),
        )
        .unwrap_err();
        assert!(!before.published());
        assert_eq!(before.kind(), FilePublishErrorKind::Remove);
        assert_eq!(fs::read(&path).unwrap(), b"synthetic-old");
        let after = remove_private_file_with(
            &path,
            |path| fs::remove_file(path),
            |_| Err(io::Error::other("synthetic barrier")),
        )
        .unwrap_err();
        assert!(
            after.published(),
            "actual unlink cannot be reported as unpublished after a failed barrier"
        );
        assert_eq!(after.kind(), FilePublishErrorKind::Durability);
        assert!(!path.exists());
        fs::remove_dir_all(root).unwrap();
    }
    #[cfg(feature = "filesystem")]
    #[test]
    fn private_removal_refuses_a_real_directory_before_publication() {
        let root = private_fixture("remove-entry");
        let path = root.join("directory");
        fs::create_dir(&path).unwrap();
        let error = remove_private_file(&path).unwrap_err();
        assert!(!error.published());
        assert_eq!(error.kind(), FilePublishErrorKind::InvalidInput);
        assert!(path.is_dir());
        fs::remove_dir_all(root).unwrap();
    }

    static FIXTURE_SERIAL: AtomicU64 = AtomicU64::new(0);

    fn fixture(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "minicon-platform-publish-{label}-{}-{}",
            std::process::id(),
            FIXTURE_SERIAL.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn prepared(root: &Path, value: &[u8]) -> PathBuf {
        fs::create_dir_all(root).expect("create fixture root");
        let staging = root.join("staging");
        fs::create_dir(&staging).expect("create staging");
        fs::write(staging.join("value"), value).expect("write staging value");
        staging
    }

    #[cfg(feature = "filesystem")]
    fn private_fixture(label: &str) -> PathBuf {
        let root = fixture(label);
        fs::create_dir(&root).unwrap();
        let root = fs::canonicalize(root).unwrap();
        crate::filesystem::protect_private_directory(&root).unwrap();
        root
    }

    #[cfg(all(feature = "filesystem", unix))]
    fn assert_private_access(path: &Path) {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[cfg(all(feature = "filesystem", windows))]
    fn assert_private_access(path: &Path) {
        use std::os::windows::ffi::OsStrExt as _;
        use std::ptr::null_mut;
        use windows_sys::Win32::{
            Foundation::LocalFree,
            Security::{
                ACCESS_ALLOWED_ACE, ACE_HEADER, ACL_SIZE_INFORMATION, AclSizeInformation,
                Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT},
                DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation,
            },
            System::SystemServices::ACCESS_ALLOWED_ACE_TYPE,
        };
        struct Descriptor(*mut std::ffi::c_void);
        impl Drop for Descriptor {
            fn drop(&mut self) {
                unsafe {
                    LocalFree(self.0);
                }
            }
        }
        let wide: Vec<u16> = fs::canonicalize(path)
            .unwrap()
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect();
        let mut dacl = null_mut();
        let mut descriptor = null_mut();
        assert_eq!(
            unsafe {
                GetNamedSecurityInfoW(
                    wide.as_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    &mut dacl,
                    null_mut(),
                    &mut descriptor,
                )
            },
            0
        );
        let _descriptor = Descriptor(descriptor);
        assert!(!dacl.is_null(), "private file has a null DACL");
        let identity = crate::user_identity::current_user_identity().unwrap();
        let sid = identity.windows_sid().unwrap();
        let mut aligned_sid = vec![0_usize; sid.len().div_ceil(std::mem::size_of::<usize>())];
        unsafe {
            std::ptr::copy_nonoverlapping(sid.as_ptr(), aligned_sid.as_mut_ptr().cast(), sid.len());
        }
        let mut size = ACL_SIZE_INFORMATION {
            AceCount: 0,
            AclBytesInUse: 0,
            AclBytesFree: 0,
        };
        assert_ne!(
            unsafe {
                GetAclInformation(
                    dacl,
                    (&mut size as *mut ACL_SIZE_INFORMATION).cast(),
                    std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            },
            0
        );
        assert!(size.AceCount > 0, "private file has an empty DACL");
        for index in 0..size.AceCount {
            let mut ace = null_mut();
            assert_ne!(unsafe { GetAce(dacl, index, &mut ace) }, 0);
            let header = unsafe { &*ace.cast::<ACE_HEADER>() };
            assert_eq!(u32::from(header.AceType), ACCESS_ALLOWED_ACE_TYPE);
            let allowed = unsafe { &*ace.cast::<ACCESS_ALLOWED_ACE>() };
            assert_ne!(allowed.Mask, 0);
            assert_ne!(
                unsafe {
                    EqualSid(
                        (&allowed.SidStart as *const u32).cast_mut().cast(),
                        aligned_sid.as_mut_ptr().cast(),
                    )
                },
                0,
                "private file grants access to another trustee"
            );
        }
    }

    #[cfg(feature = "filesystem")]
    #[test]
    fn private_publication_distinguishes_precommit_and_postcommit_failures() {
        let root = private_fixture("private-boundary");
        let destination = root.join("state");
        write_private_atomic(&destination, b"old").unwrap();
        let before = write_private_atomic_with(
            &destination,
            b"new",
            |staging, _| {
                assert_eq!(
                    crate::filesystem::read_private_bounded(staging, 3).unwrap(),
                    b"new"
                );
                assert_private_access(staging);
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected before replacement",
                ))
            },
            |_| panic!("failed replacement must not synchronize a publication"),
        )
        .unwrap_err();
        assert_eq!(before.kind(), FilePublishErrorKind::Install);
        assert_eq!(before.io_kind(), io::ErrorKind::PermissionDenied);
        assert!(!before.published());
        assert_eq!(
            crate::filesystem::read_private_bounded(&destination, 3).unwrap(),
            b"old"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let compatibility: io::Error = before.into();
        assert_eq!(compatibility.kind(), io::ErrorKind::PermissionDenied);

        let after = write_private_atomic_with(
            &destination,
            b"new",
            crate::selected::filesystem_publish::replace_file,
            |_| {
                assert_eq!(
                    crate::filesystem::read_private_bounded(&destination, 3).unwrap(),
                    b"new"
                );
                Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "injected parent barrier failure",
                ))
            },
        )
        .unwrap_err();
        assert_eq!(after.kind(), FilePublishErrorKind::Durability);
        assert!(after.published());
        assert_eq!(after.io_kind(), io::ErrorKind::TimedOut);
        assert_eq!(
            std::error::Error::source(&after)
                .unwrap()
                .downcast_ref::<io::Error>()
                .unwrap()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(
            crate::filesystem::read_private_bounded(&destination, 3).unwrap(),
            b"new"
        );
        assert_private_access(&destination);
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        let compatibility: io::Error = after.into();
        assert_eq!(compatibility.kind(), io::ErrorKind::TimedOut);
        assert!(
            compatibility
                .get_ref()
                .unwrap()
                .downcast_ref::<FilePublishError>()
                .unwrap()
                .published()
        );
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[cfg(feature = "filesystem")]
    #[test]
    fn private_publication_keeps_staging_and_legacy_replacement_private() {
        let root = private_fixture("private-access");
        let destination = root.join("state");
        write_private_atomic_with(
            &destination,
            b"first",
            |staging, target| {
                assert_private_access(staging);
                assert_eq!(
                    crate::filesystem::read_private_bounded(staging, 5).unwrap(),
                    b"first"
                );
                crate::selected::filesystem_publish::replace_file(staging, target)
            },
            crate::selected::filesystem_publish::sync_parent,
        )
        .unwrap();
        assert_private_access(&destination);
        crate::filesystem::write_private_atomic(&destination, b"second").unwrap();
        assert_private_access(&destination);
        assert_eq!(
            crate::filesystem::read_private_bounded(&destination, 6).unwrap(),
            b"second"
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[cfg(feature = "filesystem")]
    #[test]
    fn private_publication_refuses_nonregular_targets_and_preserves_io_category() {
        let root = private_fixture("private-refusal");
        let directory = root.join("directory");
        fs::create_dir(&directory).unwrap();
        let outside = root.join("outside");
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("sentinel"), b"unchanged").unwrap();
        let link = root.join("link");
        directory_link(&outside, &link);
        for destination in [&directory, &link] {
            let error = write_private_atomic(destination, b"secret").unwrap_err();
            assert_eq!(error.kind(), FilePublishErrorKind::InvalidInput);
            assert!(!error.published());
            assert_eq!(error.io_kind(), io::ErrorKind::InvalidInput);
            assert_eq!(
                crate::filesystem::write_private_atomic(destination, b"secret")
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
        assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"unchanged");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 3);
        let error = write_private_atomic(&root.join("missing/state"), b"secret").unwrap_err();
        assert_eq!(error.kind(), FilePublishErrorKind::Create);
        assert!(!error.published());
        assert_eq!(error.io_kind(), io::ErrorKind::NotFound);
        let legacy =
            crate::filesystem::write_private_atomic(&root.join("missing/state"), b"secret")
                .unwrap_err();
        assert_eq!(legacy.kind(), io::ErrorKind::NotFound);
        let native = fs::File::create(root.join("missing/probe")).unwrap_err();
        assert!(native.raw_os_error().is_some());
        assert_eq!(legacy.raw_os_error(), native.raw_os_error());
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn atomic_file_writer_replaces_and_leaves_no_temporary() {
        let root = fixture("file-replace");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("snapshot.json");
        let first = write_file_atomic(&destination, |file| {
            io::Write::write_all(file, b"first")?;
            Ok(5)
        })
        .expect("first publish");
        assert_eq!(first, 5);
        write_file_atomic(&destination, |file| io::Write::write_all(file, b"second"))
            .expect("replacement publish");
        assert_eq!(fs::read(&destination).unwrap(), b"second");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn failed_file_writer_preserves_destination_and_cleans_temporary() {
        let root = fixture("file-write-failure");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("snapshot.json");
        fs::write(&destination, b"old").unwrap();
        let error = write_file_atomic(&destination, |file| -> io::Result<()> {
            io::Write::write_all(file, b"partial")?;
            Err(io::Error::other("injected"))
        })
        .expect_err("writer failure");
        assert_eq!(error.kind(), FilePublishErrorKind::Write);
        assert!(!error.published());
        assert_eq!(fs::read(&destination).unwrap(), b"old");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn no_clobber_path_writer_installs_once_and_preserves_existing_file() {
        let root = fixture("file-no-clobber");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("capture.png");
        write_path_atomic_no_clobber(&destination, |path| fs::write(path, b"first"))
            .expect("initial publish");
        let error = write_path_atomic_no_clobber(&destination, |path| fs::write(path, b"second"))
            .expect_err("existing destination must be refused");
        assert_eq!(error.kind(), FilePublishErrorKind::Install);
        assert!(!error.published());
        assert_eq!(fs::read(&destination).unwrap(), b"first");
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_clobber_path_writer_does_not_follow_final_symlink() {
        let root = fixture("file-no-clobber-link");
        fs::create_dir_all(&root).unwrap();
        let target = root.join("target.png");
        let destination = root.join("capture.png");
        fs::write(&target, b"target").unwrap();
        std::os::unix::fs::symlink(&target, &destination).unwrap();
        let error = write_path_atomic_no_clobber(&destination, |path| fs::write(path, b"new"))
            .expect_err("final symlink must be refused");
        assert_eq!(error.kind(), FilePublishErrorKind::Install);
        assert!(!error.published());
        assert_eq!(fs::read(&target).unwrap(), b"target");
        assert!(
            fs::symlink_metadata(&destination)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 2);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn no_clobber_path_writer_rejects_replaced_reserved_object() {
        let root = fixture("file-no-clobber-staging-swap");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("capture.png");
        let error = write_path_atomic_no_clobber(&destination, |path| {
            let replacement = root.join("replacement");
            fs::write(&replacement, b"substituted")?;
            fs::rename(replacement, path)
        })
        .expect_err("replaced reserved object must be refused");
        assert_eq!(error.kind(), FilePublishErrorKind::Inspect);
        assert!(!error.published());
        assert!(!destination.exists());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 0);
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn durability_failure_reports_that_complete_file_was_published() {
        let root = fixture("file-durability");
        fs::create_dir_all(&root).unwrap();
        let staging = root.join("staging");
        let destination = root.join("live");
        fs::write(&staging, b"new").unwrap();
        fs::write(&destination, b"old").unwrap();
        let error = publish_file_with(
            &staging,
            &destination,
            &root,
            true,
            |from, to| fs::rename(from, to),
            |_parent| Err(io::Error::other("injected")),
        )
        .expect_err("durability failure");
        assert_eq!(error.kind(), FilePublishErrorKind::Durability);
        assert!(error.published());
        assert_eq!(fs::read(&destination).unwrap(), b"new");
        assert!(!staging.exists());
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn concurrent_readers_observe_only_complete_atomic_values() {
        let root = fixture("file-reader");
        fs::create_dir_all(&root).unwrap();
        let destination = root.join("live");
        write_file_atomic(&destination, |file| io::Write::write_all(file, b"left"))
            .expect("initial publish");
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let writer_barrier = &barrier;
            let writer_path = &destination;
            scope.spawn(move || {
                writer_barrier.wait();
                for index in 0..64 {
                    let value: &[u8] = if index % 2 == 0 { b"right" } else { b"left" };
                    write_file_atomic(writer_path, |file| io::Write::write_all(file, value))
                        .expect("concurrent publish");
                }
            });
            barrier.wait();
            for _ in 0..256 {
                let value = fs::read(&destination).expect("read atomic value");
                assert!(value == b"left" || value == b"right");
            }
        });
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[cfg(unix)]
    fn directory_link(target: &Path, link: &Path) {
        std::os::unix::fs::symlink(target, link).expect("create directory symlink");
    }

    #[cfg(windows)]
    fn directory_link(target: &Path, link: &Path) {
        let status = std::process::Command::new("cmd.exe")
            .args(["/d", "/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("run mklink junction fixture");
        assert!(status.success(), "mklink /J fixture failed: {status}");
    }

    #[test]
    fn installs_new_and_replaces_existing_directories() {
        let root = fixture("replace");
        let staging = prepared(&root, b"first");
        let destination = root.join("live");
        let first = publish_directory(&staging, &destination).expect("first publish");
        assert!(!first.replaced_existing());
        assert_eq!(fs::read(destination.join("value")).unwrap(), b"first");

        let staging = root.join("staging-2");
        fs::create_dir(&staging).unwrap();
        fs::write(staging.join("value"), b"second").unwrap();
        let second = publish_directory(&staging, &destination).expect("replacement publish");
        assert!(second.replaced_existing());
        assert!(second.retained_backup().is_none());
        assert_eq!(fs::read(destination.join("value")).unwrap(), b"second");
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn install_failure_restores_existing_directory() {
        let root = fixture("rollback");
        let staging = prepared(&root, b"new");
        let destination = root.join("live");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("value"), b"old").unwrap();
        let mut calls = 0;
        let error = publish_with(
            &staging,
            &destination,
            |from, to| {
                calls += 1;
                if calls == 2 {
                    Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected"))
                } else {
                    fs::rename(from, to)
                }
            },
            crate::filesystem_cleanup::remove_tree,
        )
        .expect_err("install must fail");
        assert_eq!(error.kind(), DirectoryPublishErrorKind::InstallRolledBack);
        assert!(error.retained_backup().is_none());
        assert_eq!(fs::read(destination.join("value")).unwrap(), b"old");
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn rollback_failure_reports_the_retained_backup() {
        let root = fixture("rollback-failure");
        let staging = prepared(&root, b"new");
        let destination = root.join("live");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("value"), b"old").unwrap();
        let mut calls = 0;
        let error = publish_with(
            &staging,
            &destination,
            |from, to| {
                calls += 1;
                match calls {
                    1 => fs::rename(from, to),
                    _ => Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected")),
                }
            },
            crate::filesystem_cleanup::remove_tree,
        )
        .expect_err("install and rollback must fail");
        assert_eq!(error.kind(), DirectoryPublishErrorKind::Rollback);
        assert!(error.retained_backup().is_some());
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn cleanup_failure_is_a_successful_publish_with_a_warning() {
        let root = fixture("cleanup-warning");
        let staging = prepared(&root, b"new");
        let destination = root.join("live");
        fs::create_dir(&destination).unwrap();
        fs::write(destination.join("value"), b"old").unwrap();
        let outcome = publish_with(
            &staging,
            &destination,
            |from, to| fs::rename(from, to),
            |_path| Err(io::Error::new(io::ErrorKind::PermissionDenied, "injected")),
        )
        .expect("installation succeeds");
        assert!(outcome.replaced_existing());
        assert!(outcome.retained_backup().is_some());
        assert_eq!(outcome.cleanup_error(), Some("injected"));
        assert_eq!(fs::read(destination.join("value")).unwrap(), b"new");
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
    }

    #[test]
    fn rejects_different_parents_and_non_directory_staging() {
        let left = fixture("left");
        let right = fixture("right");
        fs::create_dir_all(&left).unwrap();
        fs::create_dir_all(&right).unwrap();
        let staging = left.join("staging");
        fs::write(&staging, b"file").unwrap();
        let non_directory = publish_directory(&staging, &left.join("live")).unwrap_err();
        assert_eq!(
            non_directory.kind(),
            DirectoryPublishErrorKind::InvalidInput
        );
        fs::remove_file(&staging).unwrap();
        fs::create_dir(&staging).unwrap();
        let destination_file = left.join("live-file");
        fs::write(&destination_file, b"file").unwrap();
        let non_directory_destination = publish_directory(&staging, &destination_file).unwrap_err();
        assert_eq!(
            non_directory_destination.kind(),
            DirectoryPublishErrorKind::InvalidInput
        );
        let different_parent = publish_directory(&staging, &right.join("live")).unwrap_err();
        assert_eq!(
            different_parent.kind(),
            DirectoryPublishErrorKind::InvalidInput
        );
        crate::filesystem_cleanup::remove_tree(&left).unwrap();
        crate::filesystem_cleanup::remove_tree(&right).unwrap();
    }

    #[test]
    fn rejects_link_like_staging_and_destination() {
        let root = fixture("link-like");
        let outside = fixture("link-like-outside");
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let staging = root.join("staging");
        let destination = root.join("live");
        directory_link(&outside, &staging);

        let staging_error = publish_directory(&staging, &destination).unwrap_err();
        assert_eq!(
            staging_error.kind(),
            DirectoryPublishErrorKind::InvalidInput
        );
        crate::filesystem_cleanup::remove_tree(&staging).unwrap();

        fs::create_dir(&staging).unwrap();
        directory_link(&outside, &destination);
        let destination_error = publish_directory(&staging, &destination).unwrap_err();
        assert_eq!(
            destination_error.kind(),
            DirectoryPublishErrorKind::InvalidInput
        );
        assert!(
            staging.is_dir(),
            "rejected staging must remain caller-owned"
        );
        assert!(
            outside.is_dir(),
            "validation traversed the link-like target"
        );
        crate::filesystem_cleanup::remove_tree(&root).unwrap();
        fs::remove_dir_all(outside).unwrap();
    }
}
