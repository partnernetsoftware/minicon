//! Unix atomic file publication adapter.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
    std::fs::rename(source, destination)
}

pub fn install_file_no_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    // Creating the destination hard link is atomic and fails when any entry,
    // including a symlink, already owns that final name. Both paths are
    // siblings, so the link cannot cross filesystems.
    std::fs::hard_link(source, destination)
}

pub fn sync_parent(parent: &Path) -> std::io::Result<()> {
    std::fs::File::open(parent)?.sync_all()
}

pub fn normalize_owned_destination(destination: &Path) -> std::io::Result<std::path::PathBuf> {
    let name = destination
        .file_name()
        .ok_or_else(|| std::io::Error::other("destination file name required"))?;
    let parent = destination
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::canonicalize(parent).map(|parent| parent.join(name))
}

/// Unix decides "executable" with a file mode, and the generic atomic writer
/// stages a mode-less private file, so a published executable needs the
/// destination's mode (or 0o755 when the destination cannot be stat'ed)
/// copied onto the staged sibling or the OS refuses to run it.
pub fn copy_executable_mode(staged: &Path, destination: &Path) -> std::io::Result<()> {
    let mode = match std::fs::metadata(destination) {
        Ok(metadata) => metadata.permissions().mode(),
        Err(_) => 0o755,
    };
    std::fs::set_permissions(staged, std::fs::Permissions::from_mode(mode))
}

/// The `install-cli` mechanism: a plain symlink into a user-named prefix.
pub fn install_cli_symlink(target: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, link)
}

/// Removal has a distinct barrier contract from replacement.
#[cfg(feature = "filesystem")]
pub fn sync_removed_parent(parent: &Path) -> std::io::Result<()> {
    sync_parent(parent)
}
