//! Explicit native storage/network contexts. Ownership: PRD 28.

use std::{fmt, io::Read, path::PathBuf};

use crate::BrowserProxy;

/// The caller owns the stable pairing and pre-creates a private, canonical
/// directory. macOS uses the nonzero identifier instead of a filesystem path.
#[derive(Clone, Eq, PartialEq)]
pub struct PageProfile {
    pub data_directory: PathBuf,
    pub store_identifier: [u8; 16],
}

#[cfg(all(windows, feature = "native-page-host"))]
impl PageProfile {
    /// WebView2's UDF argument, distinct from the canonical store identity.
    /// Unsupported verbatim namespaces fail before native creation. PRD 28.01.
    pub fn webview2_user_data_folder(&self) -> Result<PathBuf, PageContextError> {
        PageContext {
            profile: self.clone(),
            proxy: BrowserProxy::default(),
        }
        .validate()?;
        windows_sdk_user_data_folder(&self.data_directory)
    }
}

#[cfg(all(windows, feature = "native-page-host"))]
fn windows_sdk_path_spelling(path: &std::path::Path) -> Result<PathBuf, PageContextError> {
    use std::path::{Component, Prefix};
    let compatible = dunce::simplified(path);
    if !compatible.is_absolute()
        || !matches!(
            compatible.components().next(),
            Some(Component::Prefix(prefix)) if matches!(prefix.kind(), Prefix::Disk(_))
        )
    {
        return Err(PageContextError::Unsupported(
            "browser-context-profile-path-unsupported",
        ));
    }
    Ok(compatible.to_path_buf())
}

#[cfg(all(windows, feature = "native-page-host"))]
pub(crate) fn windows_sdk_user_data_folder(
    path: &std::path::Path,
) -> Result<PathBuf, PageContextError> {
    let canonical = path
        .canonicalize()
        .map_err(|_| PageContextError::ProfileUnavailable)?;
    let compatible = windows_sdk_path_spelling(&canonical)?;
    if compatible
        .canonicalize()
        .map_err(|_| PageContextError::ProfileUnavailable)?
        != canonical
    {
        return Err(PageContextError::InvalidProfile);
    }
    Ok(compatible)
}

/// An explicit context never resolves network settings from the environment.
#[derive(Clone, Eq, PartialEq)]
pub struct PageContext {
    pub profile: PageProfile,
    pub proxy: BrowserProxy,
}

// Profiles and proxy endpoints are not diagnostic payloads.
impl fmt::Debug for PageProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageProfile([redacted])")
    }
}
impl fmt::Debug for PageContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageContext([redacted])")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageContextError {
    InvalidProfile,
    Unsupported(&'static str),
    InvalidProxy,
    ProfileUnavailable,
    NativeCreateFailed,
}

impl PageContextError {
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidProfile => "browser-context-invalid-profile",
            Self::Unsupported(capability) => capability,
            Self::InvalidProxy => "browser-context-invalid-proxy",
            Self::ProfileUnavailable => "browser-context-profile-unavailable",
            Self::NativeCreateFailed => "browser-context-create-failed",
        }
    }
}
impl fmt::Display for PageContextError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}
impl std::error::Error for PageContextError {}

pub(crate) const CONTEXT_FRAME_MAX: usize = 4096;

impl PageContext {
    pub fn validate(&self) -> Result<(), PageContextError> {
        let path = &self.profile.data_directory;
        if self.profile.store_identifier == [0; 16]
            || !path.is_absolute()
            || path.to_str().is_none_or(|value| value.len() > 2048)
        {
            return Err(PageContextError::InvalidProfile);
        }
        for ancestor in path.ancestors() {
            let metadata = std::fs::symlink_metadata(ancestor)
                .map_err(|_| PageContextError::ProfileUnavailable)?;
            let link = metadata.file_type().is_symlink();
            #[cfg(windows)]
            let link = {
                use std::os::windows::fs::MetadataExt;
                link || metadata.file_attributes() & 0x400 != 0
            };
            if link || !metadata.is_dir() {
                return Err(PageContextError::InvalidProfile);
            }
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata =
                std::fs::metadata(path).map_err(|_| PageContextError::ProfileUnavailable)?;
            if metadata.permissions().mode() & 0o077 != 0 {
                return Err(PageContextError::InvalidProfile);
            }
        }
        let ProxyParts {
            http,
            https,
            bypass,
        } = self.proxy_parts()?;
        drop((http, https, bypass));
        Ok(())
    }

    pub(crate) fn proxy_parts(&self) -> Result<ProxyParts, PageContextError> {
        let endpoint = |value: &Option<String>| {
            value
                .as_ref()
                .map(|value| {
                    if value.len() > 512 {
                        return Err(PageContextError::InvalidProxy);
                    }
                    crate::parse_http_proxy_endpoint(value).ok_or(PageContextError::InvalidProxy)
                })
                .transpose()
        };
        let http = endpoint(&self.proxy.http)?;
        let https = endpoint(&self.proxy.https)?;
        let mut bypass = Vec::new();
        if let Some(value) = &self.proxy.no_proxy {
            if value.len() > 512 {
                return Err(PageContextError::InvalidProxy);
            }
            for host in value
                .split([',', ';'])
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                // Only exact DNS/IPv4 hosts have consistent semantics across
                // these adapters. Do not silently reinterpret patterns/CIDR.
                if host.len() > 253
                    || !host
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b".-".contains(&c))
                {
                    return Err(PageContextError::Unsupported(
                        "browser-context-proxy-bypass-unsupported",
                    ));
                }
                if !bypass
                    .iter()
                    .any(|existing: &String| existing.eq_ignore_ascii_case(host))
                {
                    bypass.push(host.to_owned());
                }
                if bypass.len() > 32 {
                    return Err(PageContextError::InvalidProxy);
                }
            }
        }
        Ok(ProxyParts {
            http,
            https,
            bypass,
        })
    }

    // A bounded startup frame shares stdin with subsequent page commands.
    // Hex fields preserve framing without adding JSON to headless consumers.
    #[cfg(any(target_os = "linux", test))]
    pub(crate) fn frame(&self) -> Result<Vec<u8>, PageContextError> {
        self.validate()?;
        let fields = [
            hex(&self.profile.store_identifier),
            hex(self
                .profile
                .data_directory
                .to_str()
                .ok_or(PageContextError::InvalidProfile)?
                .as_bytes()),
            optional_hex(&self.proxy.http),
            optional_hex(&self.proxy.https),
            optional_hex(&self.proxy.no_proxy),
        ];
        let frame = format!("MINICON-CONTEXT-1\t{}\n", fields.join("\t")).into_bytes();
        if frame.len() > CONTEXT_FRAME_MAX {
            return Err(PageContextError::InvalidProfile);
        }
        Ok(frame)
    }

    pub(crate) fn read_frame(input: &mut impl Read) -> Result<Self, PageContextError> {
        let mut bytes = Vec::new();
        loop {
            if bytes.len() >= CONTEXT_FRAME_MAX {
                return Err(PageContextError::InvalidProfile);
            }
            let mut byte = [0];
            input
                .read_exact(&mut byte)
                .map_err(|_| PageContextError::InvalidProfile)?;
            bytes.push(byte[0]);
            if byte[0] == b'\n' {
                break;
            }
        }
        let text = std::str::from_utf8(&bytes[..bytes.len() - 1])
            .map_err(|_| PageContextError::InvalidProfile)?;
        let fields: Vec<_> = text.split('\t').collect();
        if fields.len() != 6 || fields[0] != "MINICON-CONTEXT-1" {
            return Err(PageContextError::InvalidProfile);
        }
        let store_identifier = unhex(fields[1])?
            .try_into()
            .map_err(|_| PageContextError::InvalidProfile)?;
        let directory =
            String::from_utf8(unhex(fields[2])?).map_err(|_| PageContextError::InvalidProfile)?;
        let optional = |value: &str| {
            if value == "-" {
                Ok(None)
            } else {
                String::from_utf8(unhex(value)?)
                    .map(Some)
                    .map_err(|_| PageContextError::InvalidProfile)
            }
        };
        let context = Self {
            profile: PageProfile {
                data_directory: directory.into(),
                store_identifier,
            },
            proxy: BrowserProxy {
                http: optional(fields[3])?,
                https: optional(fields[4])?,
                no_proxy: optional(fields[5])?,
            },
        };
        context.validate()?;
        Ok(context)
    }
}

pub(crate) struct ProxyParts {
    pub http: Option<(String, String)>,
    pub https: Option<(String, String)>,
    pub bypass: Vec<String>,
}

#[cfg(any(target_os = "linux", test))]
fn optional_hex(value: &Option<String>) -> String {
    value
        .as_ref()
        .map(|value| hex(value.as_bytes()))
        .unwrap_or_else(|| "-".to_owned())
}
#[cfg(any(target_os = "linux", test))]
fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|value| {
            [
                DIGITS[(value >> 4) as usize] as char,
                DIGITS[(value & 15) as usize] as char,
            ]
        })
        .collect()
}
fn unhex(value: &str) -> Result<Vec<u8>, PageContextError> {
    if !value.len().is_multiple_of(2) {
        return Err(PageContextError::InvalidProfile);
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| {
                (byte as char)
                    .to_digit(16)
                    .map(|v| v as u8)
                    .ok_or(PageContextError::InvalidProfile)
            };
            Ok(digit(pair[0])? * 16 + digit(pair[1])?)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(all(windows, feature = "native-page-host"))]
    #[test]
    fn webview2_udf_spelling_preserves_the_real_profile_without_a_verbatim_prefix() {
        let root = Fixture::new();
        let profile = root.context().profile;
        let folder = profile.webview2_user_data_folder().unwrap();
        assert!(folder.is_absolute());
        assert!(matches!(
            folder.components().next(),
            Some(std::path::Component::Prefix(prefix))
                if matches!(prefix.kind(), std::path::Prefix::Disk(_))
        ));
        assert_eq!(folder.canonicalize().unwrap(), profile.data_directory);
        assert_eq!(root.context().profile, profile);
    }

    #[cfg(all(windows, feature = "native-page-host"))]
    #[test]
    fn webview2_udf_spelling_refuses_names_that_cannot_keep_ordinary_drive_meaning() {
        use std::path::Path;
        assert_eq!(
            windows_sdk_path_spelling(Path::new(r"\\?\C:\Temp\目录 name")).unwrap(),
            Path::new(r"C:\Temp\目录 name")
        );
        for path in [
            r"C:relative",
            r"relative",
            r"\\.\pipe\fixture",
            r"\\?\UNC\fixture.invalid\share\folder",
            r"\\?\C:\Temp\CON.txt",
            r"\\?\C:\Temp\trailing.",
            r"\\?\C:\Temp\trailing ",
        ] {
            assert_eq!(
                windows_sdk_path_spelling(Path::new(path)),
                Err(PageContextError::Unsupported(
                    "browser-context-profile-path-unsupported"
                ))
            );
        }
    }

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "minicon-page-context-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            std::fs::create_dir(&path).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
            Self(path.canonicalize().unwrap())
        }
        fn context(&self) -> PageContext {
            PageContext {
                profile: PageProfile {
                    data_directory: self.0.clone(),
                    store_identifier: [7; 16],
                },
                proxy: BrowserProxy::default(),
            }
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn context_frame_round_trip_preserves_profile_and_leaves_following_command_unread() {
        let root = Fixture::new();
        let mut context = root.context();
        context.proxy = BrowserProxy {
            http: Some("http://127.0.0.1:18080".into()),
            https: Some("http://127.0.0.1:18081".into()),
            no_proxy: Some("localhost,127.0.0.1".into()),
        };
        let mut wire = context.frame().unwrap();
        wire.extend_from_slice(b"next-command\n");
        let mut input = std::io::Cursor::new(wire);
        assert_eq!(PageContext::read_frame(&mut input).unwrap(), context);
        let mut remaining = String::new();
        input.read_to_string(&mut remaining).unwrap();
        assert_eq!(remaining, "next-command\n");
        let debug = format!("{context:?}");
        assert!(!debug.contains("18080") && !debug.contains(root.0.to_str().unwrap()));
    }

    #[test]
    fn context_validation_refuses_missing_relative_zero_and_nonprivate_profiles() {
        let root = Fixture::new();
        let mut context = root.context();
        context.validate().unwrap();
        context.profile.store_identifier = [0; 16];
        assert_eq!(context.validate(), Err(PageContextError::InvalidProfile));
        context.profile.store_identifier = [7; 16];
        context.profile.data_directory = PathBuf::from("relative");
        assert_eq!(context.validate(), Err(PageContextError::InvalidProfile));
        context.profile.data_directory = root.0.join("absent");
        assert_eq!(
            context.validate(),
            Err(PageContextError::ProfileUnavailable)
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            context.profile.data_directory = root.0.clone();
            std::fs::set_permissions(&root.0, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(context.validate(), Err(PageContextError::InvalidProfile));
        }
    }

    #[test]
    fn context_frames_refuse_truncation_overflow_and_extra_fields() {
        let root = Fixture::new();
        let frame = root.context().frame().unwrap();
        assert!(PageContext::read_frame(&mut &frame[..frame.len() - 1]).is_err());
        assert!(PageContext::read_frame(&mut &b"x".repeat(CONTEXT_FRAME_MAX + 1)[..]).is_err());
        let mut extra = frame[..frame.len() - 1].to_vec();
        extra.extend_from_slice(b"\textra\n");
        assert!(PageContext::read_frame(&mut &extra[..]).is_err());
    }

    #[test]
    fn context_proxy_refuses_credentials_schemes_and_unimplemented_bypass_patterns() {
        let root = Fixture::new();
        let mut context = root.context();
        for value in [
            "http://user:secret@host:8080",
            "socks5://host:1080",
            "https://host:8080",
            "http://host:0",
        ] {
            context.proxy.http = Some(value.into());
            assert_eq!(context.validate(), Err(PageContextError::InvalidProxy));
        }
        context.proxy.http = None;
        context.proxy.no_proxy = Some("*.example.test".into());
        assert_eq!(
            context.validate(),
            Err(PageContextError::Unsupported(
                "browser-context-proxy-bypass-unsupported"
            ))
        );
    }

    #[cfg(unix)]
    #[test]
    fn context_profile_refuses_a_symlinked_directory_or_ancestor() {
        let root = Fixture::new();
        let real = root.0.join("real");
        std::fs::create_dir(&real).unwrap();
        use std::os::unix::fs::{PermissionsExt, symlink};
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o700)).unwrap();
        let linked = root.0.join("linked");
        symlink(&real, &linked).unwrap();
        let mut context = root.context();
        context.profile.data_directory = linked.clone();
        assert_eq!(context.validate(), Err(PageContextError::InvalidProfile));
        std::fs::create_dir(real.join("nested")).unwrap();
        context.profile.data_directory = linked.join("nested");
        assert_eq!(context.validate(), Err(PageContextError::InvalidProfile));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn explicit_helper_receives_startup_frame_before_the_existing_command_stream() {
        use std::{
            io::Write,
            os::unix::fs::PermissionsExt,
            time::{Duration, Instant},
        };
        struct Owned(std::process::Child);
        impl Drop for Owned {
            fn drop(&mut self) {
                let _ = self.0.kill();
                let _ = self.0.wait();
            }
        }
        let root = Fixture::new();
        let helper = root.0.join("context-helper");
        std::fs::write(&helper, b"#!/bin/sh\nIFS= read -r startup || exit 1\nprintf '%s\\n' \"$startup\" > \"$0.frame\"\nIFS= read -r command || exit 2\nprintf '%s' \"$command\" > \"$0.command\"\nprintf '%s\\n' \"$@\" > \"$0.args\"\n").unwrap();
        std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let context = root.context();
        let mut child = Owned(
            crate::spawn_page_host_in_context(&helper, "https://example.test/", &context, None)
                .unwrap(),
        );
        child
            .0
            .stdin
            .take()
            .unwrap()
            .write_all(b"next-command\n")
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        let status = loop {
            if let Some(status) = child.0.try_wait().unwrap() {
                break status;
            }
            if Instant::now() >= deadline {
                panic!("owned helper did not finish its bounded frame/command fixture");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        assert!(status.success());
        let frame = std::fs::read(helper.with_extension("frame")).unwrap();
        assert_eq!(PageContext::read_frame(&mut &frame[..]).unwrap(), context);
        assert_eq!(
            std::fs::read(helper.with_extension("command")).unwrap(),
            b"next-command"
        );
        let args = std::fs::read_to_string(helper.with_extension("args")).unwrap();
        assert_eq!(args, "--load-context\nhttps://example.test/\n");
    }
}
